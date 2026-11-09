use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc, oneshot, watch};

use super::{
    Event, FORMATS, Preferences, Progress, QUALITIES, Source, Summary, discovery, framing,
    matching, saved,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const JOB_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Songs download together, so one slow track does not idle the rest.
const MAX_PARALLEL: usize = 4;

type Emit = Arc<EmitQueue>;

/// Calls the batch's event callback from several download tasks, one at a
/// time. A lock keeps it independent of whether the callback itself is Sync.
struct EmitQueue(std::sync::Mutex<Box<dyn Fn(Event) + Send>>);

impl EmitQueue {
    fn new(callback: impl Fn(Event) + Send + 'static) -> Arc<Self> {
        Arc::new(Self(std::sync::Mutex::new(Box::new(callback))))
    }

    fn call(&self, event: Event) {
        if let Ok(callback) = self.0.lock() {
            callback(event);
        }
    }
}

const CLOSED: &str = "The Siphon helper connection is closed. Try the download again.";

/// What every reader of the helper's one connection shares: the requests
/// waiting for an answer, the download jobs waiting for their events, and
/// which jobs the helper may still be working on.
#[derive(Default)]
struct Shared {
    pending: std::sync::Mutex<HashMap<String, oneshot::Sender<Value>>>,
    jobs: std::sync::Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
    active: std::sync::Mutex<HashSet<String>>,
    broken: AtomicBool,
    serial: AtomicU64,
}

impl Shared {
    fn closed(&self) -> bool {
        self.broken.load(Ordering::SeqCst)
    }

    /// The connection is gone: fail every waiter and every job at once, and
    /// never reuse a partly read frame.
    fn fail(&self) {
        self.broken.store(true, Ordering::SeqCst);
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.clear();
        }
    }

    fn finish(&self, job: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(job);
        }
    }
}

/// One connection to the helper. Several downloads may be in flight on it:
/// results go to the request that waits for them, job events to the
/// subscription for that job.
struct Session<W> {
    writer: Mutex<W>,
    shared: Arc<Shared>,
    reader: tokio::task::JoinHandle<()>,
}

impl<W> Drop for Session<W> {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl<W: AsyncWrite + Unpin> Session<W> {
    fn start<R: AsyncRead + Unpin + Send + 'static>(reader: R, writer: W) -> Arc<Self> {
        let shared = Arc::new(Shared::default());
        let task = tokio::spawn(read_loop(reader, Arc::clone(&shared)));
        Arc::new(Self {
            writer: Mutex::new(writer),
            shared,
            reader: task,
        })
    }

    async fn request(&self, mut request: Value) -> Result<Value, String> {
        if self.shared.closed() {
            return Err(CLOSED.into());
        }
        let id = self
            .shared
            .serial
            .fetch_add(1, Ordering::SeqCst)
            .to_string();
        request["id"] = id.clone().into();
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.insert(id.clone(), sender);
        }
        let response = tokio::time::timeout(REQUEST_TIMEOUT, async {
            {
                let mut writer = self.writer.lock().await;
                framing::write(&mut *writer, &request).await?;
            }
            receiver.await.map_err(|_| CLOSED.to_string())
        })
        .await;
        match response {
            Ok(Ok(message)) => result_data(message),
            Ok(Err(error)) => {
                self.shared.fail();
                Err(error)
            }
            Err(_) => {
                if let Ok(mut pending) = self.shared.pending.lock() {
                    pending.remove(&id);
                }
                Err("The Siphon helper took too long to answer.".into())
            }
        }
    }

    fn subscribe(&self, job: &str) -> Subscription {
        let (sender, receiver) = mpsc::unbounded_channel();
        if let Ok(mut jobs) = self.shared.jobs.lock() {
            jobs.insert(job.to_owned(), sender);
        }
        if let Ok(mut active) = self.shared.active.lock() {
            active.insert(job.to_owned());
        }
        Subscription {
            job: job.to_owned(),
            shared: Arc::clone(&self.shared),
            events: receiver,
        }
    }

    async fn download(
        &self,
        job: &str,
        source: &Source,
        url: &str,
        preferences: &Preferences,
        progress: &mut Progress,
        emit: &impl Fn(Progress),
    ) -> Result<Vec<String>, String> {
        self.download_with_request(
            job,
            download_request(job, source, url, preferences),
            progress,
            emit,
        )
        .await
    }

    async fn download_with_request(
        &self,
        job: &str,
        request: Value,
        progress: &mut Progress,
        emit: &impl Fn(Progress),
    ) -> Result<Vec<String>, String> {
        let mut subscription = self.subscribe(job);
        let response = self.request(request).await?;
        if response["jobId"] != job || response["accepted"] != true {
            self.shared.fail();
            return Err("The Siphon helper did not accept the download.".into());
        }
        loop {
            let message =
                match tokio::time::timeout(JOB_IDLE_TIMEOUT, subscription.events.recv()).await {
                    Ok(Some(message)) => message,
                    Ok(None) => {
                        self.shared.fail();
                        self.shared.finish(job);
                        return Err(CLOSED.into());
                    }
                    Err(_) => {
                        self.shared.fail();
                        self.shared.finish(job);
                        return Err("The Siphon helper stopped reporting progress.".into());
                    }
                };
            let state = message["state"].as_str().unwrap_or("downloading");
            match state {
                "done" => {
                    let files: Vec<_> = message["files"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                    self.shared.finish(job);
                    if files.is_empty() {
                        return Err("The helper finished without reporting a saved file.".into());
                    }
                    return Ok(files);
                }
                "error" => {
                    self.shared.finish(job);
                    return Err(error_message(&message["error"]));
                }
                "cancelled" => {
                    self.shared.finish(job);
                    return Err("Download cancelled.".into());
                }
                _ => {}
            }
            progress.stage = state.into();
            progress.fraction = if state == "downloading" {
                message["progress"]
                    .as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f.clamp(0.0, 1.0) as f32)
            } else {
                None
            };
            emit(progress.clone());
        }
    }

    /// Writes the helper's cooperative cancellation. The read side is never
    /// cancelled, so the frame after it cannot arrive half consumed.
    async fn cancel(&self, job: &str) {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let mut writer = self.writer.lock().await;
            framing::write(
                &mut *writer,
                &json!({"id": "cancel", "type": "cancel", "jobId": job}),
            )
            .await
        })
        .await;
    }

    async fn cancel_all(&self) {
        let jobs: Vec<String> = self
            .shared
            .active
            .lock()
            .map(|active| active.iter().cloned().collect())
            .unwrap_or_default();
        for job in jobs {
            self.cancel(&job).await;
        }
    }
}

/// Job events routed to the download waiting for that job.
struct Subscription {
    job: String,
    shared: Arc<Shared>,
    events: mpsc::UnboundedReceiver<Value>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.shared.jobs.lock() {
            jobs.remove(&self.job);
        }
    }
}

/// Reads every frame the helper sends and hands each one to whoever waits
/// for it. Only this task reads, so a cancelled request never eats a frame.
async fn read_loop<R: AsyncRead + Unpin>(mut reader: R, shared: Arc<Shared>) {
    while let Ok(message) = framing::read(&mut reader).await {
        match message["type"].as_str().unwrap_or("") {
            "result" => {
                let id = message["id"].as_str().unwrap_or("").to_owned();
                let waiter = shared
                    .pending
                    .lock()
                    .ok()
                    .and_then(|mut pending| pending.remove(&id));
                if let Some(waiter) = waiter {
                    let _ = waiter.send(message);
                }
            }
            // A late analysis event after done changes no path: the helper's
            // compatibility mode analyzes inline, so it is not subscribed.
            "job" | "analysis" => {
                let id = message["jobId"].as_str().unwrap_or("").to_owned();
                let sender = shared
                    .jobs
                    .lock()
                    .ok()
                    .and_then(|jobs| jobs.get(&id).cloned());
                if let Some(sender) = sender {
                    let _ = sender.send(message);
                }
            }
            _ => {}
        }
    }
    shared.fail();
}

fn error_message(error: &Value) -> String {
    error["message"]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(600).collect())
        .unwrap_or_else(|| "The Siphon helper could not complete the download.".into())
}

fn result_data(message: Value) -> Result<Value, String> {
    if message["ok"] == true && message["data"].is_object() {
        Ok(message["data"].clone())
    } else {
        Err(error_message(&message["error"]))
    }
}

fn download_request(job: &str, source: &Source, url: &str, preferences: &Preferences) -> Value {
    // Missing options use options.rs defaults, identical to the extension:
    // MP3 320, Music/Siphon, leading-silence trim, metadata, cover and BPM/key.
    // The helper does not read chrome.storage.sync or accept tag overrides.
    let all = matches!(source, Source::SoundCloud { all: true, .. });
    let kind = if crate::soundcloud::permalink(Some(url)).is_some() {
        "soundcloud"
    } else {
        "youtube"
    };
    let mut options = serde_json::Map::new();
    if let Some(folder) = preferences
        .folder
        .as_deref()
        .map(str::trim)
        .filter(|folder| !folder.is_empty())
    {
        options.insert("outputDir".into(), folder.into());
    }
    if let Some(format) = preferences
        .format
        .as_deref()
        .filter(|format| FORMATS.contains(format))
    {
        options.insert("format".into(), format.into());
    }
    if let Some(quality) = preferences
        .quality
        .as_deref()
        .filter(|quality| QUALITIES.contains(quality))
    {
        options.insert("quality".into(), quality.into());
    }
    if all {
        options.insert("playlist".into(), "all".into());
    }
    json!({"type": "download", "jobId": job, "url": url,
        "source": kind, "options": Value::Object(options)})
}

fn command(helper: &discovery::Helper) -> tokio::process::Command {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = if helper
            .launcher
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        {
            std::process::Command::new(&helper.launcher)
        } else {
            let mut command = std::process::Command::new(
                std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()),
            );
            command
                .args(["/d", "/s", "/c"])
                .raw_arg(format!("\"\"{}\"\"", helper.launcher.display()));
            command
        };
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        tokio::process::Command::from(command)
    };
    #[cfg(not(windows))]
    let mut command = tokio::process::Command::new(&helper.launcher);
    if let Some(parent) = helper.launcher.parent() {
        command.current_dir(parent);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

pub(crate) async fn run(
    id: u64,
    sources: Vec<Source>,
    searcher: matching::Searcher,
    saved: saved::Saved,
    preferences: Preferences,
    mut cancel: watch::Receiver<bool>,
    emit: impl Fn(Event) + Send + 'static,
) -> Result<Summary, String> {
    let emit = EmitQueue::new(emit);
    let helper = tokio::select! {
        biased;
        _ = cancel.changed() => return Ok(Summary {cancelled: true, ..Default::default()}),
        helper = tokio::task::spawn_blocking(discovery::discover) => helper,
    }
    .map_err(|_| "Could not locate the Siphon helper.".to_string())?;
    let Some(helper) = helper else {
        emit.call(Event::Available(false));
        return Err(super::INSTALL_HELP.into());
    };
    if *cancel.borrow() {
        return Ok(Summary {
            cancelled: true,
            ..Default::default()
        });
    }
    let mut child = command(&helper).spawn().map_err(|error| {
        format!("Could not start the Siphon helper. Rerun its installer. {error}")
    })?;
    let reader = child
        .stdout
        .take()
        .ok_or("Could not read the Siphon helper.")?;
    let writer = child
        .stdin
        .take()
        .ok_or("Could not write to the Siphon helper.")?;
    let session = Session::start(reader, writer);
    let outcome = run_session(
        session.clone(),
        id,
        sources,
        searcher,
        saved,
        preferences,
        cancel,
        emit.clone(),
    )
    .await;
    // EOF invokes the helper's cooperative shutdown, including its process
    // tree. Reap the launcher; a hung helper is bounded by kill_on_drop.
    drop(session);
    if tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    outcome
}

enum Outcome {
    Cancelled,
    Done(Result<(), String>),
}

/// The download phase of a batch over one connection. Kept apart from
/// [`run`]'s helper discovery so tests drive it with a fake helper.
async fn run_session<W: AsyncWrite + Unpin + Send + Sync + 'static>(
    session: Arc<Session<W>>,
    id: u64,
    sources: Vec<Source>,
    searcher: matching::Searcher,
    saved: saved::Saved,
    preferences: Preferences,
    mut cancel: watch::Receiver<bool>,
    emit: Emit,
) -> Result<Summary, String> {
    let totals = Arc::new(std::sync::Mutex::new(Totals::default()));
    let outcome = tokio::select! {
        biased;
        _ = cancel.changed() => Outcome::Cancelled,
        result = run_sources(&session, id, sources, &searcher, &saved, &preferences, &totals, &emit) => Outcome::Done(result),
    };
    let summary = {
        let totals = totals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Summary {
            downloaded: totals.downloaded,
            failed: totals.failed,
            cancelled: matches!(outcome, Outcome::Cancelled),
            last_file: totals.last_file(),
        }
    };
    if matches!(outcome, Outcome::Cancelled) {
        session.cancel_all().await;
    }
    match outcome {
        Outcome::Done(Err(error)) => Err(error),
        _ => Ok(summary),
    }
}

#[derive(Default)]
struct Totals {
    downloaded: usize,
    failed: usize,
    /// The last file of the highest-numbered song that finished, so the
    /// summary does not depend on which song happened to finish last.
    last: Option<(usize, String)>,
}

impl Totals {
    fn record(&mut self, index: usize, files: &[String]) {
        self.downloaded += files.len();
        if let Some(file) = files.last()
            && self.last.as_ref().is_none_or(|(best, _)| index >= *best)
        {
            self.last = Some((index, file.clone()));
        }
    }

    fn last_file(&self) -> Option<String> {
        self.last.as_ref().map(|(_, file)| file.clone())
    }
}

async fn run_sources<W: AsyncWrite + Unpin + Send + Sync + 'static>(
    session: &Arc<Session<W>>,
    id: u64,
    sources: Vec<Source>,
    searcher: &matching::Searcher,
    saved: &saved::Saved,
    preferences: &Preferences,
    totals: &Arc<std::sync::Mutex<Totals>>,
    emit: &Emit,
) -> Result<(), String> {
    let parallel = sources.len().min(MAX_PARALLEL).max(1);
    // Leave protocolVersion out deliberately. Version 1's compatibility
    // mode performs analysis inline, so done includes the final tags/path.
    // Deferred analysis would rename the file after done, which would make
    // the path shown, revealed and dropped into a DAW wrong. maxConcurrent
    // lets the helper run this many songs at once, each analyzing inline.
    let hello = session
        .request(
            json!({"type": "hello", "extVersion": env!("CARGO_PKG_VERSION"),
            "maxConcurrent": parallel}),
        )
        .await?;
    if hello["ffmpeg"]["found"] != true || !hello["ytdlpVersion"].is_string() {
        return Err(
            "The Siphon helper needs yt-dlp and FFmpeg. Rerun the extension helper installer."
                .into(),
        );
    }
    let total = sources.len();
    let queue = Arc::new(Mutex::new(
        sources.into_iter().enumerate().collect::<VecDeque<_>>(),
    ));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..parallel {
        let session = Arc::clone(session);
        let queue = Arc::clone(&queue);
        let searcher = searcher.clone();
        let saved = saved.clone();
        let preferences = preferences.clone();
        let totals = Arc::clone(totals);
        let emit = Arc::clone(emit);
        tasks.spawn(async move {
            loop {
                let Some((index, source)) = queue.lock().await.pop_front() else {
                    return None;
                };
                if let Some(fatal) = one_source(
                    &session,
                    id,
                    total,
                    index,
                    source,
                    &searcher,
                    &saved,
                    &preferences,
                    &totals,
                    &emit,
                )
                .await
                {
                    return Some(fatal);
                }
            }
        });
    }
    let mut fatal = None;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Some(message)) => {
                fatal.get_or_insert(message);
                tasks.abort_all();
            }
            Ok(None) => {}
            Err(error) if error.is_cancelled() => {}
            Err(_) => {
                fatal.get_or_insert(CLOSED.into());
                tasks.abort_all();
            }
        }
    }
    match fatal {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// One song: find a Spotify source if needed, download it, and count it.
/// Returns a message only when the connection itself is gone.
#[allow(clippy::too_many_arguments)]
async fn one_source<W: AsyncWrite + Unpin>(
    session: &Arc<Session<W>>,
    id: u64,
    total: usize,
    index: usize,
    source: Source,
    searcher: &matching::Searcher,
    saved: &saved::Saved,
    preferences: &Preferences,
    totals: &Arc<std::sync::Mutex<Totals>>,
    emit: &Emit,
) -> Option<String> {
    let mut progress = Progress {
        title: source.title().into(),
        stage: "probing".into(),
        item: index + 1,
        total,
        fraction: None,
        cancelling: false,
    };
    let progress_event = |progress: Progress| emit.call(Event::Progress { id, progress });
    progress_event(progress.clone());
    let mut job = job_id(id, index, None);
    let mut from_saved = false;
    let url = match &source {
        Source::SoundCloud { url, .. } | Source::YouTube { url, .. } => Ok(url.clone()),
        Source::Spotify(track) => match saved.source(&track.uri).await {
            Some(url) => {
                from_saved = true;
                Ok(url)
            }
            None => remember_match(searcher, saved, track).await,
        },
    };
    let used = url.as_ref().ok().cloned();
    let mut result = match url {
        Ok(url) => {
            session
                .download(
                    &job,
                    &source,
                    &url,
                    preferences,
                    &mut progress,
                    &progress_event,
                )
                .await
        }
        Err(error) => Err(error),
    };
    // A match saved earlier can go away, for example when a video is
    // removed. Forget it and search once more, rather than failing.
    if result.is_err() && from_saved && !session.shared.closed() {
        if let Source::Spotify(track) = &source {
            saved.forget(&track.uri).await;
            if let Ok(found) = remember_match(searcher, saved, track).await
                && used.as_deref() != Some(found.as_str())
            {
                job = job_id(id, index, Some("retry"));
                result = session
                    .download(
                        &job,
                        &source,
                        &found,
                        preferences,
                        &mut progress,
                        &progress_event,
                    )
                    .await;
            }
        }
    }
    match result {
        Ok(files) => {
            if let Ok(mut totals) = totals.lock() {
                totals.record(index, &files);
            }
        }
        Err(message) => {
            if session.shared.closed() {
                return Some(message);
            }
            if let Ok(mut totals) = totals.lock() {
                totals.failed += 1;
            }
            emit.call(Event::ItemFailed {
                id,
                message: format!("{}: {message}", source.title()),
            });
        }
    }
    None
}

fn job_id(batch: u64, index: usize, attempt: Option<&str>) -> String {
    match attempt {
        Some(attempt) => format!("player-{batch}-{}-{attempt}", index + 1),
        None => format!("player-{batch}-{}", index + 1),
    }
}

/// Finds the song once and remembers where it was found, with no repeat
/// search next time.
async fn remember_match(
    searcher: &matching::Searcher,
    saved: &saved::Saved,
    track: &crate::api::models::Track,
) -> Result<String, String> {
    let found = searcher.same_song(track).await.ok_or_else(|| {
        format!(
            "No confident match for {}. Nothing was downloaded.",
            track.name
        )
    })?;
    saved.remember(&track.uri, &found).await;
    Ok(found.source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_use_helper_defaults_and_only_set_download_all_for_sets() {
        let source = Source::SoundCloud {
            url: "https://soundcloud.com/test/track".into(),
            title: "Song".into(),
            all: false,
        };
        let request = download_request(
            "a",
            &source,
            "https://soundcloud.com/test/track",
            &Preferences::default(),
        );
        assert_eq!(request["options"], json!({}));
        assert_eq!(request["type"], "download");
        assert_eq!(request["jobId"], "a");
        assert!(request.get("cookies").is_none());
        let set = Source::SoundCloud {
            url: "https://soundcloud.com/test/sets/set".into(),
            title: "Set".into(),
            all: true,
        };
        assert_eq!(
            download_request(
                "b",
                &set,
                "https://soundcloud.com/test/sets/set",
                &Preferences::default()
            )["options"],
            json!({"playlist": "all"})
        );
        let spotify = Source::Spotify(Box::default());
        let request = download_request(
            "c",
            &spotify,
            "https://www.youtube.com/watch?v=abcdefghijk",
            &Preferences::default(),
        );
        assert_eq!(request["source"], "youtube");
        assert!(!request["url"].as_str().unwrap().contains("spotify"));
        let chosen = Preferences {
            folder: Some("D:\\Music".into()),
            format: Some("flac".into()),
            quality: Some("V0".into()),
            tray: true,
        };
        let request = download_request("d", &source, "https://soundcloud.com/test/track", &chosen);
        assert_eq!(
            request["options"],
            json!({"outputDir": "D:\\Music", "format": "flac", "quality": "V0"})
        );
        // A hand-edited value the helper does not know is left out, so the
        // rest of the download still starts.
        let unknown = Preferences {
            folder: Some("  ".into()),
            format: Some("aiff".into()),
            quality: Some("999".into()),
            tray: true,
        };
        assert_eq!(
            download_request("e", &source, "https://soundcloud.com/test/track", &unknown)["options"],
            json!({})
        );
    }

    fn saved_matches() -> saved::Saved {
        saved::Saved::new(crate::test_dirs::temp_dir().join(format!(
            "siphon-matches-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        )))
    }

    fn soundcloud(url: &str, title: &str) -> Source {
        Source::SoundCloud {
            url: url.into(),
            title: title.into(),
            all: false,
        }
    }

    /// Reads a frame from the fake helper, waiting briefly so a test fails
    /// instead of hanging when a request was never sent.
    async fn read<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Value {
        tokio::time::timeout(Duration::from_secs(3), framing::read(reader))
            .await
            .expect("the helper expected another request")
            .unwrap()
    }

    async fn acknowledge<W: AsyncWrite + Unpin>(writer: &mut W, request: &Value) {
        framing::write(
            writer,
            &json!({"type": "result", "id": request["id"], "ok": true,
                "data": {"jobId": request["jobId"], "accepted": true}}),
        )
        .await
        .unwrap();
    }

    async fn hello<W: AsyncWrite + Unpin>(writer: &mut W, request: &Value, dropzone: bool) {
        framing::write(
            writer,
            &json!({"type": "result", "id": request["id"], "ok": true,
                "data": {"ffmpeg": {"found": true}, "ytdlpVersion": "2026.10",
                    "dropzone": dropzone}}),
        )
        .await
        .unwrap();
    }

    async fn finish<W: AsyncWrite + Unpin>(writer: &mut W, job: &Value, file: &str) {
        framing::write(
            writer,
            &json!({"type": "job", "jobId": job, "state": "done", "files": [file]}),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn helper_acknowledgement_progress_analysis_and_completion_are_consumed() {
        let (client, host) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let session = Session::start(reader, writer);
        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(host);
            let request = read(&mut reader).await;
            assert_eq!(request["type"], "hello");
            assert!(request.get("protocolVersion").is_none());
            hello(&mut writer, &request, false).await;
            let download = read(&mut reader).await;
            let job = download["jobId"].clone();
            acknowledge(&mut writer, &download).await;
            for (state, progress) in [
                ("downloading", Some(0.5)),
                ("analyzing", None),
                ("done", Some(1.0)),
            ] {
                framing::write(
                    &mut writer,
                    &json!({"type": "job", "jobId": job, "state": state,
                    "progress": progress, "files": ["C:\\Music\\Song [120 BPM Am].mp3"]}),
                )
                .await
                .unwrap();
            }
        });
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let totals = Arc::new(std::sync::Mutex::new(Totals::default()));
        let emit = EmitQueue::new({
            let events = Arc::clone(&events);
            move |event| events.lock().unwrap().push(event)
        });
        run_sources(
            &session,
            1,
            vec![soundcloud("https://soundcloud.com/test/track", "Song")],
            &matching::Searcher::no_network(),
            &saved_matches(),
            &Preferences::default(),
            &totals,
            &emit,
        )
        .await
        .unwrap();
        let totals = totals.lock().unwrap();
        assert_eq!(totals.downloaded, 1);
        assert!(totals.last_file().unwrap().contains("120 BPM"));
        assert!(events.lock().unwrap().iter().any(|event| matches!(event,
            Event::Progress {progress, ..} if progress.stage == "analyzing" && progress.fraction.is_none())));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn four_downloads_run_at_once_and_every_song_is_counted() {
        let (client, host) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client);
        let session = Session::start(reader, writer);
        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(host);
            let request = read(&mut reader).await;
            assert_eq!(request["type"], "hello");
            assert_eq!(request["maxConcurrent"], 4);
            hello(&mut writer, &request, false).await;
            // Four downloads must be in flight before any finishes.
            let mut first = Vec::new();
            for _ in 0..4 {
                let download = read(&mut reader).await;
                assert_eq!(download["type"], "download");
                first.push(download);
            }
            for download in &first {
                acknowledge(&mut writer, download).await;
                let file = format!("C:\\Music\\{}.mp3", download["jobId"].as_str().unwrap());
                finish(&mut writer, &download["jobId"], &file).await;
            }
            // The last two songs fit only once slots free again.
            for _ in 0..2 {
                let download = read(&mut reader).await;
                acknowledge(&mut writer, &download).await;
                let file = format!("C:\\Music\\{}.mp3", download["jobId"].as_str().unwrap());
                finish(&mut writer, &download["jobId"], &file).await;
            }
        });
        let sources: Vec<_> = (0..6)
            .map(|index| {
                soundcloud(
                    &format!("https://soundcloud.com/test/song-{index}"),
                    &format!("Song {index}"),
                )
            })
            .collect();
        let totals = Arc::new(std::sync::Mutex::new(Totals::default()));
        let emit = EmitQueue::new(|_| {});
        run_sources(
            &session,
            2,
            sources,
            &matching::Searcher::no_network(),
            &saved_matches(),
            &Preferences::default(),
            &totals,
            &emit,
        )
        .await
        .unwrap();
        let totals = totals.lock().unwrap();
        assert_eq!(totals.downloaded, 6);
        assert_eq!(totals.failed, 0);
        assert_eq!(totals.last.as_ref().unwrap().0, 5);
        drop(totals);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_song_is_counted_without_stopping_the_batch() {
        let (client, host) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client);
        let session = Session::start(reader, writer);
        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(host);
            let request = read(&mut reader).await;
            hello(&mut writer, &request, false).await;
            for _ in 0..3 {
                let download = read(&mut reader).await;
                acknowledge(&mut writer, &download).await;
                let job = download["jobId"].clone();
                if download["url"].as_str().unwrap().ends_with("/bad") {
                    framing::write(
                        &mut writer,
                        &json!({"type": "job", "jobId": job, "state": "error",
                            "error": {"code": "NETWORK", "message": "That video is gone."}}),
                    )
                    .await
                    .unwrap();
                } else {
                    finish(&mut writer, &job, "C:\\Music\\Kept.mp3").await;
                }
            }
            // Stay connected until the client closes, so an earlier end of
            // the fake helper cannot look like a broken connection.
            while framing::read(&mut reader).await.is_ok() {}
        });
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let totals = Arc::new(std::sync::Mutex::new(Totals::default()));
        let emit = EmitQueue::new({
            let events = Arc::clone(&events);
            move |event| events.lock().unwrap().push(event)
        });
        run_sources(
            &session,
            3,
            vec![
                soundcloud("https://soundcloud.com/test/one", "One"),
                soundcloud("https://soundcloud.com/test/bad", "Bad"),
                soundcloud("https://soundcloud.com/test/three", "Three"),
            ],
            &matching::Searcher::no_network(),
            &saved_matches(),
            &Preferences::default(),
            &totals,
            &emit,
        )
        .await
        .unwrap();
        let totals = totals.lock().unwrap();
        assert_eq!(totals.downloaded, 2);
        assert_eq!(totals.failed, 1);
        drop(totals);
        assert!(events.lock().unwrap().iter().any(|event| matches!(event,
            Event::ItemFailed { message, .. } if message.starts_with("Bad: ") && message.contains("gone"))));
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_keeps_finished_songs_and_cancels_the_rest() {
        let (client, host) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(client);
        let session = Session::start(reader, writer);
        let (done_sender, done_receiver) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(host);
            let request = read(&mut reader).await;
            hello(&mut writer, &request, false).await;
            let mut jobs = Vec::new();
            for _ in 0..4 {
                let download = read(&mut reader).await;
                acknowledge(&mut writer, &download).await;
                jobs.push(download["jobId"].clone());
            }
            finish(&mut writer, &jobs[0], "C:\\Music\\First.mp3").await;
            // The freed slot takes the fifth song: the first one is recorded.
            let fifth = read(&mut reader).await;
            acknowledge(&mut writer, &fifth).await;
            let _ = done_sender.send(());
            let cancel = read(&mut reader).await;
            assert_eq!(cancel["type"], "cancel");
            assert_ne!(cancel["jobId"], jobs[0]);
            // Hold the helper's answer to the last song: only cancellation ends it.
            let _ = tokio::time::timeout(Duration::from_secs(1), framing::read(&mut reader)).await;
        });
        let (cancel, receiver) = watch::channel(false);
        let sources: Vec<_> = (0..5)
            .map(|index| {
                soundcloud(
                    &format!("https://soundcloud.com/test/song-{index}"),
                    &format!("Song {index}"),
                )
            })
            .collect();
        let running = tokio::spawn(run_session(
            Arc::clone(&session),
            4,
            sources,
            matching::Searcher::no_network(),
            saved_matches(),
            Preferences::default(),
            receiver,
            EmitQueue::new(|_| {}),
        ));
        done_receiver.await.unwrap();
        cancel.send(true).unwrap();
        let summary = running.await.unwrap().unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.downloaded, 1);
        assert_eq!(summary.failed, 0);
        assert!(summary.last_file.unwrap().ends_with("First.mp3"));
        drop(session);
        let _ = server.await;
    }

    #[tokio::test]
    async fn truncated_responses_poison_the_session_instead_of_reusing_partial_frames() {
        let (client, host) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let session = Session::start(reader, writer);
        let server = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut reader, mut writer) = tokio::io::split(host);
            framing::read(&mut reader).await.unwrap();
            writer.write_all(&[5, 0, 0, 0, b'{']).await.unwrap();
        });
        assert!(session.request(json!({"type": "hello"})).await.is_err());
        tokio::time::timeout(Duration::from_secs(2), async {
            while !session.shared.closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the failed read closes the session");
        assert!(
            session
                .request(json!({"type": "hello"}))
                .await
                .unwrap_err()
                .contains("connection is closed")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_writes_the_helpers_cancel_protocol() {
        let (client, host) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let session = Session::start(reader, writer);
        session.cancel("player-1-2").await;
        let (mut reader, _) = tokio::io::split(host);
        let request = read(&mut reader).await;
        assert_eq!(request["type"], "cancel");
        assert_eq!(request["jobId"], "player-1-2");
    }

    /// Kevin MacLeod, Crypto, CC BY. Uses the real registered installation
    /// and full default pipeline, with an isolated destination that is deleted.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires an installed Siphon helper and live SoundCloud access"]
    async fn installed_helper_download_smoke() {
        live_download(Source::SoundCloud {
            url: "https://soundcloud.com/kevin-9-1/crypto".into(),
            title: "Crypto".into(),
            all: false,
        })
        .await;
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "requires an installed Siphon helper and live YouTube access"]
    async fn installed_helper_spotify_match_download_smoke() {
        use crate::api::models::{ArtistRef, Track};
        // The same CC BY recording, described by catalogue metadata. No
        // Spotify service, playback session, or audio is read by this test.
        live_download(Source::Spotify(Box::new(Track {
            uri: "spotify:track:metadata-only-test".into(),
            name: "Crypto".into(),
            duration_ms: 206_000,
            artists: vec![ArtistRef {
                name: "Kevin MacLeod".into(),
                ..Default::default()
            }],
            ..Default::default()
        })))
        .await;
    }

    #[cfg(windows)]
    async fn live_download(source: Source) {
        use std::path::PathBuf;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".cache")
            .join(format!(
                "download-smoke-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root).unwrap();
        let _cleanup = Cleanup(root.clone());
        let helper = discovery::discover().expect("registered Siphon helper");
        let mut child = command(&helper).spawn().expect("launch registered helper");
        let session = Session::start(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        let hello = session
            .request(json!({"type": "hello", "extVersion": env!("CARGO_PKG_VERSION")}))
            .await
            .unwrap();
        assert_eq!(hello["ffmpeg"]["found"], true);
        let url = match &source {
            Source::SoundCloud { url, .. } | Source::YouTube { url, .. } => url.clone(),
            Source::Spotify(track) => {
                let searcher = matching::Searcher::new(
                    crate::youtube::Catalogue {
                        resolver: Default::default(),
                        cache: root.join("youtube-metadata"),
                        links: std::sync::Arc::new(crate::youtube::Links::default()),
                    },
                    None,
                );
                searcher
                    .same_song(track)
                    .await
                    .map(|found| found.source)
                    .expect("confident match")
            }
        };
        let mut request = download_request("player-smoke", &source, &url, &Preferences::default());
        request["options"]["outputDir"] = root.to_string_lossy().as_ref().into();
        let mut progress = Progress::queued("Crypto".into());
        let result = session
            .download_with_request("player-smoke", request, &mut progress, &|progress| {
                eprintln!("{} {:?}", progress.stage, progress.fraction);
            })
            .await;
        drop(session);
        if tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
        let files = result.expect("real helper download");
        assert!(!files.is_empty());
        for file in files {
            let path = PathBuf::from(file);
            assert!(path.starts_with(&root));
            assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("mp3"));
            assert!(std::fs::metadata(&path).unwrap().len() > 1024);
            assert!(path.file_name().unwrap().to_string_lossy().contains("BPM"));
            eprintln!("Verified real helper output: {}", path.display());
        }
    }
}
