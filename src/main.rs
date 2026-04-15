mod commands;
mod model;
mod result;
pub mod schema;
mod messages;
pub mod chat_address;
pub mod files;
pub mod timeout;

use crate::result::*;
use commands::Command;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::*;
use log::*;
use teloxide::prelude::*;
use teloxide::types::*;

type PgPool = Pool<ConnectionManager<PgConnection>>;

fn get_connection_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    Pool::builder()
        .build(ConnectionManager::new(database_url))
        .expect("Failed to create connection pool")
}

#[tokio::main]
async fn main() {
    env_logger::init();
    dotenvy::dotenv().ok();

    let pool = get_connection_pool();

    info!("Starting bot");

    let bot = Bot::from_env();
    let handler = dptree::entry()
        .map(move |_: Update| pool.clone())
        .branch(
            Update::filter_message()
                .filter_command::<Command>()
                .endpoint(on_command),
        )
        .branch(Update::filter_message().endpoint(on_message));
    Dispatcher::builder(bot, handler).build().dispatch().await;
}

async fn on_command(bot: Bot, pool: PgPool, msg: Message, command: Command) -> ResponseResult<()> {
    info!(
        "Received command {:?} from {} on {}",
        command,
        msg.from.as_ref().map(|u| u.id.0.to_string()).as_deref().unwrap_or("unknown"),
        msg.chat.title().as_deref().unwrap_or("unknown")
    );
    match commands::handle_command(bot, pool, msg, command).await {
        Ok(_) => Ok(()),
        Err(BotError::TelegramError(err)) => Err(err.into()),
        Err(err) => {
            error!("Error handling command: {:?}", err);
            Ok(())
        }
    }
}

async fn on_message(bot: Bot, pool: PgPool, msg: Message) -> ResponseResult<()> {
    let (
        MessageKind::Common(MessageCommon {
            media_kind: MediaKind::Text(MediaText { text, .. }),
            reply_to_message,
            ..
        }),
        Some(user),
        chat,
    ) = (msg.kind, msg.from, msg.chat)
    else {
        return Ok(());
    };
    info!("Received message from {} on {}", user.id, chat.id);
    match messages::handle_message(bot, pool, chat, user, msg.id, text, reply_to_message.map(|r| *r)).await {
        Ok(()) => Ok(()),
        Err(BotError::TelegramError(err)) => Err(err.into()),
        Err(e) => {
            error!("Error handling message: {:?}", e);
            Ok(())
        }
    }
}

