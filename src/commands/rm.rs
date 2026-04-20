use crate::prelude::*;
use clap::Parser;
use teloxide::prelude::*;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct RmCommand {
    paths: Vec<String>,
    #[clap(short, long)]
    recursive: bool,
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
    RmCommand { paths, recursive }: RmCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    for path in paths {
        match files::remove(&mut cn, connected_chat_id, user_id, &path, recursive) {
            Ok(_) => {}
            Err(err) => {
                bot.send_code(chat_id, format!("rm: {}", err))
                    .await?;
            }
        }
    }
    Ok(())
}
