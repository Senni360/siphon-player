use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;

use super::{Video, valid_id};

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("Siphon helper or yt-dlp is missing. Install or repair the Siphon helper.")]
    Missing,
    #[error(
        "YouTube is private or age-restricted and needs sign-in. Google sign-in is not supported."
    )]
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
    executable: Option<PathBuf>,
    timeout: Duration,
}

impl Default for Resolver {
    fn default() -> Self {
        // A development fixture must never become shipping helper discovery.
        #[cfg(debug_assertions)]
        let override_path = std::env::var_os("SIPHON_YTDLP").map(PathBuf::from);
        #[cfg(not(debug_assertions))]
        let override_path = None;
        #[cfg(windows)]
        let installed = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("Siphon/bin/yt-dlp/yt-dlp.exe"));
        #[cfg(not(windows))]
        let installed = None;
        Self {
            executable: override_path
                .filter(|path| path.is_absolute())
                .or(installed),
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
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::Metadata)?
            .trim();
        audio_url(text)
    }

    async fn run(&self, id: &str, args: &[&str]) -> Result<Vec<u8>, Error> {
        if !valid_id(id) {
            return Err(Error::Unavailable);
        }
        let executable = self.executable.clone().ok_or(Error::Missing)?;
        let args: Vec<String> = args.iter().map(|arg| (*arg).into()).collect();
        let url = format!("https://www.youtube.com/watch?v={id}");
        let timeout = self.timeout;
        let (cancel, mut cancelled) = oneshot::channel::<()>();
        // The owning task survives a cancelled caller just long enough to kill
        // and reap its process before releasing the helper's running lock.
        let _cancel = Cancel(Some(cancel));
        tokio::spawn(async move {
            let start = Instant::now();
            static PROCESSES: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
            let permits = Arc::clone(PROCESSES.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2))));
            let _permit = tokio::select! {
                result = tokio::time::timeout(timeout, permits.acquire_owned()) => result.map_err(|_| Error::Timeout)?.map_err(|_| Error::Network)?,
                _ = &mut cancelled => return Err(Error::Timeout),
            };
            let lock = tokio::select! {
                result = acquire(&executable, timeout.saturating_sub(start.elapsed())) => result?,
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
            // Include pipe draining in the deadline. A child that exits while
            // another process holds its pipes must not hang a skipped track.
            let result = tokio::select! {
                result = async {
                    let (bytes, errors, status) = tokio::try_join!(
                        read_bounded(stdout, 4 * 1024 * 1024),
                        read_bounded(stderr, 64 * 1024),
                        async { child.wait().await.map_err(|_| Error::Network) },
                    )?;
                    if status.success() { Ok(bytes) }
                    else { Err(classify(&String::from_utf8_lossy(&errors))) }
                } => result,
                _ = &mut cancelled => Err(Error::Timeout),
                _ = tokio::time::sleep(timeout.saturating_sub(start.elapsed())) => Err(Error::Timeout),
            };
            if result.is_err() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            drop(lock);
            result
        }).await.map_err(|_| Error::Network)?
    }
}

fn audio_url(text: &str) -> Result<String, Error> {
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::Metadata);
    }
    let url = reqwest::Url::parse(text).map_err(|_| Error::Metadata)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || !url
            .host_str()
            .is_some_and(|host| host.ends_with(".googlevideo.com"))
        || super::url_expired(text)
    {
        return Err(Error::Metadata);
    }
    Ok(url.into())
}

struct Cancel(Option<oneshot::Sender<()>>);
impl Drop for Cancel {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            let _ = cancel.send(());
        }
    }
}

async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Error::Network)?;
    if bytes.len() as u64 > limit {
        Err(Error::Metadata)
    } else {
        Ok(bytes)
    }
}

async fn acquire(executable: &Path, timeout: Duration) -> Result<File, Error> {
    // Same parent and open options as helper ytdlp_update::Running. Never
    // install, update or apply a staged helper release from the player.
    let parent = executable
        .parent()
        .and_then(Path::parent)
        .ok_or(Error::Missing)?;
    if !parent.is_dir() {
        return Err(Error::Missing);
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join("yt-dlp.running.lock"))
        .map_err(|_| Error::Missing)?;
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
    if !executable.is_file() {
        return Err(Error::Missing);
    }
    Ok(lock)
}

fn classify(stderr: &str) -> Error {
    let text = stderr.to_ascii_lowercase();
    if [
        "sign in",
        "sign-in",
        "age-restricted",
        "private video",
        "confirm your age",
        "cookies",
        "members-only",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Error::SignIn
    } else if text.contains("requested format") {
        Error::Metadata
    } else if [
        "unavailable",
        "removed",
        "not available",
        "copyright",
        "does not exist",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Error::Unavailable
    } else {
        Error::Network
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_resolution_rejects_multiple_urls_credentials_and_expired_links() {
        assert!(
            audio_url("https://r1---sn-test.googlevideo.com/videoplayback?expire=99999999999")
                .is_ok()
        );
        for text in [
            "http://r1.googlevideo.com/audio",
            "https://r1.googlevideo.com.attacker.test/audio",
            "https://user:pass@r1.googlevideo.com/audio",
            "https://r1.googlevideo.com:444/audio",
            "https://r1.googlevideo.com/audio#fragment",
            "https://r1.googlevideo.com/audio?expire=1",
            "https://r1.googlevideo.com/a\nhttps://r2.googlevideo.com/b",
        ] {
            assert_eq!(audio_url(text), Err(Error::Metadata));
        }
    }

    #[test]
    fn helper_diagnostics_are_categories_never_secret_links() {
        assert_eq!(
            classify("Private video https://youtube.com/watch?v=secret"),
            Error::SignIn
        );
        assert_eq!(
            classify("Video removed https://youtube.com/watch?v=secret"),
            Error::Unavailable
        );
        assert_eq!(
            classify("Requested format is not available"),
            Error::Metadata
        );
        assert_eq!(
            classify("network failure https://r1.googlevideo.com/audio?sig=secret"),
            Error::Network
        );
    }

    #[cfg(unix)]
    mod process {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        static PROCESS_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

        struct Tool {
            root: PathBuf,
            resolver: Resolver,
        }
        impl Tool {
            fn new(script: &str, timeout: Duration) -> Self {
                let root = std::env::current_dir()
                    .unwrap()
                    .join("target")
                    .join(format!("youtube-process-{:016x}", rand::random::<u64>()));
                std::fs::create_dir_all(root.join("yt-dlp")).unwrap();
                let path = root.join("yt-dlp/fake-tool");
                std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
                Self {
                    root,
                    resolver: Resolver {
                        executable: Some(path),
                        timeout,
                    },
                }
            }

            fn lock(&self) -> File {
                OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(self.root.join("yt-dlp.running.lock"))
                    .unwrap()
            }
        }
        impl Drop for Tool {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }

        #[tokio::test]
        async fn fake_tool_gets_only_fixed_options_and_a_canonical_url() {
            let _serial = PROCESS_TESTS.lock().await;
            let tool = Tool::new("printf '%s\\n' \"$@\"", Duration::from_secs(5));
            let result = tool
                .resolver
                .run("aB9_-012xyz", &["-g", "-f", "140"])
                .await
                .unwrap();
            let args = std::str::from_utf8(&result)
                .unwrap()
                .lines()
                .collect::<Vec<_>>();
            assert_eq!(
                &args[args.len() - 5..],
                &[
                    "-g",
                    "-f",
                    "140",
                    "--",
                    "https://www.youtube.com/watch?v=aB9_-012xyz"
                ]
            );
            assert_eq!(
                tool.resolver.run("--exec=bad", &[]).await,
                Err(Error::Unavailable)
            );
            assert!(tool.lock().try_lock().is_ok());
        }

        #[tokio::test]
        async fn timeout_and_output_limit_kill_and_release_the_update_lock() {
            let _serial = PROCESS_TESTS.lock().await;
            let tool = Tool::new("exec sleep 30", Duration::from_millis(100));
            assert_eq!(
                tool.resolver.run("aB9_-012xyz", &[]).await,
                Err(Error::Timeout)
            );
            assert!(tool.lock().try_lock().is_ok());
            let noisy = Tool::new("exec yes x", Duration::from_secs(5));
            assert_eq!(
                noisy.resolver.run("aB9_-012xyz", &[]).await,
                Err(Error::Metadata)
            );
            assert!(noisy.lock().try_lock().is_ok());
        }

        #[tokio::test]
        async fn updater_lock_blocks_launching_and_cancellation_reaps_the_process() {
            let _serial = PROCESS_TESTS.lock().await;
            let blocked = Tool::new("printf 'should not launch'", Duration::from_millis(80));
            let lock = blocked.lock();
            lock.lock().unwrap();
            assert_eq!(
                blocked.resolver.run("aB9_-012xyz", &[]).await,
                Err(Error::Updating)
            );
            drop(lock);

            let tool = Tool::new(
                "printf '%s' $$ > \"$0.pid\"\nexec sleep 30",
                Duration::from_secs(10),
            );
            let pid_path = tool
                .resolver
                .executable
                .as_ref()
                .unwrap()
                .with_file_name("fake-tool.pid");
            let resolver = tool.resolver.clone();
            let request = tokio::spawn(async move { resolver.run("aB9_-012xyz", &[]).await });
            tokio::time::timeout(Duration::from_secs(5), async {
                while !pid_path.is_file() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            request.abort();
            let _ = request.await;
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let lock = tool.lock();
                    if lock.try_lock().is_ok() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            #[cfg(target_os = "linux")]
            {
                let pid = std::fs::read_to_string(pid_path).unwrap();
                assert!(!Path::new("/proc").join(pid).exists());
            }
        }
    }
}
