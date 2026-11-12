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
        // One failing path should not stop the others, so each is reported
        // on its own instead of bailing out with `?`.
        if let Err(err) = files::remove(&mut cn, connected_chat_id, user_id, &path, recursive) {
            report_error(&bot, chat_id, "rm", &err.into()).await;
        }
    }
    Ok(())
}
