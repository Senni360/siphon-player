//! Working out one song's BPM and key from its audio.
//!
//! A row's BPM and key tool asks for a single song. The audio comes from the
//! file that song downloaded to, or from the same SoundCloud or YouTube
//! stream that playback would use. Decoding and the analysis run on a
//! blocking worker, never on the interface thread or the runtime's workers,
//! and the answer comes back as an event. Nothing here writes audio to disk
//! or sends it anywhere.

use super::*;

/// How much of a song is measured. Enough for several bars at any tempo the
/// analyser names, and short enough that a click answers in seconds.
const SAMPLE_SECS: f64 = 30.0;

/// Below this the reading is a guess, so it is not shown at all.
const MIN_SECS: f64 = 4.0;

impl Worker {
    /// Starts measuring `uri`. `file` is a downloaded or local copy, which
    /// needs no network; `link` is a SoundCloud share link for a private
    /// track whose id alone does not resolve.
    pub(super) fn analyze_bpm_key(
        &mut self,
        uri: String,
        request: u64,
        file: Option<std::path::PathBuf>,
        link: Option<String>,
    ) {
        let events = self.events.clone();
        let waker = self.waker.clone();
        if let Some(path) = file {
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || from_file(&path))
                    .await
                    .unwrap_or_else(|_| Err(ANALYSIS_STOPPED.into()));
                let _ = events.send(Event::BpmKey {
                    uri,
                    request,
                    result,
                });
                waker.wake();
            });
            return;
        }
        let http = self.http.clone();
        let client = self.soundcloud_client();
        let catalogue = self.youtube_catalogue();
        tokio::spawn(async move {
            let _ = events.send(Event::BpmKeyProgress {
                uri: uri.clone(),
                request,
                stage: crate::bpm_key::Stage::Finding,
            });
            waker.wake();
            let result = match stream_for(&uri, link, client, catalogue).await {
                Ok(stream) => {
                    let _ = events.send(Event::BpmKeyProgress {
                        uri: uri.clone(),
                        request,
                        stage: crate::bpm_key::Stage::Listening,
                    });
                    waker.wake();
                    let handle = tokio::runtime::Handle::current();
                    tokio::task::spawn_blocking(move || from_stream(&http, handle, &stream))
                        .await
                        .unwrap_or_else(|_| Err(ANALYSIS_STOPPED.into()))
                }
                Err(error) => Err(error),
            };
            let _ = events.send(Event::BpmKey {
                uri,
                request,
                result,
            });
            waker.wake();
        });
    }
}

const ANALYSIS_STOPPED: &str = "The analysis stopped before it could answer.";

/// The stream playback would use for `uri`, so the analyser hears the same
/// audio. Spotify and shared group songs are refused before this point.
async fn stream_for(
    uri: &str,
    link: Option<String>,
    client: Option<crate::soundcloud::api::Client>,
    catalogue: crate::youtube::Catalogue,
) -> Result<crate::soundcloud::engine::Stream, String> {
    if crate::youtube::is_track_uri(uri) {
        let (track, url) = catalogue
            .playable(uri, false)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(crate::soundcloud::engine::Stream {
            uri: uri.to_owned(),
            url,
            protocol: crate::soundcloud::engine::Protocol::Progressive,
            mime: "audio/mp4".into(),
            duration_ms: track.duration_ms,
            is_preview: false,
        });
    }
    if crate::soundcloud::is_track_uri(uri) {
        let client = client.ok_or("Connect SoundCloud to measure its songs' BPM and key.")?;
        let track = client
            .track_for_playback(uri, link.as_deref(), None)
            .await
            .map_err(|error| error.to_string())?;
        let info = client
            .resolve_stream(&track)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(crate::soundcloud::engine::Stream {
            uri: uri.to_owned(),
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
            is_preview: info.is_preview,
        });
    }
    Err(crate::bpm_key::unhearable(uri)
        .unwrap_or("Encore can't hear this song's audio to measure BPM and key.")
        .to_owned())
}

fn from_file(path: &std::path::Path) -> Result<crate::beat_analysis::Reading, String> {
    let (mono, rate) = crate::audio_probe::mono(path, SAMPLE_SECS)?;
    answer(&mono, rate)
}

fn from_stream(
    http: &crate::http::Http,
    handle: tokio::runtime::Handle,
    stream: &crate::soundcloud::engine::Stream,
) -> Result<crate::beat_analysis::Reading, String> {
    let mono = crate::soundcloud::engine::sample(http, handle, stream, SAMPLE_SECS)?;
    // Four source frames averaged into one, as the passive collector does:
    // the analyser needs no more than 11 kHz and a quarter of the work.
    let mono: Vec<f32> = mono
        .as_chunks::<4>()
        .0
        .iter()
        .map(|four| (four[0] + four[1] + four[2] + four[3]) / 4.0)
        .collect();
    answer(&mono, crate::beat_analysis::RATE)
}

fn answer(mono: &[f32], rate: u32) -> Result<crate::beat_analysis::Reading, String> {
    if rate == 0 || mono.len() < (f64::from(rate) * MIN_SECS) as usize {
        return Err(
            "There wasn't enough sound to hear a beat. Play more of the song and try again.".into(),
        );
    }
    crate::beat_analysis::analyze(mono, rate)
        .ok_or_else(|| "Encore couldn't hear a beat in this audio.".to_string())
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
        let samples = clicks(20.0, 140.0, crate::beat_analysis::RATE);
        let reading = answer(&samples, crate::beat_analysis::RATE).expect("a click track answers");
        assert!((f64::from(reading.bpm) - 140.0).abs() <= 1.5, "{reading:?}");
    }

    #[test]
    fn too_little_sound_says_so_instead_of_guessing() {
        let samples = clicks(1.0, 140.0, crate::beat_analysis::RATE);
        let error = answer(&samples, crate::beat_analysis::RATE).unwrap_err();
        assert!(error.contains("enough sound"), "{error}");
    }
}
