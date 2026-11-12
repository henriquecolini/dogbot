use crate::prelude::*;
use teloxide::prelude::*;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct MountCommand {}

/// The host users connect to, as `/mount` prints it.
pub fn ssh_host() -> String {
    std::env::var("SFTP_HOST").unwrap_or_else(|_| "localhost".to_string())
}

/// The port users connect to, as `/mount` prints it.
pub fn ssh_port() -> String {
    std::env::var("SFTP_PORT").unwrap_or_else(|_| "2222".to_string())
}

/// A chat's SSH username: its hostname, or `chat<id>` without one. The prefix
/// is there because a username beginning with `-` is unusable with ssh.
pub fn ssh_username(chat: Option<&model::Chat>, chat_id: ChatId) -> String {
    match chat.and_then(|c| c.alias.as_deref()) {
        Some(alias) => alias.to_string(),
        None => format!("chat{}", chat_id.0),
    }
}

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        chat_id,
        user_id,
        connected_chat_id,
        ..
    }: Context,
    _: MountCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;

    let chat = model::Chat::find(&mut cn, ChatAddress::Id(connected_chat_id))?;
    let has_alias = chat.as_ref().is_some_and(|c| c.alias.is_some());
    let user = escape_html(ssh_username(chat.as_ref(), connected_chat_id));
    let host = escape_html(ssh_host());
    let port = escape_html(ssh_port());
    let fingerprint = crate::sftp::host_key_fingerprint(&mut cn);
    let keys = model::SshKey::list(&mut cn, user_id)?;

    let mut reply = String::new();

    if keys.is_empty() {
        reply.push_str(
            "⚠️ You have no SSH key registered yet. Send \
             <code>/key add github:yourname</code>, or <code>/key add</code> followed by \
             your public key.\n\n",
        );
    }

    // One `Host` entry serves every chat: the username picks the chat, so the
    // commands below read the way the access model works.
    reply.push_str(&format!(
        "<b>Reaching {user} from your machine</b>\n\n\
         Once, add this to <code>~/.ssh/config</code>:\n\
         <pre>Host dogbot\n    HostName {host}\n    Port {port}</pre>\n\
         Then, from a terminal:\n\
         <pre>ssh {user}@dogbot            # talk to main.dog\n\
         ssh {user}@dogbot 'ping'     # one message, one reply\n\
         sftp {user}@dogbot           # browse and copy files</pre>\n"
    ));

    // `uid`/`gid` make the files show up as the local user's. Without them a
    // listing shows the owner's Telegram id, and tools that try to preserve
    // ownership on save — sed, cp -p — warn about it on every write.
    //
    // `reconnect` is what keeps the mount usable when the bot is redeployed:
    // the container restart drops every connection, and sshfs on its own
    // leaves a dead mount behind that only `fusermount -u` clears.
    // `ServerAliveInterval` is how it notices promptly.
    reply.push_str(&format!(
        "To use your own editor, mount it (Linux, or macOS with macFUSE):\n\
         <pre>mkdir -p ~/mnt/{user}\n\
         sshfs {user}@dogbot:/ ~/mnt/{user} \\\n\
         \x20 -o reconnect,ServerAliveInterval=15,ServerAliveCountMax=3 \\\n\
         \x20 -o uid=$(id -u),gid=$(id -g)</pre>\n\
         Unmount with <code>fusermount3 -u ~/mnt/{user}</code> (<code>umount</code> on macOS).\n"
    ));

    if let Some(fingerprint) = fingerprint {
        reply.push_str(&format!(
            "\nThe first time, ssh asks whether to trust the server. Say yes if it shows \
             <code>{}</code>.\n",
            escape_html(fingerprint)
        ));
    }

    if !has_alias {
        reply.push_str(&format!(
            "\nTip: /hostname <i>name</i> gives this chat a friendlier username than {user}.\n"
        ));
    }

    bot.send_html(chat_id, reply).await?;
    Ok(())
}
