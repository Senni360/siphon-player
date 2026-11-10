//! A tiny HTTP server for the edge tests. It records every request and answers
//! from a closure, so no test ever reaches the real edge. `SELF` in a handler's
//! answer is replaced with this server's address, for the fake mint response.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::credentials::Store as CredentialStore;
use crate::edge::Source;
use crate::friends::Client;
use crate::http::Http;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

pub struct Server {
    url: String,
    hits: Arc<Mutex<Vec<Recorded>>>,
}

impl Server {
    pub fn new(handler: impl Fn(&Recorded) -> (u16, Value) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a test listener");
        let port = listener.local_addr().expect("a test port").port();
        let url = format!("http://127.0.0.1:{port}");
        let hits = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&hits);
        let answer = Arc::new(handler);
        let own = url.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let Some(request) = read(&mut stream) else {
                    continue;
                };
                recorded
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(request.clone());
                let (status, value) = answer(&request);
                let mut body = value.to_string();
                if body.contains("\"SELF\"") {
                    body = body.replace("\"SELF\"", &format!("\"{own}\""));
                }
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { url, hits }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn hits(&self) -> Vec<Recorded> {
        self.hits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn read(stream: &mut std::net::TcpStream) -> Option<Recorded> {
    let mut buffer = Vec::new();
    let mut head_end = None;
    while head_end.is_none() {
        let mut chunk = [0u8; 1024];
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        head_end = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    }
    let head_end = head_end? + 4;
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut request = lines.next()?.split_whitespace();
    let method = request.next()?.to_owned();
    let path = request.next()?.split('?').next()?;
    // The group service is mounted under `/api`, as it is on Vercel. The
    // handlers here name their routes the way the service does, without it.
    let path = path.strip_prefix("/api").unwrap_or(path).to_owned();
    let mut length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "content-length" {
            length = value.parse().unwrap_or(0);
        }
    }
    let mut body = buffer[head_end..].to_vec();
    while body.len() < length {
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    Some(Recorded { method, path, body })
}

/// A [`Source`] whose group service is this fake server. A throwaway group
/// identity is stored so minting behaves exactly as it does for a real
/// listener; the fake answers the mint route directly.
pub async fn group_source(server: &Server) -> Arc<Source> {
    let http = Http::new(reqwest::Client::new());
    let group =
        Client::new(http.clone(), &format!("{}/api", server.url())).expect("a local group address");
    let root = crate::test_dirs::temp_dir().join(format!(
        "siphon-edge-credentials-{}-{:?}-{}",
        std::process::id(),
        std::thread::current().id(),
        rand::random::<u64>()
    ));
    let dirs = crate::paths::AppDirs::from(fastframe_dirs::Dirs::inside(&root));
    let credentials = CredentialStore::in_memory(dirs);
    let identity = crate::friends::Identity {
        user_id: "test-account".into(),
        secret: "a".repeat(64),
        // Exactly as the client spells it, or it is refused as another
        // service's identity.
        base_url: group.base_url().to_owned(),
    };
    credentials
        .lease(crate::credentials::Slot::Friends)
        .save_secret(serde_json::to_vec(&identity).expect("an identity"))
        .await
        .expect("a stored identity");
    Arc::new(Source::new(http, group, credentials))
}
