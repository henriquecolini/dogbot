pub mod chat_address;
mod commands;
pub mod files;
mod formatted;
mod messages;
mod model;
pub mod prelude;
mod result;
pub mod schema;
mod sftp;

use crate::commands::*;
use crate::formatted::SendFormatted;
use crate::result::*;
use commands::Command;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::*;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use log::*;
use std::ops::ControlFlow;
use teloxide::dispatching::DpHandlerDescription;
use teloxide::dptree::{self, case, Handler};
use teloxide::prelude::DependencyMap;
use teloxide::prelude::*;
use teloxide::types::*;
use teloxide::utils::command::{BotCommands, ParseError};

#[derive(Clone, Debug)]
pub struct Context {
    chat_id: ChatId,
    user_id: UserId,
    connected_chat_id: ChatId,
    message_id: MessageId,
    chat_name: String,
    chat_is_private: bool,
    user_first_name: String,
    user_last_name: Option<String>,
    user_full_name: String,
    user_username: Option<String>,
    message_content: String,
    reply_to_content: Option<String>,
    reply_to_from: Option<String>,
    document: Option<Document>,
}

#[derive(Debug, Clone)]
enum MaybeCommand {
    NotACommand,
    Command(Command),
}

type PgPool = Pool<ConnectionManager<PgConnection>>;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

fn run_migrations(cn: &mut impl MigrationHarness<pg::Pg>) {
    info!("Running migrations");
    cn.run_pending_migrations(MIGRATIONS)
        .expect("Could not run migrations");
}

/// Runs migrations once per test process.
///
/// Tests run in parallel and each builds its own connection; migrating
/// concurrently double-applies. A single shared guard is needed rather than one
/// per test module, since separate `Once`s don't serialise against each other.
#[cfg(test)]
fn run_migrations_once(cn: &mut impl MigrationHarness<pg::Pg>) {
    static MIGRATED: std::sync::Once = std::sync::Once::new();
    MIGRATED.call_once(|| run_migrations(cn));
}

fn get_connection_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    info!("Connecting to database at {}", database_url);
    Pool::builder()
        .build(ConnectionManager::new(database_url))
        .expect("Failed to create connection pool")
}

#[tokio::main]
async fn main() {
    env_logger::init();
    dotenvy::dotenv().ok();

    let pool = get_connection_pool();
    let mut cn = pool
        .get()
        .expect("Failed to get connection from pool to run migrations");

    run_migrations(&mut cn);

    info!("Starting bot");

    // The SFTP server shares the pool and runs alongside the dispatcher.
    let sftp_pool = pool.clone();
    let sftp_bind = std::env::var("SFTP_BIND").unwrap_or_else(|_| "0.0.0.0:2222".to_string());
    let sftp = tokio::spawn(async move {
        if let Err(err) = sftp::run(sftp_pool, sftp_bind).await {
            error!("SFTP server stopped: {err}");
        }
    });

    let bot = Bot::from_env();

    // The menu Telegram shows behind the `/` button. Without it, a newcomer
    // has to know the commands before they can find them.
    if let Err(err) = bot.set_my_commands(Command::bot_commands()).await {
        warn!("Could not set the command menu: {err}");
    }

    Dispatcher::builder(bot, schema(pool)).build().dispatch().await;

    sftp.abort();
}

/// The dispatcher's handler tree.
///
/// dptree type-checks the tree while it is built and panics on a mistake, so
/// building it is covered by a test rather than discovered at startup.
fn schema(pool: PgPool) -> Handler<'static, BotResult<()>, DpHandlerDescription> {
    Update::filter_message()
        .map(move || pool.clone())
        .branch(
            dptree::filter(added_to_group).endpoint(greet_group),
        )
        .branch(
            dptree::entry()
                .filter_map_async(extract_context)
                .inspect(|pool, ctx| {
                    register_message(pool, ctx).ok();
                })
                .filter_map_async(extract_maybe_command)
                .branch(
                    dptree::entry()
                        .filter_map(filter_map_command)
                        .inspect(|cmd: Command| info!("Received command: {cmd:?}"))
                        .branch(report_errors(
                            dptree::entry()
                            .branch(case![Command::Start(c)].endpoint(start::handle))
                            .branch(case![Command::Help].endpoint(help::handle))
                            .branch(case![Command::Init(c)].endpoint(init::handle))
                            .branch(case![Command::Hostname(c)].endpoint(hostname::handle))
                            .branch(case![Command::Connect(c)].endpoint(connect::handle))
                            .branch(case![Command::Disconnect].endpoint(disconnect::handle))
                            .branch(case![Command::Ls(c)].endpoint(ls::handle))
                            .branch(case![Command::Mkdir(c)].endpoint(mkdir::handle))
                            .branch(case![Command::Write(c)].endpoint(write::handle))
                            .branch(case![Command::Read(c)].endpoint(read::handle))
                            .branch(case![Command::Rm(c)].endpoint(rm::handle))
                            .branch(case![Command::Mv(c)].endpoint(mv::handle))
                            .branch(case![Command::Chmod(c)].endpoint(chmod::handle))
                            .branch(case![Command::Chown(c)].endpoint(chown::handle))
                            .branch(case![Command::Id].endpoint(id::handle))
                            .branch(case![Command::Key(c)].endpoint(key::handle))
                            .branch(case![Command::Mount(c)].endpoint(mount::handle))
                            .branch(case![Command::Upload(c)].endpoint(upload::handle))
                            .branch(case![Command::Download(c)].endpoint(download::handle))
                        )),
                )
                .branch(
                    dptree::entry()
                        .filter_map(filter_map_not_command)
                        .inspect(|msg: Message| info!("Received message: {}", msg.id))
                        .endpoint(messages::handle),
                ),
        )
        .endpoint(default_endpoint)
}

fn register_message(
    pool: PgPool,
    Context {
        chat_id,
        user_id,
        message_id,
        message_content,
        chat_is_private,
        chat_name,
        user_first_name,
        user_last_name,
        user_username,
        ..
    }: Context,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    model::Chat::upsert(&mut cn, chat_id, Some(&chat_name), !chat_is_private)?;
    model::User::upsert(
        &mut cn,
        user_id,
        &user_first_name,
        user_last_name.as_deref(),
        user_username.as_deref(),
    )?;
    model::UserInChat::upsert(&mut cn, user_id, chat_id)?;
    model::Message::insert(
        &mut cn,
        message_id,
        chat_id,
        Some(user_id),
        &message_content,
    )?;
    Ok(())
}

async fn extract_context(pool: PgPool, msg: Message) -> Option<Context> {
    let mut cn = match pool.get() {
        Ok(cn) => cn,
        Err(err) => {
            error!("Failed to get connection from pool: {err}");
            return None;
        }
    };
    let chat = msg.chat;
    let Some(user) = msg.from else {
        return None;
    };
    let (message_content, document, reply_to_message) = match msg.kind {
        MessageKind::Common(msg) => match msg.media_kind {
            MediaKind::Text(MediaText { text, .. }) => (text, None, msg.reply_to_message),
            MediaKind::Document(MediaDocument {
                caption, document, ..
            }) => (
                caption.unwrap_or_else(String::new),
                Some(document),
                msg.reply_to_message,
            ),
            MediaKind::Animation(MediaAnimation { caption, .. })
            | MediaKind::Audio(MediaAudio { caption, .. })
            | MediaKind::Photo(MediaPhoto { caption, .. })
            | MediaKind::Video(MediaVideo { caption, .. })
            | MediaKind::Voice(MediaVoice { caption, .. }) => (
                caption.unwrap_or_else(String::new),
                None,
                msg.reply_to_message,
            ),
            _ => return None,
        },
        _ => return None,
    };
    let (chat_is_private, chat_name) = match chat.kind {
        ChatKind::Public(chat) => (false, chat.title.unwrap_or_default()),
        ChatKind::Private(_) => (true, user.full_name()),
    };
    let connected_chat_id = if let Ok(db_user) = model::User::get(&mut cn, user.id) {
        db_user
            .current_connection
            .map(|id| ChatId(id))
            .unwrap_or(chat.id)
    } else {
        chat.id
    };
    let (reply_to_content, reply_to_from) = match reply_to_message {
        None => (None, None),
        Some(msg) => (
            msg.text().map(|t| t.to_owned()),
            msg.from.map(|u| u.full_name()),
        ),
    };
    Some(Context {
        chat_id: chat.id,
        user_id: user.id,
        connected_chat_id,
        message_id: msg.id,
        chat_name,
        chat_is_private,
        user_full_name: user.full_name(),
        user_first_name: user.first_name,
        user_last_name: user.last_name,
        user_username: user.username,
        message_content,
        reply_to_content,
        reply_to_from,
        document,
    })
}

async fn extract_maybe_command(
    bot: Bot,
    Context {
        message_content,
        chat_id,
        chat_is_private,
        ..
    }: Context,
    me: Me,
) -> Option<MaybeCommand> {
    let bot_name = me.user.username.expect("Bots must have a username");
    if !message_content.starts_with('/') {
        return Some(MaybeCommand::NotACommand);
    }
    match Command::parse(&message_content, &bot_name) {
        Ok(command) => Some(MaybeCommand::Command(command)),
        Err(ParseError::WrongBotName(_)) => None,
        // In a group, unknown commands only get an error when explicitly
        // addressed to this bot (`/blah@bot_name`); otherwise they may belong
        // to another bot. A private chat has no other bot to answer them.
        Err(ParseError::UnknownCommand(_))
            if !chat_is_private && !is_addressed_to(&message_content, &bot_name) =>
        {
            None
        }
        Err(ParseError::UnknownCommand(command)) => {
            let command = command.trim_start_matches('/');
            let message = format!("unknown command /{command}; /help lists them");
            if let Err(err) = bot.send_error(chat_id, None, message).await {
                error!("Failed to send error message: {}", err);
            }
            None
        }
        Err(err) => {
            if let Err(err) = bot.send_error(chat_id, None, err).await {
                error!("Failed to send error message: {}", err);
            }
            None
        }
    }
}

/// Whether the leading `/command` token carries an `@bot_name` suffix for this bot.
fn is_addressed_to(message_content: &str, bot_name: &str) -> bool {
    message_content
        .split_whitespace()
        .next()
        .and_then(|token| token.split_once('@'))
        .is_some_and(|(_, name)| name.eq_ignore_ascii_case(bot_name))
}

/// Wraps the command handlers so that an error they return reaches the chat
/// through [`report_error`], instead of the dispatcher only logging it.
fn report_errors(
    inner: Handler<'static, BotResult<()>, DpHandlerDescription>,
) -> Handler<'static, BotResult<()>, DpHandlerDescription> {
    // Borrow the inner handler's signature. `Entry` would make dptree panic
    // as soon as this is passed to `.branch()`, which happens at startup.
    let sig = inner.sig().clone();
    dptree::from_fn_with_description(
        inner.description().clone(),
        move |deps: DependencyMap, _cont| {
            let inner = inner.clone();
            async move {
                let bot = deps.get::<Bot>();
                let chat_id = deps.get::<Context>().chat_id;
                let cmd = deps.get::<Command>().name();
                match inner.dispatch(deps.clone()).await {
                    ControlFlow::Break(Err(err)) => {
                        report_error(&bot, chat_id, &cmd, &err).await;
                        ControlFlow::Break(Ok(()))
                    }
                    ControlFlow::Break(Ok(())) => ControlFlow::Break(Ok(())),
                    ControlFlow::Continue(_) => ControlFlow::Continue(deps),
                }
            }
        },
        sig,
    )
}

/// Whether this message is the bot being added to a group.
fn added_to_group(msg: Message, me: Me) -> bool {
    msg.new_chat_members()
        .is_some_and(|members| members.iter().any(|m| m.id == me.user.id))
}

/// Says hello to a group that has just added the bot, so nobody is left
/// wondering why it sits there in silence.
async fn greet_group(bot: Bot, msg: Message) -> BotResult<()> {
    bot.send_html(msg.chat.id, start::group_welcome()).await?;
    Ok(())
}

fn filter_map_command(maybe_command: MaybeCommand) -> Option<Command> {
    match maybe_command {
        MaybeCommand::Command(command) => Some(command),
        _ => None,
    }
}

fn filter_map_not_command(maybe_command: MaybeCommand) -> Option<()> {
    match maybe_command {
        MaybeCommand::NotACommand => Some(()),
        _ => None,
    }
}

async fn default_endpoint() -> BotResult<()> {
    Ok(())
}

#[cfg(test)]
mod error_tests {
    use super::*;
    use crate::formatted::{Formatted, error};
    use clap::Parser;

    fn context() -> Context {
        Context {
            chat_id: ChatId(1),
            user_id: UserId(1),
            connected_chat_id: ChatId(1),
            message_id: MessageId(1),
            chat_name: String::new(),
            chat_is_private: true,
            user_first_name: String::new(),
            user_last_name: None,
            user_full_name: String::new(),
            user_username: None,
            message_content: String::new(),
            reply_to_content: None,
            reply_to_from: None,
            document: None,
        }
    }

    async fn run(
        handler: Handler<'static, BotResult<()>, DpHandlerDescription>,
    ) -> ControlFlow<BotResult<()>, DependencyMap> {
        // Nothing listens on this port, so reporting fails fast and is only logged.
        let bot = Bot::new("1:test").set_api_url("http://127.0.0.1:9".parse().unwrap());
        let mut deps = DependencyMap::new();
        deps.insert(bot);
        deps.insert(context());
        deps.insert(Command::Help);
        report_errors(handler).dispatch(deps).await
    }

    /// Mirrors `main`: dptree checks the tree while `schema` builds it, and
    /// teloxide checks its dependencies in `build`. Both panic at startup.
    #[tokio::test]
    async fn the_dispatcher_builds() {
        // Never connects: the pool and bot are only needed to build the tree.
        let pool = Pool::builder().build_unchecked(ConnectionManager::new("postgres://unused"));
        let bot = Bot::new("1:test");
        let _ = Dispatcher::builder(bot, schema(pool)).build();
    }

    #[tokio::test]
    async fn a_failing_command_is_handled_instead_of_bubbling_up() {
        let failing = dptree::entry().endpoint(|| async { Err(BotError::user("nope")) });
        assert!(matches!(run(failing).await, ControlFlow::Break(Ok(()))));
    }

    #[tokio::test]
    async fn success_and_non_matching_handlers_pass_through() {
        let ok = dptree::entry().endpoint(|| async { Ok(()) });
        assert!(matches!(run(ok).await, ControlFlow::Break(Ok(()))));
        let skipped = dptree::entry().filter(|| false).endpoint(|| async { Ok(()) });
        assert!(matches!(run(skipped).await, ControlFlow::Continue(_)));
    }

    #[test]
    fn command_names_are_what_the_user_types() {
        assert_eq!(Command::Help.name(), "help");
        assert_eq!(Command::Disconnect.name(), "disconnect");
        let ls = Command::Ls(ls::LsCommand::parse_from(["ls", "-l"]));
        assert_eq!(ls.name(), "ls");
    }

    #[test]
    fn errors_look_the_same_everywhere() {
        let Formatted::Code(with_cmd) = error(Some("ls"), "x: file not found") else {
            panic!("errors are sent as code");
        };
        assert_eq!(with_cmd, "ls: x: file not found");
        let Formatted::Code(bare) = error(None, "boom") else {
            panic!("errors are sent as code");
        };
        assert_eq!(bare, "boom");
    }
}
