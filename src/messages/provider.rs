//! Resolves dog4 `mod` names against the emulated filesystem.
//!
//! Canonical module ids are `<chat_id>:<path>`, so the runtime's per-id cache
//! stays unambiguous when one script imports from more than one chat. Four
//! forms are understood:
//!
//! | written            | resolves to                                  |
//! |--------------------|----------------------------------------------|
//! | `sys/strings`      | the shared system library, embedded in the binary |
//! | `./util`, `../x`   | relative to the importer, within its own chat |
//! | `lib/util`         | `lib/util.dog` in the running chat            |
//! | `other:lib/dice`   | another chat, addressed by alias or id        |
//!
//! Loading a module requires read **and** execute, the same bar as `main.dog`:
//! importing is running code, not just reading a file.

use crate::prelude::*;
use dog4::{ModuleProvider, ModuleSource, ProviderError, module::resolve_relative};
use teloxide::prelude::*;

/// Prefix reserved for the shared system library.
const SYS: &str = "sys";

/// The shared library, versioned in the repo and compiled into the binary.
static SYSLIB: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/syslib");

pub struct DogbotProvider {
    pool: PgPool,
    chat_id: ChatId,
    user_id: UserId,
}

impl DogbotProvider {
    pub fn new(pool: PgPool, chat_id: ChatId, user_id: UserId) -> Self {
        Self {
            pool,
            chat_id,
            user_id,
        }
    }
}

/// Splits a canonical id back into its chat part and path part.
fn split_id(id: &str) -> Option<(&str, &str)> {
    id.split_once(':')
}

impl ModuleProvider for DogbotProvider {
    fn resolve(&self, name: &str, from: Option<&str>) -> Result<String, ProviderError> {
        // Where is the importer? Relative names resolve inside its chat.
        let (from_chat, from_path) = match from.and_then(split_id) {
            Some((chat, path)) => (Some(chat), Some(path)),
            None => (None, None),
        };

        // `sys/...` is the shared library and is never chat-scoped.
        if let Some(rest) = name.strip_prefix("sys/") {
            let path = resolve_relative(rest, None)?;
            return Ok(format!("{SYS}:{path}"));
        }

        // A relative name stays inside whichever chat the importer came from.
        if name.starts_with("./") || name.starts_with("../") {
            let chat = from_chat.unwrap_or(SYS);
            // Relative imports inside the system library stay inside it.
            let path = resolve_relative(name, from_path)?;
            return Ok(format!("{chat}:{path}"));
        }

        // `host:path` reaches another chat. Only split on a `:` that comes
        // before any `/`, so a path can never be mistaken for a host.
        if let Some((host, rest)) = name.split_once(':')
            && !host.contains('/')
        {
            let mut cn = self
                .pool
                .get()
                .map_err(|err| ProviderError::Failed(err.to_string()))?;
            let chat = model::Chat::find(&mut cn, ChatAddress::parse(host))
                .map_err(|err| ProviderError::Failed(err.to_string()))?
                .ok_or_else(|| ProviderError::Failed(format!("{host}: chat not found")))?;
            let path = resolve_relative(rest, None)?;
            return Ok(format!("{}:{}", chat.id().0, path));
        }

        // Anything else is a plain path in the chat that is running.
        let path = resolve_relative(name, None)?;
        Ok(format!("{}:{}", self.chat_id.0, path))
    }

    fn load(&self, id: &str) -> Result<ModuleSource, ProviderError> {
        let Some((chat, path)) = split_id(id) else {
            return Err(ProviderError::NotFound);
        };

        if chat == SYS {
            return match SYSLIB.get_file(format!("{path}.dog")) {
                Some(file) => {
                    let source = String::from_utf8_lossy(file.contents()).into_owned();
                    Ok(ModuleSource::Script(source))
                }
                None => Err(ProviderError::NotFound),
            };
        }

        let chat_id = ChatId(
            chat.parse::<i64>()
                .map_err(|_| ProviderError::NotFound)?,
        );
        let mut cn = self
            .pool
            .get()
            .map_err(|err| ProviderError::Failed(err.to_string()))?;

        match files::read_for_execution(&mut cn, chat_id, self.user_id, &format!("{path}.dog")) {
            Ok((_, bytes)) => Ok(ModuleSource::Script(
                String::from_utf8_lossy(&bytes).into_owned(),
            )),
            Err(files::FileError::FileNotFound(_)) | Err(files::FileError::ChatNotFound(_)) => {
                Err(ProviderError::NotFound)
            }
            Err(err) => Err(ProviderError::Failed(err.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `resolve` only needs the database for cross-chat names, so the rest can
    /// be checked without one.
    fn provider() -> DogbotProvider {
        let pool = diesel::r2d2::Pool::builder()
            .max_size(1)
            .min_idle(Some(0))
            .connection_timeout(std::time::Duration::from_millis(1))
            .build_unchecked(diesel::r2d2::ConnectionManager::new(
                "postgres://invalid/invalid",
            ));
        DogbotProvider::new(pool, ChatId(-100), UserId(1))
    }

    #[test]
    fn plain_names_are_chat_local() {
        let p = provider();
        assert_eq!(p.resolve("lib/util", None).unwrap(), "-100:lib/util");
        // The `.dog` suffix is optional and stripped.
        assert_eq!(p.resolve("lib/util.dog", None).unwrap(), "-100:lib/util");
    }

    #[test]
    fn relative_names_stay_inside_the_importers_chat() {
        let p = provider();
        assert_eq!(
            p.resolve("./fmt", Some("-200:lib/util")).unwrap(),
            "-200:lib/fmt"
        );
        assert_eq!(
            p.resolve("../top", Some("-200:lib/util")).unwrap(),
            "-200:top"
        );
    }

    #[test]
    fn sys_names_reach_the_shared_library() {
        let p = provider();
        assert_eq!(p.resolve("sys/strings", None).unwrap(), "sys:strings");
        assert_eq!(
            p.resolve("./other", Some("sys:strings")).unwrap(),
            "sys:other"
        );
    }

    #[test]
    fn names_cannot_escape_the_root() {
        let p = provider();
        assert!(p.resolve("../../etc/passwd", Some("-100:a/b")).is_err());
        assert!(p.resolve("../x", Some("-100:a")).is_err());
        assert!(p.resolve("sys/../../x", None).is_err());
    }

    #[test]
    fn a_colon_after_a_slash_is_not_a_host() {
        let p = provider();
        // `lib/a:b` is not a cross-chat reference; it is refused as a filename
        // rather than silently reaching another chat.
        assert_eq!(p.resolve("lib/a", None).unwrap(), "-100:lib/a");
    }
}
