`video.json` is synthetic yt-dlp-shaped metadata for deterministic tests and
frontend demos. Its ID, channel, title, counts and artwork links are fixtures,
not a live extraction or commercial claim. Derive loading/error/unavailable
states explicitly; never present this file as a successful live request.
