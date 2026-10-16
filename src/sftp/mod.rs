//! An SSH server exposing each chat's emulated filesystem over SFTP, so it can
//! be mounted with `sshfs` and edited with ordinary tools.
//!
//! Authentication mirrors the `/connect` command: the SSH **username** names
//! the chat, and the **public key** identifies the user. A session is only
//! granted when that user is already a member of that chat, and every
//! filesystem operation then runs under their existing permission bits.

mod fs;

#[cfg(test)]
mod tests;

use crate::prelude::*;
use fs::DogFs;
use log::*;
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Config, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use tokio::sync::Mutex;

/// Where the persistent SSH host key is stored.
const HOST_KEY_SETTING: &str = "ssh_host_key";

/// How often to ask an idle client whether it is still there.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// Unanswered keepalives before the connection is dropped.
const KEEPALIVE_MAX: usize = 3;

/// Backstop for a connection that goes quiet without the keepalives noticing.
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(3600);

/// Loads the host key, generating and persisting one on first boot.
///
/// It lives in the database rather than on disk so no extra volume has to be
/// mounted, and so the key survives a container being replaced — otherwise
/// every deploy would trip clients' known_hosts checks.
fn load_or_create_host_key(pool: &PgPool) -> anyhow::Result<PrivateKey> {
    let mut cn = pool.get()?;

    if let Some(bytes) = model::Setting::get(&mut cn, HOST_KEY_SETTING)? {
        let pem = String::from_utf8(bytes)?;
        return Ok(PrivateKey::from_openssh(&pem)?);
    }

    info!("Generating a new SSH host key");
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let pem = key.to_openssh(russh::keys::ssh_key::LineEnding::LF)?;
    model::Setting::set(&mut cn, HOST_KEY_SETTING, pem.as_bytes())?;
    Ok(key)
}

/// Parses an SSH username into the chat it refers to.
///
/// Aliases set with `/hostname` are the normal form. Numeric chat ids are
/// written with a `chat` prefix (`chat-1001234567890`) because a username
/// starting with `-` is unusable on an `ssh` command line.
fn parse_username(user: &str) -> ChatAddress<'_> {
    match user.strip_prefix("chat") {
        Some(rest) if rest.parse::<i64>().is_ok() => ChatAddress::parse(rest),
        _ => ChatAddress::parse(user),
    }
}

pub async fn run(pool: PgPool, bind: String) -> anyhow::Result<()> {
    let addr: SocketAddr = bind.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    run_on_listener(pool, listener).await
}

/// Serves on an already-bound listener. Taking the socket rather than an
/// address lets a caller learn the port before the server starts, which
/// matters when the port was chosen by the OS.
pub async fn run_on_listener(pool: PgPool, listener: tokio::net::TcpListener) -> anyhow::Result<()> {
    run_with_timings(pool, listener, KEEPALIVE_INTERVAL, INACTIVITY_TIMEOUT).await
}

/// `run_on_listener` with the SSH timings spelled out, so a test can use ones
/// short enough to observe.
async fn run_with_timings(
    pool: PgPool,
    listener: tokio::net::TcpListener,
    keepalive: Duration,
    inactivity: Duration,
) -> anyhow::Result<()> {
    let host_key = load_or_create_host_key(&pool)?;

    // A mount sits idle for hours at a time, which is indistinguishable from a
    // client that has gone away unless the server asks. Without keepalives the
    // only thing bounding a connection is `inactivity_timeout`, so an idle
    // mount was dropped on the hour and sshfs — which does not reconnect by
    // itself — was left wedged. russh resets the inactivity timer on any
    // traffic, a keepalive reply included, so asking every 30 seconds keeps a
    // live connection up indefinitely while a dead one is let go after
    // `KEEPALIVE_MAX` unanswered probes, about two minutes.
    let config = Config {
        auth_rejection_time: Duration::from_secs(1),
        inactivity_timeout: Some(inactivity),
        keepalive_interval: Some(keepalive),
        keepalive_max: KEEPALIVE_MAX,
        keys: vec![host_key],
        ..Default::default()
    };

    info!("SFTP server listening on {}", listener.local_addr()?);

    let mut server = SftpServer { pool };
    server.run_on_socket(Arc::new(config), &listener).await?;
    Ok(())
}

#[derive(Clone)]
struct SftpServer {
    pool: PgPool,
}

impl russh::server::Server for SftpServer {
    type Handler = SshSession;

    fn new_client(&mut self, peer: Option<SocketAddr>) -> SshSession {
        SshSession {
            pool: self.pool.clone(),
            peer,
            identity: None,
            channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

struct SshSession {
    pool: PgPool,
    peer: Option<SocketAddr>,
    /// Set once authentication succeeds.
    identity: Option<(UserId, ChatId)>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl russh::server::Handler for SshSession {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        let fingerprint = public_key
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string();
        let username = user.to_string();
        let pool = self.pool.clone();

        let resolved = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(UserId, ChatId)>> {
            let mut cn = pool.get()?;

            let Some(key) = model::SshKey::find_by_fingerprint(&mut cn, &fingerprint)? else {
                return Ok(None);
            };
            let user_id = UserId(key.user_id as u64);

            let Some(chat) = model::Chat::find(&mut cn, parse_username(&username))? else {
                return Ok(None);
            };
            let chat_id = chat.id();

            // Membership is the gate, exactly as it is for `/connect`.
            if !model::UserInChat::exists(&mut cn, user_id, chat_id)? {
                return Ok(None);
            }
            Ok(Some((user_id, chat_id)))
        })
        .await??;

        match resolved {
            Some((user_id, chat_id)) => {
                info!(
                    "SFTP auth accepted: user {} on chat {} from {:?}",
                    user_id.0, chat_id.0, self.peer
                );
                self.identity = Some((user_id, chat_id));
                Ok(Auth::Accept)
            }
            None => {
                warn!("SFTP auth rejected for {user:?} from {:?}", self.peer);
                Ok(Auth::reject())
            }
        }
    }

    /// Passwords are never accepted; a registered key is the only way in.
    async fn auth_password(&mut self, _user: &str, _password: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::reject())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some((user_id, chat_id)) = self.identity else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };

        if name != "sftp" {
            // No shell, no exec: this server only serves files.
            session.channel_failure(channel_id)?;
            return Ok(());
        }

        let Some(channel) = self.channels.lock().await.remove(&channel_id) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };

        session.channel_success(channel_id)?;
        let handler = DogFs::new(self.pool.clone(), user_id, chat_id);
        russh_sftp::server::run(channel.into_stream(), handler).await;
        Ok(())
    }
}
