use crate::prelude::*;
use files::perm::Clause;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct ChmodCommand {
    clause: Clause,
    path: String,
}

pub async fn handle(
    pool: PgPool,
    Context {
        user_id,
        connected_chat_id,
        ..
    }: Context,
    ChmodCommand { clause, path }: ChmodCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    files::set_permission(&mut cn, connected_chat_id, user_id, &path, clause)?;
    Ok(())
}
