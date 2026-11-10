//! Working out one song's BPM and key from its audio.
//!
//! A row's BPM and key tool asks for a single song. The audio comes from the
//! file that song downloaded to, or from the same SoundCloud or YouTube
//! stream that playback would use. Decoding and the analysis run on a
//! blocking worker, never on the interface thread or the runtime's workers,
//! and the answer comes back as an event. At most two run at once, and a
//! cancelled request stops between packets. Nothing here writes audio to
//! disk or sends it anywhere.
//!
//! Spotify's own audio never reaches Encore. A Spotify song can still be
//! measured from a download the app already has, or from the match the
//! download matcher remembered for it; with neither, the answer says why.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Semaphore;

use super::*;

/// How much of a song is measured. Enough for several bars at any tempo the
/// analyser names, and short enough that a click answers in seconds.
const SAMPLE_SECS: f64 = 30.0;

/// Below this the reading is a guess, so it is not shown at all.
const MIN_SECS: f64 = 4.0;

/// More than two at once would make the machine fan noise for no gain.
const MAX_RUNNING: usize = 2;

/// The running analyses, so a stop can reach one and a finished one can
/// forget its flag.
pub(super) struct Jobs {
    cancels: Mutex<BTreeMap<u64, Arc<AtomicBool>>>,
    permits: Arc<Semaphore>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            cancels: Mutex::new(BTreeMap::new()),
            permits: Arc::new(Semaphore::new(MAX_RUNNING)),
        }
    }
}

impl Jobs {
    fn start(&self, request: u64) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request, Arc::clone(&cancel));
        cancel
    }

    pub(super) fn cancel(&self, request: u64) {
        if let Some(cancel) = self
            .cancels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request)
        {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    fn finish(&self, request: u64) {
        self.cancels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request);
    }

    fn cancel_all(&self) {
        let mut cancels = self.cancels.lock().unwrap_or_else(PoisonError::into_inner);
        for cancel in cancels.values() {
            cancel.store(true, Ordering::SeqCst);
        }
        cancels.clear();
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

/// Forgets the request however its task leaves, including a panic in the
/// blocking decoder.
struct Done {
    jobs: Arc<Jobs>,
    request: u64,
}

impl Drop for Done {
    fn drop(&mut self) {
        self.jobs.finish(self.request);
    }
}

impl Worker {
    /// Starts measuring `uri`. `file` is a downloaded or local copy, which
    /// needs no network; `link` is a SoundCloud share link for a private
    /// track whose id alone does not resolve.
    pub(super) fn analyze_bpm_key(
        &self,
        uri: String,
        request: u64,
        file: Option<std::path::PathBuf>,
        link: Option<String>,
    ) {
        let cancel = self.bpm_key_jobs.start(request);
        let jobs = Arc::clone(&self.bpm_key_jobs);
        let events = self.events.clone();
        let waker = self.waker.clone();
        let http = self.http.clone();
        let client = self.soundcloud_client();
        let catalogue = self.youtube_catalogue();
        let saved = self.saved_matches.clone();
        let permits = Arc::clone(&jobs.permits);
        tokio::spawn(async move {
            let _done = Done {
                jobs: Arc::clone(&jobs),
                request,
            };
            let _ = events.send(Event::BpmKeyProgress {
                uri: uri.clone(),
                request,
                stage: crate::bpm_key::Stage::Finding,
            });
            waker.wake();
            let Ok(_permit) = permits.acquire_owned().await else {
                return;
            };
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            let result = match file {
                Some(path) => {
                    let _ = events.send(Event::BpmKeyProgress {
                        uri: uri.clone(),
                        request,
                        stage: crate::bpm_key::Stage::Listening,
                    });
                    waker.wake();
                    let reading = cancel.clone();
                    tokio::task::spawn_blocking(move || from_file(&path, &reading))
                        .await
                        .unwrap_or_else(|_| Err(crate::bpm_key::CANCELLED.into()))
                }
                None => match stream_for(&uri, link, client, catalogue, saved).await {
                    Ok(stream) => {
                        let _ = events.send(Event::BpmKeyProgress {
                            uri: uri.clone(),
                            request,
                            stage: crate::bpm_key::Stage::Listening,
                        });
                        waker.wake();
                        let handle = tokio::runtime::Handle::current();
                        let reading = cancel.clone();
                        tokio::task::spawn_blocking(move || {
                            from_stream(&http, handle, &stream, &reading)
                        })
                        .await
                        .unwrap_or_else(|_| Err(crate::bpm_key::CANCELLED.into()))
                    }
                    Err(error) => Err(error),
                },
            };
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            let _ = events.send(Event::BpmKey {
                uri,
                request,
                result,
            });
            waker.wake();
        });
    }
}

/// The stream playback would use for `uri`, so the analyser hears the same
/// audio. Spotify is resolved through the download matcher's remembered
/// match; a song with neither a file nor a match is refused with the reason.
async fn stream_for(
    uri: &str,
    link: Option<String>,
    client: Option<crate::soundcloud::api::Client>,
    catalogue: crate::youtube::Catalogue,
    saved: crate::downloads::saved::Saved,
) -> Result<crate::soundcloud::engine::Stream, String> {
    let mut target = uri.to_owned();
    let mut share = link;
    if target.starts_with("spotify:") {
        let found = saved
            .get(&target)
            .await
            .ok_or_else(|| crate::bpm_key::SPOTIFY_UNAVAILABLE.to_owned())?;
        if found.uri.trim().is_empty() {
            return Err(crate::bpm_key::SPOTIFY_UNAVAILABLE.into());
        }
        target = found.uri;
        share = None;
    }
    if crate::youtube::is_track_uri(&target) {
        let (track, url) = catalogue
            .playable(&target, false)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(crate::soundcloud::engine::Stream {
            uri: target,
            url,
            protocol: crate::soundcloud::engine::Protocol::Progressive,
            mime: "audio/mp4".into(),
            duration_ms: track.duration_ms,
            is_preview: false,
        });
    }
    if crate::soundcloud::is_track_uri(&target) {
        let client = client.ok_or("Connect SoundCloud to measure its songs' BPM and key.")?;
        let track = client
            .track_for_playback(&target, share.as_deref(), None)
            .await
            .map_err(|error| error.to_string())?;
        let info = client
            .resolve_stream(&track)
            .await
            .map_err(|error| error.to_string())?;
        if info.is_preview {
            return Err("SoundCloud only plays a preview of this song.".into());
        }
        return Ok(crate::soundcloud::engine::Stream {
            uri: target,
            url: info.url,
            protocol: match info.protocol {
                crate::soundcloud::models::Protocol::Hls => {
                    crate::soundcloud::engine::Protocol::Hls
                }
                crate::soundcloud::models::Protocol::Progressive => {
                    crate::soundcloud::engine::Protocol::Progressive
                }
            },
            mime: info.mime,
            duration_ms: info.duration_ms,
            is_preview: false,
        });
    }
    Err(crate::bpm_key::unavailable(&target)
        .unwrap_or("Encore can't hear this song's audio to measure BPM and key.")
        .to_owned())
}

fn from_file(
    path: &std::path::Path,
    cancel: &AtomicBool,
) -> Result<crate::bpm_key::Reading, String> {
    let (mono, rate) = crate::audio_probe::mono(path, SAMPLE_SECS, cancel)?;
    answer(&mono, rate, cancel)
}

fn from_stream(
    http: &crate::http::Http,
    handle: tokio::runtime::Handle,
    stream: &crate::soundcloud::engine::Stream,
    cancel: &AtomicBool,
) -> Result<crate::bpm_key::Reading, String> {
    let mono = crate::soundcloud::engine::sample(http, handle, stream, SAMPLE_SECS, cancel)?;
    answer(&mono, 44_100, cancel)
}

fn answer(mono: &[f32], rate: u32, cancel: &AtomicBool) -> Result<crate::bpm_key::Reading, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(crate::bpm_key::CANCELLED.into());
    }
    if rate == 0 || mono.len() < (f64::from(rate) * MIN_SECS) as usize {
        return Err(
            "There wasn't enough sound to hear a beat. Play more of the song and try again.".into(),
        );
    }
    match crate::bpm_key::analyze_cancellable(mono, rate, cancel) {
        Some(reading) => Ok(reading),
        None if cancel.load(Ordering::SeqCst) => Err(crate::bpm_key::CANCELLED.into()),
        None => Err("Encore couldn't hear a beat in this audio.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks(seconds: f64, bpm: f64, rate: u32) -> Vec<f32> {
        let len = (seconds * f64::from(rate)).round() as usize;
        let mut out = vec![0.0_f32; len];
        let period = f64::from(rate) * 60.0 / bpm;
        let onset = 512_usize;
        let mut next = 0.0_f64;
        while next < len as f64 {
            let at = next.round() as usize;
            for i in 0..onset {
                if at + i < len {
                    let decay = (-(i as f32) / onset as f32 * 7.0).exp();
                    let noise = (((i * 2654435761_usize) % 1000) as f32 / 1000.0) - 0.5;
                    out[at + i] += 2.0 * noise * decay;
                }
            }
            next += period;
        }
        out
    }

    #[test]
    fn a_sampled_song_answers_with_its_tempo() {
        let samples = clicks(20.0, 140.0, crate::bpm_key::RATE);
        let reading = answer(&samples, crate::bpm_key::RATE, &AtomicBool::new(false))
            .expect("a click track answers");
        assert!((f64::from(reading.bpm) - 140.0).abs() <= 1.0, "{reading:?}");
    }

    #[test]
    fn too_little_sound_says_so_instead_of_guessing() {
        let samples = clicks(1.0, 140.0, crate::bpm_key::RATE);
        let error = answer(&samples, crate::bpm_key::RATE, &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("enough sound"), "{error}");
    }

    #[test]
    fn a_cancelled_answer_stops_before_reading_the_audio() {
        let samples = clicks(20.0, 140.0, crate::bpm_key::RATE);
        let error = answer(&samples, crate::bpm_key::RATE, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(error, crate::bpm_key::CANCELLED);
    }

    #[tokio::test]
    async fn spotify_without_a_saved_match_says_why() {
        let catalogue = crate::youtube::Catalogue {
            resolver: Default::default(),
            cache: crate::test_dirs::temp_dir(),
            links: Default::default(),
        };
        let error = stream_for(
            "spotify:track:no-match",
            None,
            None,
            catalogue,
            crate::downloads::saved::Saved::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, crate::bpm_key::SPOTIFY_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_cancelled_job_is_forgotten_and_flagged() {
        let jobs = Arc::new(Jobs::default());
        let cancel = jobs.start(7);
        jobs.cancel(7);
        assert!(cancel.load(Ordering::SeqCst));
        assert!(jobs.cancels.lock().unwrap().is_empty());
    }
}
