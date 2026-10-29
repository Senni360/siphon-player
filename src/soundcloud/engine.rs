//! Local SoundCloud playback through Spotifast's existing output.
//!
//! Integration: pause Spotify first, obtain `player::Engine::external_output`,
//! then send `Load`. Keep Spotify/Connect paused until `Stop` has reported
//! `OutputReleased`. The sink gates late Spotify writes and transport events while
//! SoundCloud owns it. Pause and end-of-track retain ownership; Stop and Drop
//! discard external audio and release it. Route volume to the existing
//! Spotify mixer (`PlayerCommand::Volume`/`VolumePreview`) for both sources.
//! Keep the Spotify engine and Tokio runtime alive until Stop acknowledges
//! the handoff. See `docs/_reference/soundcloud-engine.md` for integration.
//!
//! Commands and snapshots never wait for network, decoding or the device.
//! A dedicated feeder decodes Symphonia packets and writes stereo 44.1 kHz
//! PCM before EQ. The shared RodioSink converts that rate to the device's
//! current rate and retains its normal device selection/reopening behavior.
//! HTTP and one-segment HLS lookahead run on the caller's Tokio runtime.
//! Pause cancels lookahead and parks the feeder on its command channel.

mod decode;
mod source;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use librespot_playback::convert::Converter;
use tokio::runtime::Handle;

use crate::player::{LocalState, LocalTrack, Playback};
use crate::sink::ExternalOutput;
use decode::Decoder;
use source::{Fetch, Interrupt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Progressive,
    Hls,
}

/// The API layer adapts its StreamInfo to this input. Signed URLs are
/// deliberately omitted from Debug, errors, state and events.
#[derive(Clone, PartialEq, Eq)]
pub struct Stream {
    pub uri: String,
    pub url: String,
    pub protocol: Protocol,
    pub mime: String,
    pub duration_ms: u32,
    pub is_preview: bool,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("uri", &self.uri)
            .field("protocol", &self.protocol)
            .field("mime", &self.mime)
            .field("duration_ms", &self.duration_ms)
            .field("is_preview", &self.is_preview)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported(String),
    Network(String),
    Decode(String),
    Output(String),
    /// A 403/404 from a playlist, segment, initialization object or range.
    Expired,
    /// Internal cancellation, never shown as a playback failure.
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(s) => write!(f, "Unsupported SoundCloud stream: {s}"),
            Self::Network(s) | Self::Decode(s) | Self::Output(s) => f.write_str(s),
            Self::Expired => f.write_str("The SoundCloud stream URL expired"),
            Self::Cancelled => f.write_str("Playback request cancelled"),
        }
    }
}
impl std::error::Error for Error {}

pub enum Command {
    /// Runtime adapter: stamps all following events with this load identity.
    Tagged {
        tag: crate::player::PlaybackTag,
        command: Box<Command>,
    },
    Load {
        stream: Stream,
        start_position_ms: u32,
        autoplay: bool,
    },
    Play,
    Pause,
    Toggle,
    Seek(u32),
    Stop,
    /// Prepare the next decoder without changing what is playing. A Load of
    /// the identical stream at zero consumes it, never auto-advances a queue.
    /// While paused, preparation is deferred until Play.
    Preload(Stream),
    /// Answer RefreshRequired. Old requests and another track's URLs are
    /// ignored. The engine remembers position and play/pause intent.
    Refresh {
        request_id: u64,
        stream: Stream,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum Event {
    Tagged {
        tag: crate::player::PlaybackTag,
        event: Box<Event>,
    },
    State(Box<LocalState>),
    /// Explicit Stop acknowledgement, after external ownership is released.
    /// Unlike a Stopped state at EOF, this permits returning to Spotify.
    OutputReleased,
    EndOfTrack {
        uri: String,
    },
    RefreshRequired {
        request_id: u64,
        uri: String,
        position_ms: u32,
    },
    Error {
        uri: String,
        error: Error,
    },
}

pub type Notify = Arc<dyn Fn(Event) + Send + Sync>;

pub struct Engine {
    commands: Mutex<mpsc::Sender<(u64, Command)>>,
    interrupt: Arc<Interrupt>,
    state: Arc<Mutex<LocalState>>,
}

impl Engine {
    pub fn new(
        runtime: Handle,
        http: crate::http::Http,
        output: ExternalOutput,
        notify: Notify,
    ) -> std::io::Result<Self> {
        Self::with_output(runtime, http, Box::new(output), notify)
    }

    fn with_output(
        runtime: Handle,
        http: crate::http::Http,
        output: Box<dyn Output>,
        notify: Notify,
    ) -> std::io::Result<Self> {
        let (commands, receiver) = mpsc::channel();
        let interrupt = Arc::new(Interrupt::default());
        let state = Arc::new(Mutex::new(LocalState::default()));
        let fetch = Fetch {
            runtime,
            http,
            interrupt: Arc::clone(&interrupt),
            revision: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(Interrupt::default()),
        };
        let shared = Arc::clone(&state);
        thread::Builder::new()
            .name("soundcloud-audio".into())
            .spawn(move || {
                Worker::new(fetch, output, notify, shared).run(receiver);
            })?;
        Ok(Self {
            commands: Mutex::new(commands),
            interrupt,
            state,
        })
    }

    /// Enqueues a command; never waits for the feeder or the runtime.
    pub fn command(&self, command: Command) -> bool {
        // Keep revision changes in channel order even when transport and
        // refresh replies arrive from different threads.
        let commands = self.commands.lock().unwrap_or_else(PoisonError::into_inner);
        let inner = match &command {
            Command::Tagged { command, .. } => command.as_ref(),
            command => command,
        };
        let interrupts = matches!(
            inner,
            Command::Load { .. }
                | Command::Pause
                | Command::Toggle
                | Command::Seek(_)
                | Command::Stop
                | Command::Shutdown
        );
        let revision = if interrupts {
            let revision = self.interrupt.revision.fetch_add(1, Ordering::SeqCst) + 1;
            self.interrupt.wake.notify_waiters();
            revision
        } else {
            self.interrupt.revision.load(Ordering::SeqCst)
        };
        commands.send((revision, command)).is_ok()
    }

    pub fn state(&self) -> LocalState {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.command(Command::Shutdown);
    }
}

/// Narrow test seam. The production implementation always writes through
/// the shared output, never opens an independent device or bypasses EQ.
trait Output: Send {
    fn claim(&mut self) -> Result<(), Error>;
    fn start(&mut self) -> Result<(), Error>;
    fn pause(&mut self) -> Result<(), Error>;
    fn reset(&mut self);
    fn release(&mut self) -> Result<(), Error>;
    fn write(&mut self, pcm: Vec<f64>) -> Result<(), Error>;
    fn pending_ms(&self) -> u32;
}

impl Output for ExternalOutput {
    fn claim(&mut self) -> Result<(), Error> {
        ExternalOutput::claim(self).map_err(|e| Error::Output(e.to_string()))
    }
    fn start(&mut self) -> Result<(), Error> {
        ExternalOutput::start(self).map_err(|e| Error::Output(e.to_string()))
    }
    fn pause(&mut self) -> Result<(), Error> {
        ExternalOutput::pause(self).map_err(|e| Error::Output(e.to_string()))
    }
    fn reset(&mut self) {
        ExternalOutput::reset(self);
    }
    fn release(&mut self) -> Result<(), Error> {
        ExternalOutput::release(self).map_err(|e| Error::Output(e.to_string()))
    }
    fn write(&mut self, pcm: Vec<f64>) -> Result<(), Error> {
        ExternalOutput::write(self, pcm, &mut Converter::new(None))
            .map_err(|e| Error::Output(e.to_string()))
    }
    fn pending_ms(&self) -> u32 {
        ExternalOutput::pending_ms(self)
    }
}

struct Preloaded {
    stream: Stream,
    task: tokio::task::JoinHandle<Result<Decoder, Error>>,
    interrupt: Arc<Interrupt>,
    transferred: bool,
}

impl Drop for Preloaded {
    fn drop(&mut self) {
        if !self.transferred {
            self.interrupt.revision.fetch_add(1, Ordering::SeqCst);
            self.interrupt.wake.notify_waiters();
        }
        self.task.abort();
    }
}

struct Worker {
    tag: Option<crate::player::PlaybackTag>,
    fetch: Fetch,
    output: Box<dyn Output>,
    notify: Notify,
    shared: Arc<Mutex<LocalState>>,
    state: LocalState,
    stream: Option<Stream>,
    decoder: Option<Decoder>,
    preload: Option<Preloaded>,
    deferred_preload: Option<Stream>,
    submitted: u64,
    start_ms: u32,
    playing: bool,
    claimed: bool,
    /// EOF with a prepared successor: let its Load reuse the queued tail.
    tail: bool,
    refresh: Option<u64>,
    next_refresh: u64,
    last_report: Instant,
}

impl Worker {
    fn new(
        fetch: Fetch,
        output: Box<dyn Output>,
        notify: Notify,
        shared: Arc<Mutex<LocalState>>,
    ) -> Self {
        Self {
            tag: None,
            fetch,
            output,
            notify,
            shared,
            state: LocalState::default(),
            stream: None,
            decoder: None,
            preload: None,
            deferred_preload: None,
            submitted: 0,
            start_ms: 0,
            playing: false,
            claimed: false,
            tail: false,
            refresh: None,
            next_refresh: 0,
            last_report: Instant::now(),
        }
    }

    fn report(&mut self) {
        *self.shared.lock().unwrap_or_else(PoisonError::into_inner) = self.state.clone();
        self.emit(Event::State(Box::new(self.state.clone())));
        self.last_report = Instant::now();
    }

    fn emit(&self, event: Event) {
        (self.notify)(match self.tag {
            Some(tag) => Event::Tagged {
                tag,
                event: Box::new(event),
            },
            None => event,
        });
    }

    fn position(&self) -> u32 {
        self.start_ms
            .saturating_add((self.submitted * 1_000 / 44_100).min(u64::from(u32::MAX)) as u32)
            .saturating_sub(self.output.pending_ms())
            .min(self.stream.as_ref().map_or(u32::MAX, |s| {
                if s.duration_ms == 0 {
                    u32::MAX
                } else {
                    s.duration_ms
                }
            }))
    }

    fn anchor(&mut self) {
        self.state.position_ms = self.position();
        self.state.position_at = (self.playing && self.refresh.is_none()).then(Instant::now);
    }

    fn run(&mut self, receiver: mpsc::Receiver<(u64, Command)>) {
        loop {
            let command = if self.playing && self.refresh.is_none() {
                match receiver.try_recv() {
                    Ok(command) => Some(command),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            } else if self.tail {
                match receiver
                    .recv_timeout(Duration::from_millis(u64::from(self.output.pending_ms())))
                {
                    Ok(command) => Some(command),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        self.tail = false;
                        if let Err(error) = self.output.pause() {
                            self.failed(error);
                        }
                        self.anchor();
                        self.report();
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match receiver.recv() {
                    Ok(command) => Some(command),
                    Err(_) => break,
                }
            };
            if let Some((revision, command)) = command {
                if matches!(command, Command::Shutdown) {
                    break;
                }
                self.fetch.revision = Arc::new(AtomicU64::new(revision));
                if let Err(error) = self.apply(command) {
                    self.failed(error);
                }
            } else if let Err(error) = self.pump() {
                self.failed(error);
            }
        }
        self.decoder = None;
        self.preload = None;
        if self.claimed {
            let _ = self.output.release();
        }
    }

    fn apply(&mut self, command: Command) -> Result<(), Error> {
        match command {
            Command::Tagged { tag, command } => {
                self.tag = Some(tag);
                self.apply(*command)?;
            }
            Command::Load {
                stream,
                start_position_ms,
                autoplay,
            } => {
                if !super::is_track_uri(&stream.uri) {
                    return Err(Error::Unsupported("Expected a SoundCloud track URI".into()));
                }
                if !self.claimed {
                    self.output.claim()?;
                    self.claimed = true;
                }
                let prepared = start_position_ms == 0
                    && self
                        .preload
                        .as_ref()
                        .is_some_and(|p| p.stream == stream && p.task.is_finished());
                if !(self.tail && prepared && autoplay) {
                    self.output.reset();
                    self.output.pause()?;
                }
                self.tail = false;
                self.decoder = None;
                self.refresh = None;
                self.start_ms = clamp_position(&stream, start_position_ms);
                self.submitted = 0;
                self.playing = autoplay;
                self.state = LocalState {
                    playback: if autoplay {
                        Playback::Playing
                    } else {
                        Playback::Paused
                    },
                    loading: true,
                    track: Some(LocalTrack {
                        uri: stream.uri.clone(),
                        duration_ms: stream.duration_ms,
                        ..Default::default()
                    }),
                    position_ms: self.start_ms,
                    track_sequence: self.state.track_sequence + 1,
                    ..Default::default()
                };
                self.stream = Some(stream.clone());
                self.report();
                if prepared {
                    let mut preload = self.preload.take().unwrap();
                    if let Ok(Ok(decoder)) = self.fetch.runtime.block_on(&mut preload.task) {
                        preload.transferred = true;
                        self.decoder = Some(decoder);
                    }
                }
                self.preload = None;
                self.deferred_preload = None;
                self.ensure_decoder()?;
                if autoplay {
                    self.output.start()?;
                } else if let Some(decoder) = &mut self.decoder {
                    decoder.suspend();
                }
                self.state.loading = false;
                self.anchor();
                self.report();
            }
            Command::Play => self.play()?,
            Command::Pause => self.pause()?,
            Command::Toggle => {
                if self.playing {
                    self.pause()?;
                } else {
                    self.play()?;
                }
            }
            Command::Seek(position) => {
                if self.stream.is_some() {
                    self.tail = false;
                    self.output.reset();
                    self.output.pause()?;
                    self.decoder = None;
                    self.start_ms = clamp_position(self.stream.as_ref().unwrap(), position);
                    self.submitted = 0;
                    self.state.seek_sequence += 1;
                    self.state.loading = true;
                    self.anchor();
                    self.report();
                    if self.refresh.is_none() {
                        self.ensure_decoder()?;
                        if self.playing {
                            self.output.start()?;
                        } else if let Some(decoder) = &mut self.decoder {
                            decoder.suspend();
                        }
                        self.state.loading = false;
                        self.report();
                    }
                }
            }
            Command::Stop => {
                self.tail = false;
                self.playing = false;
                self.decoder = None;
                self.preload = None;
                self.deferred_preload = None;
                self.refresh = None;
                self.stream = None;
                if self.claimed {
                    self.output.release()?;
                    self.claimed = false;
                }
                self.state.playback = Playback::Stopped;
                self.state.loading = false;
                self.state.position_at = None;
                self.report();
                self.emit(Event::OutputReleased);
            }
            Command::Preload(stream) => {
                // Remember the intent without doing any work while paused.
                if self.state.playback == Playback::Paused {
                    self.deferred_preload = Some(stream);
                } else {
                    self.prepare(stream);
                }
            }
            Command::Refresh { request_id, stream } => {
                if self.refresh == Some(request_id)
                    && self.stream.as_ref().is_some_and(|s| s.uri == stream.uri)
                {
                    self.stream = Some(stream);
                    if let Some(track) = &mut self.state.track {
                        track.duration_ms = self.stream.as_ref().unwrap().duration_ms;
                    }
                    self.refresh = None;
                    if self.playing {
                        self.ensure_decoder()?;
                        self.output.start()?;
                    }
                    self.state.loading = false;
                    self.state.playback = if self.playing {
                        Playback::Playing
                    } else {
                        Playback::Paused
                    };
                    self.anchor();
                    self.report();
                }
            }
            Command::Shutdown => {}
        }
        Ok(())
    }

    fn prepare(&mut self, stream: Stream) {
        let mut fetch = self.fetch.clone();
        let interrupt = Arc::new(Interrupt::default());
        fetch.cancel = Arc::clone(&interrupt);
        fetch.revision = Arc::new(AtomicU64::new(self.fetch.revision.load(Ordering::SeqCst)));
        let next = stream.clone();
        let task = self
            .fetch
            .runtime
            .spawn_blocking(move || Decoder::open(fetch, &next, 0));
        self.preload = Some(Preloaded {
            stream,
            task,
            interrupt,
            transferred: false,
        });
    }

    fn ensure_decoder(&mut self) -> Result<(), Error> {
        if self.decoder.is_none()
            && let Some(stream) = &self.stream
        {
            if stream.duration_ms != 0 && self.start_ms >= stream.duration_ms {
                return Ok(());
            }
            self.decoder = Some(Decoder::open(self.fetch.clone(), stream, self.start_ms)?);
        }
        if self.playing
            && let Some(decoder) = &mut self.decoder
        {
            decoder.resume(self.fetch.revision.load(Ordering::SeqCst));
        }
        Ok(())
    }

    fn play(&mut self) -> Result<(), Error> {
        if self.stream.is_none() {
            return Ok(());
        }
        if self.state.playback == Playback::Stopped {
            self.tail = false;
            self.output.reset();
            self.output.pause()?;
            self.start_ms = 0;
            self.submitted = 0;
        }
        self.playing = true;
        self.state.error = None;
        if self.refresh.is_none() {
            self.ensure_decoder()?;
            self.output.start()?;
        }
        self.state.playback = Playback::Playing;
        if let Some(stream) = self.deferred_preload.take() {
            self.prepare(stream);
        }
        self.anchor();
        self.report();
        Ok(())
    }

    fn pause(&mut self) -> Result<(), Error> {
        self.tail = false;
        self.playing = false;
        if let Some(decoder) = &mut self.decoder {
            decoder.suspend();
        }
        if let Some(preload) = self.preload.take() {
            self.deferred_preload = Some(preload.stream.clone());
        }
        if self.claimed {
            self.output.pause()?;
        }
        self.state.playback = Playback::Paused;
        self.state.loading = self.refresh.is_some();
        self.anchor();
        self.report();
        Ok(())
    }

    fn pump(&mut self) -> Result<(), Error> {
        self.ensure_decoder()?;
        let Some(decoder) = &mut self.decoder else {
            return self.ended();
        };
        match decoder.next()? {
            Some(pcm) => {
                let frames = pcm.len() as u64 / 2;
                self.output.write(pcm)?;
                self.submitted += frames;
                if self.last_report.elapsed() >= Duration::from_millis(250) {
                    self.anchor();
                    self.report();
                }
            }
            None => {
                self.ended()?;
            }
        }
        Ok(())
    }

    fn ended(&mut self) -> Result<(), Error> {
        self.tail = self.output.pending_ms() > 0
            && self.preload.as_ref().is_some_and(|p| p.task.is_finished());
        if !self.tail {
            self.output.pause()?;
        }
        self.playing = false;
        self.decoder = None;
        self.state.playback = Playback::Stopped;
        self.state.loading = false;
        self.anchor();
        self.report();
        if let Some(stream) = &self.stream {
            self.emit(Event::EndOfTrack {
                uri: stream.uri.clone(),
            });
        }
        Ok(())
    }

    fn failed(&mut self, error: Error) {
        self.tail = false;
        if error == Error::Cancelled {
            // A queued transport command superseded a blocking range read.
            // Rebuild at the played position after processing that command.
            self.decoder = None;
            if self.claimed {
                let _ = self.output.pause();
            }
            self.start_ms = self.position();
            self.submitted = 0;
            return;
        }
        self.preload = None;
        self.deferred_preload = None;
        if self.claimed {
            let _ = self.output.pause();
        }
        self.start_ms = self.position();
        self.submitted = 0;
        self.decoder = None;
        self.state.position_ms = self.start_ms;
        self.state.position_at = None;
        let uri = self
            .stream
            .as_ref()
            .map_or_else(String::new, |s| s.uri.clone());
        if error == Error::Expired {
            self.next_refresh += 1;
            self.refresh = Some(self.next_refresh);
            self.state.loading = true;
            self.report();
            self.emit(Event::RefreshRequired {
                request_id: self.next_refresh,
                uri,
                position_ms: self.start_ms,
            });
        } else {
            self.playing = false;
            self.state.loading = false;
            self.state.playback = Playback::Paused;
            self.state.error = Some(error.to_string());
            self.report();
            self.emit(Event::Error { uri, error });
        }
    }
}

fn clamp_position(stream: &Stream, position: u32) -> u32 {
    if stream.duration_ms == 0 {
        position
    } else {
        position.min(stream.duration_ms)
    }
}
