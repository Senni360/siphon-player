//! Optional, user-installed Spotify Soloist. Siphon owns only the child it
//! started, and keeps its files and credentials separate from librespot.
pub mod install;
mod protocol;
use crate::{
    credentials::{Grant, Lease},
    http::Http,
    paths::AppDirs,
};
use anyhow::{Context, Result, bail, ensure};
pub use protocol::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

/// No Debug: a personal Soloist key is a usable secret.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub account: String,
    pub value: String,
}
impl Key {
    pub fn valid(&self) -> bool {
        !self.account.is_empty()
            && !self.value.is_empty()
            && self.value.len() <= 4096
            && self.value.bytes().all(|b| b.is_ascii_graphic())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Off,
    Starting,
    NeedsKey,
    Downloading {
        received: u64,
        total: u64,
    },
    Pairing {
        name: String,
        version: String,
    },
    Ready {
        name: String,
        version: String,
        expires: u64,
    },
    Failed(String),
}
impl Status {
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Pairing { name, .. } | Self::Ready { name, .. } => Some(name),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Update {
    Status(Status),
    Changed { queue: bool },
    UpdateReady(String),
    UpdateFailed,
}
pub type Notify = Arc<dyn Fn(Update) + Send + Sync>;

pub struct Handle {
    pub account: String,
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
    pub client: Client,
}
impl Handle {
    pub fn start(
        dirs: AppDirs,
        account: String,
        key: Option<String>,
        lease: Lease,
        http: Http,
        notify: Notify,
    ) -> Self {
        let (stop, stopped) = watch::channel(false);
        let client = Client::default();
        let running = client.clone();
        let own_account = account.clone();
        let task = tokio::spawn(async move {
            let result = run(
                Run {
                    dirs,
                    account,
                    key,
                    lease,
                    http,
                    notify: notify.clone(),
                    client: running.clone(),
                },
                stopped,
            )
            .await;
            running.clear();
            if let Err(error) = result {
                notify(Update::Status(Status::Failed(error.to_string())));
            }
        });
        Self {
            account: own_account,
            stop,
            task: Some(task),
            client,
        }
    }
    pub fn is_running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }
    pub async fn stop(mut self) {
        self.client.clear();
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.client.clear();
        let _ = self.stop.send(true);
    }
}

fn account_dir(dirs: &AppDirs, account: &str) -> PathBuf {
    dirs.state
        .join("soloist")
        .join("accounts")
        .join(format!("{:x}", Sha256::digest(account.as_bytes())))
}
pub async fn forget(dirs: &AppDirs) -> Result<()> {
    let path = dirs.state.join("soloist").join("accounts");
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

struct Run {
    dirs: AppDirs,
    account: String,
    key: Option<String>,
    lease: Lease,
    http: Http,
    notify: Notify,
    client: Client,
}
async fn run(args: Run, mut stopped: watch::Receiver<bool>) -> Result<()> {
    let Run {
        dirs,
        account,
        key,
        lease,
        http,
        notify,
        client,
    } = args;
    ensure!(
        install::supported(),
        "Spotify does not publish Soloist for this platform yet."
    );
    notify(Update::Status(Status::Starting));
    let prepare = async {
        let key = match key {
            Some(value) => {
                let key = Key {
                    account: account.clone(),
                    value: value.trim().into(),
                };
                ensure!(
                    key.valid(),
                    "Enter a valid personal Soloist key from Spotify."
                );
                lease.save(Grant::Soloist(key.clone())).await.context(
                "Could not save the Soloist key. Unlock the system credential store and try again.",
            )?;
                key
            }
            None => match lease.load().await?.grant {
                Some(Grant::Soloist(key)) if key.account == account => key,
                _ => {
                    notify(Update::Status(Status::NeedsKey));
                    return Ok(None);
                }
            },
        };
        let root = dirs.state.join("soloist").join("builds");
        let data = account_dir(&dirs, &account);
        let prepare_root = root.clone();
        let prepare_data = data.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(parent) = prepare_root.parent() {
                install::private_dir(parent)?;
            }
            install::private_dir(&prepare_root)?;
            install::private_dir(&prepare_data)?;
            Ok(())
        })
        .await??;
        let instance =
            install::instance_id(root.parent().expect("Soloist state directory")).await?;
        // Include the installation identity as well as the account/profile:
        // different computers can have identical state paths and usernames.
        let suffix = format!(
            "{:x}",
            Sha256::digest(format!("{instance}:{}", data.to_string_lossy()).as_bytes())
        );
        let name = format!("Siphon Soloist ({})", &suffix[..8]);
        let build = if let Some(build) = install::saved(&root, "pending.json").await {
            build
        } else if let Some(build) = install::saved(&root, "current.json").await {
            build
        } else {
            install::download(&root, &http, &|received, total| {
                notify(Update::Status(Status::Downloading { received, total }))
            })
            .await?
        };
        Ok::<_, anyhow::Error>(Some((key, root, data, name, build)))
    };
    let prepared = tokio::select! {
        _ = stopped.changed() => return Ok(()),
        result = prepare => result?,
    };
    let Some((key, root, data, name, mut build)) = prepared else {
        return Ok(());
    };
    let mut keep = vec![build.digest.clone()];
    if let Some(previous) = install::saved(&root, "current.json").await {
        keep.push(previous.digest);
    }
    let _ = install::collect(&root, &keep).await;
    let mut expired_retries = 0;
    let mut restarts = 0;
    loop {
        if *stopped.borrow() {
            return Ok(());
        }
        let previous = install::saved(&root, "current.json").await;
        let started = Instant::now();
        let outcome = process(
            Player {
                root: &root,
                data: &data,
                name: &name,
                key: &key,
                build: &build,
                http: &http,
                notify: &notify,
                client: &client,
            },
            stopped.clone(),
        )
        .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(_)
                if previous
                    .as_ref()
                    .is_some_and(|old| old.digest != build.digest) =>
            {
                build = previous.expect("checked previous build");
                // Restore the pointer before retrying. If the old player also
                // fails, it must report the error instead of bouncing forever
                // between two installed builds.
                install::remember(&root, "current.json", &build).await?;
                let _ = tokio::fs::remove_file(root.join("pending.json")).await;
                notify(Update::UpdateFailed);
                continue;
            }
            // It stopped after running for a while, so bring it back rather than
            // leave the person without a player. One that keeps stopping at
            // once has a reason to report, such as a bad key.
            Err(error) => {
                if started.elapsed() >= HEALTHY_RUN {
                    restarts = 0;
                }
                if restarts >= RESTART_LIMIT {
                    return Err(error);
                }
                restarts += 1;
                log::warn!("Soloist stopped; starting it again ({restarts}/{RESTART_LIMIT})");
                tokio::select! {
                    _ = stopped.changed() => return Ok(()),
                    _ = tokio::time::sleep(RESTART_DELAY) => {}
                }
                continue;
            }
        };
        match outcome {
            Outcome::Stopped => return Ok(()),
            Outcome::Update(next) => build = next,
            Outcome::Expired if expired_retries == 0 => {
                expired_retries += 1;
                let progress = |received, total| {
                    notify(Update::Status(Status::Downloading { received, total }))
                };
                build = tokio::select! {
                    _ = stopped.changed() => return Ok(()),
                    result = install::download(&root, &http, &progress) => result?,
                };
            }
            Outcome::Expired => bail!(
                "Spotify's current Soloist build has expired. Try again when Spotify publishes its replacement."
            ),
        }
    }
}

enum Outcome {
    Stopped,
    Update(install::Build),
    Expired,
}
struct Player<'a> {
    root: &'a std::path::Path,
    data: &'a std::path::Path,
    name: &'a str,
    key: &'a Key,
    build: &'a install::Build,
    http: &'a Http,
    notify: &'a Notify,
    client: &'a Client,
}
struct CancelDownload(tokio::task::AbortHandle);
impl Drop for CancelDownload {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn process(player: Player<'_>, mut stopped: watch::Receiver<bool>) -> Result<Outcome> {
    let Player {
        root,
        data,
        name,
        key,
        build,
        http,
        notify,
        client,
    } = player;
    for file in ["ws.addr", "ws.port"] {
        let _ = tokio::fs::remove_file(data.join(file)).await;
    }
    let reclaim_builds = root.to_path_buf();
    let reclaim_data = data.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || reclaim(&reclaim_builds, &reclaim_data)).await;
    let mut command = tokio::process::Command::new(build.path(root));
    command
        .args([
            "--device-name",
            name,
            "--api-key",
            &key.value,
            "--ws",
            "127.0.0.1:0",
            "--cache-size",
            "512",
        ])
        .arg("--data-dir")
        .arg(data)
        .arg("--cache-dir")
        .arg(data.join("cache"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    // Soloist currently requires the key in argv. Never log this command or
    // the child's output. See the documented upstream limitation.
    let mut child = command.spawn().context(
        "Soloist could not start. Check that this operating system supports Spotify's build.",
    )?;
    keep_output(&data.join("soloist-output.log"), &key.value, &mut child).await;
    let mut cancellation = stopped.clone();
    let work = async {
        let port = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(exit) = child.try_wait()? {
                    if exit.code() == Some(10) { return Ok(None); }
                    bail!("Soloist stopped before it was ready. Check your key and this computer's audio service, then retry.");
                }
                if let Ok(text) = tokio::fs::read_to_string(data.join("ws.port")).await
                    && let Ok(port) = text.trim().parse::<u16>() && port != 0 {
                    let address = tokio::fs::read_to_string(data.join("ws.addr")).await?;
                    ensure!(address.trim() == "127.0.0.1", "Soloist did not bind its API to this computer");
                    break Ok(Some(port));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }).await.context("Soloist took too long to start. Retry after checking the audio service.")??;
        let Some(port) = port else {
            return Ok(Outcome::Expired);
        };
        let mut socket =
            tokio::time::timeout(Duration::from_secs(5), protocol::connect(port)).await??;
        install::remember(root, "current.json", build).await?;
        let _ = tokio::fs::remove_file(root.join("pending.json")).await;
        client.listening(Some(port));
        notify(Update::Status(Status::Pairing {
            name: name.into(),
            version: build.version.clone(),
        }));
        let mut idle = false;
        let mut active = true;
        let mut player_idle = false;
        let mut logged_in = false;
        let mut pending = None;
        let mut downloading: Option<tokio::task::JoinHandle<Result<install::Build>>> = None;
        let mut cancel_download = None;
        let mut checked = build.checked;
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        let mut changed = Instant::now() - Duration::from_secs(10);
        let mut changes = tokio::time::interval(Duration::from_millis(400));
        changes.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut pending_change = false;
        let mut pending_queue = false;
        let mut last_reconnect = None;
        loop {
            let mut reconnected = false;
            tokio::select! {
                _ = stopped.changed() => return Ok(Outcome::Stopped),
                exit = child.wait() => {
                    if exit?.code() == Some(10) { return Ok(Outcome::Expired); }
                    bail!("Soloist stopped. Retry in Settings to start it again.");
                }
                _ = changes.tick(), if pending_change => {
                    changed = Instant::now();
                    pending_change = false;
                    notify(Update::Changed { queue: std::mem::take(&mut pending_queue) });
                }
                _ = tick.tick() => {
                    reconnected |= protocol::send_reconnecting(&mut socket, port, json!({"command":"get_auth_state"})).await?;
                    if logged_in { reconnected |= protocol::send_reconnecting(&mut socket, port, json!({"command":"get_state"})).await?; }
                    let mut current = build.clone();
                    current.checked = checked;
                    if current.due(install::now()) && downloading.is_none() && pending.is_none() {
                        checked = install::now();
                        let root = root.to_owned();
                        let http = http.clone();
                        let task = tokio::spawn(async move { install::download(&root, &http, &|_, _| {}).await });
                        cancel_download = Some(CancelDownload(task.abort_handle()));
                        downloading = Some(task);
                    }
                    if downloading.as_ref().is_some_and(|task| task.is_finished()) {
                        let result = downloading.take().expect("finished download").await?;
                        drop(cancel_download.take());
                        match result {
                            Ok(mut next) => {
                                next.checked = checked;
                                if next.digest != build.digest && next.built > build.built {
                                    install::remember(root, "pending.json", &next).await?;
                                    notify(Update::UpdateReady(next.version.clone()));
                                    pending = Some(next);
                                } else {
                                    let mut same = build.clone(); same.checked = checked;
                                    install::remember(root, "current.json", &same).await?;
                                }
                            }
                            Err(_) => notify(Update::UpdateFailed),
                        }
                    }
                }
                message = protocol::receive_reconnecting(&mut socket, port) => {
                    let message = message?;
                    reconnected |= message.is_none();
                    // A replaced connection has no event; everything below
                    // reads fields that are then simply absent.
                    let message = message.unwrap_or_default();
                    let kind = message["type"].as_str().unwrap_or_default();
                    if let Some(value) = message["is_active"].as_bool() { active = value; client.active(active); }
                    if kind == "auth_state" {
                        let authenticated = message["logged_in"].as_bool().unwrap_or(false);
                        if authenticated != logged_in {
                            logged_in = authenticated;
                            if logged_in { notify(Update::Status(Status::Ready { name: name.into(), version: build.version.clone(), expires: build.built + 90 * install::DAY })); }
                            else { client.bind(None); notify(Update::Status(Status::Pairing { name: name.into(), version: build.version.clone() })); }
                        }
                    }
                    // Pausing retains the queue and context in the live child.
                    // Only an inactive device or an idle player may restart.
                    if matches!(kind, "playback_state" | "playback_changed") {
                        if message["status"] == "idle" { player_idle = true; }
                        else if matches!(message["status"].as_str(), Some("playing" | "buffering" | "paused")) { player_idle = false; }
                    }
                    idle = !active || player_idle;
                    if matches!(kind, "track_changed" | "playback_changed" | "device_changed" | "context_changed" | "options_changed" | "queue_changed" | "position_sync" | "volume_changed") {
                        pending_queue |= matches!(kind, "queue_changed" | "track_changed" | "options_changed");
                        if changed.elapsed() >= Duration::from_millis(400) {
                            changed = Instant::now();
                            pending_change = false;
                            notify(Update::Changed { queue: std::mem::take(&mut pending_queue) });
                        } else { pending_change = true; }
                    }
                }
            }
            if reconnected {
                // Soloist drops this connection as a matter of course while it
                // keeps playing, so it never ends the player. Dropping it again
                // at once only slows the retries.
                log::info!("Reconnected to Soloist's local API");
                if last_reconnect.is_some_and(|at: Instant| at.elapsed() < RECONNECT_PAUSE) {
                    tokio::time::sleep(RECONNECT_PAUSE).await;
                }
                last_reconnect = Some(Instant::now());
                notify(Update::Changed { queue: true });
            }
            if idle && let Some(next) = pending.take() {
                return Ok(Outcome::Update(next));
            }
        }
    };
    let result = tokio::select! {
        _ = cancellation.changed() => Ok(Outcome::Stopped),
        result = work => result,
    };
    client.listening(None);
    // Always reap our own child before another launch or sign-out cleanup.
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

/// A connection dropped again this soon waits this long before the next try.
const RECONNECT_PAUSE: Duration = Duration::from_secs(1);
/// How many times in a row a player that stopped is started again.
const RESTART_LIMIT: u32 = 3;
const RESTART_DELAY: Duration = Duration::from_secs(2);
/// A run this long was working, so a stop after it starts the count afresh.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
const OUTPUT_LIMIT: usize = 1 << 20;

/// Soloist's own account of why it stopped goes to a file beside its data, with
/// the key removed. Each start replaces the file, and a long run stops at
/// `OUTPUT_LIMIT`, so what led to an early exit is what stays.
async fn keep_output(path: &std::path::Path, key: &str, child: &mut tokio::process::Child) {
    let Ok(file) = tokio::fs::File::create(path).await else {
        return;
    };
    let file = Arc::new(tokio::sync::Mutex::new((file, 0)));
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(copy_output(stdout, key.to_owned(), file.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(copy_output(stderr, key.to_owned(), file));
    }
}

/// Drains one of the child's streams to the end, as a full pipe would stall it.
async fn copy_output(
    stream: impl tokio::io::AsyncRead + Unpin,
    key: String,
    file: Arc<tokio::sync::Mutex<(tokio::fs::File, usize)>>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut lines = tokio::io::BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = format!("{}\n", line.replace(&key, "<key>"));
        let mut file = file.lock().await;
        if file.1 + line.len() <= OUTPUT_LIMIT {
            file.1 += line.len();
            let _ = file.0.write_all(line.as_bytes()).await;
            let _ = file.0.flush().await;
        }
    }
}

/// Stops a Soloist an earlier run left behind, for example after Encore was
/// force-closed. It still holds the data folder's lock, so a new player would
/// exit before it is ready. Only a process started from our own builds is
/// stopped, because the recorded id may now belong to another program.
fn reclaim(builds: &std::path::Path, data: &std::path::Path) {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let Some(pid) = std::fs::read_to_string(data.join("soloist.pid"))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .map(Pid::from_u32)
    else {
        return;
    };
    let Ok(builds) = builds.canonicalize() else {
        return;
    };
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let Some(process) = system.process(pid) else {
        return;
    };
    let ours = process
        .exe()
        .and_then(|exe| exe.canonicalize().ok())
        .is_some_and(|exe| exe.starts_with(&builds));
    if !ours || !process.kill() {
        return;
    }
    log::info!("Stopped a Soloist left running by an earlier run");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        if system.process(pid).is_none() || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_are_account_scoped_and_reject_control_characters() {
        assert!(
            Key {
                account: "a".into(),
                value: "dummy-key".into()
            }
            .valid()
        );
        assert!(
            !Key {
                account: "".into(),
                value: "dummy-key".into()
            }
            .valid()
        );
        assert!(
            !Key {
                account: "a".into(),
                value: "key\n--pair".into()
            }
            .valid()
        );
    }
    /// Explicit live smoke test, never run by normal tests or CI. The caller
    /// provides a personal key via a secret manager. No playback is started.
    #[tokio::test]
    #[ignore = "downloads and starts Spotify Soloist; requires SOLOIST_QA_KEY from a secret manager"]
    async fn native_start_and_stop() {
        let value = std::env::var("SOLOIST_QA_KEY").expect("SOLOIST_QA_KEY is required");
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("soloist-smoke-{}", rand::random::<u64>()));
        let dirs = AppDirs {
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
        };
        let store = crate::credentials::Store::in_memory(dirs.clone());
        let (events, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let handle = Handle::start(
            dirs,
            "native-smoke-test".into(),
            Some(value),
            store.lease(crate::credentials::Slot::Soloist),
            Http::default(),
            Arc::new(move |event| {
                let _ = events.send(event);
            }),
        );
        let result = tokio::time::timeout(Duration::from_secs(120), async {
            while let Some(event) = receive.recv().await {
                match event {
                    Update::Status(Status::Pairing { version, .. }) => {
                        println!("Official Soloist {version} started and exposed its loopback API");
                        return Ok(());
                    }
                    Update::Status(Status::Failed(error)) => return Err(error),
                    _ => {}
                }
            }
            Err("Soloist ended without a setup status".into())
        })
        .await;
        handle.stop().await;
        tokio::fs::remove_dir_all(root).await.unwrap();
        assert!(
            matches!(&result, Ok(Ok(()))),
            "Live smoke test failed: {result:?}"
        );
    }
}
