use crate::prelude::*;
use teloxide::prelude::*;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct ChownCommand {
    /// New owner: a @username, a bare username, or a numeric user ID.
    owner: String,
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
    ChownCommand { owner, path }: ChownCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;

    let lookup = owner.strip_prefix('@').unwrap_or(&owner);
    let new_owner = match lookup.parse::<u64>() {
        Ok(id) => Some(UserId(id)),
        Err(_) => model::User::find_by_username(&mut cn, lookup)?.map(|u| UserId(u.id as u64)),
    };

    let Some(new_owner) = new_owner else {
        bot.send_code(chat_id, format!("chown: {owner}: no such user"))
            .await?;
        return Ok(());
    };

    if let Err(err) = files::set_owner(&mut cn, connected_chat_id, user_id, &path, new_owner) {
        bot.send_code(chat_id, format!("chown: {err}")).await?;
    }
    Ok(())
}
