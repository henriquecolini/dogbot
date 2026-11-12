use crate::prelude::*;
use teloxide::prelude::*;

/// What `/init` writes. Kept as files so they read as what they are, and so
/// tests can run the script.
pub const MAIN_TEMPLATE: &str = include_str!("templates/main.dog");
pub const README_TEMPLATE: &str = include_str!("templates/README.md");

#[derive(clap::Parser, Clone, Debug, PartialEq)]
pub struct InitCommand {
    /// A hostname for the chat, which is its SSH username.
    hostname: Option<String>,
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
    InitCommand { hostname }: InitCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;

    let wrote_readme = init(&mut cn, connected_chat_id, user_id, hostname.as_deref())?;

    let chat = model::Chat::find(&mut cn, ChatAddress::Id(connected_chat_id))?;
    let has_alias = chat.as_ref().is_some_and(|c| c.alias.is_some());
    let has_key = !model::SshKey::list(&mut cn, user_id)?.is_empty();

    let mut reply = format!(
        "Created <code>main.dog</code>{}. Send <code>ping</code> to try it.\n\n<b>Next</b>\n",
        if wrote_readme { " and <code>README.md</code>" } else { "" }
    );
    if !has_alias {
        reply.push_str("• /hostname <i>name</i> gives this chat a name to use over SSH.\n");
    }
    if !has_key {
        reply.push_str("• <code>/key add github:yourname</code> registers your SSH keys.\n");
    }
    reply.push_str("• /mount shows how to edit the files from your machine.");

    bot.send_html(chat_id, reply).await?;
    Ok(())
}

/// Names the chat, if asked to, and writes the starter files, returning
/// whether the README was written too. All or nothing: a refused `/init`
/// leaves neither a new name nor stray files behind.
///
/// `main.dog` gets mode 750: the script runs with the *sender's* permissions,
/// so in a group it has to be readable and executable by the group, or it
/// would only ever answer whoever wrote it.
pub fn init(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    hostname: Option<&str>,
) -> BotResult<bool> {
    cn.transaction(|cn| write_starter(cn, chat_id, user_id, hostname))
}

fn write_starter(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    hostname: Option<&str>,
) -> BotResult<bool> {
    match files::stat(cn, chat_id, user_id, "main.dog") {
        Ok(_) => {
            return Err(BotError::user(
                "main.dog already exists. /rm main.dog first if you want the starter back",
            ));
        }
        Err(files::FileError::FileNotFound(_)) => {}
        Err(err) => return Err(err.into()),
    }
    if let Some(hostname) = hostname {
        super::hostname::set(cn, chat_id, Some(hostname))?;
    }
    files::write(cn, chat_id, user_id, "main.dog", MAIN_TEMPLATE.as_bytes())?;
    files::set_mode(cn, chat_id, user_id, "main.dog", 0o750)?;

    // The README is a courtesy for whoever mounts the chat; never overwrite one.
    let wrote_readme = match files::stat(cn, chat_id, user_id, "README.md") {
        Err(files::FileError::FileNotFound(_)) => {
            files::write(cn, chat_id, user_id, "README.md", README_TEMPLATE.as_bytes())?;
            files::set_mode(cn, chat_id, user_id, "README.md", 0o640)?;
            true
        }
        _ => false,
    };
    Ok(wrote_readme)
}
