//! The voice conversation loop, independent of the socket and the speech
//! upstream: utterances in, control frames and synthesized sentences out.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use cctui_proto::voice::{SpokenStyle, VoiceClientMsg, VoiceServerMsg, VoiceState};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};
use tracing::Instrument;

use super::audio::{MAX_CHUNK_BYTES, split_utterance};
use super::mainline::{Delta, MainThread, squash};
use super::text::{SentenceBuffer, spoken_prompt};
use crate::speech::{SpeechError, SttEvent, SttStream};

pub type AudioStream = BoxStream<'static, Result<Bytes, String>>;

/// What the loop needs from the server: speech both ways and the session.
pub trait VoiceBackend: Send + Sync + 'static {
    /// `language` is the spoken language picked for the session, `None` for
    /// the instance default.
    fn transcribe(
        &self,
        audio: Bytes,
        language: Option<&str>,
    ) -> impl Future<Output = Result<String, String>> + Send;
    /// Loads the speech-to-text model `language` routes to, so the next
    /// utterance does not pay for the load.
    fn warm_up(&self, language: Option<&str>) -> impl Future<Output = Result<(), String>> + Send;
    /// Whether the model `language` routes to transcribes live.
    fn streams(&self, language: Option<&str>) -> bool;
    /// Speech the browser gathers before sending a part to a batch model;
    /// `None` keeps its default.
    fn min_part_ms(&self) -> Option<u32>;
    fn stream(
        &self,
        language: Option<&str>,
    ) -> impl Future<Output = Result<SttStream, SpeechError>> + Send;
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
    /// A whole utterance, or the last part of one: 16 kHz mono 16-bit PCM.
    Audio(Bytes),
    /// A non-final part of the utterance being spoken.
    Part(Bytes),
    /// The utterance being spoken ended with no more audio.
    UtteranceEnd,
    Control(VoiceClientMsg),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    Control(VoiceServerMsg),
    Audio(Bytes),
}

/// What the session says, as the socket glue reads it off the bus.
#[derive(Debug, PartialEq, Eq)]
pub enum Signal {
    /// A prompt reached the agent: a new turn starts.
    TurnStart(String),
    /// A model request the gateway streams, by its latest user text.
    Request {
        id: String,
        user_text: String,
    },
    /// Streamed text of the block `item`.
    Delta {
        item: String,
        text: String,
    },
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
    pub language: Option<String>,
}

type SharedPrefs = Arc<Mutex<Prefs>>;

const UTTERANCE_QUEUE: usize = 8;
/// Further attempts at a chunk whose transcription failed.
const STT_RETRIES: usize = 2;
/// Under the speech router's 90 s idle unload.
const STT_KEEPALIVE: Duration = Duration::from_mins(1);
/// A stream quiet this long speaks what it has buffered.
const PAUSE_FLUSH: Duration = Duration::from_millis(700);
/// The part cadence asked of the browser while utterances are heard live.
pub const STREAM_PART_MS: u32 = 320;
/// A live utterance whose final text takes longer is transcribed in batch.
const STREAM_FINAL_TIMEOUT: Duration = Duration::from_secs(15);

/// Milestones of the turn a spoken prompt started, for its time-to-first-audio
/// log line.
#[derive(Debug, Clone, Copy)]
struct Probe {
    sent: Instant,
    first_text: Option<(Instant, &'static str)>,
    first_sentence: Option<Instant>,
}

impl Probe {
    fn start() -> Self {
        Self { sent: Instant::now(), first_text: None, first_sentence: None }
    }
}

type SharedProbe = Arc<Mutex<Option<Probe>>>;

fn log_first_audio(p: &Probe) {
    let ms = |t: Instant| u64::try_from(t.duration_since(p.sent).as_millis()).unwrap_or(u64::MAX);
    let (first_text_ms, source) = p.first_text.map_or((None, "none"), |(t, s)| (Some(ms(t)), s));
    tracing::info!(
        ttfa_ms = ms(Instant::now()),
        first_text_ms,
        first_sentence_ms = p.first_sentence.map(ms),
        source,
        "voice: time to first audio"
    );
}

#[derive(Debug)]
struct Notice {
    generation: u64,
    busy: bool,
    /// Sentences this worker has finished.
    done: u64,
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
    probe: SharedProbe,
    out: mpsc::Sender<Outbound>,
    notices: mpsc::UnboundedSender<Notice>,
    generation: u64,
) -> Synth {
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let task = tokio::spawn(
        async move {
            let mut n = 0u64;
            while let Some(text) = rx.recv().await {
                n += 1;
                let _ = notices.send(Notice { generation, busy: true, done: n - 1 });
                let line_id = format!("{generation}-{n}");
                speak_one(&*backend, &prefs, &probe, &out, &line_id, &text).await;
                if rx.is_empty() {
                    let _ = notices.send(Notice { generation, busy: false, done: n });
                }
            }
        }
        .in_current_span(),
    );
    Synth { tx, task }
}

async fn speak_one<B: VoiceBackend>(
    backend: &B,
    prefs: &SharedPrefs,
    probe: &SharedProbe,
    out: &mpsc::Sender<Outbound>,
    line_id: &str,
    text: &str,
) {
    let (voice, speed) = {
        let p = prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (p.voice.clone(), p.speed)
    };
    let mut attempt = backend.synthesize(text, voice.as_deref(), speed).await;
    if attempt.is_err() {
        attempt = backend.synthesize(text, voice.as_deref(), speed).await;
    }
    let mut stream = match attempt {
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
    let first = probe.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(p) = first {
        log_first_audio(&p);
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

/// What became of an utterance. `Sending` comes before the prompt leaves,
/// so the loop counts it before the agent can answer.
#[derive(Debug)]
enum Heard {
    Sending(String),
    Sent,
    Failed,
    /// Nothing to send: no speech, or transcription failed.
    Idle,
    /// A warm-up finished, loaded or not.
    Warm,
    /// Relayed by the loop so it follows the state the loop emits first.
    Transcript(String),
    /// A non-final chunk was heard; the utterance goes on.
    Partial,
}

#[derive(Debug)]
enum Job {
    /// A chunk of the utterance being heard; `last` closes it.
    Hear {
        audio: Bytes,
        last: bool,
    },
    /// The utterance being heard is over with no more audio.
    End,
    Warm,
}

/// Transcribes utterances chunk by chunk, in arrival order, off the loop,
/// and sends each utterance once its last chunk is heard. It warms the model
/// before the first utterance and keeps it warm while idle. Its queue is
/// unbounded: audio is never dropped, and the loop must not wait on it while
/// it waits on the loop's `heard` queue. Dropping it aborts whatever was in
/// flight.
struct Listener {
    tx: mpsc::UnboundedSender<Job>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn spawn_listener<B: VoiceBackend>(
    backend: Arc<B>,
    prefs: SharedPrefs,
    out: mpsc::Sender<Outbound>,
    heard: mpsc::Sender<Heard>,
) -> Listener {
    let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
    let task = tokio::spawn(async move {
        let mut ear = Ear {
            backend,
            prefs,
            out,
            heard,
            pending: String::new(),
            shown: String::new(),
            open: false,
            live: None,
            refused: false,
            told: (None, None),
        };
        let mut job = Job::Warm;
        loop {
            let outcome = match job {
                Job::Hear { audio, last } => ear.hear(audio, last).await,
                Job::End => ear.end().await,
                Job::Warm => {
                    ear.refused = false;
                    ear.listen().await;
                    warm_up(&*ear.backend, &ear.prefs).await;
                    Heard::Warm
                }
            };
            if ear.heard.send(outcome).await.is_err() {
                break;
            }
            match ear.next_job(&mut rx).await {
                Some(next) => job = next,
                None => break,
            }
        }
    });
    Listener { tx, task }
}

fn language(prefs: &SharedPrefs) -> Option<String> {
    prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).language.clone()
}

async fn warm_up<B: VoiceBackend>(backend: &B, prefs: &SharedPrefs) {
    if let Err(err) = backend.warm_up(language(prefs).as_deref()).await {
        tracing::debug!(%err, "voice: speech-to-text warm-up failed");
    }
}

/// An utterance heard live, with its audio so far for the batch fallback.
struct Live {
    /// `None` once the stream failed.
    stream: Option<SttStream>,
    audio: BytesMut,
}

async fn live_event(live: &mut Option<Live>) -> Option<Result<SttEvent, SpeechError>> {
    match live.as_mut().and_then(|l| l.stream.as_mut()) {
        Some(stream) => stream.event().await,
        None => std::future::pending().await,
    }
}

/// The listener's side of one utterance: the text of its chunks heard so
/// far, sent as one message when the last chunk is in. When the model
/// streams, the utterance is heard live over one stream instead.
struct Ear<B> {
    backend: Arc<B>,
    prefs: SharedPrefs,
    out: mpsc::Sender<Outbound>,
    heard: mpsc::Sender<Heard>,
    pending: String,
    /// The transcript last relayed for the utterance.
    shown: String,
    /// An utterance is being heard.
    open: bool,
    live: Option<Live>,
    /// The service refused to stream the current language.
    refused: bool,
    /// The part cadence last asked of the browser: live and batch.
    told: (Option<u32>, Option<u32>),
}

impl<B: VoiceBackend> Ear<B> {
    /// Waits for the next job, relaying the live utterance meanwhile and
    /// warming the model every [`STT_KEEPALIVE`] of silence.
    async fn next_job(&mut self, rx: &mut mpsc::UnboundedReceiver<Job>) -> Option<Job> {
        loop {
            tokio::select! {
                next = rx.recv() => return next,
                event = live_event(&mut self.live) => self.live_event(event).await,
                () = tokio::time::sleep(STT_KEEPALIVE) => warm_up(&*self.backend, &self.prefs).await,
            }
        }
    }

    async fn hear(&mut self, audio: Bytes, last: bool) -> Heard {
        if !self.open {
            self.open = true;
            self.go_live().await;
        }
        if let Some(live) = &mut self.live {
            live.audio.extend_from_slice(&audio);
            if let Some(stream) = &live.stream {
                stream.send_audio(&audio);
            }
        } else {
            self.hear_chunk(audio).await;
        }
        if last { self.end().await } else { Heard::Partial }
    }

    async fn hear_chunk(&mut self, audio: Bytes) {
        match self.transcribe(audio).await {
            Ok(text) if !text.is_empty() => {
                if !self.pending.is_empty() {
                    self.pending.push(' ');
                }
                self.pending.push_str(&text);
                self.show(self.pending.clone()).await;
            }
            Ok(_) => {}
            Err(message) => self.error(message).await,
        }
    }

    async fn show(&mut self, text: String) {
        if text.is_empty() || text == self.shown {
            return;
        }
        self.shown.clone_from(&text);
        let _ = self.heard.send(Heard::Transcript(text)).await;
    }

    /// Opens a live stream for the utterance starting, when the model
    /// streams; a stream that fails to open leaves the utterance to batch.
    async fn go_live(&mut self) {
        self.live = None;
        let language = language(&self.prefs);
        if self.refused || !self.backend.streams(language.as_deref()) {
            return;
        }
        let stream = match self.backend.stream(language.as_deref()).await {
            Ok(stream) => Some(stream),
            Err(err) => {
                self.stream_failed(&err).await;
                None
            }
        };
        self.live = Some(Live { stream, audio: BytesMut::new() });
    }

    async fn live_event(&mut self, event: Option<Result<SttEvent, SpeechError>>) {
        match event {
            Some(Ok(SttEvent::Partial(text))) => self.show(text.trim().to_owned()).await,
            Some(Ok(SttEvent::Final(_))) => {}
            Some(Err(err)) => {
                if let Some(live) = &mut self.live {
                    live.stream = None;
                }
                self.stream_failed(&err).await;
            }
            None => {
                if let Some(live) = &mut self.live {
                    live.stream = None;
                }
            }
        }
    }

    async fn stream_failed(&mut self, err: &SpeechError) {
        tracing::debug!(%err, "voice: live transcription failed, hearing in batch");
        if err.refuses_streaming() {
            self.refused = true;
            self.listen().await;
        }
    }

    /// Tells the browser the part cadence the current model wants.
    async fn listen(&mut self) {
        let language = language(&self.prefs);
        let streams = !self.refused && self.backend.streams(language.as_deref());
        let told = (streams.then_some(STREAM_PART_MS), self.backend.min_part_ms());
        if told != self.told {
            self.told = told;
            let (part_ms, min_part_ms) = told;
            let msg = VoiceServerMsg::Listen { part_ms, min_part_ms };
            let _ = self.out.send(Outbound::Control(msg)).await;
        }
    }

    /// The utterance is over: its live final text, or its audio heard in
    /// batch when the stream failed, then the message.
    async fn end(&mut self) -> Heard {
        self.open = false;
        if let Some(live) = self.live.take() {
            match self.live_final(live.stream).await {
                Some(text) => {
                    self.pending = text;
                    self.show(self.pending.clone()).await;
                }
                None => {
                    for chunk in split_utterance(&live.audio.freeze(), MAX_CHUNK_BYTES) {
                        self.hear_chunk(chunk).await;
                    }
                }
            }
        }
        self.shown.clear();
        self.finish().await
    }

    /// Closes the live utterance and waits for its final text.
    async fn live_final(&mut self, stream: Option<SttStream>) -> Option<String> {
        let mut stream = stream?;
        stream.end();
        let deadline = tokio::time::sleep(STREAM_FINAL_TIMEOUT);
        tokio::pin!(deadline);
        loop {
            let event = tokio::select! {
                event = stream.event() => event,
                () = &mut deadline => {
                    tracing::debug!("voice: live transcription final timed out");
                    return None;
                }
            };
            match event {
                Some(Ok(SttEvent::Partial(text))) => self.show(text.trim().to_owned()).await,
                Some(Ok(SttEvent::Final(text))) => return Some(text.trim().to_owned()),
                Some(Err(err)) => {
                    self.stream_failed(&err).await;
                    return None;
                }
                None => return None,
            }
        }
    }

    /// Up to [`STT_RETRIES`] more attempts, so one failed chunk does not
    /// lose the utterance.
    async fn transcribe(&self, audio: Bytes) -> Result<String, String> {
        let language = language(&self.prefs);
        let mut attempt = 0;
        loop {
            match self.backend.transcribe(audio.clone(), language.as_deref()).await {
                Ok(text) => return Ok(text.trim().to_owned()),
                Err(message) if attempt >= STT_RETRIES => return Err(message),
                Err(err) => {
                    attempt += 1;
                    tracing::debug!(%err, attempt, "voice: transcription retried");
                }
            }
        }
    }

    async fn finish(&mut self) -> Heard {
        let text = std::mem::take(&mut self.pending);
        if text.is_empty() {
            return Heard::Idle;
        }
        let style =
            self.prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).spoken_style;
        let message = spoken_prompt(&text, style);
        let _ = self.heard.send(Heard::Sending(message.clone())).await;
        match self.backend.send_message(message).await {
            Ok(()) => Heard::Sent,
            Err(message) => {
                self.error(message).await;
                Heard::Failed
            }
        }
    }

    async fn error(&self, message: String) {
        let _ = self.out.send(Outbound::Control(VoiceServerMsg::Error { message })).await;
    }
}

/// Numbers the session's turns as the bus shows them, so a cut turn's
/// leftovers are told apart from the next turn.
///
/// A turn opens at its prompt echo or its first text and closes at its
/// `TurnEnd`; either marker alone moves on, as adapters do not all emit both.
#[derive(Debug, Default)]
struct Turns {
    current: u64,
    open: bool,
    /// Prompts sent whose turn has not opened yet.
    pending: u64,
    /// Turns below this id were cut by a barge-in.
    cut_below: u64,
}

impl Turns {
    const fn sent(&mut self) {
        self.pending += 1;
    }

    const fn unsent(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    const fn open(&mut self) {
        if !self.open {
            self.open = true;
            self.pending = self.pending.saturating_sub(1);
        }
    }

    /// A prompt echo inside an open turn starts the next one.
    const fn start(&mut self) -> u64 {
        if self.open {
            self.current += 1;
            self.open = false;
        }
        self.open();
        self.current
    }

    const fn text(&mut self) -> u64 {
        self.open();
        self.current
    }

    const fn end(&mut self) -> u64 {
        let id = self.current;
        self.current += 1;
        self.open = false;
        id
    }

    /// Cuts the open turn and every prompt already sent.
    fn cut(&mut self) {
        self.cut_below = self.current + u64::from(self.open) + self.pending;
    }

    const fn is_cut(&self, id: u64) -> bool {
        id < self.cut_below
    }
}

/// Utterances handed to the listener and not yet reported back.
#[derive(Debug, Default)]
struct Hearing {
    count: usize,
    /// The current utterance still has parts to come.
    open: bool,
}

impl Hearing {
    const fn done(&mut self) {
        self.count = self.count.saturating_sub(1);
    }
}

struct Engine<B: VoiceBackend> {
    backend: Arc<B>,
    prefs: SharedPrefs,
    out: mpsc::Sender<Outbound>,
    /// Unbounded: the worker must never wait on the loop while the loop
    /// waits on the worker's sentence queue.
    notice_tx: mpsc::UnboundedSender<Notice>,
    synth: Synth,
    /// Sentences handed to `synth`.
    queued: u64,
    listener: Listener,
    hearing: Hearing,
    generation: u64,
    probe: SharedProbe,
    sentences: SentenceBuffer,
    /// Streamed text no final block has accounted for yet, so a block is not
    /// read twice.
    streamed: String,
    /// `streamed` as the last turn end left it: claude-code's `Stop` hook can
    /// end the turn before its final block arrives.
    ended: String,
    /// Block text spoken before its deltas arrived, so they are not spoken
    /// again.
    ahead: String,
    /// The block the last delta belonged to.
    delta_item: Option<String>,
    /// When a quiet stream speaks its buffered tail.
    pause_at: Option<Instant>,
    turn_active: bool,
    speaking: bool,
    /// A warm-up is in flight.
    warming: bool,
    turns: Turns,
    main: MainThread,
    state: Option<VoiceState>,
}

impl<B: VoiceBackend> Engine<B> {
    fn new(
        backend: Arc<B>,
        prefs: Prefs,
        out: mpsc::Sender<Outbound>,
        notice_tx: mpsc::UnboundedSender<Notice>,
        heard_tx: mpsc::Sender<Heard>,
    ) -> Self {
        let prefs = Arc::new(Mutex::new(prefs));
        let probe = SharedProbe::default();
        let synth = spawn_synth(
            backend.clone(),
            prefs.clone(),
            probe.clone(),
            out.clone(),
            notice_tx.clone(),
            1,
        );
        let listener = spawn_listener(backend.clone(), prefs.clone(), out.clone(), heard_tx);
        Self {
            backend,
            prefs,
            out,
            notice_tx,
            synth,
            queued: 0,
            listener,
            hearing: Hearing::default(),
            generation: 1,
            probe,
            sentences: SentenceBuffer::default(),
            streamed: String::new(),
            ended: String::new(),
            ahead: String::new(),
            delta_item: None,
            pause_at: None,
            turn_active: false,
            speaking: false,
            warming: true,
            turns: Turns::default(),
            main: MainThread::default(),
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
        } else if self.warming {
            VoiceState::Warming
        } else if self.hearing.count > usize::from(self.hearing.open) {
            VoiceState::Thinking
        } else {
            VoiceState::Listening
        };
        if self.state != Some(next) {
            self.state = Some(next);
            self.send(VoiceServerMsg::State { state: next }).await;
        }
    }

    /// `audio` continues the utterance being heard, or opens one; `last`
    /// closes it. Audio over the chunk cap is split so none of it is lost.
    /// A reply being spoken goes on: the browser barges in with an
    /// `interrupt` when the user talks over it, and audio landing meanwhile
    /// was spoken before the reply started.
    async fn utterance(&mut self, audio: Bytes, last: bool) {
        if !self.hearing.open {
            self.hearing.count += 1;
        }
        self.hearing.open = !last;
        let chunks = split_utterance(&audio, MAX_CHUNK_BYTES);
        let count = chunks.len();
        for (i, chunk) in chunks.into_iter().enumerate() {
            let job = Job::Hear { audio: chunk, last: last && i + 1 == count };
            if self.listener.tx.send(job).is_err() {
                break;
            }
        }
        if last && count == 0 {
            let _ = self.listener.tx.send(Job::End);
        }
        self.emit_state().await;
    }

    async fn utterance_end(&mut self) {
        if !self.hearing.open {
            return;
        }
        self.hearing.open = false;
        let _ = self.listener.tx.send(Job::End);
        self.emit_state().await;
    }

    async fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Transcript(text) => {
                self.send(VoiceServerMsg::Transcript { text }).await;
                return;
            }
            Heard::Sending(message) => {
                *self.probe() = Some(Probe::start());
                self.turns.sent();
                self.turn_active = true;
                self.main.new_turn();
                let ready = self.main.prompt(&message);
                self.play(ready).await;
            }
            Heard::Sent | Heard::Idle => self.hearing.done(),
            Heard::Partial => {}
            Heard::Warm => self.warming = false,
            Heard::Failed => {
                self.hearing.done();
                self.turns.unsent();
                self.turn_active = false;
            }
        }
        self.emit_state().await;
    }

    fn probe(&self) -> std::sync::MutexGuard<'_, Option<Probe>> {
        self.probe.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn reset_stream(&mut self) {
        self.sentences.clear();
        self.streamed.clear();
        self.ended.clear();
        self.ahead.clear();
        self.delta_item = None;
        self.pause_at = None;
    }

    fn mark_text(&self, source: &'static str) {
        let mut probe = self.probe();
        if let Some(p) = probe.as_mut() {
            p.first_text.get_or_insert_with(|| (Instant::now(), source));
        }
    }

    fn mark_sentence(&self) {
        let mut probe = self.probe();
        if let Some(p) = probe.as_mut() {
            p.first_sentence.get_or_insert_with(Instant::now);
        }
    }

    async fn barge_in(&mut self) {
        self.generation += 1;
        self.synth = spawn_synth(
            self.backend.clone(),
            self.prefs.clone(),
            self.probe.clone(),
            self.out.clone(),
            self.notice_tx.clone(),
            self.generation,
        );
        self.queued = 0;
        self.reset_stream();
        *self.probe() = None;
        self.speaking = false;
        if self.turn_active {
            self.turns.cut();
        }
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
            VoiceClientMsg::Mode { barge_in, spoken_style, voice, speed, language } => {
                let rewarm = {
                    let mut p =
                        self.prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    apply_mode(&mut p, barge_in, spoken_style, voice, speed, language.as_deref())
                };
                if rewarm && self.listener.tx.send(Job::Warm).is_ok() {
                    self.warming = true;
                    self.emit_state().await;
                }
            }
            VoiceClientMsg::Part {} | VoiceClientMsg::UtteranceEnd {} => {}
        }
    }

    async fn enqueue(&mut self, sentences: Vec<String>) {
        for s in sentences {
            if s.trim().is_empty() {
                continue;
            }
            self.mark_sentence();
            self.speaking = true;
            if self.synth.tx.send(s).await.is_err() {
                break;
            }
            self.queued += 1;
        }
        self.emit_state().await;
    }

    async fn signal(&mut self, sig: Signal) {
        match sig {
            Signal::TurnStart(prompt) => {
                self.ended.clear();
                self.ahead.clear();
                let id = self.turns.start();
                if !self.turns.is_cut(id) {
                    self.turn_active = true;
                    self.emit_state().await;
                }
                let ready = self.main.prompt(&prompt);
                self.play(ready).await;
            }
            Signal::Request { id, user_text } => self.main.request(id, &user_text),
            Signal::Delta { item, text } => {
                let ready = self.main.delta(item, text);
                self.play(ready).await;
            }
            Signal::Text(content) => {
                let id = self.turns.text();
                if !self.turns.is_cut(id) {
                    self.block(&content).await;
                }
            }
            Signal::TurnEnd => self.turn_end().await,
        }
    }

    async fn play(&mut self, ready: Vec<Delta>) {
        if ready.is_empty() {
            return;
        }
        let id = self.turns.text();
        if self.turns.is_cut(id) {
            return;
        }
        for (item, text) in ready {
            self.delta(item, &text).await;
        }
    }

    async fn delta(&mut self, item: String, text: &str) {
        self.mark_text("delta");
        if self.delta_item.is_none() {
            self.ended.clear();
        } else if self.delta_item.as_ref().is_some_and(|last| *last != item) {
            self.sentences.push("\n\n");
            self.streamed.push('\n');
            let ready = self.sentences.pause();
            self.enqueue(ready).await;
        }
        self.delta_item = Some(item);
        let text = &text[spoken_ahead(&mut self.ahead, text)..];
        if text.is_empty() {
            return;
        }
        self.streamed.push_str(text);
        self.sentences.push(text);
        let ready = self.sentences.drain();
        self.enqueue(ready).await;
        self.pause_at = self.sentences.has_pending().then(|| Instant::now() + PAUSE_FLUSH);
    }

    /// A final block: spoken whole when nothing streamed, otherwise only
    /// what its deltas missed.
    async fn block(&mut self, content: &str) {
        self.mark_text("block");
        if !self.streamed.trim().is_empty() {
            let (later, missed) = reconcile(&self.streamed, content);
            self.streamed = later;
            self.sentences.push(&missed);
            self.ahead = missed;
            if !self.streamed.is_empty() {
                let ready = self.sentences.drain();
                self.enqueue(ready).await;
                return;
            }
        } else if self.ended.trim().is_empty() {
            self.sentences.push(content);
            self.ahead = squash(content);
        } else {
            let (later, missed) = reconcile(&self.ended, content);
            self.ended = later;
            self.sentences.push(&missed);
        }
        self.pause_at = None;
        let ready = self.sentences.flush();
        self.enqueue(ready).await;
    }

    async fn pause(&mut self) {
        self.pause_at = None;
        let ready = self.sentences.pause();
        self.enqueue(ready).await;
    }

    async fn turn_end(&mut self) {
        self.main.new_turn();
        let id = self.turns.end();
        if !self.streamed.trim().is_empty() {
            self.ended = std::mem::take(&mut self.streamed);
        }
        self.ahead.clear();
        self.delta_item = None;
        self.pause_at = None;
        if self.turns.is_cut(id) {
            self.turn_active = self.turns.pending > 0;
        } else {
            let ready = self.sentences.flush();
            self.enqueue(ready).await;
            self.turns.pending = 0;
            self.turn_active = false;
        }
        {
            let mut probe = self.probe();
            if probe.is_some_and(|p| p.first_sentence.is_none()) {
                *probe = None;
            }
        }
        self.emit_state().await;
    }

    async fn notice(&mut self, n: Notice) {
        if n.generation == self.generation {
            self.speaking = n.busy || n.done < self.queued;
            self.emit_state().await;
        }
    }
}

/// Splits a final block against the text streamed ahead of it: the streamed
/// text later blocks still account for, and the part of the block its
/// deltas never carried.
fn reconcile(streamed: &str, block: &str) -> (String, String) {
    let (streamed, block) = (squash(streamed), squash(block));
    if let Some(later) = streamed.strip_prefix(block.as_str()) {
        return (later.trim_start().to_owned(), String::new());
    }
    if let Some(missed) = block.strip_prefix(streamed.as_str()) {
        return (String::new(), missed.to_owned());
    }
    if block.ends_with(streamed.as_str()) {
        return (String::new(), String::new());
    }
    near_split(&streamed, &block).unwrap_or((streamed, block))
}

/// How many bytes of `text` repeat `ahead`, whitespace aside, consuming them
/// from it; a delta that departs from `ahead` empties it.
fn spoken_ahead(ahead: &mut String, text: &str) -> usize {
    if ahead.is_empty() {
        return 0;
    }
    let mut said = ahead.as_str();
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            continue;
        }
        let Some(rest) = said.trim_start().strip_prefix(c) else {
            ahead.clear();
            return i;
        };
        said = rest;
    }
    *ahead = said.to_owned();
    text.len()
}

/// [`reconcile`] for a block that differs from its streamed text by a few
/// characters, which must not be read out twice.
fn near_split(streamed: &str, block: &str) -> Option<(String, String)> {
    let streamed: Vec<char> = streamed.chars().collect();
    let block: Vec<char> = block.chars().collect();
    let len = streamed.len().min(block.len());
    if len < 20 {
        return None;
    }
    let (ours, theirs) = (&streamed[..len], &block[..len]);
    let same = |(a, b): &(&char, &char)| a == b;
    let prefix = ours.iter().zip(theirs).take_while(same).count();
    let suffix = ours.iter().rev().zip(theirs.iter().rev()).take_while(same).count();
    if (prefix + suffix) * 10 < len * 9 {
        return None;
    }
    let rest = |chars: &[char]| chars[len..].iter().collect::<String>().trim_start().to_owned();
    Some((rest(&streamed), rest(&block)))
}

/// Applies a `mode` frame; true when the spoken language changed.
fn apply_mode(
    p: &mut Prefs,
    barge_in: Option<bool>,
    spoken_style: Option<SpokenStyle>,
    voice: Option<String>,
    speed: Option<f32>,
    language: Option<&str>,
) -> bool {
    if let Some(b) = barge_in {
        p.barge_in = b;
    }
    if let Some(s) = spoken_style {
        p.spoken_style = Some(s);
    }
    if let Some(v) = voice {
        p.voice = Some(v).filter(|v| !v.trim().is_empty());
    }
    if let Some(s) = speed.filter(|s| (0.25..=4.0).contains(s)) {
        p.speed = Some(s);
    }
    let Some(lang) = language else { return false };
    let lang = picked_language(lang);
    let changed = p.language != lang;
    p.language = lang;
    changed
}

/// A language pick as the speech service takes it; `None` for auto.
pub fn picked_language(raw: &str) -> Option<String> {
    Some(raw.trim().to_ascii_lowercase()).filter(|l| !l.is_empty() && l != "auto")
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
    let (heard_tx, mut heard) = mpsc::channel(UTTERANCE_QUEUE);
    let mut engine = Engine::new(backend, prefs, out, notice_tx, heard_tx);
    engine.emit_state().await;
    loop {
        let pause = engine.pause_at;
        tokio::select! {
            msg = inbound.recv() => match msg {
                Some(Inbound::Audio(audio)) => engine.utterance(audio, true).await,
                Some(Inbound::Part(audio)) => engine.utterance(audio, false).await,
                Some(Inbound::UtteranceEnd) => engine.utterance_end().await,
                Some(Inbound::Control(c)) => engine.control(c).await,
                None => break,
            },
            sig = signals.recv() => match sig {
                Some(sig) => engine.signal(sig).await,
                None => break,
            },
            Some(n) = notices.recv() => engine.notice(n).await,
            Some(h) = heard.recv() => engine.heard(h).await,
            () = sleep_until(pause.unwrap_or_else(Instant::now)), if pause.is_some() => {
                engine.pause().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::SttInput;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Semaphore;

    #[derive(Default)]
    #[allow(clippy::struct_excessive_bools)]
    struct Mock {
        transcript: Mutex<String>,
        sent: Mutex<Vec<String>>,
        synthesized: Mutex<Vec<String>>,
        speeds: Mutex<Vec<Option<f32>>>,
        interrupts: Mutex<u32>,
        /// Each synthesis streams forever, so a cancel is observable.
        stall: bool,
        /// Each transcription waits for a permit.
        stt_gate: Option<Arc<Semaphore>>,
        send_gate: Option<Arc<Semaphore>>,
        /// The transcript is the utterance's bytes.
        echo: bool,
        stt_dropped: Arc<AtomicBool>,
        /// Each warm-up waits for a permit.
        warm_gate: Option<Arc<Semaphore>>,
        /// `warm <lang>` and `hear <lang>` in call order.
        stt_calls: Mutex<Vec<String>>,
        /// Syntheses left to fail.
        synth_failures: Mutex<u32>,
        /// Transcriptions left to fail.
        stt_failures: Mutex<u32>,
        /// Each transcript is `p<n>`, numbered in call order.
        numbered: bool,
        heard_n: AtomicUsize,
        /// Utterances are heard live: each partial is the parts so far, as
        /// text, joined by spaces.
        streaming: bool,
        min_part_ms: Option<u32>,
        /// Streams left to fail to open.
        stream_open_failures: Mutex<u32>,
        /// Close codes the next streams fail with at their first audio.
        stream_closes: Mutex<Vec<u16>>,
        /// Streams their engine closed.
        streams_closed: Arc<AtomicUsize>,
    }

    /// Flags a transcription dropped before it finished.
    struct Unfinished(Option<Arc<AtomicBool>>);

    impl Drop for Unfinished {
        fn drop(&mut self) {
            if let Some(flag) = self.0.take() {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }

    impl Mock {
        fn log<T: Clone>(m: &Mutex<T>) -> T {
            m.lock().unwrap().clone()
        }

        fn stt(&self, what: &str, language: Option<&str>) {
            self.stt_calls.lock().unwrap().push(format!("{what} {}", language.unwrap_or("-")));
        }

        fn warms(&self) -> usize {
            Self::log(&self.stt_calls).iter().filter(|c| c.starts_with("warm")).count()
        }

        fn calls(&self, what: &str) -> usize {
            Self::log(&self.stt_calls).iter().filter(|c| c.starts_with(what)).count()
        }

        fn live() -> Self {
            Self { streaming: true, echo: true, ..Self::default() }
        }
    }

    impl VoiceBackend for Mock {
        async fn transcribe(&self, audio: Bytes, language: Option<&str>) -> Result<String, String> {
            self.stt("hear", language);
            let mut unfinished = Unfinished(Some(self.stt_dropped.clone()));
            if let Some(gate) = &self.stt_gate {
                gate.acquire().await.unwrap().forget();
            }
            unfinished.0 = None;
            let failed = {
                let mut failures = self.stt_failures.lock().unwrap();
                let failed = *failures > 0;
                if failed {
                    *failures -= 1;
                }
                failed
            };
            if failed {
                return Err("speech service unreachable".into());
            }
            if self.numbered {
                return Ok(format!("p{}", self.heard_n.fetch_add(1, Ordering::SeqCst) + 1));
            }
            if self.echo {
                return Ok(String::from_utf8_lossy(&audio).into_owned());
            }
            Ok(Self::log(&self.transcript))
        }
        async fn warm_up(&self, language: Option<&str>) -> Result<(), String> {
            self.stt("warm", language);
            if let Some(gate) = &self.warm_gate {
                gate.acquire().await.unwrap().forget();
            }
            Ok(())
        }
        fn streams(&self, _language: Option<&str>) -> bool {
            self.streaming
        }
        fn min_part_ms(&self) -> Option<u32> {
            self.min_part_ms
        }
        async fn stream(&self, language: Option<&str>) -> Result<SttStream, SpeechError> {
            self.stt("stream", language);
            let failed = {
                let mut failures = self.stream_open_failures.lock().unwrap();
                let failed = *failures > 0;
                if failed {
                    *failures -= 1;
                }
                failed
            };
            if failed {
                return Err(SpeechError::Transport("connection refused".into()));
            }
            let close = {
                let mut closes = self.stream_closes.lock().unwrap();
                (!closes.is_empty()).then(|| closes.remove(0))
            };
            let (stream, mut input, events) = SttStream::channel();
            let closed = self.streams_closed.clone();
            tokio::spawn(async move {
                let mut parts: Vec<String> = Vec::new();
                while let Some(cmd) = input.recv().await {
                    let event = match cmd {
                        SttInput::Audio(_) if let Some(code) = close => {
                            let detail = "nope".to_owned();
                            let _ = events.send(Err(SpeechError::StreamClosed { code, detail }));
                            return;
                        }
                        SttInput::Audio(pcm) => {
                            parts.push(String::from_utf8_lossy(&pcm).into_owned());
                            SttEvent::Partial(parts.join(" "))
                        }
                        SttInput::End => SttEvent::Final(std::mem::take(&mut parts).join(" ")),
                    };
                    let _ = events.send(Ok(event));
                }
                closed.fetch_add(1, Ordering::SeqCst);
            });
            Ok(stream)
        }
        async fn synthesize(
            &self,
            text: &str,
            _voice: Option<&str>,
            speed: Option<f32>,
        ) -> Result<AudioStream, String> {
            let mut failures = self.synth_failures.lock().unwrap();
            let failed = *failures > 0;
            if failed {
                *failures -= 1;
            }
            drop(failures);
            if failed {
                return Err("speech service unreachable".into());
            }
            self.synthesized.lock().unwrap().push(text.to_owned());
            self.speeds.lock().unwrap().push(speed);
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
            if let Some(gate) = &self.send_gate {
                gate.acquire().await.unwrap().forget();
            }
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

    fn start_cold(mock: Mock, prefs: Prefs) -> Harness {
        let mock = Arc::new(mock);
        let (inbound, in_rx) = mpsc::channel(8);
        let (signals, sig_rx) = mpsc::channel(8);
        let (out_tx, out) = mpsc::channel(64);
        let task = tokio::spawn(run_voice_loop(mock.clone(), prefs, in_rx, sig_rx, out_tx));
        Harness { mock, inbound, signals, out, _task: task }
    }

    /// A loop past its opening warm-up.
    async fn start(mock: Mock, prefs: Prefs) -> Harness {
        let mut h = start_cold(mock, prefs);
        let frames = h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(frames, vec![state(VoiceState::Warming), state(VoiceState::Listening)]);
        h
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

    fn delta(item: &str, text: &str) -> Signal {
        Signal::Delta { item: item.into(), text: text.into() }
    }

    fn turn_start() -> Signal {
        Signal::TurnStart(String::new())
    }

    fn request(id: &str, user_text: &str) -> Signal {
        Signal::Request { id: id.into(), user_text: user_text.into() }
    }

    /// A loop that sent one spoken prompt, and that prompt as it left.
    async fn sent_a_prompt() -> (Harness, String) {
        let mock = Mock { transcript: Mutex::new("what time is it".into()), ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(is_transcript).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let sent = Mock::log(&h.mock.sent).remove(0);
        (h, sent)
    }

    fn is_speak_start(f: &Outbound) -> bool {
        matches!(f, Outbound::Control(VoiceServerMsg::SpeakStart { .. }))
    }

    fn is_transcript(f: &Outbound) -> bool {
        matches!(f, Outbound::Control(VoiceServerMsg::Transcript { .. }))
    }

    /// A reply being spoken, then a new utterance barging in on it, as the
    /// browser does it: an `interrupt` at speech start, the audio after.
    async fn cut_a_turn_with_a_new_utterance() -> Harness {
        let mock = Mock { transcript: Mutex::new("go".into()), stall: true, ..Mock::default() };
        let mut h = start(mock, Prefs { barge_in: true, ..Prefs::default() }).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(is_transcript).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.signals.send(turn_start()).await.unwrap();
        h.signals.send(Signal::Text("Alpha.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Alpha."))).await;
        h.inbound.send(Inbound::Control(VoiceClientMsg::Interrupt {})).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(is_transcript).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(Mock::log(&h.mock.interrupts), 1);
        assert_eq!(Mock::log(&h.mock.sent).len(), 2);
        h
    }

    #[tokio::test]
    async fn a_cut_turns_leftovers_never_play_in_the_next_turn() {
        let mut h = cut_a_turn_with_a_new_utterance().await;
        h.signals.send(delta("m:0", "Late del")).await.unwrap();
        h.signals.send(Signal::Text("Late delta. Leftover.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        let frames = h.quiet().await;
        assert!(!frames.contains(&state(VoiceState::Listening)), "{frames:?}");
        h.signals.send(turn_start()).await.unwrap();
        h.signals.send(Signal::Text("Fresh.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Fresh."))).await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Alpha.", "Fresh."]);
    }

    #[tokio::test]
    async fn a_cut_turn_without_a_turn_end_does_not_silence_the_next_one() {
        let mut h = cut_a_turn_with_a_new_utterance().await;
        h.signals.send(Signal::Text("Leftover.".into())).await.unwrap();
        h.signals.send(turn_start()).await.unwrap();
        h.signals.send(Signal::Text("Fresh.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Fresh."))).await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Alpha.", "Fresh."]);
    }

    #[tokio::test]
    async fn a_cut_turn_followed_by_a_turn_without_a_prompt_echo_is_heard() {
        let mut h = cut_a_turn_with_a_new_utterance().await;
        h.signals.send(Signal::Text("Leftover.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.signals.send(Signal::Text("Fresh.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Fresh."))).await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Alpha.", "Fresh."]);
    }

    const PEER_REPLY: &str = "Nothing for you here. The long-voice fix is being built in another \
                              lane. It should land tonight.";
    /// [`PEER_REPLY`] as spoken: the short opening waits for the next
    /// sentence, the last one is flushed at the turn end.
    const PEER_SPOKEN: [&str; 2] = [
        "Nothing for you here. The long-voice fix is being built in another lane.",
        "It should land tonight.",
    ];

    /// [`PEER_REPLY`] streamed in awkward splits: mid-word and mid-sentence.
    async fn stream_peer_reply(h: &Harness, item: &str) {
        let parts = [
            "Noth",
            "ing for you he",
            "re. The long-voice fix is be",
            "ing built in another lane. It sho",
            "uld land tonight.",
        ];
        for part in parts {
            h.signals.send(delta(item, part)).await.unwrap();
        }
        h.signals.send(Signal::Text(PEER_REPLY.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
    }

    const NOON: &str = "It is noon, give or take a few minutes.";

    /// A spoken turn, replied to and ended.
    async fn spoken_turn() -> Harness {
        let (mut h, sent) = sent_a_prompt().await;
        h.signals.send(Signal::TurnStart(sent)).await.unwrap();
        h.signals.send(delta("m:0", NOON)).await.unwrap();
        h.signals.send(Signal::Text(NOON.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h
    }

    #[tokio::test]
    async fn a_reply_opening_with_a_short_sentence_is_spoken_in_full_from_awkward_deltas() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        stream_peer_reply(&h, "m:0").await;
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.synthesized), PEER_SPOKEN);
    }

    #[tokio::test]
    async fn a_peer_prompted_reply_after_a_spoken_turn_is_spoken_in_full() {
        let mut h = spoken_turn().await;
        h.signals.send(Signal::TurnStart("peer: anything for me?".into())).await.unwrap();
        stream_peer_reply(&h, "m:1").await;
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        let expected = [&[NOON][..], &PEER_SPOKEN[..]].concat();
        assert_eq!(Mock::log(&h.mock.synthesized), expected);
    }

    #[tokio::test]
    async fn a_reply_without_a_prompt_echo_after_a_spoken_turn_is_spoken_in_full() {
        let mut h = spoken_turn().await;
        stream_peer_reply(&h, "m:1").await;
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        let expected = [&[NOON][..], &PEER_SPOKEN[..]].concat();
        assert_eq!(Mock::log(&h.mock.synthesized), expected);
    }

    #[tokio::test]
    async fn an_utterance_landing_mid_reply_keeps_the_replys_pending_text() {
        let mock = Mock { transcript: Mutex::new("go".into()), ..Mock::default() };
        let mut h = start(mock, Prefs { barge_in: true, ..Prefs::default() }).await;
        let opening = "Nothing for you here. The long-voice fix is be";
        h.signals.send(delta("m:0", opening)).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(is_transcript).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(Mock::log(&h.mock.sent).len(), 1);
        assert_eq!(Mock::log(&h.mock.interrupts), 0);
        let rest = "ing built in another lane. It should land tonight.";
        h.signals.send(delta("m:0", rest)).await.unwrap();
        h.signals.send(Signal::Text(PEER_REPLY.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(PEER_SPOKEN[1].as_bytes())))
            .await;
        assert_eq!(Mock::log(&h.mock.synthesized), PEER_SPOKEN);
    }

    #[tokio::test]
    async fn an_utterance_landing_while_a_reply_is_spoken_does_not_cut_it() {
        let mock = Mock { transcript: Mutex::new("go".into()), stall: true, ..Mock::default() };
        let mut h = start(mock, Prefs { barge_in: true, ..Prefs::default() }).await;
        let reply = "Nothing for you here. The long-voice fix is built.";
        h.signals.send(Signal::Text(reply.into())).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Speaking)).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        let mut frames = h.collect_until(is_transcript).await;
        frames.extend(h.quiet().await);
        assert!(!frames.contains(&state(VoiceState::Thinking)), "the reply was cut: {frames:?}");
        assert_eq!(Mock::log(&h.mock.interrupts), 0);
        assert_eq!(Mock::log(&h.mock.sent).len(), 1);
    }

    #[tokio::test]
    async fn a_barge_in_before_the_prompt_echo_cuts_that_prompts_turn() {
        let mock = Mock { transcript: Mutex::new("go".into()), ..Mock::default() };
        let mut h = start(mock, Prefs { barge_in: true, ..Prefs::default() }).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(is_transcript).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.inbound.send(Inbound::Control(VoiceClientMsg::Interrupt {})).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.signals.send(turn_start()).await.unwrap();
        h.signals.send(Signal::Text("Cut.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        let frames = h.quiet().await;
        assert_eq!(frames.last(), Some(&state(VoiceState::Listening)), "{frames:?}");
        h.signals.send(turn_start()).await.unwrap();
        h.signals.send(Signal::Text("Next.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Next."))).await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Next."]);
    }

    #[tokio::test]
    async fn a_reply_racing_ahead_of_the_send_is_counted_once() {
        let gate = Arc::new(Semaphore::new(0));
        let mock = Mock {
            transcript: Mutex::new("go".into()),
            stall: true,
            send_gate: Some(gate.clone()),
            ..Mock::default()
        };
        let mut h = start(mock, Prefs { barge_in: true, ..Prefs::default() }).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(is_transcript).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.signals.send(turn_start()).await.unwrap();
        h.signals.send(Signal::Text("Quick.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Quick."))).await;
        gate.add_permits(1);
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.inbound.send(Inbound::Control(VoiceClientMsg::Interrupt {})).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Thinking)).await;
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.signals.send(turn_start()).await.unwrap();
        h.signals.send(Signal::Text("Next.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Next."))).await;
    }

    #[tokio::test]
    async fn a_slow_transcription_does_not_block_signals_or_a_barge_in() {
        let gate = Arc::new(Semaphore::new(0));
        let mock = Mock {
            transcript: Mutex::new("go".into()),
            stall: true,
            stt_gate: Some(gate.clone()),
            ..Mock::default()
        };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Thinking)).await;
        h.signals.send(Signal::Text("Alpha.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Alpha."))).await;
        h.inbound.send(Inbound::Control(VoiceClientMsg::Interrupt {})).await.unwrap();
        let frames = h.collect_until(|f| *f == state(VoiceState::Thinking)).await;
        assert!(!frames.iter().any(is_transcript), "{frames:?}");
        gate.add_permits(1);
        h.collect_until(is_transcript).await;
    }

    #[tokio::test]
    async fn utterances_are_sent_in_the_order_they_were_spoken() {
        let gate = Arc::new(Semaphore::new(0));
        let mock = Mock { echo: true, stt_gate: Some(gate.clone()), ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"one"))).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"two"))).await.unwrap();
        gate.add_permits(2);
        h.collect_until(|f| {
            matches!(f, Outbound::Control(VoiceServerMsg::Transcript { text }) if text == "two")
        })
        .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 2);
        assert!(sent[0].ends_with("one") && sent[1].ends_with("two"), "{sent:?}");
    }

    #[tokio::test]
    async fn closing_the_socket_aborts_an_in_flight_transcription() {
        let mock = Mock { stt_gate: Some(Arc::new(Semaphore::new(0))), ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Thinking)).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!h.mock.stt_dropped.load(Ordering::SeqCst));
        let Harness { mock, inbound, _task: task, .. } = h;
        drop(inbound);
        tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !mock.stt_dropped.load(Ordering::SeqCst) {
            assert!(tokio::time::Instant::now() < deadline, "transcription still running");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(Mock::log(&mock.sent).is_empty());
    }

    #[tokio::test]
    async fn an_utterance_is_transcribed_and_sent_in_spoken_style() {
        let mock = Mock { transcript: Mutex::new("what time is it".into()), ..Mock::default() };
        let mut h =
            start(mock, Prefs { spoken_style: Some(SpokenStyle::Brief), ..Prefs::default() }).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        let frames = h
            .collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::Transcript { .. })))
            .await;
        assert_eq!(
            frames,
            vec![
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
        let mut h = start(Mock::default(), Prefs::default()).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        let frames = h.quiet().await;
        assert_eq!(frames, vec![state(VoiceState::Thinking), state(VoiceState::Listening)]);
        assert!(Mock::log(&h.mock.sent).is_empty());
    }

    #[tokio::test]
    async fn a_reply_is_spoken_sentence_by_sentence_then_listening_resumes() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        h.signals.send(Signal::Text("**Sure.** Here is `one`.\n\n- two".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        let mut frames = h
            .collect_until(|f| {
                matches!(f, Outbound::Control(VoiceServerMsg::SpeakEnd { line_id }) if line_id == "1-3")
            })
            .await;
        frames.extend(h.quiet().await);
        assert_eq!(
            frames,
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
    async fn a_drained_notice_keeps_speaking_while_a_later_sentence_is_queued() {
        let (out_tx, mut out) = mpsc::channel(64);
        let (notice_tx, _notices) = mpsc::unbounded_channel();
        let (heard_tx, _heard) = mpsc::channel(UTTERANCE_QUEUE);
        let mock = Arc::new(Mock { stall: true, ..Mock::default() });
        let mut engine = Engine::new(mock, Prefs::default(), out_tx, notice_tx, heard_tx);
        engine.warming = false;
        engine.enqueue(vec!["One.".into()]).await;
        engine.enqueue(vec!["Two.".into()]).await;
        assert_eq!(engine.queued, 2);
        engine.notice(Notice { generation: 1, busy: false, done: 1 }).await;
        assert!(engine.speaking);
        assert_eq!(engine.state, Some(VoiceState::Speaking));
        engine.notice(Notice { generation: 1, busy: false, done: 2 }).await;
        assert!(!engine.speaking);
        let mut states = Vec::new();
        while let Ok(f) = out.try_recv() {
            if let Outbound::Control(VoiceServerMsg::State { state }) = f {
                states.push(state);
            }
        }
        assert_eq!(states, vec![VoiceState::Speaking, VoiceState::Listening]);
        engine.enqueue(vec!["Three.".into()]).await;
        engine.barge_in().await;
        assert_eq!(engine.queued, 0);
        engine.notice(Notice { generation: 1, busy: true, done: 0 }).await;
        assert!(!engine.speaking);
    }

    #[tokio::test]
    async fn streamed_deltas_start_speech_early_and_the_final_block_is_not_repeated() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        h.signals.send(delta("m:0", "The first sentence is long enough to sp")).await.unwrap();
        h.signals.send(delta("m:0", "eak. Second")).await.unwrap();
        h.collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::SpeakEnd { .. }))).await;
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        let block = "The first sentence is long enough to speak. Second half.";
        h.signals.send(Signal::Text(block.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::SpeakEnd { .. }))).await;
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(
            Mock::log(&h.mock.synthesized),
            vec!["The first sentence is long enough to speak.", "Second half."]
        );
    }

    #[tokio::test]
    async fn a_late_final_block_is_not_spoken_again_over_the_next_blocks_deltas() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        let first = "The first block opens with a long sentence. Its tail";
        h.signals.send(delta("a", first)).await.unwrap();
        let opening = "The second block starts streaming right away. More";
        h.signals.send(delta("b", opening)).await.unwrap();
        h.signals.send(Signal::Text(first.into())).await.unwrap();
        h.signals.send(delta("b", " to come.")).await.unwrap();
        let second = "The second block starts streaming right away. More to come.";
        h.signals.send(Signal::Text(second.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(
            Mock::log(&h.mock.synthesized),
            vec![
                "The first block opens with a long sentence.",
                "Its tail",
                "The second block starts streaming right away.",
                "More to come."
            ]
        );
    }

    #[tokio::test]
    async fn deltas_that_miss_a_blocks_end_speak_only_the_missing_part() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        let streamed = "The reply starts streaming with this sentence. And then";
        h.signals.send(delta("m:0", streamed)).await.unwrap();
        let block = format!("{streamed} it ends here.");
        h.signals.send(Signal::Text(block)).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(
            Mock::log(&h.mock.synthesized),
            vec!["The reply starts streaming with this sentence.", "And then it ends here."]
        );
    }

    #[tokio::test]
    async fn the_main_requests_gateway_text_is_spoken_before_its_block_and_once() {
        let (mut h, sent) = sent_a_prompt().await;
        let reminded = format!("<system-reminder>r</system-reminder>\n{sent}");
        h.signals.send(request("main", &reminded)).await.unwrap();
        h.signals.send(request("task", "Look up what time is it")).await.unwrap();
        let aside = "A subagent sentence nobody should ever hear. ";
        h.signals.send(delta("gw:task:0", aside)).await.unwrap();
        let opening = "The first sentence is long enough to sp";
        h.signals.send(delta("gw:main:1", opening)).await.unwrap();
        h.signals.send(delta("gw:main:1", "eak. Second")).await.unwrap();
        let first = Bytes::from_static(b"The first sentence is long enough to speak.");
        h.collect_until(|f| *f == Outbound::Audio(first.clone())).await;
        h.signals.send(Signal::TurnStart(format!("{sent}\n"))).await.unwrap();
        let block = "The first sentence is long enough to speak. Second half.";
        h.signals.send(Signal::Text(block.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(
            Mock::log(&h.mock.synthesized),
            vec!["The first sentence is long enough to speak.", "Second half."]
        );
    }

    #[tokio::test]
    async fn a_claude_code_turn_ends_on_its_first_end_marker_and_the_second_is_harmless() {
        let (mut h, sent) = sent_a_prompt().await;
        h.signals.send(Signal::TurnStart(sent.clone())).await.unwrap();
        h.signals.send(Signal::Text("Looking it up.".into())).await.unwrap();
        let frames = h.collect_until(|f| *f == state(VoiceState::Thinking)).await;
        assert!(frames.contains(&state(VoiceState::Speaking)), "{frames:?}");
        h.signals.send(Signal::Text("It is noon.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.signals.send(Signal::TurnStart(sent)).await.unwrap();
        h.signals.send(Signal::Text("Again.".into())).await.unwrap();
        let mut frames = h.collect_until(|f| *f == state(VoiceState::Thinking)).await;
        frames.extend(h.quiet().await);
        assert!(!frames.contains(&state(VoiceState::Listening)), "{frames:?}");
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Looking it up.", "It is noon.", "Again."]);
    }

    #[tokio::test]
    async fn each_request_of_a_tool_loop_is_spoken_once() {
        let (mut h, sent) = sent_a_prompt().await;
        h.signals.send(request("r1", &sent)).await.unwrap();
        let first = "The first block opens with a long sentence. Its tail";
        h.signals.send(delta("gw:r1:0", first)).await.unwrap();
        h.signals.send(Signal::Text(first.into())).await.unwrap();
        h.signals.send(request("r2", &sent)).await.unwrap();
        let second = "The second block starts streaming right away. More to come.";
        h.signals.send(delta("gw:r2:0", second)).await.unwrap();
        h.signals.send(Signal::Text(second.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(
            Mock::log(&h.mock.synthesized),
            vec![
                "The first block opens with a long sentence.",
                "Its tail",
                "The second block starts streaming right away.",
                "More to come."
            ]
        );
    }

    #[tokio::test]
    async fn gateway_text_of_an_unclaimed_request_is_never_spoken() {
        let (mut h, _) = sent_a_prompt().await;
        h.signals.send(request("task", "Summarize the repository layout")).await.unwrap();
        let aside = "The subagent reports a long finding here.";
        h.signals.send(delta("gw:task:0", aside)).await.unwrap();
        h.signals.send(Signal::Text("The reply itself.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["The reply itself."]);
    }

    const SHORT: &str =
        "Thanks. I'll keep doing that: drafts stay here until you explicitly ask me to post.";
    const SHORT_SPOKEN: [&str; 2] =
        ["Thanks.", "I'll keep doing that: drafts stay here until you explicitly ask me to post."];

    async fn stream_short(h: &Harness, upto: &str) {
        for part in ["Thanks", ". I'll keep doing that", ": drafts stay here until you"] {
            h.signals.send(delta("m:0", part)).await.unwrap();
        }
        h.signals.send(delta("m:0", upto)).await.unwrap();
    }

    #[tokio::test]
    async fn a_short_streamed_reply_then_its_block_is_spoken_once() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        stream_short(&h, " explicitly ask me to post.").await;
        h.signals.send(Signal::Text(SHORT.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.synthesized), SHORT_SPOKEN);
    }

    #[tokio::test]
    async fn a_turn_end_ahead_of_its_final_block_does_not_speak_it_again() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        stream_short(&h, " explicitly ask me to post.").await;
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.signals.send(Signal::Text(SHORT.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.synthesized), SHORT_SPOKEN);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pause_flushed_short_reply_ended_ahead_of_its_block_is_spoken_once() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        stream_short(&h, " explicitly ask me to post.").await;
        let tail = Bytes::from_static(SHORT_SPOKEN[1].as_bytes());
        h.collect_until(|f| *f == Outbound::Audio(tail.clone())).await;
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.signals.send(Signal::Text(SHORT.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.synthesized), SHORT_SPOKEN);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pause_in_the_middle_of_a_short_reply_does_not_repeat_it() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        h.signals.send(delta("m:0", "Thanks. I'll keep doing that")).await.unwrap();
        let first = Bytes::from_static(b"Thanks.");
        h.collect_until(|f| *f == Outbound::Audio(first.clone())).await;
        let rest = ": drafts stay here until you explicitly ask me to post.";
        h.signals.send(delta("m:0", rest)).await.unwrap();
        h.signals.send(Signal::Text(SHORT.into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.synthesized), SHORT_SPOKEN);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pause_inside_a_word_holds_it_until_the_word_ends() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        h.signals.send(delta("m:0", "Bl")).await.unwrap();
        tokio::time::sleep(PAUSE_FLUSH * 2).await;
        assert!(Mock::log(&h.mock.synthesized).is_empty(), "nothing spoken mid-word");
        h.signals.send(delta("m:0", "ue.")).await.unwrap();
        tokio::time::sleep(PAUSE_FLUSH * 2).await;
        h.signals.send(Signal::Text("Blue.".into())).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Blue."]);
    }

    #[tokio::test]
    async fn a_block_ahead_of_its_last_delta_is_spoken_once() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        stream_short(&h, " explicitly ask me").await;
        h.signals.send(Signal::Text(SHORT.into())).await.unwrap();
        h.signals.send(delta("m:0", " to post.")).await.unwrap();
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.synthesized), SHORT_SPOKEN);
    }

    #[test]
    fn a_delta_repeating_spoken_block_text_is_skipped_until_it_departs() {
        let mut ahead = "ask me to post. Then more".to_owned();
        assert_eq!(spoken_ahead(&mut ahead, " ask me to"), " ask me to".len());
        assert_eq!(ahead, " post. Then more");
        assert_eq!(spoken_ahead(&mut ahead, " post. Than"), " post. Th".len());
        assert!(ahead.is_empty());
        assert_eq!(spoken_ahead(&mut ahead, "anything"), 0);
    }

    #[test]
    fn a_block_a_few_characters_off_its_stream_is_not_spoken_again() {
        let streamed = "It’s a long opening sentence that keeps going. And";
        let block = "It's a long opening sentence that keeps going. And the end.";
        assert_eq!(reconcile(streamed, block), (String::new(), "the end.".to_owned()));
        let whole = "It's a long opening sentence that keeps going. And";
        assert_eq!(reconcile(streamed, whole), (String::new(), String::new()));
        let other = "Something else entirely was written in this block.";
        assert_eq!(reconcile(streamed, other), (streamed.to_owned(), other.to_owned()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_quiet_stream_speaks_its_finished_sentences_after_a_pause() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        let t0 = Instant::now();
        h.signals.send(delta("m:0", "Sure. Let me")).await.unwrap();
        tokio::time::sleep(PAUSE_FLUSH.checked_sub(Duration::from_millis(100)).unwrap()).await;
        assert!(h.out.try_recv().is_err(), "nothing spoken before the pause");
        h.collect_until(is_speak_start).await;
        assert!(t0.elapsed() >= PAUSE_FLUSH, "{:?}", t0.elapsed());
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Sure."))).await;
        h.quiet().await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Sure."]);
        h.signals.send(Signal::TurnEnd).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Let me"))).await;
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Sure.", "Let me"]);
    }

    const REPLY: &str = "The sky looks blue because air scatters short waves. Sunlight holds \
        every colour, but blue light bounces off gas molecules the most. That scattered blue \
        reaches your eyes from every direction. At sunset the light crosses more air, so the \
        blue is scattered away. What is left is the red and orange you see.";

    /// Time to the first `speak_start` and to the end of the reply, for a reply
    /// arriving 8 characters every 20 ms, streamed as deltas or as one block.
    async fn first_audio(stream: bool) -> (Duration, Duration) {
        let mut h = start(Mock::default(), Prefs::default()).await;
        let signals = h.signals.clone();
        let t0 = Instant::now();
        let feeder = tokio::spawn(async move {
            for chunk in REPLY.as_bytes().chunks(8) {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if stream {
                    let text = std::str::from_utf8(chunk).unwrap();
                    signals.send(delta("m:0", text)).await.unwrap();
                }
            }
            let done = t0.elapsed();
            signals.send(Signal::Text(REPLY.into())).await.unwrap();
            signals.send(Signal::TurnEnd).await.unwrap();
            done
        });
        h.collect_until(is_speak_start).await;
        let first = t0.elapsed();
        let done = feeder.await.unwrap();
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        let spoken = Mock::log(&h.mock.synthesized).concat().replace(' ', "");
        assert_eq!(spoken, REPLY.replace(' ', ""));
        (first, done)
    }

    #[tokio::test(start_paused = true)]
    async fn a_streamed_reply_reaches_first_audio_at_its_first_sentence_not_its_end() {
        let (streamed, done) = first_audio(true).await;
        let (block, _) = first_audio(false).await;
        eprintln!("first audio: streamed {streamed:?}, block {block:?}, reply done {done:?}");
        assert_eq!(streamed, Duration::from_millis(140), "the chunk after the first 52 chars");
        assert!(block >= done, "{block:?} < {done:?}");
        assert!(streamed * 5 < done, "{streamed:?} vs {done:?}");
    }

    #[tokio::test]
    async fn a_failed_synthesis_is_retried_once_before_its_sentence_is_skipped() {
        let mock = Mock { synth_failures: Mutex::new(1), ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        let is_error = |f: &Outbound| matches!(f, Outbound::Control(VoiceServerMsg::Error { .. }));
        h.signals.send(Signal::Text("Kept.".into())).await.unwrap();
        let frames = h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert!(!frames.iter().any(is_error), "{frames:?}");
        assert!(frames.contains(&Outbound::Audio(Bytes::from_static(b"Kept."))), "{frames:?}");
        *h.mock.synth_failures.lock().unwrap() = 2;
        h.signals.send(Signal::Text("Lost. Next.".into())).await.unwrap();
        let frames = h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(frames.iter().filter(|&f| is_error(f)).count(), 1, "{frames:?}");
        assert_eq!(Mock::log(&h.mock.synthesized), vec!["Kept.", "Next."]);
    }

    #[tokio::test]
    async fn a_block_longer_than_both_queues_is_spoken_to_the_end() {
        let mut h = start(Mock::default(), Prefs::default()).await;
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
        let mut h = start(mock, Prefs { barge_in: true, ..Prefs::default() }).await;
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
        let mut h = start(mock, Prefs::default()).await;
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
        let mut h = start(Mock::default(), Prefs::default()).await;
        h.inbound
            .send(Inbound::Control(VoiceClientMsg::Mode {
                barge_in: None,
                spoken_style: Some(SpokenStyle::Verbose),
                voice: Some("bf_emma".into()),
                speed: Some(1.5),
                language: None,
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

    #[tokio::test]
    async fn an_out_of_range_speed_keeps_the_previous_one() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        for speed in [1.5, 9.0, f32::NAN] {
            let mode = VoiceClientMsg::Mode {
                barge_in: None,
                spoken_style: None,
                voice: None,
                speed: Some(speed),
                language: None,
            };
            h.inbound.send(Inbound::Control(mode)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.signals.send(Signal::Text("Hello.".into())).await.unwrap();
        h.collect_until(|f| matches!(f, Outbound::Control(VoiceServerMsg::SpeakEnd { .. }))).await;
        assert_eq!(Mock::log(&h.mock.speeds), vec![Some(1.5)]);
    }

    fn is_error(f: &Outbound) -> bool {
        matches!(f, Outbound::Control(VoiceServerMsg::Error { .. }))
    }

    fn transcripts(frames: &[Outbound]) -> Vec<String> {
        frames
            .iter()
            .filter_map(|f| match f {
                Outbound::Control(VoiceServerMsg::Transcript { text }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Five minutes of a continuous tone: nothing in it reads as silence.
    fn five_minutes() -> Bytes {
        Bytes::from([0x00u8, 0x40].repeat(5 * 60 * 16_000))
    }

    fn ten_chunks() -> String {
        (1..=10).map(|n| format!("p{n}")).collect::<Vec<_>>().join(" ")
    }

    /// Frames until the whole transcript is in, then the settled tail.
    async fn heard_whole(h: &mut Harness, whole: &str) -> Vec<Outbound> {
        let mut frames = h
            .collect_until(|f| {
                matches!(f, Outbound::Control(VoiceServerMsg::Transcript { text }) if text == whole)
            })
            .await;
        frames.extend(h.quiet().await);
        frames
    }

    #[tokio::test]
    async fn a_five_minute_utterance_sent_whole_is_heard_in_chunks_and_sent_once() {
        let mock = Mock { numbered: true, ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::Audio(five_minutes())).await.unwrap();
        let frames = heard_whole(&mut h, &ten_chunks()).await;
        assert!(!frames.iter().any(is_error), "{frames:?}");
        assert_eq!(transcripts(&frames).len(), 10);
        assert_eq!(transcripts(&frames)[1], "p1 p2");
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].ends_with(&ten_chunks()), "{sent:?}");
    }

    #[tokio::test]
    async fn a_five_minute_utterance_streamed_in_parts_is_sent_once_in_order() {
        let mock = Mock { numbered: true, ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        let whole = five_minutes();
        let part = whole.len() / 10;
        for i in 0..9 {
            h.inbound.send(Inbound::Part(whole.slice(i * part..(i + 1) * part))).await.unwrap();
        }
        h.inbound.send(Inbound::Audio(whole.slice(9 * part..))).await.unwrap();
        let frames = heard_whole(&mut h, &ten_chunks()).await;
        assert!(!frames.iter().any(is_error), "{frames:?}");
        assert_eq!(frames.iter().filter(|f| **f == state(VoiceState::Thinking)).count(), 1);
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].ends_with(&ten_chunks()), "{sent:?}");
    }

    #[tokio::test]
    async fn streamed_parts_are_transcribed_as_they_arrive_and_joined() {
        let mock = Mock { echo: true, ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"one"))).await.unwrap();
        h.collect_until(is_transcript).await;
        assert!(Mock::log(&h.mock.sent).is_empty());
        h.inbound.send(Inbound::Part(Bytes::from_static(b"two"))).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"three"))).await.unwrap();
        let frames = heard_whole(&mut h, "one two three").await;
        assert_eq!(transcripts(&frames), vec!["one two", "one two three"]);
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].ends_with("one two three"), "{sent:?}");
    }

    #[tokio::test]
    async fn a_streamed_utterance_may_end_without_a_last_part() {
        let mock = Mock { echo: true, ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::UtteranceEnd).await.unwrap();
        assert!(h.quiet().await.is_empty());
        h.inbound.send(Inbound::Part(Bytes::from_static(b"one"))).await.unwrap();
        h.inbound.send(Inbound::UtteranceEnd).await.unwrap();
        let frames = heard_whole(&mut h, "one").await;
        assert!(!frames.iter().any(is_error), "{frames:?}");
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].ends_with("one"), "{sent:?}");
    }

    #[tokio::test]
    async fn a_failed_chunk_is_retried_before_the_utterance_goes_on() {
        let mock = Mock { echo: true, stt_failures: Mutex::new(1), ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"one"))).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"two"))).await.unwrap();
        let frames = heard_whole(&mut h, "one two").await;
        assert!(!frames.iter().any(is_error), "{frames:?}");
        assert!(Mock::log(&h.mock.sent)[0].ends_with("one two"));
    }

    #[tokio::test]
    async fn a_chunk_that_keeps_failing_is_reported_and_the_rest_is_still_sent() {
        let mock = Mock { echo: true, stt_failures: Mutex::new(3), ..Mock::default() };
        let mut h = start(mock, Prefs::default()).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"one"))).await.unwrap();
        h.inbound.send(Inbound::Part(Bytes::from_static(b"two"))).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"three"))).await.unwrap();
        let frames = heard_whole(&mut h, "two three").await;
        assert_eq!(frames.iter().filter(|f| is_error(f)).count(), 1, "{frames:?}");
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].ends_with("two three"), "{sent:?}");
    }

    #[tokio::test]
    async fn the_model_warms_on_open_and_an_early_utterance_waits_for_it() {
        let gate = Arc::new(Semaphore::new(0));
        let mock = Mock {
            transcript: Mutex::new("hello".into()),
            warm_gate: Some(gate.clone()),
            ..Mock::default()
        };
        let mut h = start_cold(mock, Prefs::default());
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        let frames = h.quiet().await;
        assert_eq!(frames, vec![state(VoiceState::Warming)]);
        assert_eq!(Mock::log(&h.mock.stt_calls), vec!["warm -"]);
        gate.add_permits(1);
        let frames = h.collect_until(is_transcript).await;
        assert_eq!(
            frames,
            vec![
                state(VoiceState::Thinking),
                Outbound::Control(VoiceServerMsg::Transcript { text: "hello".into() }),
            ]
        );
        assert_eq!(Mock::log(&h.mock.stt_calls), vec!["warm -", "hear -"]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_model_is_kept_warm_every_keepalive() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        let step = |d: Duration| async move {
            tokio::time::sleep(d).await;
            tokio::task::yield_now().await;
        };
        step(STT_KEEPALIVE.checked_sub(Duration::from_secs(1)).unwrap()).await;
        assert_eq!(h.mock.warms(), 1);
        step(Duration::from_secs(2)).await;
        assert_eq!(h.mock.warms(), 2);
        step(STT_KEEPALIVE).await;
        assert_eq!(h.mock.warms(), 3);

        step(Duration::from_secs(30)).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.quiet().await;
        step(STT_KEEPALIVE.checked_sub(Duration::from_secs(5)).unwrap()).await;
        assert_eq!(h.mock.warms(), 3, "an utterance counts as use");
        step(Duration::from_secs(10)).await;
        assert_eq!(h.mock.warms(), 4);
        assert!(!h.quiet().await.contains(&state(VoiceState::Warming)), "keepalives stay silent");
    }

    #[tokio::test]
    async fn a_language_pick_rewarms_and_routes_later_utterances() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        let pick = |language: &str| {
            Inbound::Control(VoiceClientMsg::Mode {
                barge_in: None,
                spoken_style: None,
                voice: None,
                speed: None,
                language: Some(language.into()),
            })
        };
        h.inbound.send(pick("FR")).await.unwrap();
        let frames = h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert_eq!(frames, vec![state(VoiceState::Warming), state(VoiceState::Listening)]);
        h.inbound.send(pick("fr")).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.quiet().await;
        h.inbound.send(pick("auto")).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.quiet().await;
        assert_eq!(
            Mock::log(&h.mock.stt_calls),
            vec!["warm -", "warm fr", "hear fr", "warm -", "hear -"]
        );
    }

    fn listen(part_ms: Option<u32>) -> Outbound {
        Outbound::Control(VoiceServerMsg::Listen { part_ms, min_part_ms: None })
    }

    fn transcript_is(text: &'static str) -> impl Fn(&Outbound) -> bool {
        move |f| matches!(f, Outbound::Control(VoiceServerMsg::Transcript { text: t }) if t == text)
    }

    /// A loop past its opening warm-up, asking the browser for small parts.
    async fn start_live(mock: Mock) -> Harness {
        let mut h = start_cold(mock, Prefs::default());
        let frames = h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        assert!(frames.contains(&listen(Some(STREAM_PART_MS))), "{frames:?}");
        h
    }

    #[tokio::test]
    async fn a_live_utterance_relays_partials_as_parts_arrive_and_is_sent_once() {
        let mut h = start_live(Mock::live()).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"one"))).await.unwrap();
        let mut frames = h.collect_until(transcript_is("one")).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"two"))).await.unwrap();
        frames.extend(h.collect_until(transcript_is("one two")).await);
        assert!(!frames.contains(&state(VoiceState::Thinking)), "still listening: {frames:?}");
        assert!(Mock::log(&h.mock.sent).is_empty());
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"three"))).await.unwrap();
        let frames = heard_whole(&mut h, "one two three").await;
        assert_eq!(frames[0], state(VoiceState::Thinking));
        assert_eq!(transcripts(&frames), vec!["one two three"], "the final repeats no partial");
        assert!(!frames.iter().any(is_error), "{frames:?}");
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].ends_with("\n\none two three"), "{sent:?}");
        assert_eq!(h.mock.calls("hear"), 0, "nothing heard in batch");
        assert_eq!(h.mock.calls("stream"), 1);
        assert_eq!(h.mock.streams_closed.load(Ordering::SeqCst), 1, "closed after its final");
    }

    #[tokio::test]
    async fn live_utterances_each_get_a_stream_and_are_sent_in_order() {
        let mut h = start_live(Mock::live()).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"first"))).await.unwrap();
        h.inbound.send(Inbound::UtteranceEnd).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"second"))).await.unwrap();
        heard_whole(&mut h, "second").await;
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 2);
        assert!(sent[0].ends_with("first") && sent[1].ends_with("second"), "{sent:?}");
        assert_eq!(h.mock.calls("stream"), 2);
    }

    #[tokio::test]
    async fn a_live_stream_failing_mid_utterance_falls_back_to_batch_for_that_utterance() {
        let mock = Mock { stream_closes: Mutex::new(vec![1013]), ..Mock::live() };
        let mut h = start_live(mock).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"one"))).await.unwrap();
        h.inbound.send(Inbound::Part(Bytes::from_static(b"two"))).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"three"))).await.unwrap();
        let frames = heard_whole(&mut h, "onetwothree").await;
        assert!(!frames.iter().any(is_error), "{frames:?}");
        assert!(!frames.contains(&listen(None)), "a busy model is tried again: {frames:?}");
        assert_eq!(h.mock.calls("hear"), 1);
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"four"))).await.unwrap();
        heard_whole(&mut h, "four").await;
        let sent = Mock::log(&h.mock.sent);
        assert_eq!(sent.len(), 2);
        assert!(sent[0].ends_with("onetwothree") && sent[1].ends_with("four"), "{sent:?}");
        assert_eq!(h.mock.calls("stream"), 2);
        assert_eq!(h.mock.calls("hear"), 1, "the second utterance was heard live");
    }

    #[tokio::test]
    async fn a_stream_that_will_not_open_leaves_the_utterance_to_batch() {
        let mock = Mock { stream_open_failures: Mutex::new(1), ..Mock::live() };
        let mut h = start_live(mock).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"one"))).await.unwrap();
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"two"))).await.unwrap();
        let frames = heard_whole(&mut h, "onetwo").await;
        assert!(!frames.iter().any(is_error), "{frames:?}");
        assert_eq!(Mock::log(&h.mock.sent).len(), 1);
        assert_eq!(h.mock.calls("stream"), 1, "opened once per utterance");
    }

    #[tokio::test]
    async fn a_refused_stream_asks_for_big_parts_until_the_language_changes() {
        let mock = Mock { stream_closes: Mutex::new(vec![1008]), ..Mock::live() };
        let mut h = start_live(mock).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"one"))).await.unwrap();
        let frames = heard_whole(&mut h, "one").await;
        assert!(frames.contains(&listen(None)), "{frames:?}");
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"two"))).await.unwrap();
        heard_whole(&mut h, "two").await;
        assert_eq!(h.mock.calls("stream"), 1, "a refusal is not retried");
        assert_eq!(Mock::log(&h.mock.sent).len(), 2);
        let pick = VoiceClientMsg::Mode {
            barge_in: None,
            spoken_style: None,
            voice: None,
            speed: None,
            language: Some("en".into()),
        };
        h.inbound.send(Inbound::Control(pick)).await.unwrap();
        h.collect_until(|f| *f == listen(Some(STREAM_PART_MS))).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"three"))).await.unwrap();
        heard_whole(&mut h, "three").await;
        assert_eq!(h.mock.calls("stream"), 2);
    }

    #[tokio::test]
    async fn closing_the_socket_discards_a_live_utterance() {
        let mut h = start_live(Mock::live()).await;
        h.inbound.send(Inbound::Part(Bytes::from_static(b"one"))).await.unwrap();
        h.collect_until(transcript_is("one")).await;
        let Harness { mock, inbound, _task: task, .. } = h;
        drop(inbound);
        tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while mock.streams_closed.load(Ordering::SeqCst) == 0 {
            assert!(tokio::time::Instant::now() < deadline, "stream still open");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(Mock::log(&mock.sent).is_empty());
    }

    #[tokio::test]
    async fn a_batch_model_never_asks_the_browser_for_small_parts() {
        let mut h = start(Mock { echo: true, ..Mock::default() }, Prefs::default()).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"one"))).await.unwrap();
        let frames = heard_whole(&mut h, "one").await;
        assert!(
            !frames.iter().any(|f| matches!(f, Outbound::Control(VoiceServerMsg::Listen { .. })))
        );
        assert_eq!(h.mock.calls("stream"), 0);
    }

    #[tokio::test]
    async fn a_batch_model_asks_the_browser_for_the_instance_part_length() {
        let mock = Mock { echo: true, min_part_ms: Some(2_000), ..Mock::default() };
        let mut h = start_cold(mock, Prefs::default());
        let frames = h.collect_until(|f| *f == state(VoiceState::Listening)).await;
        let part =
            Outbound::Control(VoiceServerMsg::Listen { part_ms: None, min_part_ms: Some(2_000) });
        assert_eq!(frames.iter().filter(|f| **f == part).count(), 1, "{frames:?}");
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"one"))).await.unwrap();
        let frames = heard_whole(&mut h, "one").await;
        assert!(!frames.contains(&part), "told once: {frames:?}");
    }

    #[tokio::test]
    async fn a_refused_stream_keeps_the_instance_part_length() {
        let mock = Mock {
            stream_closes: Mutex::new(vec![1008]),
            min_part_ms: Some(2_000),
            ..Mock::live()
        };
        let mut h = start_cold(mock, Prefs::default());
        let live = Outbound::Control(VoiceServerMsg::Listen {
            part_ms: Some(STREAM_PART_MS),
            min_part_ms: Some(2_000),
        });
        h.collect_until(|f| *f == live).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(b"one"))).await.unwrap();
        let frames = heard_whole(&mut h, "one").await;
        let batch =
            Outbound::Control(VoiceServerMsg::Listen { part_ms: None, min_part_ms: Some(2_000) });
        assert!(frames.contains(&batch), "{frames:?}");
    }
}
