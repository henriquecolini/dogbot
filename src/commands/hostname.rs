use crate::prelude::*;
use clap::Parser;
use diesel::result::DatabaseErrorKind;
use diesel::result::Error::DatabaseError;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct HostnameCommand {
    alias: Option<String>,
}

pub async fn handle(
    pool: PgPool,
    Context {
        connected_chat_id,
        ..
    }: Context,
    HostnameCommand { alias }: HostnameCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    if let Some(alias) = alias.as_deref() {
        if alias.is_empty() || alias.contains(['/', ':', '\n']) {
            return Err(BotError::user("invalid hostname"));
        }
    }
    match model::Chat::set_alias(&mut cn, connected_chat_id, alias.as_deref()) {
        Ok(_) => {}
        Err(DatabaseError(DatabaseErrorKind::UniqueViolation, ..)) => {
            return Err(BotError::user("hostname already in use"));
        }
        Err(err) => return Err(err.into()),
    }
    Ok(())
}
