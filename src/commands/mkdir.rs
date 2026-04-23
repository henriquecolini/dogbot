use crate::prelude::*;
use clap::Parser;
use teloxide::prelude::*;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct MkdirCommand {
    #[clap(short,long)]
    parents: bool,
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
    MkdirCommand { parents, path }: MkdirCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    match files::mkdir(&mut cn, connected_chat_id, user_id, &path, parents) {
        Ok(_) => {}
        Err(err) => {
            bot.send_code(chat_id, format!("mkdir: {}", err))
                .await?;
        }
    }
    Ok(())
}
