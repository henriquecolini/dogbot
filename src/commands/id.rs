use crate::prelude::*;
use teloxide::prelude::*;

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        connected_chat_id,
        chat_id,
        user_id,
        user_username,
        ..
    }: Context,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    let chat = model::Chat::get(&mut cn, chat_id)?;
    let connected_chat = model::Chat::get(&mut cn, connected_chat_id)?;
    bot.send_code(
        chat_id,
        format!(
            "user: {}\nchat: {}\nconnected to: {}",
            user_username.unwrap_or_else(|| user_id.to_string()),
            chat.alias.unwrap_or_else(|| chat_id.to_string()),
            connected_chat
                .alias
                .unwrap_or_else(|| connected_chat_id.to_string()),
        ),
    )
    .await?;
    Ok(())
}
