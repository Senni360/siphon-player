//! YouTube adapts to the same external PCM commands as SoundCloud.
use super::*;
use crate::player::PlaybackTag;
use crate::soundcloud::engine as pcm;

impl Worker {
    fn youtube_catalogue(&self) -> crate::youtube::Catalogue {
        crate::youtube::Catalogue {
            resolver: Default::default(),
            cache: self.dirs.state.join("youtube-metadata"),
        }
    }

    pub(super) fn cancel_youtube(&mut self) {
        for task in [&mut self.hybrid.youtube_load, &mut self.hybrid.youtube_preload, &mut self.hybrid.youtube_context] {
            if let Some(task) = task.take() { task.abort(); }
        }
    }

    pub(super) fn resolve_youtube_context(&mut self, generation: u64, uri: String) {
        if let Some(task) = self.hybrid.youtube_context.take() { task.abort(); }
        let catalogue = self.youtube_catalogue();
        let events = self.events.clone();
        let waker = self.waker.clone();
        self.hybrid.youtube_context = Some(tokio::spawn(async move {
            let result = catalogue.track(&uri).await.map(|track| crate::hybrid_queue::Context {
                uri, name: track.name.clone(), tracks: vec![track],
            }).map_err(|error| error.to_string());
            let _ = events.send(Event::HybridContext { generation, result });
            waker.wake();
        }).abort_handle());
    }

    pub(super) fn resolve_youtube_stream(&mut self, tag: PlaybackTag, uri: String, preload: bool, refresh: Option<u64>) {
        let catalogue = self.youtube_catalogue();
        let commands = self.commands.clone();
        let task = tokio::spawn(async move {
            let result = async {
                let id = crate::youtube::video_id(&uri).ok_or(crate::youtube::Error::Unavailable)?;
                let url = catalogue.resolver.audio_url(id).await?;
                let track = catalogue.track(&uri).await?;
                Ok::<_, crate::youtube::Error>(pcm::Stream {
                    uri, url, protocol: pcm::Protocol::Progressive,
                    mime: "audio/mp4".into(), duration_ms: track.duration_ms, is_preview: false,
                })
            }.await.map_err(|error| error.to_string());
            let _ = commands.send(Command::YouTubeStream { tag, preload, refresh, result });
        }).abort_handle();
        let slot = if preload { &mut self.hybrid.youtube_preload } else { &mut self.hybrid.youtube_load };
        if let Some(old) = slot.replace(task) { old.abort(); }
    }

    pub(super) fn youtube_track(&self, uri: String) {
        let catalogue = self.youtube_catalogue();
        let events = self.events.clone();
        let waker = self.waker.clone();
        tokio::spawn(async move {
            let result = catalogue.track(&uri).await.map_err(|error| error.to_string());
            let _ = events.send(Event::YouTubeTrack { uri, result: result.map(Box::new) });
            waker.wake();
        });
    }
}
