use crate::prelude::*;
use russh::keys::{HashAlg, PublicKey};
use std::time::Duration;
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
        /// The full contents of a .pub file, or github:<user> / gitlab:<user>
        /// to import that account's public keys.
        #[arg(required = true)]
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
            match forge_account(&text)? {
                Some(forge) => {
                    let keys = forge.fetch_keys().await?;
                    let label = format!("{}:{}", forge.site.prefix(), forge.user);
                    let mut lines = Vec::new();
                    let mut added = 0;
                    for key in keys {
                        match add_key(&mut cn, user_id, &key, Some(&label))? {
                            Added::New(fingerprint) => {
                                added += 1;
                                lines.push(fingerprint);
                            }
                            Added::Already(fingerprint) => {
                                lines.push(format!("{fingerprint} (already registered)"))
                            }
                            // A forge lists key types we may not parse; the
                            // others are still worth having.
                            Added::Unparsable => {}
                        }
                    }
                    if lines.is_empty() {
                        return Err(BotError::user(format!(
                            "{label} has no SSH keys this server understands"
                        )));
                    }
                    format!(
                        "added {added} key{} from {label}:\n{}",
                        if added == 1 { "" } else { "s" },
                        lines.join("\n")
                    )
                }
                None => match add_key(&mut cn, user_id, &text, None)? {
                    Added::New(fingerprint) => format!("key added: {fingerprint}"),
                    Added::Already(_) => return Err(BotError::user("already registered")),
                    Added::Unparsable => {
                        return Err(BotError::user(
                            "that is not a public key. Paste a whole line from a .pub file \
                             (ssh-ed25519 AAAA... you@host), or use /key add github:yourname",
                        ));
                    }
                },
            }
        }
        KeyAction::List => {
            let keys = model::SshKey::list(&mut cn, user_id)?;
            if keys.is_empty() {
                return Err(BotError::user("no keys registered"));
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
                None => return Err(BotError::user(format!("no key at position {index}"))),
            }
        }
    };

    bot.send_code(chat_id, reply).await?;
    Ok(())
}

/// What became of one key.
enum Added {
    New(String),
    Already(String),
    Unparsable,
}

/// Registers one public key, given as a line of a `.pub` file.
fn add_key(
    cn: &mut PgConnection,
    user_id: UserId,
    text: &str,
    label: Option<&str>,
) -> BotResult<Added> {
    let Ok(parsed) = text.trim().parse::<PublicKey>() else {
        return Ok(Added::Unparsable);
    };
    let fingerprint = parsed.fingerprint(HashAlg::Sha256).to_string();
    let comment = parsed.comment().to_string();
    let label = label.or((!comment.is_empty()).then_some(comment.as_str()));
    let openssh = parsed.to_openssh().unwrap_or_else(|_| text.to_string());
    match model::SshKey::insert(cn, user_id, &fingerprint, &openssh, label) {
        Ok(_) => Ok(Added::New(fingerprint)),
        Err(diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::UniqueViolation,
            _,
        )) => Ok(Added::Already(fingerprint)),
        Err(err) => Err(err.into()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Site {
    GitHub,
    GitLab,
}

impl Site {
    fn prefix(self) -> &'static str {
        match self {
            Site::GitHub => "github",
            Site::GitLab => "gitlab",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Site::GitHub => "GitHub",
            Site::GitLab => "GitLab",
        }
    }
}

/// An account whose public keys a forge publishes at `/<user>.keys`.
#[derive(Debug, PartialEq, Eq)]
struct Forge<'a> {
    site: Site,
    user: &'a str,
}

/// Recognises `github:<user>` and `gitlab:<user>`. `Ok(None)` is anything
/// else, to be read as a key.
fn forge_account(text: &str) -> BotResult<Option<Forge<'_>>> {
    let Some((prefix, user)) = text.trim().split_once(':') else {
        return Ok(None);
    };
    let site = match prefix.to_ascii_lowercase().as_str() {
        "github" | "gh" => Site::GitHub,
        "gitlab" | "gl" => Site::GitLab,
        _ => return Ok(None),
    };
    let user = user.trim().trim_start_matches('@');
    // Only characters usernames can have, since this goes into a URL.
    let valid = !user.is_empty()
        && user.len() <= 255
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !valid {
        return Err(BotError::user(format!("{user:?} is not a {} username", site.name())));
    }
    Ok(Some(Forge { site, user }))
}

impl Forge<'_> {
    fn url(&self) -> String {
        match self.site {
            Site::GitHub => format!("https://github.com/{}.keys", self.user),
            Site::GitLab => format!("https://gitlab.com/{}.keys", self.user),
        }
    }

    /// The account's public keys, one per line as the forge lists them.
    async fn fetch_keys(&self) -> BotResult<Vec<String>> {
        let site = self.site.name();
        let unreachable = |err: &dyn std::fmt::Display| {
            log::warn!("fetching {}: {err}", self.url());
            BotError::user(format!("couldn't reach {site}, try again later"))
        };
        let client = teloxide::net::default_reqwest_settings()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|err| unreachable(&err))?;
        let response = client
            .get(self.url())
            .send()
            .await
            .map_err(|err| unreachable(&err))?;
        if response.status().as_u16() == 404 {
            return Err(BotError::user(format!("{site} has no user called {}", self.user)));
        }
        if !response.status().is_success() {
            return Err(unreachable(&response.status()));
        }
        let body = response.text().await.map_err(|err| unreachable(&err))?;
        let keys: Vec<String> = body
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        if keys.is_empty() {
            return Err(BotError::user(format!(
                "{} has no public keys on {site}. Add one there, or paste it here with /key add",
                self.user
            )));
        }
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_accounts_are_recognised() {
        assert_eq!(
            forge_account("github:alice").unwrap(),
            Some(Forge { site: Site::GitHub, user: "alice" })
        );
        assert_eq!(
            forge_account("GitLab:@bob.c").unwrap(),
            Some(Forge { site: Site::GitLab, user: "bob.c" })
        );
        assert_eq!(forge_account("ssh-ed25519 AAAA x@y").unwrap(), None);
        // Nothing that could change the URL gets through.
        assert!(forge_account("github:../x").is_err());
        assert!(forge_account("github:a?b").is_err());
        assert!(forge_account("github:").is_err());
    }

    /// Talks to GitHub, so not part of the automatic suite:
    ///   cargo test -- --ignored forge_keys_are_fetched
    #[tokio::test]
    #[ignore]
    async fn forge_keys_are_fetched() {
        let keys = Forge { site: Site::GitHub, user: "torvalds" }.fetch_keys().await.unwrap();
        assert!(keys.iter().all(|k| k.starts_with("ssh-") || k.starts_with("ecdsa-")), "{keys:?}");
        let missing = Forge { site: Site::GitHub, user: "this-user-surely-does-not-exist-xyz123" };
        let err = missing.fetch_keys().await.unwrap_err();
        assert!(err.to_string().contains("no user called"), "{err}");
    }
}
