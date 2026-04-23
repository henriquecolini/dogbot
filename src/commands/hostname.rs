use crate::prelude::*;
use clap::Parser;
use diesel::result::DatabaseErrorKind;
use diesel::result::Error::DatabaseError;
use teloxide::prelude::*;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct HostnameCommand {
    alias: Option<String>,
}

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        chat_id,
        connected_chat_id,
        ..
    }: Context,
    HostnameCommand { alias }: HostnameCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    if let Some(alias) = alias.as_deref() {
        if alias.is_empty() || alias.contains(['/', ':', '\n']) {
            bot.send_code(chat_id, "hostname: invalid hostname").await?;
            return Ok(());
        }
    }
    match model::Chat::set_alias(&mut cn, connected_chat_id, alias.as_deref()) {
        Ok(_) => {}
        Err(DatabaseError(DatabaseErrorKind::UniqueViolation, ..)) => {
            bot.send_code(chat_id, "hostname: hostname already in use").await?;
        }
        Err(err) => return Err(err.into()),
    }
    Ok(())
}
