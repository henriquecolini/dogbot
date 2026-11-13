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
    /// The message as the history keeps it.
    record: messages::record::Record,
    /// When it was sent.
    date: time::OffsetDateTime,
    /// The text of the message it replies to, and who wrote it, as Telegram
    /// sent them along. The history's copy is preferred, since that one
    /// follows edits; these are for messages older than the history.
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

    // Scripts ask who the bot is (`bot.me`), to spot mentions.
    match bot.get_me().await {
        Ok(me) => {
            let _ = messages::ME.set(messages::Identity {
                id: me.user.id.0 as i64,
                username: me.user.username.clone().unwrap_or_default(),
                name: me.user.full_name(),
            });
        }
        Err(err) => warn!("Could not ask Telegram who the bot is: {err}"),
    }

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
    dptree::entry()
        .map(move || pool.clone())
        .branch(Update::filter_edited_message().endpoint(record_edit))
        .branch(Update::filter_message_reaction_updated().endpoint(record_reaction))
        .branch(Update::filter_message().chain(messages_schema()))
}

/// The branch for new messages.
fn messages_schema() -> Handler<'static, BotResult<()>, DpHandlerDescription> {
    dptree::entry()
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
        record,
        date,
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
    let origin = model::Origin {
        user_id: Some(user_id),
        date: Some(date),
        ..Default::default()
    };
    model::Message::insert(&mut cn, message_id, chat_id, &record, &origin)?;
    prune_history(&mut cn, chat_id, user_id, message_id);
    Ok(())
}

/// Every this many messages, a chat's history is cut back to its limit. Doing
/// it on every message would cost a query each time to delete nothing; this
/// way the history overshoots by at most this much.
const PRUNE_EVERY: i32 = 50;

/// Keeps the chat's history within `history_limit` (`config/bot.json`).
fn prune_history(cn: &mut PgConnection, chat_id: ChatId, user_id: UserId, id: MessageId) {
    if id.0 % PRUNE_EVERY != 0 {
        return;
    }
    let limit = messages::load_config(cn, chat_id, user_id).history_limit;
    match model::Message::prune(cn, chat_id, limit as i64) {
        Ok(0) => {}
        Ok(n) => info!("Pruned {n} old messages from chat {chat_id}"),
        Err(err) => warn!("Could not prune the history of chat {chat_id}: {err}"),
    }
}

/// A Telegram date (seconds since the epoch) as the database stores it.
fn to_time(unix: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
}

/// Someone edited a message: the history takes the new version. `main.dog`
/// doesn't run again; fixing a typo in a request shouldn't answer it twice.
async fn record_edit(pool: PgPool, msg: Message) -> BotResult<()> {
    let Some(record) = messages::record::describe(&msg) else {
        return Ok(());
    };
    let mut cn = pool.get()?;
    if let Some(user) = &msg.from {
        model::User::upsert(
            &mut cn,
            user.id,
            &user.first_name,
            user.last_name.as_deref(),
            user.username.as_deref(),
        )?;
    }
    let edited_at = msg
        .edit_date()
        .map(|d| to_time(d.timestamp()))
        .unwrap_or_else(time::OffsetDateTime::now_utc);
    model::Message::apply_edit(&mut cn, msg.id, msg.chat.id, &record, edited_at)?;
    Ok(())
}

/// Someone changed their reactions to a message. Telegram sends these only to
/// a bot that is an administrator of the group. Anonymous reactions (from a
/// channel or an anonymous admin) have nobody to file them under, so they are
/// left out.
async fn record_reaction(pool: PgPool, update: MessageReactionUpdated) -> BotResult<()> {
    let MaybeAnonymousUser::User(user) = &update.actor else {
        return Ok(());
    };
    let emojis = update
        .new_reaction
        .iter()
        .map(|r| match r {
            ReactionType::Emoji { emoji } => emoji.clone(),
            ReactionType::CustomEmoji { custom_emoji_id } => {
                format!("custom:{}", custom_emoji_id.0)
            }
            ReactionType::Paid => "⭐".to_string(),
        })
        .collect::<Vec<_>>();
    let mut cn = pool.get()?;
    model::User::upsert(
        &mut cn,
        user.id,
        &user.first_name,
        user.last_name.as_deref(),
        user.username.as_deref(),
    )?;
    model::Reaction::replace(&mut cn, update.chat.id, update.message_id, user.id, &emojis)?;
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
    let record = messages::record::describe(&msg)?;
    let user = msg.from.clone()?;
    let document = msg.document().cloned();
    let reply_to_message = msg.reply_to_message().cloned();
    let date = to_time(msg.date.timestamp());
    let chat = msg.chat;
    let message_content = record.content.clone();
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
            msg.text().or(msg.caption()).map(|t| t.to_owned()),
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
        record,
        date,
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
        record,
        ..
    }: Context,
    me: Me,
) -> Option<MaybeCommand> {
    // Someone joining, a pin: history, but nothing anyone said to the bot.
    if record.is_service() {
        return None;
    }
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
            record: messages::record::Record::text(""),
            date: time::OffsetDateTime::UNIX_EPOCH,
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

    /// Telegram sends reactions only to a bot that asks for them by name, and
    /// teloxide asks for what the handler tree says it handles.
    #[test]
    fn the_bot_asks_for_edits_and_reactions() {
        // The list itself is private to teloxide; its debug form shows it.
        let pool = Pool::builder().build_unchecked(ConnectionManager::new("postgres://unused"));
        let wanted = format!("{:?}", schema(pool).description());
        for kind in ["Kind(Message)", "Kind(EditedMessage)", "Kind(MessageReaction)"] {
            assert!(wanted.contains(kind), "{kind} missing from {wanted}");
        }
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
