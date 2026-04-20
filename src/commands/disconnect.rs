use crate::prelude::*;
use teloxide::prelude::*;

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        chat_is_private,
        chat_id,
        user_id,
        ..
    }: Context,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    if !chat_is_private {
        bot.send_code(
            chat_id,
            "disconnect: you may only disconnect from another chat within a private chat",
        )
        .await?;
        return Ok(());
    }
    model::User::set_connection(&mut cn, user_id, None)?;
    bot.send_code(chat_id, "disconnected")
        .await?;
    Ok(())
}
