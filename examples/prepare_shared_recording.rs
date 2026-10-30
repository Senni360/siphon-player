//! Offline selected-file preparation. No uploads, accounts or audio output.
use anyhow::{Result, anyhow, bail};
use clap::Parser;
use spotifast::shared_media::{
    Cancellation, PreparationError, RecordingMetadata, Stage, prepare_selected,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(about = "Prepare one selected MP3 offline; never scans or uploads")]
struct Args {
    /// The one explicitly selected local recording.
    file: PathBuf,
    /// Emit private identity metadata as JSON on stdout; never includes a path.
    #[arg(long)]
    json: bool,
    /// Compare size, digest and profile with an earlier private JSON result.
    #[arg(long)]
    matches: Option<PathBuf>,
    /// Exercise cooperative cancellation after this many milliseconds.
    #[arg(long, value_parser = clap::value_parser!(u64).range(0..=30_000))]
    cancel_after_ms: Option<u64>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let expected = args.matches.as_deref().map(read_expected).transpose()?;
    let cancellation = Cancellation::default();
    if args.cancel_after_ms == Some(0) {
        cancellation.cancel();
    }
    let worker_cancel = cancellation.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let started = Instant::now();
    let worker = std::thread::spawn(move || {
        prepare_selected(&args.file, &worker_cancel, |progress| {
            let _ = sender.try_send(progress);
        })
    });
    let mut stage = None;
    loop {
        if args
            .cancel_after_ms
            .is_some_and(|limit| started.elapsed() >= Duration::from_millis(limit))
        {
            cancellation.cancel();
        }
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(progress) => {
                if stage != Some(progress.stage) {
                    stage = Some(progress.stage);
                    eprintln!(
                        "{}",
                        match progress.stage {
                            Stage::Reading =>
                                "Reading the selected file into a bounded snapshot...",
                            Stage::Validating => "Validating MP3 audio without an output device...",
                        }
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let result = worker
        .join()
        .map_err(|_| anyhow!("Recording preparation worker failed"))?;
    if cancellation.is_cancelled() {
        return Err(PreparationError::Cancelled.into());
    }
    let prepared = result?;
    if let Some(expected) = expected {
        prepared.require_identity(&expected)?;
    }
    if args.json {
        println!("{}", serde_json::to_string(prepared.metadata())?);
    } else {
        println!(
            "Prepared {} bytes of {} audio, {} ms. No audio uploaded or written.",
            prepared.metadata().size,
            prepared.metadata().profile,
            prepared.metadata().duration_ms
        );
    }
    Ok(())
}

fn read_expected(path: &Path) -> Result<RecordingMetadata> {
    let file =
        std::fs::File::open(path).map_err(|_| anyhow!("Cannot open private identity metadata"))?;
    let mut bytes = Vec::new();
    file.take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("Cannot read private identity metadata"))?;
    if bytes.len() > 8192 {
        bail!("Private identity metadata exceeds 8 KiB");
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid private identity metadata"))
}
