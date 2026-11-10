//! The edge's sign-in vault. The listener's own Spotify Client ID and refresh
//! token, and their SoundCloud OAuth token, are written only as ciphertext by
//! the service and never come back. No value from here is logged.

use std::sync::Arc;

use crate::credentials::{Grant, Slot};

use super::{Scope, Source};

/// What this computer last told the vault, so an unchanged sign-in is not
/// written every minute and so turning the setting off removes both entries.
#[derive(Default)]
struct Pushed {
    enabled: Option<bool>,
    spotify: Option<(String, String)>,
    soundcloud: Option<String>,
}

pub struct Vault {
    source: Arc<Source>,
    pushed: tokio::sync::Mutex<Pushed>,
}

impl Vault {
    pub fn new(source: Arc<Source>) -> Self {
        Self {
            source,
            pushed: tokio::sync::Mutex::new(Pushed::default()),
        }
    }

    /// Writes a changed sign-in, or removes both when the setting is off.
    /// A missing grant removes what was there before, so signing out on this
    /// computer also stops the phone using that account.
    pub async fn push(&self, enabled: bool) -> Result<(), String> {
        if !enabled {
            return self.disable().await;
        }
        let mut pushed = self.pushed.lock().await;
        let was_enabled = pushed.enabled == Some(true);
        pushed.enabled = Some(true);

        let spotify = self.spotify().await;
        if spotify != pushed.spotify || (!was_enabled && spotify.is_some()) {
            match &spotify {
                Some((client_id, refresh_token)) => {
                    self.put(
                        "spotify",
                        serde_json::json!({
                            "client_id": client_id,
                            "refresh_token": refresh_token,
                        }),
                    )
                    .await?;
                }
                None if pushed.spotify.is_some() => {
                    self.delete("spotify").await?;
                }
                None => {}
            }
            pushed.spotify = spotify;
        }

        let soundcloud = self.soundcloud().await;
        if soundcloud != pushed.soundcloud || (!was_enabled && soundcloud.is_some()) {
            match &soundcloud {
                Some(token) => {
                    self.put("soundcloud", serde_json::json!({ "oauth_token": token }))
                        .await?;
                }
                None if pushed.soundcloud.is_some() => {
                    self.delete("soundcloud").await?;
                }
                None => {}
            }
            pushed.soundcloud = soundcloud;
        }
        Ok(())
    }

    /// Both deletes go once when the listener turns the setting off.
    async fn disable(&self) -> Result<(), String> {
        let mut pushed = self.pushed.lock().await;
        if pushed.enabled == Some(false) {
            return Ok(());
        }
        pushed.enabled = Some(false);
        pushed.spotify = None;
        pushed.soundcloud = None;
        self.delete("spotify").await?;
        self.delete("soundcloud").await
    }

    async fn spotify(&self) -> Option<(String, String)> {
        let loaded = self
            .source
            .credentials()
            .lease(Slot::Personal)
            .load()
            .await
            .ok()?;
        match loaded.grant? {
            Grant::Web(token)
                if !token.client_id.is_empty()
                    && !token.refresh_token.is_empty()
                    && token
                        .client_id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric()) =>
            {
                Some((token.client_id, token.refresh_token))
            }
            _ => None,
        }
    }

    async fn soundcloud(&self) -> Option<String> {
        let bytes = self
            .source
            .credentials()
            .lease(Slot::SoundCloud)
            .load_secret()
            .await
            .ok()??;
        let token = String::from_utf8(bytes).ok()?;
        let token = token.trim().to_owned();
        (!token.is_empty() && !token.chars().any(char::is_whitespace)).then_some(token)
    }

    async fn put(&self, provider: &str, body: serde_json::Value) -> Result<(), String> {
        self.request(reqwest::Method::PUT, provider, Some(body))
            .await
    }

    async fn delete(&self, provider: &str) -> Result<(), String> {
        self.request(reqwest::Method::DELETE, provider, None).await
    }

    async fn request(
        &self,
        method: reqwest::Method,
        provider: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let (tokens, token) = self.source.token(Scope::Sync).await?;
        let client = self.source.http().client()?;
        let mut request = client
            .request(method, format!("{}/v1/vault/{provider}", tokens.url))
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(15));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "The edge could not be reached.".to_owned())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err("The edge refused that vault change.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::test_support::{Server, group_source};

    #[tokio::test]
    async fn turning_the_vault_off_removes_both_sign_ins() {
        let server = Server::new(|request| {
            if request.path == "/edge/token" {
                return (
                    200,
                    serde_json::json!({
                        "url": "SELF", "live": "live", "media": "media", "sync": "sync"
                    }),
                );
            }
            if request.path.starts_with("/v1/vault/") {
                return (200, serde_json::json!({ "v": 1 }));
            }
            (404, serde_json::json!({ "error": "not_found" }))
        });
        let vault = Vault::new(group_source(&server).await);
        // Off first: both deletes are sent once, and none are repeated.
        vault.push(false).await.unwrap();
        vault.push(false).await.unwrap();
        let deletes: Vec<_> = server
            .hits()
            .into_iter()
            .filter(|hit| hit.method == "DELETE")
            .collect();
        assert_eq!(deletes.len(), 2);
        assert!(deletes.iter().any(|hit| hit.path.ends_with("/spotify")));
        assert!(deletes.iter().any(|hit| hit.path.ends_with("/soundcloud")));
    }

    #[tokio::test]
    async fn an_unchanged_sign_in_is_not_rewritten_each_minute() {
        let server = Server::new(|request| {
            if request.path == "/edge/token" {
                return (
                    200,
                    serde_json::json!({
                        "url": "SELF", "live": "live", "media": "media", "sync": "sync"
                    }),
                );
            }
            if request.path.starts_with("/v1/vault/") {
                return (200, serde_json::json!({ "v": 1 }));
            }
            (404, serde_json::json!({ "error": "not_found" }))
        });
        let vault = Vault::new(group_source(&server).await);
        // With no grant stored locally there is nothing to write.
        vault.push(true).await.unwrap();
        vault.push(true).await.unwrap();
        assert!(
            server
                .hits()
                .iter()
                .all(|hit| !hit.path.starts_with("/v1/vault/"))
        );
    }
}
