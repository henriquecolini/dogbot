use crate::prelude::*;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct MvCommand {
    source: String,
    dest: String,
}

pub async fn handle(
    pool: PgPool,
    Context {
        user_id,
        connected_chat_id,
        ..
    }: Context,
    MvCommand { source, dest }: MvCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    files::rename(&mut cn, connected_chat_id, user_id, &source, &dest)?;
    Ok(())
}
