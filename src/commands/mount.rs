use crate::prelude::*;
use teloxide::prelude::*;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct MountCommand {}

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

    // Prefer the alias: a username beginning with `-` is unusable with ssh.
    let chat = model::Chat::find(&mut cn, ChatAddress::Id(connected_chat_id))?;
    let username = match chat.as_ref().and_then(|c| c.alias.as_deref()) {
        Some(alias) => alias.to_string(),
        None => format!("chat{}", connected_chat_id.0),
    };

    let host = std::env::var("SFTP_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("SFTP_PORT").unwrap_or_else(|_| "2222".to_string());

    let keys = model::SshKey::list(&mut cn, user_id)?;
    let mut reply = String::new();

    if keys.is_empty() {
        reply.push_str("No SSH key registered yet. Add one first:\n\n");
        reply.push_str("  /key add ssh-ed25519 AAAA... you@host\n\n");
    }

    // `uid`/`gid` make the files show up as the local user's. Without them a
    // listing shows the owner's Telegram id, and tools that try to preserve
    // ownership on save — sed, cp -p — warn about it on every write.
    //
    // `reconnect` is what keeps the mount usable when the bot is redeployed:
    // the container restart drops every connection, and sshfs on its own
    // leaves a dead mount behind that only `fusermount -u` clears.
    // `ServerAliveInterval` is how it notices promptly.
    reply.push_str(&format!(
        "mkdir -p ~/mnt/dogbot\n\
         sshfs -p {port} \\\n\
         \x20 -o reconnect,ServerAliveInterval=15,ServerAliveCountMax=3 \\\n\
         \x20 -o uid=$(id -u),gid=$(id -g) \\\n\
         \x20 {username}@{host}:/ ~/mnt/dogbot\n"
    ));

    if chat.as_ref().and_then(|c| c.alias.as_deref()).is_none() {
        reply.push_str("\nTip: set a hostname with /hostname <name> for a friendlier mount.\n");
    }

    bot.send_code(chat_id, reply).await?;
    Ok(())
}
