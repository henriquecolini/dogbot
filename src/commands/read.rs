use crate::prelude::*;
use clap::Parser;
use teloxide::prelude::*;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct ReadCommand {
    path: String,
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
    ReadCommand { path }: ReadCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    match files::read(&mut cn, connected_chat_id, user_id, &path) {
        Ok((_,content)) => {
            bot.send_raw(chat_id, String::from_utf8_lossy(&content))
                .await?;
        }
        Err(err) => {
            bot.send_code(chat_id, format!("read: {}", err))
                .await?;
        }
    }
    Ok(())
}
