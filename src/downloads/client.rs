use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use super::{Event, Progress, Source, Summary, discovery, framing, matching};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const JOB_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

struct Session<R, W> {
    reader: R,
    writer: W,
    serial: u64,
    broken: bool,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Session<R, W> {
    async fn request(&mut self, mut request: Value) -> Result<Value, String> {
        if self.broken {
            return Err("The Siphon helper connection is closed. Try the download again.".into());
        }
        self.serial += 1;
        let id = self.serial.to_string();
        request["id"] = id.clone().into();
        let response = tokio::time::timeout(REQUEST_TIMEOUT, async {
            framing::write(&mut self.writer, &request).await?;
            loop {
                let message = framing::read(&mut self.reader).await?;
                if message["type"] == "result" && message["id"] == id {
                    return Ok(message);
                }
                // Log events belong to the helper. They can include URLs;
                // never forward its protocol or diagnostics to app logs.
            }
        })
        .await
        .map_err(|_| "The Siphon helper took too long to answer.".to_string())
        .and_then(|result| result);
        match response {
            Ok(message) => result_data(message),
            Err(error) => {
                self.broken = true;
                Err(error)
            }
        }
    }

    async fn spotify_match(&mut self, track: &crate::api::models::Track) -> Result<String, String> {
        if track.name.trim().is_empty() || track.duration_ms == 0 || track.artists.is_empty() {
            return Err(format!(
                "No confident YouTube match for {}: track details are incomplete.",
                track.name
            ));
        }
        let result = self
            .request(json!({
                "type": "probe", "url": matching::search_url(track), "flatPlaylist": true,
            }))
            .await?;
        let mut candidates = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in result["entries"].as_array().into_iter().flatten().take(5) {
            let Some(url) = entry["url"].as_str().and_then(matching::youtube_url) else {
                continue;
            };
            if !seen.insert(url.clone()) {
                continue;
            }
            // Flat search results do not carry uploader metadata in protocol
            // 1. Probe the first five to verify artist, duration and live status.
            if let Ok(info) = self.request(json!({"type": "probe", "url": url})).await
                && let Some(candidate) = matching::Candidate::from_probe(&info)
            {
                candidates.push(candidate);
            }
            if self.broken {
                return Err(
                    "The Siphon helper connection closed while matching the track. Try again."
                        .into(),
                );
            }
        }
        matching::best_match(track, &candidates)
            .map(|candidate| candidate.url.clone())
            .ok_or_else(|| {
                format!(
                    "No confident YouTube match for {}. Nothing was downloaded.",
                    track.name
                )
            })
    }

    async fn download(
        &mut self,
        job: &str,
        source: &Source,
        url: &str,
        progress: &mut Progress,
        emit: &impl Fn(Progress),
    ) -> Result<Vec<String>, String> {
        self.download_with_request(job, download_request(job, source, url), progress, emit)
            .await
    }

    async fn download_with_request(
        &mut self,
        job: &str,
        request: Value,
        progress: &mut Progress,
        emit: &impl Fn(Progress),
    ) -> Result<Vec<String>, String> {
        let response = self.request(request).await?;
        if response["jobId"] != job || response["accepted"] != true {
            self.broken = true;
            return Err("The Siphon helper did not accept the download.".into());
        }
        loop {
            let message = tokio::time::timeout(JOB_IDLE_TIMEOUT, framing::read(&mut self.reader))
                .await
                .map_err(|_| "The Siphon helper stopped reporting progress.".to_string())
                .and_then(|result| result);
            let message = match message {
                Ok(message) => message,
                Err(error) => {
                    self.broken = true;
                    return Err(error);
                }
            };
            if message["type"] != "job" || message["jobId"] != job {
                continue;
            }
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
                    if files.is_empty() {
                        return Err("The helper finished without reporting a saved file.".into());
                    }
                    return Ok(files);
                }
                "error" => return Err(error_message(&message["error"])),
                "cancelled" => return Err("Download cancelled.".into()),
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

    async fn cancel(&mut self, job: &str) {
        // Do not use request(): a cancelled read may have consumed part of a
        // frame. Write the helper's cooperative cancellation, then close stdin
        // and let its read loop cancel all workers. The child is reaped below.
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            framing::write(
                &mut self.writer,
                &json!({"id": "cancel", "type": "cancel", "jobId": job}),
            ),
        )
        .await;
    }
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

fn download_request(job: &str, source: &Source, url: &str) -> Value {
    // Missing options use options.rs defaults, identical to the extension:
    // MP3 320, Music/Siphon, leading-silence trim, metadata, cover and BPM/key.
    // The helper does not read chrome.storage.sync or accept tag overrides.
    let all = matches!(source, Source::SoundCloud { all: true, .. });
    json!({"type": "download", "jobId": job, "url": url,
        "source": if matches!(source, Source::SoundCloud { .. }) { "soundcloud" } else { "youtube" },
        "options": if all { json!({"playlist": "all"}) } else { json!({}) }})
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
    mut cancel: watch::Receiver<bool>,
    emit: impl Fn(Event),
) -> Result<Summary, String> {
    let helper = tokio::select! {
        biased;
        _ = cancel.changed() => return Ok(Summary {cancelled: true, ..Default::default()}),
        helper = tokio::task::spawn_blocking(discovery::discover) => helper,
    }
    .map_err(|_| "Could not locate the Siphon helper.".to_string())?;
    let Some(helper) = helper else {
        emit(Event::Available(false));
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
    let mut session = Session {
        reader,
        writer,
        serial: 0,
        broken: false,
    };
    let mut job = String::new();
    let mut summary = Summary::default();
    let outcome = tokio::select! {
        biased;
        _ = cancel.changed() => { summary.cancelled = true; Ok(()) },
        result = run_sources(&mut session, id, sources, &mut job, &mut summary, &emit) => result,
    };
    if *cancel.borrow() {
        session.cancel(&job).await;
    }
    // EOF invokes the helper's cooperative shutdown, including its process
    // tree. Reap the launcher; a hung helper is bounded by kill_on_drop.
    drop(session);
    if tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    outcome.map(|()| summary)
}

async fn run_sources<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    session: &mut Session<R, W>,
    id: u64,
    sources: Vec<Source>,
    job: &mut String,
    summary: &mut Summary,
    emit: &impl Fn(Event),
) -> Result<(), String> {
    // Leave protocolVersion out deliberately. Version 1's compatibility
    // mode performs analysis inline, so done includes the final tags/path.
    let hello = session
        .request(json!({"type": "hello", "extVersion": env!("CARGO_PKG_VERSION")}))
        .await?;
    if hello["ffmpeg"]["found"] != true || !hello["ytdlpVersion"].is_string() {
        return Err(
            "The Siphon helper needs yt-dlp and FFmpeg. Rerun the extension helper installer."
                .into(),
        );
    }
    let total = sources.len();
    for (index, source) in sources.into_iter().enumerate() {
        *job = format!("player-{id}-{}", index + 1);
        let mut progress = Progress {
            title: source.title().into(),
            stage: "probing".into(),
            item: index + 1,
            total,
            fraction: None,
            cancelling: false,
        };
        let progress_event = |progress| emit(Event::Progress { id, progress });
        progress_event(progress.clone());
        let url = match &source {
            Source::SoundCloud { url, .. } | Source::YouTube { url, .. } => Ok(url.clone()),
            Source::Spotify(track) => session.spotify_match(track).await,
        };
        let result = match url {
            Ok(url) => {
                session
                    .download(job, &source, &url, &mut progress, &progress_event)
                    .await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(files) => {
                summary.downloaded += files.len();
                summary.last_file = files.last().cloned();
            }
            Err(message) => {
                if session.broken {
                    return Err(message);
                }
                summary.failed += 1;
                emit(Event::ItemFailed {
                    id,
                    message: format!("{}: {message}", source.title()),
                });
            }
        }
    }
    Ok(())
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
        let request = download_request("a", &source, "https://soundcloud.com/test/track");
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
            download_request("b", &set, "https://soundcloud.com/test/sets/set")["options"],
            json!({"playlist": "all"})
        );
        let spotify = Source::Spotify(Box::default());
        let request =
            download_request("c", &spotify, "https://www.youtube.com/watch?v=abcdefghijk");
        assert_eq!(request["source"], "youtube");
        assert!(!request["url"].as_str().unwrap().contains("spotify"));
    }

    #[tokio::test]
    async fn helper_acknowledgement_progress_analysis_and_completion_are_consumed() {
        let (client, host) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut session = Session {
            reader,
            writer,
            serial: 0,
            broken: false,
        };
        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(host);
            let hello = framing::read(&mut reader).await.unwrap();
            assert_eq!(hello["type"], "hello");
            assert!(hello.get("protocolVersion").is_none());
            framing::write(
                &mut writer,
                &json!({"type": "result", "id": hello["id"], "ok": true,
                "data": {"ffmpeg": {"found": true}, "ytdlpVersion": "2026.10"}}),
            )
            .await
            .unwrap();
            let download = framing::read(&mut reader).await.unwrap();
            let job = download["jobId"].clone();
            framing::write(
                &mut writer,
                &json!({"type": "result", "id": download["id"], "ok": true,
                "data": {"jobId": job, "accepted": true}}),
            )
            .await
            .unwrap();
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
        let events = std::sync::Mutex::new(Vec::new());
        let mut job = String::new();
        let source = Source::SoundCloud {
            url: "https://soundcloud.com/test/track".into(),
            title: "Song".into(),
            all: false,
        };
        let mut summary = Summary::default();
        run_sources(
            &mut session,
            1,
            vec![source],
            &mut job,
            &mut summary,
            &|event| events.lock().unwrap().push(event),
        )
        .await
        .unwrap();
        assert_eq!(summary.downloaded, 1);
        assert!(summary.last_file.unwrap().contains("120 BPM"));
        assert!(events.lock().unwrap().iter().any(|event| matches!(event,
            Event::Progress {progress, ..} if progress.stage == "analyzing" && progress.fraction.is_none())));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn uncertain_spotify_match_never_sends_a_download() {
        use crate::api::models::{ArtistRef, Track};
        let (client, host) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut session = Session {
            reader,
            writer,
            serial: 0,
            broken: false,
        };
        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(host);
            let request = framing::read(&mut reader).await.unwrap();
            assert_eq!(request["type"], "probe");
            assert_eq!(request["flatPlaylist"], true);
            assert!(
                request["url"]
                    .as_str()
                    .unwrap()
                    .contains("Artist%20-%20Song")
            );
            framing::write(
                &mut writer,
                &json!({"type": "result", "id": request["id"], "ok": true,
                "data": {"entries": [{"url": "https://www.youtube.com/watch?v=abcdefghijk"}]}}),
            )
            .await
            .unwrap();
            let request = framing::read(&mut reader).await.unwrap();
            assert_eq!(request["type"], "probe");
            framing::write(&mut writer, &json!({"type": "result", "id": request["id"], "ok": true,
                "data": {"url": "https://www.youtube.com/watch?v=abcdefghijk", "title": "Song (Cover)",
                    "uploader": "Artist", "duration": 200}})).await.unwrap();
            // Dropping the client below is the only further traffic: an
            // uncertain candidate must never be submitted as a download.
            assert!(framing::read(&mut reader).await.is_err());
        });
        let track = Track {
            name: "Song".into(),
            duration_ms: 200_000,
            artists: vec![ArtistRef {
                name: "Artist".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            session
                .spotify_match(&track)
                .await
                .unwrap_err()
                .contains("No confident YouTube match")
        );
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_client_writes_the_helpers_cancel_protocol() {
        let (client, host) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut session = Session {
            reader,
            writer,
            serial: 0,
            broken: false,
        };
        session.cancel("player-1-2").await;
        let (mut reader, _) = tokio::io::split(host);
        let request = framing::read(&mut reader).await.unwrap();
        assert_eq!(request["type"], "cancel");
        assert_eq!(request["jobId"], "player-1-2");
    }

    #[tokio::test]
    async fn truncated_responses_poison_the_session_instead_of_reusing_partial_frames() {
        let (client, host) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut session = Session {
            reader,
            writer,
            serial: 0,
            broken: false,
        };
        let server = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut reader, mut writer) = tokio::io::split(host);
            framing::read(&mut reader).await.unwrap();
            writer.write_all(&[5, 0, 0, 0, b'{']).await.unwrap();
        });
        assert!(session.request(json!({"type": "hello"})).await.is_err());
        assert!(session.broken);
        assert!(
            session
                .request(json!({"type": "hello"}))
                .await
                .unwrap_err()
                .contains("connection is closed")
        );
        server.await.unwrap();
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
        let mut session = Session {
            reader: child.stdout.take().unwrap(),
            writer: child.stdin.take().unwrap(),
            serial: 0,
            broken: false,
        };
        let hello = session
            .request(json!({"type": "hello", "extVersion": env!("CARGO_PKG_VERSION")}))
            .await
            .unwrap();
        assert_eq!(hello["ffmpeg"]["found"], true);
        let url = match &source {
            Source::SoundCloud { url, .. } | Source::YouTube { url, .. } => url.clone(),
            Source::Spotify(track) => session
                .spotify_match(track)
                .await
                .expect("confident YouTube match"),
        };
        let mut request = download_request("player-smoke", &source, &url);
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
