//! Diagnostic: what Spotify's own artist overview answers about an artist's
//! upcoming release when it is asked with `includePrerelease: true`, run with
//! the stored playback credential. The answer is printed with its tokens left
//! out: release metadata only.
//!
//!   cargo run --example upcoming_probe -- <artist id> ...

use librespot_core::{Session, SessionConfig, cache::Cache};

/// Spotify's persisted Pathfinder query for an artist page's overview, as
/// `src/session_reads.rs` uses it.
const ARTIST_OVERVIEW_HASH: &str =
    "4bc52527bb77a5f8bbb9afe491e9aa725698d29ab73bff58d49169ee29800167";

fn main() -> anyhow::Result<()> {
    fastframe_log::Logging::new("spotifast", env!("CARGO_PKG_VERSION"))
        .filter("warn")
        .init()?;
    let ids: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(!ids.is_empty(), "give at least one Spotify artist id");
    let dirs = spotifast::paths::AppDirs::discover()?.dirs;
    let cache = Cache::new::<&std::path::Path>(None, None, None, None)?.with_memory_credentials();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let store = spotifast::credentials::Store::new(dirs);
        let loaded = store
            .lease(spotifast::credentials::Slot::Playback)
            .load()
            .await?;
        if let Some(warning) = loaded.warning {
            eprintln!("{warning}");
        }
        let Some(spotifast::credentials::Grant::Playback(credentials)) = loaded.grant else {
            anyhow::bail!("Enable playback in Siphon first");
        };
        let session = Session::new(SessionConfig::default(), Some(cache));
        session.connect(credentials, false).await?;
        let login = session.login5().auth_token().await?;
        let client_token = session.spclient().client_token().await?;
        let http = reqwest::Client::new();
        for id in ids {
            println!("trying {id}");
            let query = serde_json::json!({
                "operationName": "queryArtistOverview",
                "variables": {
                    "uri": format!("spotify:artist:{id}"),
                    "locale": "",
                    "includePrerelease": true,
                },
                "extensions": {
                    "persistedQuery": {"version": 1, "sha256Hash": ARTIST_OVERVIEW_HASH}
                }
            });
            let response = http
                .post("https://api-partner.spotify.com/pathfinder/v2/query")
                .bearer_auth(&login.access_token)
                .header("client-token", &client_token)
                .json(&query)
                .send()
                .await?;
            let status = response.status();
            let body: serde_json::Value = response.json().await?;
            if !status.is_success() || body.get("errors").is_some() {
                println!("  HTTP {status}: {body}");
                continue;
            }
            let artist = body.pointer("/data/artistUnion");
            println!(
                "  artist: {}",
                artist
                    .and_then(|artist| artist.pointer("/profile/name"))
                    .and_then(|name| name.as_str())
                    .unwrap_or("<missing>")
            );
            if let Some(discography) = artist.and_then(|artist| artist.get("discography")) {
                let keys: Vec<&str> = discography
                    .as_object()
                    .map(|object| object.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                println!("  discography keys: {}", keys.join(", "));
                if let Some(latest) = discography.get("latest") {
                    println!("  latest: {latest}");
                }
            }
            let mut nodes = Vec::new();
            collect_prerelease(&body, "data", &mut nodes);
            println!("  pre-release nodes: {}", nodes.len());
            for (path, value) in nodes {
                println!("  {path}: {value}");
            }
        }
        anyhow::Ok(())
    })?;
    Ok(())
}

/// Every value under a key that mentions a pre-release, with its path. Spotify
/// has named this field more than once, so the probe looks rather than assumes.
fn collect_prerelease(value: &serde_json::Value, path: &str, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object {
                let key_path = format!("{path}.{key}");
                if key
                    .to_ascii_lowercase()
                    .replace('_', "")
                    .contains("prerelease")
                {
                    out.push((key_path.clone(), child.to_string()));
                }
                collect_prerelease(child, &key_path, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_prerelease(child, &format!("{path}[{index}]"), out);
            }
        }
        _ => {}
    }
}
