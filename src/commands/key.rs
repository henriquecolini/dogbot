use crate::prelude::*;
use russh::keys::{HashAlg, PublicKey};
use teloxide::prelude::*;

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct KeyCommand {
    #[command(subcommand)]
    action: KeyAction,
}

#[derive(clap::Subcommand, Clone, Debug, PartialEq)]
enum KeyAction {
    /// Registers an SSH public key for SFTP access.
    Add {
        /// The full contents of a .pub file.
        key: Vec<String>,
    },
    /// Lists your registered keys.
    List,
    /// Removes a key by its position in `list`.
    Rm { index: usize },
}

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        chat_id, user_id, ..
    }: Context,
    KeyCommand { action }: KeyCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;

    let reply = match action {
        KeyAction::Add { key } => {
            let text = key.join(" ");
            match text.parse::<PublicKey>() {
                Ok(parsed) => {
                    let fingerprint = parsed.fingerprint(HashAlg::Sha256).to_string();
                    let comment = parsed.comment().to_string();
                    let label = (!comment.is_empty()).then_some(comment.as_str());
                    match model::SshKey::insert(
                        &mut cn,
                        user_id,
                        &fingerprint,
                        &parsed.to_openssh().unwrap_or(text),
                        label,
                    ) {
                        Ok(_) => format!("key added: {fingerprint}"),
                        Err(diesel::result::Error::DatabaseError(
                            diesel::result::DatabaseErrorKind::UniqueViolation,
                            _,
                        )) => "key: already registered".to_string(),
                        Err(err) => format!("key: {err}"),
                    }
                }
                Err(err) => format!("key: not a valid public key: {err}"),
            }
        }
        KeyAction::List => {
            let keys = model::SshKey::list(&mut cn, user_id)?;
            if keys.is_empty() {
                "key: no keys registered".to_string()
            } else {
                keys.iter()
                    .enumerate()
                    .map(|(i, k)| {
                        format!(
                            "{}. {} {}",
                            i + 1,
                            k.fingerprint,
                            k.label.as_deref().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        KeyAction::Rm { index } => {
            let keys = model::SshKey::list(&mut cn, user_id)?;
            match index.checked_sub(1).and_then(|i| keys.get(i)) {
                Some(key) => {
                    model::SshKey::delete(&mut cn, user_id, key.id)?;
                    format!("key removed: {}", key.fingerprint)
                }
                None => format!("key: no key at position {index}"),
            }
        }
    };

    bot.send_code(chat_id, reply).await?;
    Ok(())
}
