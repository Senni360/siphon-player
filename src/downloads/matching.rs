//! Conservative metadata matching. Spotify supplies descriptions, never audio.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::api::models::Track;

#[derive(Clone, Debug)]
pub(super) struct Candidate {
    pub url: String,
    pub title: String,
    pub uploader: String,
    pub duration: Option<f64>,
    pub live: bool,
}

impl Candidate {
    pub fn from_probe(value: &Value) -> Option<Self> {
        let url = value["webpageUrl"]
            .as_str()
            .or_else(|| value["url"].as_str())?;
        Some(Self {
            url: youtube_url(url)?,
            title: value["title"].as_str()?.into(),
            uploader: value["uploader"].as_str().unwrap_or("").into(),
            duration: value["duration"].as_f64(),
            live: value["isLive"].as_bool().unwrap_or(false),
        })
    }
}

pub(super) fn youtube_url(text: &str) -> Option<String> {
    let url = reqwest::Url::parse(text).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let id = match url.host_str()? {
        "youtube.com" | "www.youtube.com" | "music.youtube.com" if url.path() == "/watch" => url
            .query_pairs()
            .find(|(key, _)| key == "v")?
            .1
            .into_owned(),
        "youtu.be" => url.path().strip_prefix('/')?.to_string(),
        _ => return None,
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return None;
    }
    Some(format!("https://www.youtube.com/watch?v={id}"))
}

pub(super) fn search_url(track: &Track) -> String {
    let query = format!("{} - {}", track.artist_names(), track.name);
    // The helper rejects ytsearch pseudo-URLs. yt-dlp's search-results
    // extractor accepts this real HTTP URL through the existing probe API.
    format!(
        "https://www.youtube.com/results?search_query={}",
        urlencoding::encode(&query)
    )
}

fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn coverage(wanted: &BTreeSet<String>, actual: &BTreeSet<String>) -> f64 {
    if wanted.is_empty() {
        return 0.0;
    }
    wanted.intersection(actual).count() as f64 / wanted.len() as f64
}

pub(super) fn score(track: &Track, candidate: &Candidate) -> Option<f64> {
    let duration = candidate.duration.filter(|d| d.is_finite() && *d > 0.0)?;
    let difference = (duration - f64::from(track.duration_ms) / 1000.0).abs();
    if track.duration_ms == 0 || difference > 3.0 || candidate.live {
        return None;
    }
    let title = words(&track.name);
    let artists = words(&track.artist_names());
    let primary = words(&track.artists.first()?.name);
    let actual_title = words(&candidate.title);
    let actual = words(&format!("{} {}", candidate.title, candidate.uploader));
    if coverage(&title, &actual_title) < 0.9 || coverage(&primary, &actual) < 1.0 {
        return None;
    }
    let expected: BTreeSet<_> = title.union(&artists).cloned().collect();
    // Do not turn a studio song into a cover, live version, remix, or speed
    // edit just because the duration happens to match. Compare both ways.
    for qualifier in [
        "live",
        "remix",
        "acoustic",
        "cover",
        "karaoke",
        "instrumental",
        "remaster",
        "remastered",
        "slowed",
        "sped",
        "nightcore",
        "reverb",
        "mashup",
        "tribute",
        "demo",
        "extended",
        "edit",
    ] {
        if expected.contains(qualifier) != actual_title.contains(qualifier) {
            return None;
        }
    }
    let mut meaningful = actual_title;
    for boilerplate in [
        "official",
        "audio",
        "video",
        "music",
        "lyrics",
        "lyric",
        "visualizer",
        "hd",
        "hq",
        "4k",
    ] {
        if !expected.contains(boilerplate) {
            meaningful.remove(boilerplate);
        }
    }
    let precision = coverage(&meaningful, &expected);
    if precision < 0.75 {
        return None;
    }
    Some(0.5 * precision + 0.35 * coverage(&artists, &actual) + 0.15 * (1.0 - difference / 3.0))
}

pub(super) fn best_match<'a>(track: &Track, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    candidates
        .iter()
        .filter_map(|candidate| score(track, candidate).map(|score| (candidate, score)))
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(candidate, _)| candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::ArtistRef;

    fn track() -> Track {
        Track {
            uri: "spotify:track:abc".into(),
            name: "Northern Lights".into(),
            duration_ms: 200_000,
            artists: vec![ArtistRef {
                name: "Aurora".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    fn candidate(title: &str, uploader: &str, duration: Option<f64>) -> Candidate {
        Candidate {
            title: title.into(),
            uploader: uploader.into(),
            duration,
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            live: false,
        }
    }

    #[test]
    fn prefers_metadata_and_duration_and_accepts_topic_uploads() {
        let candidates = [
            candidate(
                "Aurora - Northern Lights (Official Audio)",
                "Aurora",
                Some(202.9),
            ),
            candidate("Northern Lights", "Aurora - Topic", Some(200.0)),
            candidate("Northern Lights", "Someone else", Some(200.0)),
        ];
        assert_eq!(
            best_match(&track(), &candidates).unwrap().uploader,
            "Aurora - Topic"
        );
    }

    #[test]
    fn refuses_missing_duration_wrong_artist_titles_and_alternate_recordings() {
        for candidate in [
            candidate("Northern Lights", "Aurora", None),
            candidate("Northern Lights", "Aurora", Some(203.1)),
            candidate("Northern Lights", "Unknown", Some(200.0)),
            candidate("Southern Lights", "Aurora", Some(200.0)),
            candidate("Aurora - Northern Lights (Live)", "Aurora", Some(200.0)),
            candidate("Aurora - Northern Lights (Remix)", "Aurora", Some(200.0)),
            candidate("Northern Lights (Cover)", "Aurora", Some(200.0)),
            candidate("Northern Lights (Sped Up)", "Aurora", Some(200.0)),
            candidate(
                "Northern Lights Full Album Compilation",
                "Aurora",
                Some(200.0),
            ),
        ] {
            assert!(score(&track(), &candidate).is_none(), "{candidate:?}");
        }
        assert!(best_match(&track(), &[]).is_none());
    }

    #[test]
    fn preserves_explicit_versions_and_unicode_names() {
        let mut track = track();
        track.name = "Northern Lights (Acoustic)".into();
        assert!(score(&track, &candidate("Northern Lights", "Aurora", Some(200.0))).is_none());
        assert!(score(&track, &candidate(&track.name, "Aurora", Some(200.0))).is_some());
        track.name = "Jóga".into();
        track.artists[0].name = "Björk".into();
        assert!(score(&track, &candidate("BJÖRK - JÓGA", "Björk", Some(197.0))).is_some());
    }

    #[test]
    fn search_is_an_encoded_http_url_and_download_urls_are_canonical_youtube_only() {
        assert!(search_url(&track()).ends_with("Aurora%20-%20Northern%20Lights"));
        assert_eq!(
            youtube_url("https://youtu.be/abcdefghijk?t=4").unwrap(),
            "https://www.youtube.com/watch?v=abcdefghijk"
        );
        for url in [
            "http://youtube.com/watch?v=abcdefghijk",
            "https://youtube.com.evil/watch?v=abcdefghijk",
            "https://youtube.com/results?q=x",
            "https://youtube.com/watch?v=bad",
            "ytsearch5:test",
        ] {
            assert!(youtube_url(url).is_none());
        }
    }
}
