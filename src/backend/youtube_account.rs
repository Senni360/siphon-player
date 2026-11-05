//! Runs the optional YouTube connection: signing in, keeping the grant in the
//! credential store, and carrying subscriptions and likes both ways.
use super::*;
use crate::api::models::Track;
use crate::youtube::account::{self, ClientKeys, Error, Profile, Session, Stored};
use crate::youtube::likes::Channel;

/// The live connection, shared by whichever requests are running.
pub(super) type SharedSession = Arc<tokio::sync::Mutex<Option<Session>>>;

pub enum Request {
    /// Open the browser and wait for the listener to sign in.
    Connect {
        generation: u64,
        keys: ClientKeys,
    },
    /// The listener gave up on signing in.
    CancelConnect,
    /// Reconnect from the credential store, if a grant is kept there.
    Restore {
        generation: u64,
    },
    Disconnect,
    /// Read subscriptions and likes. `serial` says which edits it follows.
    Sync {
        generation: u64,
        serial: u64,
    },
    Subscribe {
        generation: u64,
        channel: Channel,
        subscribe: bool,
    },
    Rate {
        generation: u64,
        track: Box<Track>,
        video: String,
        liked: bool,
    },
}

pub enum Response {
    Connected {
        generation: u64,
        result: Result<Profile, String>,
    },
    Restored {
        generation: u64,
        result: Result<Option<Profile>, String>,
    },
    Synced {
        generation: u64,
        serial: u64,
        result: Result<(Vec<Channel>, Vec<Track>), Error>,
    },
    Subscribed {
        generation: u64,
        channel: Channel,
        subscribe: bool,
        result: Result<Option<String>, Error>,
    },
    Rated {
        generation: u64,
        track: Box<Track>,
        video: String,
        liked: bool,
        result: Result<(), Error>,
    },
}

/// A token for the live connection, kept locked only while it is renewed.
async fn token(session: &SharedSession, http: &reqwest::Client) -> Result<String, Error> {
    let mut session = session.lock().await;
    let session = session.as_mut().ok_or(Error::Revoked)?;
    session.access_token(http).await
}

impl Worker {
    pub(super) fn youtube_account(&mut self, request: Request) {
        let events = self.events.clone();
        let waker = self.waker.clone();
        let emit = move |response: Response| {
            let _ = events.send(Event::YouTubeAccount(response));
            waker.wake();
        };
        let session = Arc::clone(&self.youtube_session);
        let http = self.http.client();
        match request {
            Request::CancelConnect => {
                if let Some(cancel) = self.youtube_login.take() {
                    let _ = cancel.send(true);
                }
            }
            Request::Connect { generation, keys } => {
                if let Some(cancel) = self.youtube_login.take() {
                    let _ = cancel.send(true);
                }
                let (cancel, cancelled) = tokio::sync::watch::channel(false);
                self.youtube_login = Some(cancel);
                let lease = self.credentials.lease(CredentialSlot::YouTube);
                tokio::spawn(async move {
                    let result = sign_in(http, keys, cancelled, lease, session).await;
                    emit(Response::Connected { generation, result });
                });
            }
            Request::Restore { generation } => {
                let lease = self.credentials.lease(CredentialSlot::YouTube);
                tokio::spawn(async move {
                    let result = match lease.load_secret().await {
                        Ok(Some(bytes)) => match Stored::decode(&bytes) {
                            Some(stored) => {
                                let profile = stored.profile.clone();
                                *session.lock().await = Some(Session::new(stored));
                                Ok(Some(profile))
                            }
                            None => Ok(None),
                        },
                        Ok(None) => Ok(None),
                        Err(_) => Err("Could not read the saved YouTube sign-in.".to_owned()),
                    };
                    emit(Response::Restored { generation, result });
                });
            }
            Request::Disconnect => {
                self.credentials.invalidate(CredentialSlot::YouTube);
                let lease = self.credentials.lease(CredentialSlot::YouTube);
                tokio::spawn(async move {
                    let stored = session.lock().await.take();
                    if let (Some(session), Ok(http)) = (stored, http) {
                        account::revoke(&http, &session.stored.refresh_token).await;
                    }
                    let _ = lease.clear_secret(false).await;
                });
            }
            Request::Sync { generation, serial } => {
                tokio::spawn(async move {
                    let result = async {
                        let http = http.map_err(|_| Error::Network)?;
                        let access = token(&session, &http).await?;
                        let (channels, likes) = tokio::join!(
                            account::subscriptions(&http, &access),
                            account::likes(&http, &access)
                        );
                        Ok((channels?, likes?))
                    }
                    .await;
                    emit(Response::Synced {
                        generation,
                        serial,
                        result,
                    });
                });
            }
            Request::Subscribe {
                generation,
                channel,
                subscribe,
            } => {
                tokio::spawn(async move {
                    let result = async {
                        let http = http.map_err(|_| Error::Network)?;
                        let access = token(&session, &http).await?;
                        if subscribe {
                            account::subscribe(&http, &access, &channel.id)
                                .await
                                .map(Some)
                        } else if let Some(id) = &channel.subscription {
                            account::unsubscribe(&http, &access, id)
                                .await
                                .map(|()| None)
                        } else {
                            Ok(None)
                        }
                    }
                    .await;
                    emit(Response::Subscribed {
                        generation,
                        channel,
                        subscribe,
                        result,
                    });
                });
            }
            Request::Rate {
                generation,
                track,
                video,
                liked,
            } => {
                tokio::spawn(async move {
                    let result = async {
                        let http = http.map_err(|_| Error::Network)?;
                        let access = token(&session, &http).await?;
                        account::rate(&http, &access, &video, liked).await
                    }
                    .await;
                    emit(Response::Rated {
                        generation,
                        track,
                        video,
                        liked,
                        result,
                    });
                });
            }
        }
    }
}

/// The whole sign-in: the browser, the code, the tokens, and keeping the grant.
async fn sign_in(
    http: Result<reqwest::Client, String>,
    keys: ClientKeys,
    cancelled: tokio::sync::watch::Receiver<bool>,
    lease: crate::credentials::Lease,
    session: SharedSession,
) -> Result<Profile, String> {
    let http = http?;
    // Google lets a desktop client use any port on the loopback address.
    let listener = crate::auth::listen_for_redirect(0)
        .map_err(|_| "Could not start the sign-in.".to_owned())?;
    let port = listener
        .local_addr()
        .map_err(|_| "Could not start the sign-in.".to_owned())?
        .port();
    let redirect = format!("http://127.0.0.1:{port}{}", crate::auth::REDIRECT_PATH);
    let state = crate::auth::random_token(16);
    let verifier = crate::auth::random_token(48);
    let challenge = crate::auth::pkce_challenge(&verifier);
    let url = account::authorize_url(&keys, &redirect, &state, &challenge);
    crate::opener::open(&url).map_err(|_| "Could not open your browser.".to_owned())?;
    let code = crate::auth::wait_for_code_on_listener(listener, &state, cancelled)
        .await
        .map_err(|_| "Signing in to Google didn't finish. Try again.".to_owned())?;
    let tokens = account::exchange_code(&http, &keys, &code, &verifier, &redirect)
        .await
        .map_err(|error| error.to_string())?;
    let refresh_token = tokens
        .refresh_token
        .ok_or_else(|| "Google did not give Encore a lasting sign-in. Try again.".to_owned())?;
    let profile = account::profile(&http, &tokens.access_token)
        .await
        .map_err(|error| error.to_string())?;
    let stored = Stored {
        keys,
        refresh_token,
        profile: profile.clone(),
    };
    let bytes = stored
        .encode()
        .ok_or_else(|| "Could not save the YouTube sign-in.".to_owned())?;
    lease
        .save_secret(bytes)
        .await
        .map_err(|_| "Could not save the YouTube sign-in.".to_owned())?;
    *session.lock().await = Some(Session::new(stored));
    Ok(profile)
}
