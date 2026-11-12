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
    set(&mut cn, connected_chat_id, alias.as_deref())?;
    let reply = match alias {
        Some(alias) => format!(
            "hostname set to {alias}. Over SSH this chat is {alias}@{} (see /mount).",
            super::mount::ssh_host()
        ),
        None => "hostname removed".to_string(),
    };
    bot.send_code(chat_id, reply).await?;
    Ok(())
}

/// Names a chat, or removes its name with `None`.
pub fn set(cn: &mut PgConnection, chat_id: ChatId, alias: Option<&str>) -> BotResult<()> {
    if let Some(alias) = alias {
        validate(alias)?;
    }
    match model::Chat::set_alias(cn, chat_id, alias) {
        Ok(_) => Ok(()),
        Err(DatabaseError(DatabaseErrorKind::UniqueViolation, ..)) => {
            Err(BotError::user("hostname already in use"))
        }
        Err(err) => Err(err.into()),
    }
}

/// A hostname is an SSH username and a path prefix (`name:lib/x`), so it has
/// to survive both. It must not look like a chat id either: `123` and
/// `chat-123` already mean chats by number, and an alias spelled that way
/// could never be reached.
pub fn validate(alias: &str) -> BotResult<()> {
    let well_formed = alias.len() <= 32
        && alias.starts_with(|c: char| c.is_ascii_alphabetic())
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !well_formed {
        return Err(BotError::user(
            "invalid hostname: use up to 32 letters, digits, '-', '_' and '.', \
             starting with a letter (it becomes an SSH username)",
        ));
    }
    let looks_numeric = alias
        .strip_prefix("chat")
        .is_some_and(|rest| rest.parse::<i64>().is_ok());
    if looks_numeric {
        return Err(BotError::user(
            "invalid hostname: chat<number> is reserved for chats without a hostname",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn hostnames_must_work_as_ssh_usernames() {
        for good in ["myproject", "cs101-team.3", "A_b"] {
            assert!(validate(good).is_ok(), "{good}");
        }
        for bad in ["", "3d", "my project", "a@b", "a:b", "a/b", "chat123", "chat-5", &"x".repeat(33)] {
            assert!(validate(bad).is_err(), "{bad}");
        }
    }
}
