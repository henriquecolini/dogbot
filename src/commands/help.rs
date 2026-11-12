use crate::prelude::*;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;

pub async fn handle(bot: Bot, Context { chat_id, .. }: Context) -> BotResult<()> {
    bot.send_html(chat_id, text()).await?;
    Ok(())
}

/// The command list, framed for someone who may not know what the bot is.
/// Commands are left as plain `/name` so Telegram makes them tappable.
pub fn text() -> String {
    format!(
        "<b>dogbot</b> {version}\n\
         I run this chat's <code>main.dog</code> on every message that isn't a command, \
         and reply with what it outputs.\n\n\
         {commands}\n\n\
         New here? /start walks you through it.\n\
         Guide and dog4 cheat sheet: {docs}",
        version = escape_html(option_env!("APP_VERSION").unwrap_or("(dev)")),
        commands = escape_html(super::Command::descriptions().to_string()),
        docs = escape_html(super::docs_url()),
    )
}
