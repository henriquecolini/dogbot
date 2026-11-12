//! An SSH server exposing each chat's emulated filesystem over SFTP, so it can
//! be mounted with `sshfs` and edited with ordinary tools, and a console
//! (`ssh chat@host`) that runs the chat's `main.dog` from a terminal.
//!
//! Authentication mirrors the `/connect` command: the SSH **username** names
//! the chat, and the **public key** identifies the user. A session is only
//! granted when that user is already a member of that chat, and every
//! filesystem operation then runs under their existing permission bits.
//!
//! A login that fails is told why, through keyboard-interactive: OpenSSH shows
//! that method's instructions, which makes it the one way to get a message to
//! someone whose key was refused. Passwords are not offered at all, so nobody is
//! ever asked for one that doesn't exist.

mod console;
mod fs;

#[cfg(test)]
mod tests;

use crate::prelude::*;
use fs::DogFs;
use log::*;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Config, Msg, Server as _, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet, Pty};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
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

/// The SHA-256 fingerprint of the server's host key, for users to check
/// against what `ssh` shows on first connect. `None` before the SSH server has
/// started for the first time and made one.
pub fn host_key_fingerprint(cn: &mut PgConnection) -> Option<String> {
    let bytes = model::Setting::get(cn, HOST_KEY_SETTING).ok()??;
    let key = PrivateKey::from_openssh(String::from_utf8(bytes).ok()?).ok()?;
    Some(key.public_key().fingerprint(HashAlg::Sha256).to_string())
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
        methods: MethodSet::from(&[MethodKind::PublicKey, MethodKind::KeyboardInteractive][..]),
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
            refusal: None,
            explained: false,
            channels: Arc::new(Mutex::new(HashMap::new())),
            ptys: HashSet::new(),
            consoles: HashSet::new(),
        }
    }
}

struct SshSession {
    pool: PgPool,
    peer: Option<SocketAddr>,
    /// Set once authentication succeeds.
    identity: Option<(UserId, ChatId)>,
    /// Why the most telling key so far was refused, to explain once the client
    /// runs out of keys.
    refusal: Option<Refusal>,
    /// Whether the refusal has been shown. OpenSSH retries keyboard-interactive
    /// up to three times, and once is enough.
    explained: bool,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    /// Channels that asked for a terminal, so their console echoes and edits.
    ptys: HashSet<ChannelId>,
    /// Channels running a console, which reads its own end-of-input.
    consoles: HashSet<ChannelId>,
}

/// Why a key did not get in. Ordered from least to most specific: a key we
/// know says more about what went wrong than one we don't, so it wins when a
/// client offers several.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Refusal {
    UnknownKey { fingerprint: String },
    NoSuchChat { chat: String },
    NotAMember { chat: String },
}

impl Refusal {
    fn explain(&self) -> String {
        match self {
            Refusal::UnknownKey { fingerprint } => format!(
                "dogbot doesn't know your key ({fingerprint}).\n\
                 Register it by sending the bot, on Telegram:\n\
                 \n    /key add github:<your GitHub username>\n\
                 \nor /key add followed by the contents of your public key file\n\
                 (e.g. ~/.ssh/id_ed25519.pub). Then try again."
            ),
            Refusal::NoSuchChat { chat } => format!(
                "dogbot has no chat called {chat:?}.\n\
                 The SSH username is a chat's hostname (set with /hostname). Send /mount\n\
                 in the chat you want and it prints the exact command."
            ),
            Refusal::NotAMember { chat } => format!(
                "Your key is fine, but you haven't talked in {chat:?} yet.\n\
                 Send any message in that chat first: that's how dogbot learns you belong there."
            ),
        }
    }
}

/// The help no-one asked for: no key was offered at all.
const NO_KEY_OFFERED: &str = "dogbot only accepts SSH keys, and your client didn't offer one.\n\
Create one with `ssh-keygen -t ed25519`, register it by sending the bot\n\
/key add github:<your GitHub username> (or the contents of ~/.ssh/id_ed25519.pub),\n\
then try again.";

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

        let resolved = tokio::task::spawn_blocking(move || -> anyhow::Result<Result<(UserId, ChatId), Refusal>> {
            let mut cn = pool.get()?;

            let Some(key) = model::SshKey::find_by_fingerprint(&mut cn, &fingerprint)? else {
                return Ok(Err(Refusal::UnknownKey { fingerprint }));
            };
            let user_id = UserId(key.user_id as u64);

            // Only a registered key learns whether a chat exists, the same
            // thing `/connect` tells any user.
            let Some(chat) = model::Chat::find(&mut cn, parse_username(&username))? else {
                return Ok(Err(Refusal::NoSuchChat { chat: username }));
            };
            let chat_id = chat.id();

            // Membership is the gate, exactly as it is for `/connect`.
            if !model::UserInChat::exists(&mut cn, user_id, chat_id)? {
                return Ok(Err(Refusal::NotAMember { chat: username }));
            }
            Ok(Ok((user_id, chat_id)))
        })
        .await??;

        match resolved {
            Ok((user_id, chat_id)) => {
                info!(
                    "SFTP auth accepted: user {} on chat {} from {:?}",
                    user_id.0, chat_id.0, self.peer
                );
                self.identity = Some((user_id, chat_id));
                Ok(Auth::Accept)
            }
            Err(refusal) => {
                warn!("SFTP auth rejected for {user:?} from {:?}: {refusal:?}", self.peer);
                if self.refusal.as_ref().is_none_or(|seen| refusal > *seen) {
                    self.refusal = Some(refusal);
                }
                Ok(Auth::reject())
            }
        }
    }

    /// Never lets anyone in: it exists to tell a refused client why, once its
    /// keys have run out. The first round shows the explanation with no
    /// prompts; the client answers with nothing, and is refused.
    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        response: Option<russh::server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if response.is_some() || self.explained {
            return Ok(Auth::reject());
        }
        self.explained = true;
        let explanation = match &self.refusal {
            Some(refusal) => refusal.explain(),
            None => NO_KEY_OFFERED.to_string(),
        };
        Ok(Auth::Partial {
            name: Cow::Borrowed(""),
            instructions: Cow::Owned(explanation),
            prompts: Cow::Borrowed(&[]),
        })
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
        // A console reads end-of-input itself: piped messages still have to be
        // answered after stdin closes.
        if !self.consoles.contains(&channel) {
            // The SFTP subsystem is done, as a real `sftp-server` would be when
            // its input ends. `scp` treats a channel that closes without an
            // exit status as a failure, even after a good transfer.
            session.exit_status_request(channel, 0)?;
            session.close(channel)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ptys.insert(channel);
        session.channel_success(channel)?;
        Ok(())
    }

    /// `ssh chat@host`: the interactive console, or one message per line of
    /// stdin when there is no terminal.
    async fn shell_request(
        &mut self,
        channel_id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(console) = self.console(channel_id, session) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        let Some(channel) = self.channels.lock().await.remove(&channel_id) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        self.consoles.insert(channel_id);
        session.channel_success(channel_id)?;
        // Spawned: the session must keep reading, or the console would never
        // see what is typed into it.
        tokio::spawn(console::shell(console, channel));
        Ok(())
    }

    /// `ssh chat@host 'message'`: runs one message and exits with its status.
    async fn exec_request(
        &mut self,
        channel_id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(console) = self.console(channel_id, session) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        // The message is the command line, so stdin is read and thrown away:
        // left unread, a client piping something large would stall the session.
        if let Some(mut channel) = self.channels.lock().await.remove(&channel_id) {
            tokio::spawn(async move { while channel.wait().await.is_some() {} });
        }
        self.consoles.insert(channel_id);
        session.channel_success(channel_id)?;
        let command = String::from_utf8_lossy(data).into_owned();
        tokio::spawn(console::exec(console, command));
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

impl SshSession {
    /// A console for `channel`, as the authenticated user.
    fn console(&self, channel: ChannelId, session: &Session) -> Option<console::Console> {
        let (user_id, chat_id) = self.identity?;
        Some(console::Console {
            pool: self.pool.clone(),
            user_id,
            chat_id,
            out: console::Out {
                handle: session.handle(),
                id: channel,
                pty: self.ptys.contains(&channel),
            },
        })
    }
}
