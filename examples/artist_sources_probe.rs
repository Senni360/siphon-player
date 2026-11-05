//! Diagnostic: what the artist page would find for a name, reading public SoundCloud
//! and YouTube with no account, to judge the matching against real artists.
//!
//!   cargo run --example artist_sources_probe -- "Slayer" "Raining Blood" "Angel of Death"

use spotifast::artist_sources::{self as sources, Catalogue, Finding};
use spotifast::soundcloud::api::Client;
use spotifast::youtube::Resolver;

fn show(side: &str, finding: &Finding, took: std::time::Duration) {
    println!("--- {side} ({} ms)", took.as_millis());
    match &finding.account {
        Some(account) => println!(
            "account: {} ({:?} followers, verified {}) {:?}",
            account.name, account.followers, account.verified, finding.confidence
        ),
        None => println!("no account"),
    }
    for other in &finding.candidates {
        println!("  maybe: {} ({:?})", other.name, other.followers);
    }
    println!("on Spotify too: {}", finding.on_spotify);
    println!("exclusives: {}", finding.exclusives.len());
    for track in finding.exclusives.iter().take(12) {
        println!("  {} [{:?}]", track.name, track.play_count);
    }
    println!("edits: {}", finding.edits.len());
    for track in finding.edits.iter().take(6) {
        println!("  {} [{:?}]", track.name, track.play_count);
    }
    println!("unreleased: {}", finding.unreleased.len());
    for track in finding.unreleased.iter().take(14) {
        println!(
            "  {} | by {:?} [{:?}]",
            track.name,
            track.artists.first().map(|artist| artist.name.as_str()),
            track.play_count
        );
    }
    for archive in &finding.archives {
        println!("  posted by: {} ({:?})", archive.name, archive.followers);
    }
    println!("produced: {}", finding.produced.len());
    for track in finding.produced.iter().take(14) {
        println!(
            "  {} | by {:?} [{:?}]",
            track.name,
            track.artists.first().map(|artist| artist.name.as_str()),
            track.play_count
        );
    }
    if let Some(error) = &finding.error {
        println!("error: {error}");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let artist = args.next().unwrap_or_else(|| "Slayer".into());
    let titles: Vec<String> = args.collect();
    let catalogue = Catalogue::new(&artist, &titles);
    let client = Client::new(reqwest::Client::new(), None, None);
    let resolver = Resolver::default();
    let started = std::time::Instant::now();
    let mut found = sources::soundcloud(&client, &artist, &catalogue, None).await;
    found.add(sources::soundcloud_beyond(&client, &artist, &catalogue).await);
    show("SoundCloud", &found, started.elapsed());
    let started = std::time::Instant::now();
    let mut found = sources::youtube(&resolver, &artist, &catalogue, None).await;
    found.add(sources::youtube_beyond(&resolver, &artist, &catalogue).await);
    show("YouTube", &found, started.elapsed());
    Ok(())
}
