use clap::Parser;
use teloxide::prelude::*;
use crate::prelude::*;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct ConnectCommand {
    address: String,
}

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        chat_is_private,
        chat_id,
        user_id,
        ..
    }: Context,
    ConnectCommand { address }: ConnectCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    if !chat_is_private {
        return Err(BotError::user(
            "you may only connect to another chat within a private chat",
        ));
    }
    let address = ChatAddress::parse(&address);
    let chat = model::Chat::find(&mut cn, address)?;
    if let Some(chat) = chat {
        if model::UserInChat::exists(&mut cn, user_id, chat.id())? {
            model::User::set_connection(&mut cn, user_id, Some(chat.id()))?;
            bot.send_code(chat_id, format!("connected to {address}"))
                .await?;
        } else {
            return Err(BotError::user("unauthorized"));
        }
    } else {
        return Err(BotError::user(format!("could not resolve hostname {address}")));
    }
    Ok(())
}
