//! Source-provided credits, independent of the playing song and of presentation.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::model::Loadable;

/// Verified in Spotify's public web-player bundles on 2026-10-30:
/// xpui-root-dialogs.e544a0c2.js and xpui-routes-track-v2.755ee45c.js.
pub const OPERATION: &str = "queryTrackCreditsGroupedModal";
pub const HASH: &str = "f135fb9be58a72d041ab5d214d817021a272405d883860468e2627afb01a3ca9";
pub const PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credit {
    pub name: String,
    pub role: Option<String>,
    pub group: Option<String>,
    pub artist_uri: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Credits {
    pub contributors: Vec<Credit>,
    pub sources: Vec<String>,
}

impl Credits {
    fn nonempty(self) -> Option<Self> {
        (!self.contributors.is_empty() || !self.sources.is_empty()).then_some(self)
    }
}

/// Per-track UI cache. Loading is deduplicated; failures can be requested again.
/// Request IDs also prevent an old account's answer from filling a cleared cache.
#[derive(Default)]
pub struct State {
    next_request: u64,
    entries: HashMap<String, (u64, Loadable<Option<Credits>>)>,
}

impl State {
    pub fn get(&self, uri: &str) -> Option<&Loadable<Option<Credits>>> {
        self.entries.get(uri).map(|(_, state)| state)
    }

    pub fn begin(&mut self, uri: &str) -> Option<u64> {
        if self
            .get(uri)
            .is_some_and(|state| matches!(state, Loadable::Loading | Loadable::Loaded(_)))
        {
            return None;
        }
        self.next_request += 1;
        self.entries
            .insert(uri.to_owned(), (self.next_request, Loadable::Loading));
        Some(self.next_request)
    }

    pub fn finish(&mut self, uri: &str, request: u64, result: Result<Option<Credits>, String>) {
        if let Some((held, state)) = self.entries.get_mut(uri)
            && *held == request
        {
            *state = match result {
                Ok(credits) => Loadable::Loaded(credits),
                Err(error) => Loadable::Failed(error),
            };
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

pub fn query(uri: &str, offset: usize) -> Value {
    serde_json::json!({
        "operationName": OPERATION,
        "variables": {"trackUri": uri, "contributorsLimit": PAGE_SIZE, "contributorsOffset": offset},
        "extensions": {"persistedQuery": {"version": 1, "sha256Hash": HASH}}
    })
}

pub(crate) struct Page {
    pub credits: Credits,
    pub rows: usize,
    pub total: Option<usize>,
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Fields checked against the public contributor renderer (module 78297).
/// Schema failures are distinct from a successful empty credits list.
pub(crate) fn spotify_page(body: &Value) -> Result<Option<Page>> {
    let track = body
        .pointer("/data/trackUnion")
        .context("Spotify credits are unavailable")?;
    if track.get("__typename").and_then(Value::as_str) != Some("Track") {
        anyhow::bail!("Spotify credits are unavailable");
    }
    let Some(credits) = track.get("creditsTrait").filter(|value| !value.is_null()) else {
        // A GraphQL failure can be accompanied by a null field. Do not cache it.
        anyhow::ensure!(
            body.get("errors").is_none(),
            "Spotify credits are unavailable"
        );
        return Ok(None);
    };
    let rows = credits
        .pointer("/contributors/items")
        .and_then(Value::as_array)
        .context("Spotify credit contributors are unavailable")?;
    let contributors = rows
        .iter()
        .filter_map(|row| {
            let name = text(row.get("name"))?;
            let uri = text(row.get("uri")).or_else(|| text(row.pointer("/reference/uri")));
            Some(Credit {
                name,
                role: text(row.get("role")),
                group: text(row.pointer("/roleGroup/name")),
                artist_uri: uri.filter(|uri| {
                    matches!(
                        librespot_core::SpotifyUri::from_uri(uri),
                        Ok(librespot_core::SpotifyUri::Artist { .. })
                    )
                }),
            })
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        rows.is_empty() || !contributors.is_empty(),
        "Spotify credits could not be decoded"
    );
    let sources = credits
        .pointer("/sources/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|source| text(source.get("name")))
        .collect();
    Ok(Some(Page {
        credits: Credits {
            contributors,
            sources,
        },
        rows: rows.len(),
        total: credits
            .pointer("/contributors/totalCount")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok()),
    }))
}

pub(crate) fn finish(credits: Credits) -> Option<Credits> {
    credits.nonempty()
}

/// A narrow wire projection, intentionally independent of catalogue model
/// expansion. Only publisher-supplied fields count as credits, never uploader.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SoundCloudMetadata {
    pub id: u64,
    #[serde(default)]
    pub duration: u32,
    pub full_duration: Option<u32>,
    pub publisher_metadata: Option<Publisher>,
    pub label_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Publisher {
    pub artist: Option<String>,
    pub isrc: Option<String>,
    pub writer_composer: Option<String>,
    pub publisher: Option<String>,
    pub p_line: Option<String>,
}

impl SoundCloudMetadata {
    pub fn duration_ms(&self) -> u32 {
        self.full_duration.unwrap_or(self.duration)
    }

    pub fn credits(&self) -> Option<Credits> {
        let mut credits = Credits::default();
        let publisher = self.publisher_metadata.as_ref();
        for (role, name) in [
            ("Artist", publisher.and_then(|p| p.artist.as_deref())),
            (
                "Writer / Composer",
                publisher.and_then(|p| p.writer_composer.as_deref()),
            ),
            ("Publisher", publisher.and_then(|p| p.publisher.as_deref())),
            ("Label", self.label_name.as_deref()),
            (
                "Phonographic copyright",
                publisher.and_then(|p| p.p_line.as_deref()),
            ),
        ] {
            if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
                credits.contributors.push(Credit {
                    name: name.to_owned(),
                    role: Some(role.into()),
                    group: None,
                    artist_uri: None,
                });
            }
        }
        credits.nonempty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grouped_credits_preserve_roles_sources_and_real_artist_uris() {
        let body = json!({"data":{"trackUnion":{"__typename":"Track","creditsTrait":{
            "contributors":{"totalCount":4,"items":[
                {"name":"Performer","role":"vocals","roleGroup":{"name":"Performers"},"reference":{"uri":"spotify:artist:0OdUWJ0sBjDrqHygGUXeCF"}},
                {"name":"Writer","role":"composer","roleGroup":{"name":"Writers"}},
                {"name":"Producer","role":"producer","roleGroup":{"name":"Production"},"uri":"spotify:artist:0OdUWJ0sBjDrqHygGUXeCF"},
                {"name":"Engineer","role":"mixing engineer","roleGroup":{"name":"Engineering"},"reference":{"uri":"https://example.com"}}
            ]},"sources":{"items":[{"name":"Record label"},{"name":"Publisher"}]}
        }}}});
        let page = spotify_page(&body).unwrap().unwrap();
        assert_eq!(page.total, Some(4));
        assert_eq!(
            page.credits.contributors[0].artist_uri.as_deref(),
            Some("spotify:artist:0OdUWJ0sBjDrqHygGUXeCF")
        );
        assert_eq!(
            page.credits.contributors[3].role.as_deref(),
            Some("mixing engineer")
        );
        assert!(page.credits.contributors[3].artist_uri.is_none());
        assert_eq!(page.credits.sources, ["Record label", "Publisher"]);
        assert!(spotify_page(&json!({"errors":[{"message":"PersistedQueryNotFound"}]})).is_err());
        assert!(
            spotify_page(
                &json!({"data":{"trackUnion":{"__typename":"Track","creditsTrait":null}}})
            )
            .unwrap()
            .is_none()
        );
        assert!(finish(spotify_page(&json!({"data":{"trackUnion":{"__typename":"Track","creditsTrait":{"contributors":{"items":[]}}}}})).unwrap().unwrap().credits).is_none());
    }

    #[test]
    fn soundcloud_credits_never_guess_from_the_uploader() {
        let metadata: SoundCloudMetadata = serde_json::from_value(json!({
            "id":1,"duration":30_000,"full_duration":200_000,"user":{"username":"Uploader"},
            "publisher_metadata":{"artist":"Artist","isrc":"GBABC2212345","writer_composer":"Writer A / Writer B","publisher":"Publisher","p_line":"2026 Label"},
            "label_name":"Label"
        })).unwrap();
        assert_eq!(metadata.duration_ms(), 200_000);
        let credits = metadata.credits().unwrap();
        assert_eq!(credits.contributors.len(), 5);
        assert_eq!(credits.contributors[1].name, "Writer A / Writer B");
        assert!(
            credits
                .contributors
                .iter()
                .all(|credit| credit.artist_uri.is_none())
        );
        assert!(
            serde_json::from_value::<SoundCloudMetadata>(
                json!({"id":2,"user":{"username":"Uploader"}})
            )
            .unwrap()
            .credits()
            .is_none()
        );
    }

    #[test]
    fn cache_is_per_track_and_ignores_stale_results() {
        let mut state = State::default();
        let first = state.begin("a").unwrap();
        assert!(state.begin("a").is_none());
        let second = state.begin("b").unwrap();
        state.finish("a", second, Ok(None));
        assert!(matches!(state.get("a"), Some(Loadable::Loading)));
        state.finish("a", first, Ok(None));
        assert!(state.begin("a").is_none());
        state.clear();
        let next = state.begin("a").unwrap();
        state.finish("a", first, Ok(None));
        assert!(matches!(state.get("a"), Some(Loadable::Loading)));
        state.finish("a", next, Err("Unavailable".into()));
        assert!(state.begin("a").is_some());
    }
}
