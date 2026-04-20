use crate::prelude::*;
use teloxide::prelude::*;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct ChownCommand {}

pub async fn handle(bot: Bot, Context { chat_id, .. }: Context) -> BotResult<()> {
    bot.send_code(chat_id, "chown: not implemented yet").await?;
    Ok(())
}
