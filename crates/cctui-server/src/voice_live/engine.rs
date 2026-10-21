//! The voice conversation loop, independent of the socket and the speech
//! upstream: utterances in, control frames and synthesized sentences out.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use cctui_proto::voice::{SpokenStyle, VoiceClientMsg, VoiceServerMsg, VoiceState};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};
use tracing::Instrument;

use super::mainline::{Delta, MainThread, squash};
use super::text::{SentenceBuffer, spoken_prompt};

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
    /// An utterance over the size limit, dropped unread.
    TooLong,
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
/// Under the speech router's 90 s idle unload.
const STT_KEEPALIVE: Duration = Duration::from_mins(1);
/// A stream quiet this long speaks what it has buffered.
const PAUSE_FLUSH: Duration = Duration::from_millis(700);

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
}

#[derive(Debug)]
enum Job {
    Hear(Bytes),
    Warm,
}

/// Transcribes and sends utterances one at a time, in arrival order, off
/// the loop. It warms the model before the first utterance and keeps it warm
/// while idle; utterances arriving meanwhile wait in its queue. Dropping it
/// aborts whatever was in flight.
struct Listener {
    tx: mpsc::Sender<Job>,
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
    let (tx, mut rx) = mpsc::channel::<Job>(UTTERANCE_QUEUE);
    let task = tokio::spawn(async move {
        let mut job = Job::Warm;
        loop {
            let outcome = match job {
                Job::Hear(audio) => hear_one(&*backend, &prefs, &out, &heard, audio).await,
                Job::Warm => {
                    warm_up(&*backend, &prefs).await;
                    Heard::Warm
                }
            };
            if heard.send(outcome).await.is_err() {
                break;
            }
            match next_job(&mut rx, &*backend, &prefs).await {
                Some(next) => job = next,
                None => break,
            }
        }
    });
    Listener { tx, task }
}

/// Waits for the next job, warming the model every [`STT_KEEPALIVE`] of
/// silence.
async fn next_job<B: VoiceBackend>(
    rx: &mut mpsc::Receiver<Job>,
    backend: &B,
    prefs: &SharedPrefs,
) -> Option<Job> {
    loop {
        tokio::select! {
            next = rx.recv() => return next,
            () = tokio::time::sleep(STT_KEEPALIVE) => warm_up(backend, prefs).await,
        }
    }
}

fn language(prefs: &SharedPrefs) -> Option<String> {
    prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).language.clone()
}

async fn warm_up<B: VoiceBackend>(backend: &B, prefs: &SharedPrefs) {
    if let Err(err) = backend.warm_up(language(prefs).as_deref()).await {
        tracing::debug!(%err, "voice: speech-to-text warm-up failed");
    }
}

async fn hear_one<B: VoiceBackend>(
    backend: &B,
    prefs: &SharedPrefs,
    out: &mpsc::Sender<Outbound>,
    heard: &mpsc::Sender<Heard>,
    audio: Bytes,
) -> Heard {
    let error = |message| Outbound::Control(VoiceServerMsg::Error { message });
    let text = match backend.transcribe(audio, language(prefs).as_deref()).await {
        Ok(text) => text.trim().to_owned(),
        Err(message) => {
            let _ = out.send(error(message)).await;
            return Heard::Idle;
        }
    };
    if text.is_empty() {
        return Heard::Idle;
    }
    let _ = heard.send(Heard::Transcript(text.clone())).await;
    let style = prefs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).spoken_style;
    let message = spoken_prompt(&text, style);
    let _ = heard.send(Heard::Sending(message.clone())).await;
    match backend.send_message(message).await {
        Ok(()) => Heard::Sent,
        Err(message) => {
            let _ = out.send(error(message)).await;
            Heard::Failed
        }
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
    /// Utterances handed to the listener and not yet reported back.
    hearing: usize,
    generation: u64,
    probe: SharedProbe,
    sentences: SentenceBuffer,
    /// Streamed text no final block has accounted for yet, so a block is not
    /// read twice.
    streamed: String,
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
            hearing: 0,
            generation: 1,
            probe,
            sentences: SentenceBuffer::default(),
            streamed: String::new(),
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
        } else if self.hearing > 0 {
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
        if self.listener.tx.try_send(Job::Hear(audio)).is_err() {
            let message = "utterance dropped: still transcribing".to_owned();
            self.send(VoiceServerMsg::Error { message }).await;
            return;
        }
        self.hearing += 1;
        self.emit_state().await;
    }

    async fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Transcript(text) => {
                self.send(VoiceServerMsg::Transcript { text }).await;
                return;
            }
            Heard::Sending(message) => {
                self.reset_stream();
                *self.probe() = Some(Probe::start());
                self.turns.sent();
                self.turn_active = true;
                self.main.new_turn();
                let ready = self.main.prompt(&message);
                self.play(ready).await;
            }
            Heard::Sent | Heard::Idle => self.hearing = self.hearing.saturating_sub(1),
            Heard::Warm => self.warming = false,
            Heard::Failed => {
                self.hearing = self.hearing.saturating_sub(1);
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
                if rewarm && self.listener.tx.try_send(Job::Warm).is_ok() {
                    self.warming = true;
                    self.emit_state().await;
                }
            }
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
        if self.delta_item.as_ref().is_some_and(|last| *last != item) {
            self.sentences.push("\n\n");
            self.streamed.push('\n');
            let ready = self.sentences.pause();
            self.enqueue(ready).await;
        }
        self.delta_item = Some(item);
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
        if self.streamed.trim().is_empty() {
            self.sentences.push(content);
        } else {
            let (later, missed) = reconcile(&self.streamed, content);
            self.streamed = later;
            self.sentences.push(&missed);
            if !self.streamed.is_empty() {
                let ready = self.sentences.drain();
                self.enqueue(ready).await;
                return;
            }
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
        self.streamed.clear();
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
                Some(Inbound::Audio(audio)) => engine.utterance(audio).await,
                Some(Inbound::TooLong) => {
                    let message = "utterance too long".to_owned();
                    engine.send(VoiceServerMsg::Error { message }).await;
                }
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::sync::Semaphore;

    #[derive(Default)]
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
    }

    impl VoiceBackend for Mock {
        async fn transcribe(&self, audio: Bytes, language: Option<&str>) -> Result<String, String> {
            self.stt("hear", language);
            let mut unfinished = Unfinished(Some(self.stt_dropped.clone()));
            if let Some(gate) = &self.stt_gate {
                gate.acquire().await.unwrap().forget();
            }
            unfinished.0 = None;
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
        async fn synthesize(
            &self,
            text: &str,
            _voice: Option<&str>,
            speed: Option<f32>,
        ) -> Result<AudioStream, String> {
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

    /// A reply being spoken, then a new utterance barging in on it.
    async fn cut_a_turn_with_a_new_utterance() -> Harness {
        let mock = Mock { transcript: Mutex::new("go".into()), stall: true, ..Mock::default() };
        let mut h = start(mock, Prefs { barge_in: true, ..Prefs::default() }).await;
        h.inbound.send(Inbound::Audio(Bytes::from_static(&[0, 0]))).await.unwrap();
        h.collect_until(is_transcript).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.signals.send(turn_start()).await.unwrap();
        h.signals.send(Signal::Text("Alpha.".into())).await.unwrap();
        h.collect_until(|f| *f == Outbound::Audio(Bytes::from_static(b"Alpha."))).await;
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
    async fn a_quiet_stream_speaks_its_short_tail_after_a_pause() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        let t0 = Instant::now();
        h.signals.send(delta("m:0", "Sure. Let me")).await.unwrap();
        tokio::time::sleep(PAUSE_FLUSH.checked_sub(Duration::from_millis(100)).unwrap()).await;
        assert!(h.out.try_recv().is_err(), "nothing spoken before the pause");
        h.collect_until(is_speak_start).await;
        assert!(t0.elapsed() >= PAUSE_FLUSH, "{:?}", t0.elapsed());
        h.collect_until(|f| *f == state(VoiceState::Listening)).await;
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

    #[tokio::test]
    async fn an_oversized_utterance_is_reported() {
        let mut h = start(Mock::default(), Prefs::default()).await;
        h.inbound.send(Inbound::TooLong).await.unwrap();
        let frames = h.quiet().await;
        assert_eq!(
            frames,
            vec![Outbound::Control(VoiceServerMsg::Error { message: "utterance too long".into() })]
        );
        assert!(Mock::log(&h.mock.sent).is_empty());
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
}
