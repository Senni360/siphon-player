//! The always-on edge runs beside the group relay. It is optional: without a
//! group sign-in, or with no edge configured, the relay stays exactly as it
//! was and nothing here is started.

use super::*;

impl Worker {
    /// Starts the edge the first time a phone is linked, then hands it work.
    pub(super) fn edge(&mut self, request: crate::edge::Request) {
        self.edge_start();
        if let Some(edge) = &self.edge {
            edge.send(request);
        }
    }

    pub(super) fn edge_start(&mut self) {
        if self.edge.is_some() {
            return;
        }
        let http = crate::http::build_collection_client(&self.engine_config.proxy)
            .map(Http::new)
            .unwrap_or_else(Http::unavailable);
        let Ok(group) = crate::friends::Client::configured(http.clone()) else {
            return;
        };
        let source = Arc::new(crate::edge::Source::new(
            http,
            group,
            self.credentials.clone(),
        ));
        let events = self.events.clone();
        let waker = self.waker.clone();
        let emit = Arc::new(move |event| {
            let _ = events.send(Event::Edge(event));
            waker.wake();
        });
        let temp = self.dirs.cache.join("edge-fetch");
        self.edge = Some(crate::edge::Runtime::spawn(
            source,
            self.saved_matches.clone(),
            temp,
            emit,
        ));
        // The saved matches are pushed, and the edge's automatic ones taken
        // back, as soon as the socket exists.
        if let Some(edge) = &self.edge {
            edge.send(crate::edge::Request::Matches);
        }
    }
}
