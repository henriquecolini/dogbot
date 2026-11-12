use crate::prelude::*;
use clap::Parser;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct MkdirCommand {
    #[clap(short,long)]
    parents: bool,
    path: String,
}

pub async fn handle(
    pool: PgPool,
    Context {
        user_id,
        connected_chat_id,
        ..
    }: Context,
    MkdirCommand { parents, path }: MkdirCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    files::mkdir(&mut cn, connected_chat_id, user_id, &path, parents)?;
    Ok(())
}
