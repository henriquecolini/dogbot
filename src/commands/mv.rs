use crate::prelude::*;
use teloxide::prelude::*;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct MvCommand {
    source: String,
    dest: String,
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
    MvCommand { source, dest }: MvCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    if let Err(err) = files::rename(&mut cn, connected_chat_id, user_id, &source, &dest) {
        bot.send_code(chat_id, format!("mv: {err}")).await?;
    }
    Ok(())
}
