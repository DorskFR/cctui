//! The voice conversation loop, independent of the socket and the speech
//! upstream: utterances in, control frames and synthesized sentences out.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use cctui_proto::voice::{SpokenStyle, VoiceClientMsg, VoiceServerMsg, VoiceState};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use tokio::sync::mpsc;

use super::text::{SentenceBuffer, spoken_prompt};

pub type AudioStream = BoxStream<'static, Result<Bytes, String>>;

/// What the loop needs from the server: speech both ways and the session.
pub trait VoiceBackend: Send + Sync + 'static {
    fn transcribe(&self, audio: Bytes) -> impl Future<Output = Result<String, String>> + Send;
    fn synthesize(
        &self,
        text: &str,
        voice: Option<&str>,
        speed: Option<f32>,
    ) -> impl Future<Output = Result<AudioStream, String>> + Send;
    fn send_message(&self, text: String) -> impl Future<Output = Result<(), String>> + Send;
    fn interrupt(&self) -> impl Future<Output = Result<(), String>> + Send;
}

#[derive(Debug)]
pub enum Inbound {
    /// One utterance: 16 kHz mono 16-bit PCM.
    Audio(Bytes),
    Control(VoiceClientMsg),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    Control(VoiceServerMsg),
    Audio(Bytes),
}

/// What the session says, as the socket glue reads it off the bus.
#[derive(Debug)]
pub enum Signal {
    Delta(String),
    /// A final assistant prose block.
    Text(String),
    TurnEnd,
}

#[derive(Debug, Clone, Default)]
pub struct Prefs {
    pub barge_in: bool,
    pub spoken_style: Option<SpokenStyle>,
    pub voice: Option<String>,
    pub speed: Option<f32>,
}

type SharedPrefs = Arc<Mutex<Prefs>>;

#[derive(Debug)]
struct Notice {
    generation: u64,
    busy: bool,
}

/// One synthesis worker: sentences queued in order, audio out as each is
/// ready. Dropping it aborts whatever was in flight.
struct Synth {
    tx: mpsc::Sender<String>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Synth {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn spawn_synth<B: VoiceBackend>(
    backend: Arc<B>,
    prefs: SharedPrefs,
    out: mpsc::Sender<Outbound>,
    notices: mpsc::UnboundedSender<Notice>,
    generation: u64,
) -> Synth {
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let task = tokio::spawn(async move {
        let mut n = 0u64;
        while let Some(text) = rx.recv().await {
            n += 1;
            let _ = notices.send(Notice { generation, busy: true });
            let line_id = format!("{generation}-{n}");
            speak_one(&*backend, &prefs, &out, &line_id, &text).await;
            if rx.is_empty() {
                let _ = notices.send(Notice { generation, busy: false });
            }
        }
    });
    Synth { tx, task }
}

async fn speak_one<B: VoiceBackend>(
    backend: &B,
    prefs: &SharedPrefs,
    out: &mpsc::Sender<Outbound>,
    line_id: &str,
    text: &str,
) {
    let (voice, speed) = {
        let p = prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (p.voice.clone(), p.speed)
    };
    let mut stream = match backend.synthesize(text, voice.as_deref(), speed).await {
        Ok(s) => s,
        Err(message) => {
            let _ = out.send(Outbound::Control(VoiceServerMsg::Error { message })).await;
            return;
        }
    };
    let start = VoiceServerMsg::SpeakStart { line_id: line_id.to_owned(), text: text.to_owned() };
    if out.send(Outbound::Control(start)).await.is_err() {
        return;
    }
    while let Some(chunk) = stream.next().await {
        let frame = match chunk {
            Ok(bytes) if bytes.is_empty() => continue,
            Ok(bytes) => Outbound::Audio(bytes),
            Err(message) => Outbound::Control(VoiceServerMsg::Error { message }),
        };
        if out.send(frame).await.is_err() {
            return;
        }
    }
    let _ =
        out.send(Outbound::Control(VoiceServerMsg::SpeakEnd { line_id: line_id.to_owned() })).await;
}

struct Engine<B: VoiceBackend> {
    backend: Arc<B>,
    prefs: SharedPrefs,
    out: mpsc::Sender<Outbound>,
    /// Unbounded: the worker must never wait on the loop while the loop
    /// waits on the worker's sentence queue.
    notice_tx: mpsc::UnboundedSender<Notice>,
    synth: Synth,
    generation: u64,
    sentences: SentenceBuffer,
    /// Streamed text of the block in progress, so its final copy is not read twice.
    delta_acc: String,
    turn_active: bool,
    speaking: bool,
    /// The rest of a barged-in turn stays silent.
    dropping: bool,
    state: Option<VoiceState>,
}

impl<B: VoiceBackend> Engine<B> {
    fn new(
        backend: Arc<B>,
        prefs: Prefs,
        out: mpsc::Sender<Outbound>,
        notice_tx: mpsc::UnboundedSender<Notice>,
    ) -> Self {
        let prefs = Arc::new(Mutex::new(prefs));
        let synth = spawn_synth(backend.clone(), prefs.clone(), out.clone(), notice_tx.clone(), 1);
        Self {
            backend,
            prefs,
            out,
            notice_tx,
            synth,
            generation: 1,
            sentences: SentenceBuffer::default(),
            delta_acc: String::new(),
            turn_active: false,
            speaking: false,
            dropping: false,
            state: None,
        }
    }

    async fn send(&self, msg: VoiceServerMsg) -> bool {
        self.out.send(Outbound::Control(msg)).await.is_ok()
    }

    async fn emit_state(&mut self) {
        let next = if self.speaking {
            VoiceState::Speaking
        } else if self.turn_active {
            VoiceState::Thinking
        } else {
            VoiceState::Listening
        };
        if self.state != Some(next) {
            self.state = Some(next);
            self.send(VoiceServerMsg::State { state: next }).await;
        }
    }

    async fn utterance(&mut self, audio: Bytes) {
        if self.speaking {
            self.barge_in().await;
        }
        self.turn_active = true;
        self.emit_state().await;
        let text = match self.backend.transcribe(audio).await {
            Ok(text) => text.trim().to_owned(),
            Err(message) => {
                self.send(VoiceServerMsg::Error { message }).await;
                String::new()
            }
        };
        if text.is_empty() {
            self.turn_active = false;
            self.emit_state().await;
            return;
        }
        self.send(VoiceServerMsg::Transcript { text: text.clone() }).await;
        let style =
            self.prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).spoken_style;
        match self.backend.send_message(spoken_prompt(&text, style)).await {
            Ok(()) => {
                self.delta_acc.clear();
                self.sentences.clear();
                self.dropping = false;
            }
            Err(message) => {
                self.send(VoiceServerMsg::Error { message }).await;
                self.turn_active = false;
            }
        }
        self.emit_state().await;
    }

    async fn barge_in(&mut self) {
        self.generation += 1;
        self.synth = spawn_synth(
            self.backend.clone(),
            self.prefs.clone(),
            self.out.clone(),
            self.notice_tx.clone(),
            self.generation,
        );
        self.sentences.clear();
        self.delta_acc.clear();
        self.speaking = false;
        self.dropping = self.turn_active;
        let cut = self.prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).barge_in;
        if self.turn_active
            && cut
            && let Err(message) = self.backend.interrupt().await
        {
            self.send(VoiceServerMsg::Error { message }).await;
        }
        self.emit_state().await;
    }

    async fn control(&mut self, msg: VoiceClientMsg) {
        match msg {
            VoiceClientMsg::Interrupt {} => self.barge_in().await,
            VoiceClientMsg::Mode { barge_in, spoken_style, voice, speed } => {
                let mut p = self.prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(b) = barge_in {
                    p.barge_in = b;
                }
                if let Some(s) = spoken_style {
                    p.spoken_style = Some(s);
                }
                if let Some(v) = voice {
                    p.voice = Some(v).filter(|v| !v.trim().is_empty());
                }
                if let Some(s) = speed {
                    p.speed = Some(s);
                }
            }
        }
    }

    async fn enqueue(&mut self, sentences: Vec<String>) {
        for s in sentences {
            if s.trim().is_empty() {
                continue;
            }
            self.speaking = true;
            if self.synth.tx.send(s).await.is_err() {
                break;
            }
        }
        self.emit_state().await;
    }

    async fn signal(&mut self, sig: Signal) {
        match sig {
            Signal::Delta(delta) => {
                if self.dropping {
                    return;
                }
                self.delta_acc.push_str(&delta);
                self.sentences.push(&delta);
                let ready = self.sentences.drain();
                self.enqueue(ready).await;
            }
            Signal::Text(content) => {
                if self.dropping {
                    return;
                }
                let streamed = std::mem::take(&mut self.delta_acc);
                let streamed = streamed.trim();
                if streamed.is_empty() {
                    self.sentences.push(&content);
                } else if let Some(rest) = content.trim().strip_prefix(streamed) {
                    self.sentences.push(rest);
                }
                let ready = self.sentences.flush();
                self.enqueue(ready).await;
            }
            Signal::TurnEnd => {
                let ready = self.sentences.flush();
                self.enqueue(ready).await;
                self.delta_acc.clear();
                self.turn_active = false;
                self.dropping = false;
                self.emit_state().await;
            }
        }
    }

    async fn notice(&mut self, n: Notice) {
        if n.generation == self.generation {
            self.speaking = n.busy;
            self.emit_state().await;
        }
    }
}

/// Runs until the browser side closes. `signals` is the session's bus as
/// the glue reads it; `out` carries control frames and audio back.
pub async fn run_voice_loop<B: VoiceBackend>(
    backend: Arc<B>,
    prefs: Prefs,
    mut inbound: mpsc::Receiver<Inbound>,
    mut signals: mpsc::Receiver<Signal>,
    out: mpsc::Sender<Outbound>,
) {
    let (notice_tx, mut notices) = mpsc::unbounded_channel();
    let mut engine = Engine::new(backend, prefs, out, notice_tx);
    engine.emit_state().await;
    loop {
        tokio::select! {
            msg = inbound.recv() => match msg {
                Some(Inbound::Audio(audio)) => engine.utterance(audio).await,
                Some(Inbound::Control(c)) => engine.control(c).await,
                None => break,
            },
            sig = signals.recv() => match sig {
                Some(sig) => engine.signal(sig).await,
                None => break,
            },
            Some(n) = notices.recv() => engine.notice(n).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[derive(Default)]
    struct Mock {
        transcript: Mutex<String>,
        sent: Mutex<Vec<String>>,
        synthesized: Mutex<Vec<String>>,
        interrupts: Mutex<u32>,
        /// Each synthesis streams forever, so a cancel is observable.
        stall: bool,
    }

    impl Mock {
        fn log<T: Clone>(m: &Mutex<T>) -> T {
            m.lock().unwrap().clone()
        }
    }

    impl VoiceBackend for Mock {
        async fn transcribe(&self, _audio: Bytes) -> Result<String, String> {
            Ok(Self::log(&self.transcript))
        }
        async fn synthesize(
            &self,
            text: &str,
            _voice: Option<&str>,
            _speed: Option<f32>,
        ) -> Result<AudioStream, String> {
            self.synthesized.lock().unwrap().push(text.to_owned());
            let bytes = Bytes::from(text.to_owned());
            if self.stall {
                let s = futures_util::stream::once(async move { Ok(bytes) })
                    .chain(futures_util::stream::pending());
                return Ok(s.boxed());
            }
            Ok(futures_util::stream::iter(vec![Ok(bytes)]).boxed())
        }
        async fn send_message(&self, text: String) -> Result<(), String> {
            self.sent.lock().unwrap().push(text);
            Ok(())
        }
        async fn interrupt(&self) -> Result<(), String> {
            *self.interrupts.lock().unwrap() += 1;
            Ok(())
        }
    }

    struct Harness {
        mock: Arc<Mock>,
        inbound: mpsc::Sender<Inbound>,
        signals: mpsc::Sender<Signal>,
        out: mpsc::Receiver<Outbound>,
        _task: tokio::task::JoinHandle<()>,
    }

    fn start(mock: Mock, prefs: Prefs) -> Harness {
        let mock = Arc::new(mock);
        let (inbound, in_rx) = mpsc::channel(8);
        let (signals, sig_rx) = mpsc::channel(8);
        let (out_tx, out) = mpsc::channel(64);
        let task = tokio::spawn(run_voice_loop(mock.clone(), prefs, in_rx, sig_rx, out_tx));
        Harness { mock, inbound, signals, out, _task: task }
    }

    impl Harness {
        /// Frames until `pred` matches one (included), or a timeout.
        async fn collect_until(&mut self, pred: impl Fn(&Outbound) -> bool) -> Vec<Outbound> {
            let mut got = Vec::new();
            let deadline = tokio::time::sleep(Duration::from_secs(2));
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    Some(f) = self.out.recv() => {
                        let done = pred(&f);
                        got.push(f);
                        if done { return got; }
                    }
                    () = &mut deadline => panic!("timed out; frames so far: {got:?}"),
                }
            }
        }

        async fn quiet(&mut self) -> Vec<Outbound> {
            let mut got = Vec::new();
            while let Ok(Some(f)) =
                tokio::time::timeout(Duration::from_millis(100), self.out.recv()).await
            {
                got.push(f);
            }
            got
        }
    }

    fn state(s: VoiceState) -> Outbound {
        Outbound::Control(VoiceServerMsg::State { state: s })
    }

    #[tokio::test]
    async fn an_utterance_is_transcribed_and_sent_in_spoken_style() {
        let mock = Mock { transcript: Mutex::new("what time is it".into()), ..Mock::default() };
        let mut h =
            start(mock, Prefs { spoken_style: Some(SpokenStyle::Brief), ..Prefs::default() });
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        let frames = h
            .collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::Transcript { .. })))
            .await;
        assert_eq!(
            frames,
            vec![
                state(VoiceState::Listening),
                state(VoiceState::Thinking),
                Outbound::Control(VoiceServerMsg::Transcript { text: "what time is it".into() }),
            ]
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].starts_with("<cctui-session-context voice=\"spoken\">"), "{}", sent[0]);
        assert!(sent[0].ends_with("\n\nwhat time is it"));
        assert!(sent[0].contains("one to three short sentences"));
    }

    #[tokio::test]
    async fn an_empty_transcript_goes_back_to_listening_without_a_message() {
        let mut h = start(Mock::default(), Prefs::default());
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        let frames = h.quiet().await;
        assert_eq!(
            frames,
            vec![
                state(VoiceState::Listening),
                state(VoiceState::Thinking),
                state(VoiceState::Listening)
            ]
        );
        assert!(Mock::log(&h.mock.sent).is_empty());
    }

    #[tokio::test]
    async fn a_reply_is_spoken_sentence_by_sentence_then_listening_resumes() {
        let mut h = start(Mock::default(), Prefs::default());
        h.signals.send(Signal::Text("**Sure.** Here is `one`.\n\n- two".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        let mut frames = h
            .collect_until(|f| {
                matches!(f, Outbound::Control(VoiceServerMsg::SpeakEnd { line_id }) if line_id == "1-3")
            })
            .await;
        frames.extend(h.quiet().await);
        assert_eq!(
            frames[1..].to_vec(),
            vec![
                state(VoiceState::Speaking),
                Outbound::Control(VoiceServerMsg::SpeakStart {
                    line_id: "1-1".into(),
                    text: "Sure.".into()
                }),
                Outbound::Audio(Bytes::from_static(b"Sure.")),
                Outbound::Control(VoiceServerMsg::SpeakEnd { line_id: "1-1".into() }),
                Outbound::Control(VoiceServerMsg::SpeakStart {
                    line_id: "1-2".into(),
                    text: "Here is one.".into()
                }),
                Outbound::Audio(Bytes::from_static(b"Here is one.")),
                Outbound::Control(VoiceServerMsg::SpeakEnd { line_id: "1-2".into() }),
                Outbound::Control(VoiceServerMsg::SpeakStart {
                    line_id: "1-3".into(),
                    text: "two".into()
                }),
                Outbound::Audio(Bytes::from_static(b"two")),
                Outbound::Control(VoiceServerMsg::SpeakEnd { line_id: "1-3".into() }),
                state(VoiceState::Listening),
            ]
        );
    }

    #[tokio::test]
    async fn streamed_deltas_start_speech_early_and_the_final_block_is_not_repeated() {
        let mut h = start(Mock::default(), Prefs::default());
        h.signals.send(Signal::Delta("First sen".into())).await.unwrap();
        h.signals.send(Signal::Delta("tence. Second".into())).await.unwrap();
        h.collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::SpeakEnd { .. }))).await;
        h.signals.send(Signal::Text("First sentence. Second half.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| {
            matches!(f, Outbound::Control(VoiceServerMsg::SpeakEnd { line_id }) if line_id == "1-2")
        })
        .await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["First sentence.", "Second half."]);
    }

    #[tokio::test]
    async fn a_block_longer_than_both_queues_is_spoken_to_the_end() {
        let mut h = start(Mock::default(), Prefs::default());
        let block = (1..=200).map(|i| format!("Line {i}.")).collect::<Vec<_>>().join(" ");
        h.signals.send(Signal::Text(block)).await.unwrap();
        h.collect_until(|f| {
            matches!(f, Outbound::Control(VoiceServerMsg::SpeakEnd { line_id }) if line_id == "1-200")
        })
        .await;
    }

    #[tokio::test]
    async fn barge_in_cancels_synthesis_and_interrupts_a_running_turn() {
        let mock = Mock { transcript: Mutex::new("go".into()), stall: true, ..Mock::default() };
        let mut h = start(mock, Prefs { barge_in: true, ..Prefs::default() });
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::Transcript { .. })))
            .await;
        h.signals.send(Signal::Text("Alpha. Beta.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Alpha."))).await;

        h.inbound.send(Inbound::Control(VoiceClientMsg::Interrupt {})).await.unwrap();
        let frames = h.quiet().await;
        assert!(frames.contains(&state(VoiceState::Thinking)), "{frames:?}");
        assert!(!frames.iter().any(|f| matches!(f, Outbound::Audio(_))), "{frames:?}");
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Alpha."], "Beta was never synthesized");
        assert_eq!(Mock::log(&h.mock.interrupts), 1);

        h.signals.send(Signal::Text("Leftover of the cut turn.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        let frames = h.quiet().await;
        assert_eq!(frames, vec![state(VoiceState::Listening)]);
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Alpha."]);
    }

    #[tokio::test]
    async fn barge_in_without_the_pref_only_stops_playback() {
        let mock = Mock { transcript: Mutex::new("go".into()), stall: true, ..Mock::default() };
        let mut h = start(mock, Prefs::default());
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::Transcript { .. })))
            .await;
        h.signals.send(Signal::Text("Alpha.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Alpha."))).await;
        h.inbound.send(Inbound::Control(VoiceClientMsg::Interrupt {})).await.unwrap();
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.interrupts), 0);
    }

    #[tokio::test]
    async fn mode_updates_apply_to_later_sentences() {
        let mut h = start(Mock::default(), Prefs::default());
        h.inbound
            .send(Inbound::Control(VoiceClientMsg::Mode {
                barge_in: None,
                spoken_style: Some(SpokenStyle::Verbose),
                voice: Some("bf_emma".into()),
                speed: Some(1.5),
            }))
            .await
            .unwrap();
        h.mock.transcript.lock().unwrap().push_str("hello");
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::Transcript { .. })))
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(Mock::log(&h.mock.sent)[0].contains("as much detail as needed"));
    }
}
