//! Conservative metadata matching. Spotify supplies descriptions, never audio.

use std::collections::BTreeSet;

use crate::api::models::Track;

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    /// The public URL the Siphon helper downloads from.
    pub url: String,
    pub title: String,
    pub uploader: String,
    pub duration: Option<f64>,
    pub live: bool,
}

/// The same song as a Spotify one: what the player calls it and where the
/// download helper fetches it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Match {
    /// The URI the phone plays: a YouTube link or a `soundcloud:track:` URI.
    pub uri: String,
    /// A public URL the Siphon helper accepts for downloading.
    pub source: String,
}

/// Both places that resolve a Spotify song share this one search: the phone's
/// player and the download queue. Prefer the listing's own title, uploader and
/// length; never fetch each candidate separately.
#[derive(Clone)]
pub(crate) struct Searcher {
    catalogue: crate::youtube::Catalogue,
    soundcloud: Option<crate::soundcloud::api::Client>,
}

impl Searcher {
    pub fn new(
        catalogue: crate::youtube::Catalogue,
        soundcloud: Option<crate::soundcloud::api::Client>,
    ) -> Self {
        Self {
            catalogue,
            soundcloud,
        }
    }

    /// One search on YouTube and SoundCloud; the best listing entry that
    /// passes [`score`] wins. None rather than a near miss.
    pub async fn same_song(&self, wanted: &Track) -> Option<Match> {
        let query = format!("{} - {}", wanted.artist_names(), wanted.name);
        let video = crate::youtube::browse::videos_for(&self.catalogue.resolver, &query);
        let sound = async {
            match &self.soundcloud {
                Some(client) => client
                    .search_tracks(&query, 15, 0)
                    .await
                    .map(|page| {
                        page.collection
                            .iter()
                            .filter(|track| !track.to_track(None).preview_only)
                            .map(|track| track.to_track(None))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        };
        let (videos, sounds) = tokio::join!(video, sound);
        pick(wanted, &videos.unwrap_or_default(), &sounds)
    }

    #[cfg(test)]
    pub fn no_network() -> Self {
        Self {
            catalogue: crate::youtube::Catalogue {
                resolver: Default::default(),
                cache: std::path::PathBuf::new(),
                links: std::sync::Arc::new(crate::youtube::Links::default()),
            },
            soundcloud: None,
        }
    }
}

/// The best of the listing entries both sources returned. SoundCloud entries
/// need their public permalink, which is what the download helper fetches.
pub(crate) fn pick(wanted: &Track, videos: &[Track], sounds: &[Track]) -> Option<Match> {
    let mut best: Option<(f64, Match)> = None;
    for track in videos.iter().chain(sounds) {
        let Some(candidate) = candidate(track) else {
            continue;
        };
        let Some(found) = score(wanted, &candidate) else {
            continue;
        };
        if best.as_ref().is_none_or(|(best, _)| found > *best) {
            best = Some((
                found,
                Match {
                    uri: track.uri.clone(),
                    source: candidate.url,
                },
            ));
        }
    }
    best.map(|(_, found)| found)
}

/// A listing entry as the scorer sees it. Its `url` is the download source.
fn candidate(track: &Track) -> Option<Candidate> {
    let url = if crate::youtube::is_track_uri(&track.uri) {
        crate::youtube::permalink(&track.uri)?
    } else if crate::soundcloud::is_track_uri(&track.uri) {
        crate::soundcloud::permalink(track.external_urls.spotify.as_deref())?.to_owned()
    } else {
        return None;
    };
    Some(Candidate {
        url,
        title: track.name.clone(),
        uploader: track
            .artists
            .first()
            .map(|artist| artist.name.clone())
            .unwrap_or_default(),
        duration: (track.duration_ms > 0).then(|| f64::from(track.duration_ms) / 1000.0),
        live: false,
    })
}

#[cfg(test)]
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

pub(crate) fn score(track: &Track, candidate: &Candidate) -> Option<f64> {
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

#[cfg(test)]
pub(crate) fn best_match<'a>(track: &Track, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
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
    fn download_urls_are_canonical_youtube_only() {
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

    #[test]
    fn one_search_picks_from_youtube_and_soundcloud_listings() {
        use crate::api::models::ExternalUrls;
        let listing = |uri: &str, name: &str, channel: &str, duration_ms: u32| Track {
            uri: uri.into(),
            name: name.into(),
            artists: vec![ArtistRef {
                name: channel.into(),
                ..Default::default()
            }],
            duration_ms,
            ..Default::default()
        };
        let mut soundcloud = listing("soundcloud:track:12", "Northern Lights", "Aurora", 200_000);
        soundcloud.external_urls = ExternalUrls {
            spotify: Some("https://soundcloud.com/aurora/northern-lights".into()),
        };
        // A cover on YouTube loses to the real recording on SoundCloud, and
        // the pick carries the permalink the download helper needs.
        let found = pick(
            &track(),
            &[listing(
                "youtube:video:abcdefghijk",
                "Northern Lights (Cover)",
                "Aurora",
                200_000,
            )],
            &[soundcloud.clone()],
        )
        .unwrap();
        assert_eq!(found.uri, "soundcloud:track:12");
        assert_eq!(
            found.source,
            "https://soundcloud.com/aurora/northern-lights"
        );
        // A private or unresolved SoundCloud entry has no download source,
        // so the canonical YouTube listing wins instead.
        soundcloud.external_urls.spotify = None;
        let found = pick(
            &track(),
            &[listing(
                "youtube:video:abcdefghijk",
                "Aurora - Northern Lights",
                "Aurora",
                200_000,
            )],
            &[soundcloud],
        )
        .unwrap();
        assert_eq!(found.uri, "youtube:video:abcdefghijk");
        assert_eq!(
            found.source,
            "https://www.youtube.com/watch?v=abcdefghijk"
        );
        // No qualifying listing means no match at all.
        assert!(pick(&track(), &[], &[]).is_none());
    }

    /// The phone's Spotify songs are found this way on real YouTube:
    /// `cargo test --lib same_song_is_found_on_youtube_for_real -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the helper and the network"]
    async fn same_song_is_found_on_youtube_for_real() {
        let resolver = crate::youtube::Resolver::default();
        for (artist, name, duration_ms) in [
            ("The Weeknd", "Blinding Lights", 200_040),
            ("Queen", "Bohemian Rhapsody", 354_320),
        ] {
            let wanted = Track {
                uri: "spotify:track:x".into(),
                name: name.into(),
                artists: vec![ArtistRef {
                    name: artist.into(),
                    ..Default::default()
                }],
                duration_ms,
                ..Default::default()
            };
            let query = format!("{artist} - {name}");
            let videos = crate::youtube::browse::videos_for(&resolver, &query)
                .await
                .unwrap();
            let best = videos
                .iter()
                .filter_map(|video| {
                    let found = Candidate {
                        url: video.uri.clone(),
                        title: video.name.clone(),
                        uploader: video
                            .artists
                            .first()
                            .map(|artist| artist.name.clone())
                            .unwrap_or_default(),
                        duration: Some(f64::from(video.duration_ms) / 1000.0),
                        live: false,
                    };
                    score(&wanted, &found).map(|score| (video, score))
                })
                .max_by(|left, right| left.1.total_cmp(&right.1));
            println!(
                "{query}: {:?}",
                best.map(|(video, score)| (&video.name, &video.uri, score))
            );
            assert!(best.is_some(), "{query} is on YouTube");
        }
    }
}
