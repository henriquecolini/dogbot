use crate::prelude::*;
use teloxide::prelude::*;

/// Telegram sends `/start` when someone first opens the bot, sometimes with a
/// deep-link payload after it, which is accepted and ignored.
#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct StartCommand {
    #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
    _payload: Vec<String>,
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
    _: StartCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    let has_main = files::stat(&mut cn, connected_chat_id, user_id, "main.dog").is_ok();
    bot.send_html(chat_id, welcome(has_main)).await?;
    Ok(())
}

/// The introduction: what the bot is, and the shortest path to a script
/// that answers.
pub fn welcome(has_main: bool) -> String {
    let first_step = if has_main {
        "This chat already has a <code>main.dog</code>: send a message and see what it says."
    } else {
        "/init writes a starter <code>main.dog</code>. Then send <code>ping</code>."
    };
    format!(
        "<b>Hi! I'm a bot you program yourself.</b>\n\n\
         Every chat with me has its own little Unix filesystem. When someone sends a \
         message, I run that chat's <code>main.dog</code>, a script in \
         <a href=\"{dog4}\">dog4</a>, and whatever it outputs is my reply. You edit the \
         files from your own machine over SSH.\n\n\
         <b>Getting going</b>\n\
         1. {first_step}\n\
         2. <code>/key add github:yourname</code> registers your SSH keys \
         (or paste a public key after <code>/key add</code>).\n\
         3. /mount shows how to reach the files, and how to talk to your script from \
         a terminal without anyone watching.\n\
         4. Edit <code>main.dog</code>, send a message, repeat.\n\n\
         /help lists every command. The guide, with a dog4 cheat sheet: {docs}",
        dog4 = super::DOG4_PLAYGROUND,
        docs = escape_html(super::docs_url()),
    )
}

/// What the bot says when it is added to a group.
pub fn group_welcome() -> String {
    "<b>Hi! I'm a bot this group programs itself.</b>\n\n\
     This group now has its own filesystem. I run its <code>main.dog</code> on every \
     message and reply with what it outputs. There isn't one yet, so I'll stay quiet \
     until someone writes it.\n\n\
     /init sets up a starter that only answers <code>ping</code>, <code>hi</code> and \
     <code>count</code>. /start explains the rest."
        .to_string()
}
