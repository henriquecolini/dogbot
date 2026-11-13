pub mod courier;
mod provider;
pub mod record;
mod runtime;

use crate::prelude::*;
pub use courier::Courier;
use dog4::{Builder, Capability, Limits, OutputBuffer, Value};
use log::*;
use provider::DogbotProvider;
use serde::Deserialize;
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use teloxide::prelude::*;
use tokio::time::interval;

/// Per-chat settings, read from `config/bot.json`.
#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default = "default_timeout")]
    timeout: f32,
    /// Whether scripts may make outbound HTTP requests.
    #[serde(default = "default_net")]
    net: bool,
    /// How many messages the chat's history keeps; older ones are deleted.
    #[serde(default = "default_history_limit")]
    pub history_limit: u32,
    /// The largest attachment, in bytes, `bot.download` and `bot.image` will
    /// fetch. Telegram itself serves bots nothing over 20 MiB.
    #[serde(default = "default_download_limit")]
    pub download_limit: u64,
}

fn default_timeout() -> f32 {
    90.0
}

fn default_net() -> bool {
    true
}

fn default_history_limit() -> u32 {
    10_000
}

fn default_download_limit() -> u64 {
    5 * 1024 * 1024
}

/// What Telegram lets a bot download at all.
pub const TELEGRAM_DOWNLOAD_LIMIT: u64 = 20 * 1024 * 1024;

impl Default for Config {
    fn default() -> Self {
        Config {
            timeout: default_timeout(),
            net: default_net(),
            history_limit: default_history_limit(),
            download_limit: default_download_limit(),
        }
    }
}

/// The bot's own account, as `bot.me` tells scripts. Set once at startup.
#[derive(Clone, Debug)]
pub struct Identity {
    pub id: i64,
    pub username: String,
    pub name: String,
}

pub static ME: std::sync::OnceLock<Identity> = std::sync::OnceLock::new();

/// Hard cap on what one script run may print, so a runaway loop cannot fill
/// memory. Replies are separately shortened to Telegram's own limit when sent
/// (see `formatted.rs`); this is the larger, cheaper guard.
const MAX_OUTPUT_BYTES: usize = 8 * 1024;

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        connected_chat_id,
        chat_id,
        chat_is_private,
        user_id,
        message_id,
        message_content,
        user_full_name,
        record,
        reply_to_content,
        reply_to_from,
        ..
    }: Context,
) -> BotResult<()> {
    let mut cn = pool.get()?;

    // The replied message as the history has it now, edits included; what
    // Telegram sent along is for messages older than the history.
    let reply_id = record.reply_to_id;
    let stored_reply = reply_id
        .and_then(|id| model::Message::get(&mut cn, chat_id, id).ok().flatten())
        .map(|(m, _, _)| m.content);
    let reply = stored_reply.or(reply_to_content).unwrap_or_default();

    let (source, config) = match load_main(&mut cn, connected_chat_id, user_id) {
        Ok(found) => found,
        Err(err) => {
            // A group whose bot has nothing to say stays quiet: it may simply
            // not be set up, and a reply to every message would be noise. A
            // private chat is someone talking to the bot directly, wondering
            // why it doesn't answer.
            if let Some(why) = explain_missing(&err) {
                if chat_is_private && !message_content.is_empty() {
                    bot.send_html(chat_id, why).await?;
                }
            } else {
                error!("Could not load main.dog: {err}");
            }
            return Ok(());
        }
    };
    drop(cn);

    let typer = {
        let bot = bot.clone();
        tokio::task::spawn(async move {
            let mut inter = interval(Duration::from_secs(4));
            tokio::time::sleep(Duration::from_secs(1)).await;
            loop {
                inter.tick().await;
                bot.send_chat_action(chat_id, ChatAction::Typing).await.ok();
            }
        })
    };

    // Ids name messages of the chat the message came from. Running another
    // chat's files with `/connect`, `history` lists that chat, where these ids
    // mean nothing.
    let here = chat_id == connected_chat_id;
    let ctx = ScriptContext {
        message: message_content,
        sender: user_full_name,
        reply,
        reply_from: reply_to_from.unwrap_or_default(),
        message_id: here.then_some(message_id.0 as i64),
        reply_id: reply_id.filter(|_| here),
    };
    // Messages go to the chat the message came from, even when the script runs
    // against another chat's files via `/connect`.
    let courier: Arc<dyn Courier> = Arc::new(courier::TelegramCourier {
        bot: bot.clone(),
        pool: pool.clone(),
        chat_id,
        handle: tokio::runtime::Handle::current(),
    });
    // dog4's Runtime is synchronous and not Send, so the whole run happens on a
    // blocking thread. Diesel is blocking too, which makes this the correct
    // place for the natives to reach the database.
    let result = {
        let (pool, config, ctx, courier) = (pool.clone(), config.clone(), ctx.clone(), courier.clone());
        tokio::task::spawn_blocking(move || {
            run_message(pool, connected_chat_id, user_id, &source, &config, &ctx, courier)
        })
        .await
    };

    typer.abort();

    let (text, later, tag) = match result {
        Ok(Run { output: Ok(output), later, tag }) => (raw(output), later, tag),
        Ok(Run { output: Err(err), later, .. }) => {
            let text = match group_tip(&pool, chat_id, chat_is_private, connected_chat_id) {
                Some(tip) => error_with_tip(err, tip),
                None => error(None, err),
            };
            // An error report is the bot's, whatever the script meant to say.
            (text, later, None)
        }
        Err(err) => {
            error!("Script task failed: {err}");
            (error(None, "The bot failed to run the script."), Vec::new(), None)
        }
    };

    let origin = model::Origin {
        trigger: Some(message_id),
        tag,
        ..Default::default()
    };
    let sent = send_reply(&bot, &pool, chat_id, text, origin).await;
    // Only now, with the reply out and recorded, so the scripts see it in
    // `history`. Queued even when the script failed after calling `later`: the
    // upkeep it asked for is usually independent of whatever went wrong.
    run_later(courier, pool, connected_chat_id, user_id, later, config, ctx);
    sent
}

/// Loads what a message runs: the chat's `main.dog`, as `user_id` may run it,
/// and the chat's configuration.
pub fn load_main(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
) -> Result<(String, Config), files::FileError> {
    let config = load_config(cn, chat_id, user_id);
    let (_, source) = files::read_for_execution(cn, chat_id, user_id, "main.dog")?;
    Ok((String::from_utf8_lossy(&source).into_owned(), config))
}

/// The chat's `config/bot.json`, or the defaults when it is missing or broken.
pub fn load_config(cn: &mut PgConnection, chat_id: ChatId, user_id: UserId) -> Config {
    files::read(cn, chat_id, user_id, "config/bot.json")
        .ok()
        .and_then(|(_, v)| serde_json::from_slice(&v).ok())
        .unwrap_or_default()
}

/// Why `load_main` found nothing to run, worded for the person who expected an
/// answer, as HTML. `None` for a failure that is the bot's, not theirs.
pub fn explain_missing(err: &files::FileError) -> Option<String> {
    use files::FileError::*;
    match err {
        FileNotFound(_) => Some(
            "This chat has no <code>main.dog</code> yet, so I have nothing to answer with.\n\n\
             /init writes a starter one. /start explains how this works."
                .to_string(),
        ),
        NotEnoughPermissions(_) => Some(
            "<code>main.dog</code> is here, but you can't run it: it has to be readable \
             <i>and</i> executable by whoever sends the message.\n\n\
             <code>/ls -l main.dog</code> shows its mode. Its owner can fix it with \
             <code>/chmod u+x main.dog</code>, or <code>/chmod g+rx main.dog</code> so it \
             answers everyone in a group."
                .to_string(),
        ),
        NotAFile(_) => Some(
            "<code>main.dog</code> is a directory; it has to be a file.".to_string(),
        ),
        QueryError(_) => None,
        other => Some(escape_html(other.to_string())),
    }
}

/// Chats already told how to test a script privately. The tip is for someone
/// who has just broken their bot in front of everyone; repeating it on every
/// error would be noise, so it is given once per chat per process.
static TIPPED: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(Default::default);

/// The tip to attach to a script error in a group, the first time one happens.
fn group_tip(pool: &PgPool, chat_id: ChatId, private: bool, target: ChatId) -> Option<String> {
    if private || !TIPPED.lock().unwrap().insert(chat_id.0) {
        return None;
    }
    // `/connect` takes the hostname, or the numeric id of a chat without one.
    let address = pool
        .get()
        .ok()
        .and_then(|mut cn| model::Chat::find(&mut cn, ChatAddress::Id(target)).ok().flatten())
        .and_then(|chat| chat.alias)
        .unwrap_or_else(|| target.0.to_string());
    Some(format!(
        "Tip: debug without an audience. In a private chat with me, \
         /connect {address} runs this chat's main.dog for you alone; \
         /mount shows how to do it from a terminal with ssh."
    ))
}

/// Sends a script's output, if there is any, and records it in the history.
async fn send_reply(
    bot: &Bot,
    pool: &PgPool,
    chat_id: ChatId,
    text: Formatted,
    origin: model::Origin,
) -> BotResult<()> {
    if text.is_empty() {
        return Ok(());
    }

    match bot.send_formatted(chat_id, text).await {
        Ok(msg) => {
            info!("Sent message successfully");
            // Bot messages are recorded with a null user, so `history` can show them.
            let mut cn = pool.get()?;
            let record = record::describe(&msg)
                .unwrap_or_else(|| record::Record::text(msg.text().unwrap_or_default()));
            model::Message::insert(&mut cn, msg.id, chat_id, &record, &origin)?;
        }
        Err(err) => {
            error!("Failed to send message: {err}");
            let _ = bot.send_error(chat_id, None, err).await;
        }
    };
    Ok(())
}

/// Chats with a deferred run in progress. A chat gets one at a time: the
/// scripts are upkeep, and a second copy racing the first over the same files
/// would only do the same work twice.
static BACKGROUND: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(Default::default);

/// Runs the scripts a message queued with `later`, one after another, in the
/// background.
///
/// Detached from the message's handler, so the chat's next message is answered
/// without waiting for them. They see the same globals as the run that queued
/// them; their output is discarded and their errors only logged, since by now
/// the reply has been sent.
pub fn run_later(
    courier: Arc<dyn Courier>,
    pool: PgPool,
    chat_id: ChatId,
    user_id: UserId,
    paths: Vec<String>,
    config: Config,
    ctx: ScriptContext,
) {
    if paths.is_empty() {
        return;
    }
    if !BACKGROUND.lock().unwrap().insert(chat_id.0) {
        info!("Skipping deferred scripts {paths:?}: chat {chat_id} already has one running");
        return;
    }
    tokio::task::spawn(async move {
        let task = tokio::task::spawn_blocking(move || {
            for path in paths {
                let source = match pool.get().map_err(|e| e.to_string()).and_then(|mut cn| {
                    files::read_for_execution(&mut cn, chat_id, user_id, &path)
                        .map_err(|e| e.to_string())
                }) {
                    Ok((_, bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(err) => {
                        warn!("Deferred script {path}: {err}");
                        continue;
                    }
                };
                // A deferred script can talk to the chat but not queue more work.
                let hooks = Hooks {
                    later: None,
                    courier: Some(courier.clone()),
                    tag: None,
                };
                match run_script(pool.clone(), chat_id, user_id, &source, &config, &ctx, hooks) {
                    Ok(_) => info!("Deferred script {path} finished"),
                    Err(err) => warn!("Deferred script {path} failed: {err}"),
                }
            }
        });
        if let Err(err) = task.await {
            error!("Deferred script task failed: {err}");
        }
        BACKGROUND.lock().unwrap().remove(&chat_id.0);
    });
}

/// What running a message produced.
pub struct Run {
    /// The reply, or the error report that replaces it.
    pub output: Result<String, String>,
    /// Scripts the run queued with `later`, for [`run_later`].
    pub later: Vec<String>,
    /// What the script tagged its reply as, with `bot.tag`.
    pub tag: Option<String>,
}

/// Runs `source` the way a message runs `main.dog`: able to send through
/// `courier` and to queue deferred scripts.
///
/// Blocking, like [`run_script`].
pub fn run_message(
    pool: PgPool,
    chat_id: ChatId,
    user_id: UserId,
    source: &str,
    config: &Config,
    ctx: &ScriptContext,
    courier: Arc<dyn Courier>,
) -> Run {
    let later = runtime::Later::default();
    let tag = runtime::Tag::default();
    let hooks = Hooks {
        later: Some(later.clone()),
        courier: Some(courier),
        tag: Some(tag.clone()),
    };
    let output = run_script(pool, chat_id, user_id, source, config, ctx, hooks);
    Run {
        output,
        later: later.take(),
        tag: tag.take(),
    }
}

/// The context values a script sees as globals.
#[derive(Clone, Default)]
pub struct ScriptContext {
    pub message: String,
    pub sender: String,
    pub reply: String,
    pub reply_from: String,
    /// The message being handled, as `history` and `bot.message` know it.
    /// `None` where there is no such message (the console, `/connect`).
    pub message_id: Option<i64>,
    /// The message it replies to.
    pub reply_id: Option<i64>,
}

/// What a run can reach beyond its own computation.
#[derive(Default)]
pub struct Hooks {
    /// Collects the scripts the run queues with `bot.later`; `None` makes that
    /// function raise, which is what a deferred run itself gets.
    pub later: Option<runtime::Later>,
    /// Delivers `bot.send` and `bot.edit`; `None` makes them raise.
    pub courier: Option<Arc<dyn courier::Courier>>,
    /// Where `bot.tag` leaves the run's tag. `None` gives the run one of its
    /// own that nobody reads, which is what a deferred run gets: its reply is
    /// discarded, and its `send`s carry the tag themselves.
    pub tag: Option<runtime::Tag>,
}

/// Compiles and runs one script, returning what to reply with.
///
/// `Ok` is a normal result, `Err` an error report; both may carry buffered
/// output, because a script can print before it fails.
///
/// Synchronous on purpose: dog4's `Runtime` is not `Send`, and Diesel blocks,
/// so this must run on a blocking thread.
fn run_script(
    pool: PgPool,
    chat_id: ChatId,
    user_id: UserId,
    source: &str,
    config: &Config,
    ctx: &ScriptContext,
    hooks: Hooks,
) -> Result<String, String> {
    let out = OutputBuffer::default();
    let state = runtime::State {
        pool: pool.clone(),
        chat_id,
        user_id,
        later: hooks.later,
        courier: hooks.courier,
        budget: Rc::new(Cell::new(runtime::MAX_MESSAGES)),
        tag: hooks.tag.unwrap_or_default(),
        trigger: ctx.message_id,
        download_limit: config.download_limit.min(TELEGRAM_DOWNLOAD_LIMIT),
    };
    let id = |id: Option<i64>| id.map(Value::Int).unwrap_or(Value::Null);

    let mut rt = Builder::sandboxed()
        .limits(Limits {
            timeout: Some(Duration::from_secs_f32(config.timeout)),
            max_output_bytes: Some(MAX_OUTPUT_BYTES),
            ..Limits::sandbox()
        })
        .allow(Capability::Net, config.net)
        .output(out.clone())
        .module("bot", runtime::build(state))
        .provider(DogbotProvider::new(pool, chat_id, user_id))
        .global("message", Value::str(&ctx.message))
        .global("sender", Value::str(&ctx.sender))
        .global("reply", Value::str(&ctx.reply))
        .global("reply_from", Value::str(&ctx.reply_from))
        .global("message_id", id(ctx.message_id))
        .global("reply_id", id(ctx.reply_id))
        .build();

    // `print`/`println` go to the buffer; the script's folded value is its
    // return. The CLI shows both, so the bot does too.
    match rt.run("main", source) {
        Ok(value) => Ok(format!("{}{value}", out.take())),
        Err(err) => Err(format!("{}{err}", out.take())),
    }
}

#[cfg(test)]
mod tests;
