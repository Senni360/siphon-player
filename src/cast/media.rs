//! Opt-in hosted-media diagnostic. Separate from Spotify activation and tokens.
//! Blocking operations belong on a worker, never the application UI thread.

use super::*;

const APP: &str = "CC1AD845";
const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";

/// Only the fields needed for measurements. Receiver metadata and signed URLs
/// never enter logs, errors or this value.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub session: u64,
    pub state: String,
    pub position_seconds: f64,
}

/// A connection to Google's Default Media Receiver, for an explicit test only.
/// Keep polling while connected to service receiver heartbeats. There is no
/// automatic retry of LOAD/SEEK after an ambiguous network failure.
pub struct Receiver {
    stream: TlsStream,
    transport: String,
    request: u32,
    session: Option<u64>,
}

impl Receiver {
    /// Launching replaces the receiver app currently running on this device.
    pub fn connect(address: IpAddr, port: u16) -> Result<Self> {
        let deadline = Instant::now() + TIMEOUT;
        let mut stream = connect(address, port, deadline)?;
        let mut channel = Channel {
            stream: &mut stream,
            deadline,
        };
        channel.authenticate(&GOOGLE_ROOTS, UnixTime::now())?;
        let transport = launch(&mut channel)?;
        Ok(Self {
            stream,
            transport,
            request: 1,
            session: None,
        })
    }

    /// Load only an explicitly approved MP3 test recording. An HTTPS bearer
    /// URL is given to the receiver, without any Spotify or SoundCloud grant.
    pub fn load(&mut self, url: &str, position_seconds: f64) -> Result<Status> {
        validate_url(url)?;
        validate_position(position_seconds)?;
        let result = self.command(
            json!({
                "type": "LOAD", "autoplay": true, "currentTime": position_seconds,
                "media": { "contentId": url, "contentType": "audio/mpeg", "streamType": "BUFFERED" }
            }),
            None,
        )?;
        self.session = Some(result.session);
        Ok(result)
    }

    pub fn status(&mut self) -> Result<Status> {
        let session = self.session.context("load a test recording first")?;
        self.command(
            json!({"type": "GET_STATUS", "mediaSessionId": session}),
            Some(session),
        )
    }

    pub fn seek(&mut self, position_seconds: f64) -> Result<Status> {
        validate_position(position_seconds)?;
        let session = self.session.context("load a test recording first")?;
        self.command(
            json!({"type": "SEEK", "mediaSessionId": session,
            "currentTime": position_seconds, "resumeState": "PLAYBACK_START"}),
            Some(session),
        )
    }

    pub fn stop(&mut self) -> Result<()> {
        let session = self.session.context("load a test recording first")?;
        self.command(
            json!({"type": "STOP", "mediaSessionId": session}),
            Some(session),
        )?;
        self.session = None;
        Ok(())
    }

    fn command(&mut self, mut payload: Value, session: Option<u64>) -> Result<Status> {
        self.request = self
            .request
            .checked_add(1)
            .context("Cast request IDs exhausted")?;
        payload["requestId"] = self.request.into();
        let mut channel = Channel {
            stream: &mut self.stream,
            deadline: Instant::now() + TIMEOUT,
        };
        command(
            &mut channel,
            &self.transport,
            payload,
            self.request,
            session,
        )
    }
}

fn command<S: Stream>(
    channel: &mut Channel<'_, S>,
    transport: &str,
    payload: Value,
    request: u32,
    session: Option<u64>,
) -> Result<Status> {
    channel.send(transport, NS_MEDIA, payload)?;
    channel.wait_for(NS_MEDIA, |message, payload| {
        if message.source != transport || message.destination != SENDER {
            return None;
        }
        media_reply(payload?, request, session)
    })
}

fn launch<S: Stream>(channel: &mut Channel<'_, S>) -> Result<String> {
    channel.send(PLATFORM, NS_CONNECTION, json!({"type": "CONNECT"}))?;
    channel.send(
        PLATFORM,
        NS_RECEIVER,
        json!({"type": "LAUNCH", "appId": APP, "requestId": 1}),
    )?;
    let transport = channel.wait_for(NS_RECEIVER, |message, payload| {
        if message.source != PLATFORM || message.destination != SENDER {
            return None;
        }
        let payload = payload?;
        if payload["requestId"] != 1 {
            return None;
        }
        match payload["type"].as_str()? {
            "LAUNCH_ERROR" | "INVALID_REQUEST" => Some(Err(anyhow!(
                "the Cast device could not start the media receiver; return to local playback"
            ))),
            "RECEIVER_STATUS" => payload["status"]["applications"]
                .as_array()?
                .iter()
                .find(|app| app["appId"] == APP)
                .and_then(|app| app["transportId"].as_str())
                .map(|id| Ok(id.to_owned())),
            _ => None,
        }
    })?;
    channel.send(&transport, NS_CONNECTION, json!({"type": "CONNECT"}))?;
    Ok(transport)
}

fn media_reply(payload: &Value, request: u32, session: Option<u64>) -> Option<Result<Status>> {
    if payload["requestId"] != request {
        return None;
    }
    match payload["type"].as_str()? {
        "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST" | "INVALID_PLAYER_STATE" => {
            Some(Err(anyhow!(
                "the media receiver refused the command; check format, temporary access and device reachability, or return to local playback"
            )))
        }
        "MEDIA_STATUS" => {
            let statuses = payload["status"].as_array()?;
            if statuses.is_empty() {
                return Some(
                    session
                        .map(|session| Status {
                            session,
                            state: "IDLE".into(),
                            position_seconds: 0.0,
                        })
                        .context("the media receiver has no loaded recording"),
                );
            }
            let value = statuses
                .iter()
                .find(|value| session.is_none_or(|id| value["mediaSessionId"] == id))?;
            let id = value["mediaSessionId"].as_u64()?;
            let position = value["currentTime"].as_f64().unwrap_or(0.0);
            let state = value["playerState"].as_str()?;
            if !position.is_finite()
                || position < 0.0
                || !matches!(state, "IDLE" | "PLAYING" | "PAUSED" | "BUFFERING")
            {
                return Some(Err(anyhow!("invalid media receiver status")));
            }
            if value["idleReason"] == "ERROR" {
                return Some(Err(anyhow!(
                    "media playback failed; check access expiry and receiver format support"
                )));
            }
            Some(Ok(Status {
                session: id,
                state: state.to_owned(),
                position_seconds: position,
            }))
        }
        _ => None,
    }
}

pub(crate) fn validate_url(url: &str) -> Result<()> {
    let url = reqwest::Url::parse(url).map_err(|_| anyhow!("invalid media URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("media requires an HTTPS URL without user information or fragment");
    }
    Ok(())
}

fn validate_position(position: f64) -> Result<()> {
    if !position.is_finite() || position < 0.0 {
        bail!("position must be finite and nonnegative");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_requests_and_other_sessions_do_not_satisfy_a_command() {
        let message = json!({"type": "MEDIA_STATUS", "requestId": 4, "status": [{
            "mediaSessionId": 9, "currentTime": 12.5, "playerState": "PLAYING"}]});
        assert!(media_reply(&message, 3, Some(9)).is_none());
        assert!(media_reply(&message, 4, Some(8)).is_none());
        assert_eq!(
            media_reply(&message, 4, Some(9))
                .unwrap()
                .unwrap()
                .position_seconds,
            12.5
        );
    }

    #[test]
    fn errors_and_statuses_cannot_expose_signed_urls() {
        let message = json!({"type": "LOAD_FAILED", "requestId": 2,
            "reason": "https://media.example/song?secret=private"});
        let error = media_reply(&message, 2, None).unwrap().unwrap_err();
        assert!(!format!("{error:?}").contains("private"));
    }

    #[test]
    fn only_explicit_https_media_and_valid_positions_are_accepted() {
        for url in [
            "http://media.example/x",
            "file:///x",
            "https://user:secret@media.example/x",
            "https://media.example/x#secret",
        ] {
            assert!(validate_url(url).is_err());
        }
        assert!(validate_url("https://media.example/x?token=private").is_ok());
        for position in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(validate_position(position).is_err());
        }
    }

    #[test]
    fn stopped_sessions_and_invalid_player_state_return_without_timing_out() {
        let empty = json!({"type": "MEDIA_STATUS", "requestId": 4, "status": []});
        assert_eq!(
            media_reply(&empty, 4, Some(9)).unwrap().unwrap().state,
            "IDLE"
        );
        assert!(media_reply(&empty, 4, None).unwrap().is_err());
        let invalid = json!({"type": "INVALID_PLAYER_STATE", "requestId": 4});
        assert!(media_reply(&invalid, 4, Some(9)).unwrap().is_err());
    }

    struct Script {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Script {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buffer)
        }
    }
    impl Write for Script {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Stream for Script {
        fn set_read_timeout(&mut self, _: Duration) -> std::io::Result<()> {
            Ok(())
        }
        fn peer_certificate(&self) -> Option<&[u8]> {
            None
        }
    }
    fn script(messages: Vec<Message>) -> Script {
        let mut bytes = Vec::new();
        for message in messages {
            let frame = message.encode().unwrap();
            bytes.extend_from_slice(&(frame.len() as u32).to_be_bytes());
            bytes.extend(frame);
        }
        Script {
            input: std::io::Cursor::new(bytes),
            output: Vec::new(),
        }
    }
    fn reply(source: &str, request: u32, session: u64) -> Message {
        Message {source: source.into(), destination: SENDER.into(), namespace: NS_MEDIA.into(), payload: Payload::Text(json!({
            "type": "MEDIA_STATUS", "requestId": request, "status": [{"mediaSessionId": session, "currentTime": 5.0, "playerState": "PLAYING"}]
        }).to_string())}
    }
    #[test]
    fn wire_commands_ignore_stale_sources_sessions_and_duplicate_answers() {
        let mut stream = script(vec![
            reply("other", 2, 9),
            reply("web-7", 1, 9),
            reply("web-7", 2, 8),
            reply("web-7", 2, 9),
            reply("web-7", 2, 9),
            reply("web-7", 3, 9),
        ]);
        let mut channel = Channel {
            stream: &mut stream,
            deadline: Instant::now() + TIMEOUT,
        };
        for request in [2, 3] {
            let payload = json!({"type": "SEEK", "requestId": request, "mediaSessionId": 9, "currentTime": 5});
            assert_eq!(
                command(&mut channel, "web-7", payload, request, Some(9))
                    .unwrap()
                    .session,
                9
            );
        }
        let mut wire = std::io::Cursor::new(stream.output);
        for request in [2, 3] {
            let mut length = [0; 4];
            wire.read_exact(&mut length).unwrap();
            let mut frame = vec![0; u32::from_be_bytes(length) as usize];
            wire.read_exact(&mut frame).unwrap();
            let sent = Message::decode(&frame).unwrap();
            assert_eq!(sent.destination, "web-7");
            assert_eq!(sent.namespace, NS_MEDIA);
            assert_eq!(sent.json().unwrap()["requestId"], request);
            assert_eq!(sent.json().unwrap()["mediaSessionId"], 9);
        }
    }

    #[test]
    fn generic_launch_uses_only_the_default_receiver_and_connection_namespace() {
        let message = Message {source: PLATFORM.into(), destination: SENDER.into(), namespace: NS_RECEIVER.into(), payload: Payload::Text(json!({
            "type": "RECEIVER_STATUS", "requestId": 1, "status": {"applications": [{"appId": APP, "transportId": "media-1"}]}
        }).to_string())};
        let mut stream = script(vec![message]);
        let mut channel = Channel {
            stream: &mut stream,
            deadline: Instant::now() + TIMEOUT,
        };
        assert_eq!(launch(&mut channel).unwrap(), "media-1");
        let wire = String::from_utf8_lossy(&stream.output);
        assert!(wire.contains(APP));
        assert!(!wire.contains(SPOTIFY_APP));
        assert!(!wire.contains(NS_SPOTIFY));
    }
}
