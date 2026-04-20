use crate::prelude::*;
use files::perm::Clause;
use teloxide::prelude::*;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct ChmodCommand {
    clause: Clause,
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
    ChmodCommand { clause, path }: ChmodCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    match files::set_permission(&mut cn, connected_chat_id, user_id, &path, clause) {
        Ok(_) => {}
        Err(err) => {
            bot.send_code(chat_id, format!("chmod: {}", err))
                .await?;
        }
    }
    Ok(())
}
