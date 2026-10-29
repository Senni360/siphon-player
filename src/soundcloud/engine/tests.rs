use super::*;
use std::io::{Read, Seek, SeekFrom};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const MP3: &[u8] = include_bytes!("../../testdata/soundcloud/tone.mp3");
const AAC: &[u8] = include_bytes!("../../testdata/soundcloud/tone.aac");

struct Server {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
    changed: Arc<tokio::sync::Notify>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let recorded = Arc::clone(&requests);
        let changed = Arc::new(tokio::sync::Notify::new());
        let notify = Arc::clone(&changed);
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let requests = Arc::clone(&recorded);
                let notify = Arc::clone(&notify);
                tokio::spawn(async move {
                    let mut request = vec![];
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        let count = socket.read(&mut buffer).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..count]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let path = request.split_whitespace().nth(1).unwrap();
                    requests.lock().unwrap().push(path.to_string());
                    notify.notify_one();
                    if path == "/slow.mp3" {
                        // Do not respond. Cancellation must close the HTTP
                        // request rather than wait for the 15-second deadline.
                        let _ = socket.read(&mut buffer).await;
                        requests.lock().unwrap().push("/cancelled".into());
                        notify.notify_one();
                        return;
                    }
                    // A virtual large object exercises more than the first
                    // range without committing another binary fixture.
                    let large = MP3.repeat(16);
                    let bytes: &[u8] = match path {
                        "/large.mp3" | "/expires-on-range.mp3" => &large,
                        "/tone.mp3" | "/next.mp3" => MP3,
                        "/range-only.mp3" if request.to_ascii_lowercase().contains("range: bytes=") => MP3,
                        "/tone.aac" => AAC,
                        "/aac.m3u8" | "/fresh/aac.m3u8" | "/expired/aac.m3u8" => include_bytes!("../../testdata/soundcloud/aac.m3u8"),
                        "/master.m3u8" => b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=32000,CODECS=\"mp4a.40.2\"\naac.m3u8\n",
                        "/adts.m3u8" => b"#EXTM3U\n#EXTINF:0.441179,\ntone.aac\n#EXTINF:0.441179,\ntone.aac\n#EXT-X-ENDLIST\n",
                        "/mp3.m3u8" => b"#EXTM3U\n#EXTINF:1.2,\ntone.mp3\n#EXTINF:1.2,\nnext.mp3\n#EXT-X-ENDLIST\n",
                        _ if path.ends_with("/init.mp4") => include_bytes!("../../testdata/soundcloud/init.mp4"),
                        "/expired/aac1.m4s" => {
                            socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                            return;
                        }
                        _ if path.ends_with("/aac0.m4s") => include_bytes!("../../testdata/soundcloud/aac0.m4s"),
                        _ if path.ends_with("/aac1.m4s") => include_bytes!("../../testdata/soundcloud/aac1.m4s"),
                        _ if path.ends_with("/aac2.m4s") => include_bytes!("../../testdata/soundcloud/aac2.m4s"),
                        _ if path.ends_with("/aac3.m4s") => include_bytes!("../../testdata/soundcloud/aac3.m4s"),
                        _ => {
                            socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                            return;
                        }
                    };
                    let range = request.lines().find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .and_then(|s| {
                                let (start, end) = s.split_once('-')?;
                                Some((start.parse::<usize>().ok()?, end.parse::<usize>().ok()?))
                            })
                    });
                    let (status, extra, body) = if let Some((start, end)) = range {
                        if path == "/expires-on-range.mp3" && start >= 32 * 1024 {
                            let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            return;
                        }
                        if start >= bytes.len() {
                            let _ = socket.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            return;
                        }
                        let end = end.min(bytes.len() - 1);
                        (
                            "206 Partial Content",
                            format!("Content-Range: bytes {start}-{end}/{}\r\n", bytes.len()),
                            &bytes[start..=end],
                        )
                    } else {
                        ("200 OK", String::new(), bytes)
                    };
                    let header = format!(
                        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    if socket.write_all(header.as_bytes()).await.is_ok() {
                        let _ = socket.write_all(body).await;
                    }
                });
            }
        });
        Self {
            base,
            requests,
            task,
            changed,
        }
    }

    async fn wait_for(&self, path: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let changed = self.changed.notified();
                if self.requests.lock().unwrap().iter().any(|s| s == path) {
                    return;
                }
                changed.await;
            }
        })
        .await
        .unwrap();
    }

    fn stream(&self, path: &str, protocol: Protocol, duration_ms: u32) -> Stream {
        Stream {
            uri: "soundcloud:track:1".into(),
            url: format!("{}{path}", self.base),
            protocol,
            mime: if path.contains("adts") || path.ends_with(".aac") {
                "audio/aac"
            } else if path.contains("aac") || path.contains("master") {
                "audio/mp4; codecs=mp4a.40.2"
            } else {
                "audio/mpeg"
            }
            .into(),
            duration_ms,
            is_preview: false,
        }
    }
}

#[derive(Default)]
struct Recorded {
    pcm: Vec<f64>,
    writes: usize,
    running: bool,
    claimed: bool,
    pending_ms: u32,
    pauses: usize,
    resets: usize,
}

struct TestOutput(Arc<Mutex<Recorded>>);
impl Output for TestOutput {
    fn claim(&mut self) -> Result<(), Error> {
        self.0.lock().unwrap().claimed = true;
        Ok(())
    }
    fn start(&mut self) -> Result<(), Error> {
        self.0.lock().unwrap().running = true;
        Ok(())
    }
    fn pause(&mut self) -> Result<(), Error> {
        let mut recorded = self.0.lock().unwrap();
        recorded.running = false;
        recorded.pending_ms = 0;
        recorded.pauses += 1;
        Ok(())
    }
    fn reset(&mut self) {
        let mut recorded = self.0.lock().unwrap();
        recorded.resets += 1;
        recorded.pending_ms = 0;
    }
    fn release(&mut self) -> Result<(), Error> {
        self.0.lock().unwrap().claimed = false;
        self.pause()
    }
    fn write(&mut self, pcm: Vec<f64>) -> Result<(), Error> {
        let mut recorded = self.0.lock().unwrap();
        assert!(recorded.running && recorded.claimed);
        recorded.pcm.extend(pcm);
        recorded.writes += 1;
        Ok(())
    }
    fn pending_ms(&self) -> u32 {
        self.0.lock().unwrap().pending_ms
    }
}

type Events = Arc<Mutex<Vec<Event>>>;

fn worker() -> (Worker, Arc<Mutex<Recorded>>, Events) {
    let pcm = Arc::new(Mutex::new(Recorded::default()));
    let events = Arc::new(Mutex::new(vec![]));
    let notify = Arc::clone(&events);
    let fetch = Fetch {
        runtime: Handle::current(),
        http: crate::http::Http::new(reqwest::Client::builder().no_proxy().build().unwrap()),
        interrupt: Arc::new(Interrupt::default()),
        revision: Arc::new(AtomicU64::new(0)),
        cancel: Arc::new(Interrupt::default()),
    };
    (
        Worker::new(
            fetch,
            Box::new(TestOutput(Arc::clone(&pcm))),
            Arc::new(move |event| notify.lock().unwrap().push(event)),
            Arc::new(Mutex::new(LocalState::default())),
        ),
        pcm,
        events,
    )
}

fn load(worker: &mut Worker, stream: Stream, position: u32, autoplay: bool) {
    worker
        .apply(Command::Load {
            stream,
            start_position_ms: position,
            autoplay,
        })
        .unwrap();
}

fn finish(worker: &mut Worker) {
    for _ in 0..10_000 {
        if !worker.playing {
            return;
        }
        worker.pump().unwrap();
    }
    panic!("fixture did not finish");
}

#[tokio::test]
async fn progressive_load_pause_resume_seek_and_end() {
    let server = Server::new().await;
    let stream = server.stream("/tone.mp3", Protocol::Progressive, 1200);
    let (mut worker, recorded, events) = worker();
    tokio::task::spawn_blocking(move || {
        load(&mut worker, stream, 0, false);
        assert_eq!(worker.state.playback, Playback::Paused);
        assert_eq!(worker.state.position_at, None);
        worker.apply(Command::Play).unwrap();
        for _ in 0..4 {
            worker.pump().unwrap();
        }
        worker.apply(Command::Pause).unwrap();
        let paused = worker.state.position_now();
        assert!(paused > 0);
        assert!(!recorded.lock().unwrap().running);
        assert_eq!(worker.state.position_now(), paused);
        worker.apply(Command::Toggle).unwrap();
        worker.pump().unwrap();
        assert!(worker.position() > paused);
        worker.apply(Command::Seek(700)).unwrap();
        assert_eq!(worker.state.position_ms, 700);
        finish(&mut worker);
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::EndOfTrack { .. }))
        );
        let pcm = recorded.lock().unwrap();
        assert!(pcm.pcm.iter().any(|s| s.abs() > 0.01));
        assert!(
            pcm.pcm.as_chunks::<2>().0.iter().all(|s| s[0] == s[1]),
            "mono is duplicated to stereo"
        );
        assert!(!pcm.running);
        drop(pcm);
        worker.apply(Command::Stop).unwrap();
        assert!(!recorded.lock().unwrap().claimed);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn tagged_loads_stamp_each_completion_and_the_release_acknowledgement() {
    let server = Server::new().await;
    let stream = server.stream("/tone.mp3", Protocol::Progressive, 1200);
    let (mut worker, recorded, events) = worker();
    tokio::task::spawn_blocking(move || {
        for generation in [1, 3] {
            let tag = crate::player::PlaybackTag { generation, occurrence: crate::hybrid_queue::EntryId(5) };
            worker.apply(Command::Tagged { tag, command: Box::new(Command::Load { stream: stream.clone(), start_position_ms: 0, autoplay: true }) }).unwrap();
            finish(&mut worker);
        }
        worker.apply(Command::Stop).unwrap();
        assert!(!recorded.lock().unwrap().claimed);
        let events = events.lock().unwrap();
        let completions: Vec<_> = events.iter().filter_map(|event| match event {
            Event::Tagged { tag, event } if matches!(event.as_ref(), Event::EndOfTrack { .. }) => Some(tag.generation),
            _ => None,
        }).collect();
        assert_eq!(completions, [1, 3]);
        assert!(matches!(events.last(), Some(Event::Tagged { tag, event }) if tag.generation == 3 && matches!(event.as_ref(), Event::OutputReleased)));
    }).await.unwrap();
}

#[tokio::test]
async fn hls_seek_starts_at_the_later_segment_and_decodes_forward() {
    let server = Server::new().await;
    let stream = server.stream("/aac.m3u8", Protocol::Hls, 1223);
    let requests = Arc::clone(&server.requests);
    let (mut worker, recorded, _) = worker();
    tokio::task::spawn_blocking(move || {
        load(&mut worker, stream, 900, true);
        assert_eq!(worker.state.position_ms, 900);
        finish(&mut worker);
        let requests = requests.lock().unwrap();
        assert!(requests.iter().any(|s| s.ends_with("aac2.m4s")));
        assert!(
            !requests
                .iter()
                .any(|s| s.ends_with("aac0.m4s") || s.ends_with("aac1.m4s"))
        );
        let frames = recorded.lock().unwrap().pcm.len() / 2;
        assert!(
            (13_000..15_000).contains(&frames),
            "exact suffix has {frames} frames"
        );
        assert_eq!(
            frames,
            53 * 1024 - 900 * 44_100 / 1_000,
            "seeking skips precisely 900 ms of the 53 AAC frames"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn hls_master_adts_and_mp3_segments_decode() {
    let server = Server::new().await;
    let streams: Vec<_> = ["/master.m3u8", "/adts.m3u8", "/mp3.m3u8"]
        .iter()
        .map(|path| server.stream(path, Protocol::Hls, 3000))
        .collect();
    let (mut worker, recorded, _) = worker();
    tokio::task::spawn_blocking(move || {
        for stream in streams {
            let before = recorded.lock().unwrap().pcm.len();
            load(&mut worker, stream, 0, true);
            finish(&mut worker);
            assert!(recorded.lock().unwrap().pcm.len() > before + 1000);
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_preloaded_decoder_is_reused_by_the_next_load() {
    let server = Server::new().await;
    let stream = server.stream("/next.mp3", Protocol::Progressive, 1200);
    let requests = Arc::clone(&server.requests);
    let (mut worker, _, _) = worker();
    tokio::task::spawn_blocking(move || {
        worker.apply(Command::Preload(stream.clone())).unwrap();
        // Wait for preparation deterministically, without consuming its result.
        worker.fetch.runtime.block_on(async {
            while !worker.preload.as_ref().unwrap().task.is_finished() {
                tokio::task::yield_now().await;
            }
        });
        let count = requests.lock().unwrap().len();
        assert!(count > 0);
        // An interrupted old decoder must not discard a ready successor
        // before the queued Load gets a chance to consume it.
        worker.fetch.interrupt.revision.store(1, Ordering::SeqCst);
        worker.fetch.revision = Arc::new(AtomicU64::new(1));
        worker.failed(Error::Cancelled);
        assert!(worker.preload.is_some());
        load(&mut worker, stream, 0, true);
        assert_eq!(requests.lock().unwrap().len(), count);
        finish(&mut worker);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn expired_hls_url_requests_refresh_and_resumes_the_same_position() {
    let server = Server::new().await;
    let expired = server.stream("/expired/aac.m3u8", Protocol::Hls, 1223);
    let fresh = server.stream("/fresh/aac.m3u8", Protocol::Hls, 1223);
    let (mut worker, _, events) = worker();
    tokio::task::spawn_blocking(move || {
        load(&mut worker, expired, 0, true);
        loop {
            if let Err(error) = worker.pump() {
                assert_eq!(error, Error::Expired);
                worker.failed(error);
                break;
            }
        }
        let request = worker.refresh.unwrap();
        let position = worker.state.position_ms;
        assert!(position > 350 && position < 500, "position {position}");
        assert_eq!(worker.state.position_at, None);
        worker
            .apply(Command::Refresh {
                request_id: request + 1,
                stream: fresh.clone(),
            })
            .unwrap();
        assert_eq!(worker.refresh, Some(request));
        worker.apply(Command::Pause).unwrap();
        worker
            .apply(Command::Refresh {
                request_id: request,
                stream: fresh,
            })
            .unwrap();
        assert_eq!(worker.state.playback, Playback::Paused);
        assert_eq!(worker.state.position_ms, position);
        worker.apply(Command::Play).unwrap();
        finish(&mut worker);
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::RefreshRequired { .. }))
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn progressive_source_uses_ranges_and_is_seekable() {
    let server = Server::new().await;
    let url = format!("{}/range-only.mp3", server.base);
    let (worker, _, _) = worker();
    tokio::task::spawn_blocking(move || {
        let mut source =
            source::RangeSource::new(worker.fetch, url, Arc::new(Mutex::new(None))).unwrap();
        source.seek(SeekFrom::End(-12)).unwrap();
        let mut last = [0; 12];
        source.read_exact(&mut last).unwrap();
        assert_eq!(&last, &MP3[MP3.len() - 12..]);
        source.seek(SeekFrom::Start(40)).unwrap();
        source.read_exact(&mut last).unwrap();
        assert_eq!(&last, &MP3[40..52]);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn opus_is_explicitly_unsupported_and_urls_are_redacted() {
    let server = Server::new().await;
    let mut stream = server.stream("/never?secret=token", Protocol::Hls, 1000);
    stream.mime = "audio/ogg; codecs=opus".into();
    assert!(!format!("{stream:?}").contains("secret"));
    let (worker, _, _) = worker();
    tokio::task::spawn_blocking(move || {
        assert!(matches!(
            Decoder::open(worker.fetch, &stream, 0),
            Err(Error::Unsupported(_))
        ));
    })
    .await
    .unwrap();
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_paused_engine_parks_until_play_and_stop_releases_ownership() {
    let server = Server::new().await;
    let stream = server.stream("/tone.mp3", Protocol::Progressive, 1200);
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let (notify, mut events) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::with_output(
        Handle::current(),
        crate::http::Http::new(reqwest::Client::builder().no_proxy().build().unwrap()),
        Box::new(TestOutput(Arc::clone(&recorded))),
        Arc::new(move |event| {
            let _ = notify.send(event);
        }),
    )
    .unwrap();
    assert!(engine.command(Command::Load {
        stream,
        start_position_ms: 0,
        autoplay: false
    }));
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = events.recv().await {
            if matches!(event, Event::State(ref state) if state.playback == Playback::Paused && !state.loading) { break; }
        }
    }).await.unwrap();
    let requests = server.requests.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(server.requests.lock().unwrap().len(), requests);
    assert_eq!(recorded.lock().unwrap().writes, 0);
    assert_eq!(engine.state().position_ms, 0);
    assert!(engine.command(Command::Play));
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = events.recv().await {
            if matches!(event, Event::EndOfTrack { .. }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(recorded.lock().unwrap().claimed, "EOF retains ownership");
    assert!(engine.command(Command::Stop));
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = events.recv().await {
            if matches!(event, Event::OutputReleased) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(!recorded.lock().unwrap().claimed);
    assert_eq!(engine.state().playback, Playback::Stopped);
}

#[tokio::test]
async fn pause_cancels_a_stalled_load_and_preload_does_no_work_while_paused() {
    let server = Server::new().await;
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let (notify, mut events) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::with_output(
        Handle::current(),
        crate::http::Http::new(reqwest::Client::builder().no_proxy().build().unwrap()),
        Box::new(TestOutput(Arc::clone(&recorded))),
        Arc::new(move |event| {
            let _ = notify.send(event);
        }),
    )
    .unwrap();
    engine.command(Command::Load {
        stream: server.stream("/slow.mp3", Protocol::Progressive, 1200),
        start_position_ms: 0,
        autoplay: true,
    });
    server.wait_for("/slow.mp3").await;
    engine.command(Command::Pause);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await.unwrap() {
                Event::Error { error, .. } => {
                    panic!("cancellation became a playback error: {error}")
                }
                Event::State(state) if state.playback == Playback::Paused && !state.loading => {
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    server.wait_for("/cancelled").await;
    engine.command(Command::Preload(server.stream(
        "/next.mp3",
        Protocol::Progressive,
        1200,
    )));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|s| s == "/next.mp3")
    );
    assert_eq!(recorded.lock().unwrap().writes, 0);
    assert_eq!(engine.state().position_now(), 0);
}

#[tokio::test]
async fn stop_cancels_a_preload_that_already_started_on_a_blocking_thread() {
    let server = Server::new().await;
    let (mut worker, _, _) = worker();
    worker
        .apply(Command::Preload(server.stream(
            "/slow.mp3",
            Protocol::Progressive,
            1200,
        )))
        .unwrap();
    server.wait_for("/slow.mp3").await;
    worker.apply(Command::Stop).unwrap();
    server.wait_for("/cancelled").await;
}

#[tokio::test]
async fn a_preloaded_successor_keeps_the_output_tail_and_uses_the_new_command_revision() {
    let server = Server::new().await;
    let first = server.stream("/tone.mp3", Protocol::Progressive, 1200);
    let mut next = server.stream("/next.mp3", Protocol::Progressive, 1200);
    next.uri = "soundcloud:track:2".into();
    let (mut worker, recorded, _) = worker();
    tokio::task::spawn_blocking(move || {
        load(&mut worker, first, 0, true);
        worker.apply(Command::Preload(next.clone())).unwrap();
        worker.fetch.runtime.block_on(async {
            while !worker.preload.as_ref().unwrap().task.is_finished() {
                tokio::task::yield_now().await;
            }
        });
        recorded.lock().unwrap().pending_ms = 100;
        finish(&mut worker);
        assert!(worker.tail);
        let (pauses, resets) = {
            let recorded = recorded.lock().unwrap();
            (recorded.pauses, recorded.resets)
        };
        worker.fetch.interrupt.revision.store(1, Ordering::SeqCst);
        worker.fetch.revision = Arc::new(AtomicU64::new(1));
        load(&mut worker, next, 0, true);
        assert_eq!(recorded.lock().unwrap().pauses, pauses);
        assert_eq!(recorded.lock().unwrap().resets, resets);
        worker.pump().unwrap();
        recorded.lock().unwrap().pending_ms = 0;
        finish(&mut worker);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn an_unknown_duration_does_not_clamp_a_requested_seek_to_zero() {
    let server = Server::new().await;
    let stream = server.stream("/tone.mp3", Protocol::Progressive, 0);
    let (mut worker, _, _) = worker();
    tokio::task::spawn_blocking(move || {
        load(&mut worker, stream, 500, false);
        assert_eq!(worker.state.position_ms, 500);
        worker.apply(Command::Seek(700)).unwrap();
        assert_eq!(worker.state.position_ms, 700);
        worker.apply(Command::Play).unwrap();
        finish(&mut worker);
        assert!(worker.state.position_ms > 1000);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn later_progressive_ranges_are_fetched_and_404_propagates_for_refresh() {
    let server = Server::new().await;
    let fresh = format!("{}/large.mp3", server.base);
    let expired = format!("{}/expires-on-range.mp3", server.base);
    let (worker, _, _) = worker();
    tokio::task::spawn_blocking(move || {
        let error = Arc::new(Mutex::new(None));
        let mut source =
            source::RangeSource::new(worker.fetch.clone(), fresh, Arc::clone(&error)).unwrap();
        source.seek(SeekFrom::Start(40_000)).unwrap();
        let mut bytes = [0; 12];
        source.read_exact(&mut bytes).unwrap();
        let at = 40_000 % MP3.len();
        assert_eq!(&bytes, &MP3[at..at + 12]);
        let mut source =
            source::RangeSource::new(worker.fetch, expired, Arc::clone(&error)).unwrap();
        source.seek(SeekFrom::Start(40_000)).unwrap();
        assert!(source.read_exact(&mut bytes).is_err());
        assert_eq!(*error.lock().unwrap(), Some(Error::Expired));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn paused_preload_waits_for_play_and_hls_does_not_look_ahead() {
    let server = Server::new().await;
    let stream = server.stream("/aac.m3u8", Protocol::Hls, 1223);
    let next = server.stream("/next.mp3", Protocol::Progressive, 1200);
    let requests = Arc::clone(&server.requests);
    let (mut worker, _, _) = worker();
    tokio::task::spawn_blocking(move || {
        load(&mut worker, stream, 0, false);
        assert!(!worker.playing);
        worker.apply(Command::Preload(next)).unwrap();
        assert!(worker.preload.is_none());
        assert!(worker.deferred_preload.is_some());
        assert!(
            !requests
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.ends_with("aac1.m4s") || s == "/next.mp3")
        );
        worker.apply(Command::Play).unwrap();
        worker.fetch.runtime.block_on(async {
            while !worker.preload.as_ref().unwrap().task.is_finished() {
                tokio::task::yield_now().await;
            }
        });
        assert!(worker.deferred_preload.is_none());
        assert!(requests.lock().unwrap().iter().any(|s| s == "/next.mp3"));
    })
    .await
    .unwrap();
}
