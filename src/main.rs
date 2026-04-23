pub mod chat_address;
mod commands;
pub mod files;
mod formatted;
mod messages;
mod model;
pub mod prelude;
mod result;
pub mod schema;

use crate::commands::*;
use crate::formatted::SendFormatted;
use crate::result::*;
use commands::Command;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::*;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use log::*;
use teloxide::dptree::case;
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

    let bot = Bot::from_env();
    let handler = Update::filter_message()
        .map(move || pool.clone())
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
                        .branch(case![Command::Help].endpoint(help::handle))
                        .branch(case![Command::Hostname(c)].endpoint(hostname::handle))
                        .branch(case![Command::Connect(c)].endpoint(connect::handle))
                        .branch(case![Command::Disconnect].endpoint(disconnect::handle))
                        .branch(case![Command::Ls(c)].endpoint(ls::handle))
                        .branch(case![Command::Mkdir(c)].endpoint(mkdir::handle))
                        .branch(case![Command::Write(c)].endpoint(write::handle))
                        .branch(case![Command::Read(c)].endpoint(read::handle))
                        .branch(case![Command::Rm(c)].endpoint(rm::handle))
                        .branch(case![Command::Chmod(c)].endpoint(chmod::handle))
                        .branch(case![Command::Chown(c)].endpoint(chown::handle))
                        .branch(case![Command::Id].endpoint(id::handle)),
                )
                .branch(
                    dptree::entry()
                        .filter_map(filter_map_not_command)
                        .inspect(|msg: Message| info!("Received message: {}", msg.id))
                        .endpoint(messages::handle),
                ),
        )
        .endpoint(default_endpoint);
    Dispatcher::builder(bot, handler).build().dispatch().await;
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
    let (
        MessageKind::Common(MessageCommon {
            media_kind:
                MediaKind::Text(MediaText {
                    text: message_content,
                    ..
                }),
            reply_to_message,
            ..
        }),
        Some(user),
        chat,
    ) = (msg.kind, msg.from, msg.chat)
    else {
        warn!("Received message without text: {:?}", msg.id);
        return None;
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
    })
}

async fn extract_maybe_command(bot: Bot, msg: Message, me: Me) -> Option<MaybeCommand> {
    let bot_name = me.user.username.expect("Bots must have a username");
    let Some(text) = msg.text().or_else(|| msg.caption()) else {
        return None;
    };
    match Command::parse(text, &bot_name) {
        Ok(command) => Some(MaybeCommand::Command(command)),
        Err(err) => {
            if let ParseError::UnknownCommand(uc) = &err {
                if !uc.starts_with('/') {
                    return Some(MaybeCommand::NotACommand);
                }
            }
            if let Err(err) = bot.send_code(msg.chat.id, err.to_string()).await {
                error!("Failed to send error message: {}", err);
            }
            None
        }
    }
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
