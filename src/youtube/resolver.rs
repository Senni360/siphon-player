use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;

use super::{Video, valid_id};

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("Siphon helper or yt-dlp is missing. Install or repair the Siphon helper.")]
    Missing,
    #[error("YouTube is private or age-restricted and needs sign-in. Google sign-in is not supported.")]
    SignIn,
    #[error("This YouTube video is unavailable or is a live stream.")]
    Unavailable,
    #[error("YouTube could not be reached. Check your connection and try again.")]
    Network,
    #[error("YouTube took too long to respond. Try again.")]
    Timeout,
    #[error("The Siphon helper is updating yt-dlp. Try again shortly.")]
    Updating,
    #[error("YouTube returned unsupported metadata or audio.")]
    Metadata,
}

#[derive(Clone)]
pub struct Resolver {
    executable: PathBuf,
    timeout: Duration,
}

impl Default for Resolver {
    fn default() -> Self {
        Self {
            executable: std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default()
                .join("Siphon/bin/yt-dlp/yt-dlp.exe"),
            timeout: Duration::from_secs(30),
        }
    }
}

impl Resolver {
    pub async fn metadata(&self, id: &str) -> Result<Video, Error> {
        let bytes = self.run(id, &["-J", "--skip-download"]).await?;
        let video: Video = serde_json::from_slice(&bytes).map_err(|_| Error::Metadata)?;
        if video.id != id {
            return Err(Error::Metadata);
        }
        Ok(video)
    }

    pub async fn audio_url(&self, id: &str) -> Result<String, Error> {
        let bytes = self.run(id, &["-f", "140", "-g"]).await?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::Metadata)?.trim();
        let url = reqwest::Url::parse(text).map_err(|_| Error::Metadata)?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
            || !url.host_str().is_some_and(|host| host.ends_with(".googlevideo.com"))
        {
            return Err(Error::Metadata);
        }
        Ok(url.into())
    }

    async fn run(&self, id: &str, args: &[&str]) -> Result<Vec<u8>, Error> {
        if !valid_id(id) {
            return Err(Error::Unavailable);
        }
        let executable = self.executable.clone();
        let args: Vec<String> = args.iter().map(|arg| (*arg).into()).collect();
        let url = format!("https://www.youtube.com/watch?v={id}");
        let timeout = self.timeout;
        let (cancel, mut cancelled) = oneshot::channel::<()>();
        // Dropping the caller (skip, sign-out, shutdown) notifies the owning
        // task. That task kills AND reaps before releasing the helper lock.
        let _cancel = Cancel(Some(cancel));
        tokio::spawn(async move {
            let start = Instant::now();
            let lock = tokio::select! {
                result = acquire(&executable, timeout) => result?,
                _ = &mut cancelled => return Err(Error::Timeout),
            };
            let mut command = tokio::process::Command::new(executable);
            command.args(["--ignore-config", "--no-playlist", "--no-progress", "--no-warnings", "--no-cache-dir", "--socket-timeout", "10", "--retries", "1"])
                .args(args).arg("--").arg(url)
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            #[cfg(windows)]
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let mut child = command.spawn().map_err(|_| Error::Missing)?;
            let stdout = child.stdout.take().ok_or(Error::Metadata)?;
            let stderr = child.stderr.take().ok_or(Error::Metadata)?;
            let output = tokio::spawn(read_bounded(stdout, 4 * 1024 * 1024));
            let errors = tokio::spawn(read_bounded(stderr, 64 * 1024));
            let status = tokio::select! {
                status = child.wait() => status.map_err(|_| Error::Network),
                _ = &mut cancelled => Err(Error::Timeout),
                _ = tokio::time::sleep(timeout.saturating_sub(start.elapsed())) => Err(Error::Timeout),
            };
            if status.is_err() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            drop(lock);
            let bytes = output.await.map_err(|_| Error::Metadata)?;
            let stderr = errors.await.map_err(|_| Error::Metadata)?;
            if !status?.success() {
                return Err(classify(&String::from_utf8_lossy(&stderr?)));
            }
            bytes
        }).await.map_err(|_| Error::Network)?
    }
}

struct Cancel(Option<oneshot::Sender<()>>);
impl Drop for Cancel {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            let _ = cancel.send(());
        }
    }
}

async fn read_bounded(reader: impl tokio::io::AsyncRead + Unpin, limit: u64) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes).await.map_err(|_| Error::Network)?;
    if bytes.len() as u64 > limit { Err(Error::Metadata) } else { Ok(bytes) }
}

async fn acquire(executable: &Path, timeout: Duration) -> Result<File, Error> {
    // Same parent and open options as helper ytdlp_update::Running. Never
    // install, update or apply a staged helper release from the player.
    let parent = executable.parent().and_then(Path::parent).ok_or(Error::Missing)?;
    if !parent.is_dir() { return Err(Error::Missing); }
    let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(parent.join("yt-dlp.running.lock")).map_err(|_| Error::Missing)?;
    let start = Instant::now();
    loop {
        match lock.try_lock_shared() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < timeout => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(_) => return Err(Error::Updating),
        }
    }
    if !executable.is_file() { return Err(Error::Missing); }
    Ok(lock)
}

fn classify(stderr: &str) -> Error {
    let text = stderr.to_ascii_lowercase();
    if ["sign in", "sign-in", "age-restricted", "private video", "confirm your age", "cookies", "members-only"].iter().any(|s| text.contains(s)) {
        Error::SignIn
    } else if ["unavailable", "removed", "not available", "copyright", "does not exist"].iter().any(|s| text.contains(s)) {
        Error::Unavailable
    } else if text.contains("requested format") {
        Error::Metadata
    } else {
        Error::Network
    }
}
