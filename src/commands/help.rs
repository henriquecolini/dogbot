use crate::prelude::*;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;

pub async fn handle(bot: Bot, Context { chat_id, .. }: Context) -> BotResult<()> {
    bot.send_code(chat_id, Command::descriptions().to_string())
        .await?;
    Ok(())
}
