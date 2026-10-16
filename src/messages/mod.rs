mod provider;
mod runtime;

use crate::prelude::*;
use dog4::{Builder, Capability, Limits, OutputBuffer, Value};
use log::*;
use provider::DogbotProvider;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use teloxide::prelude::*;
use tokio::time::interval;

/// Per-chat settings, read from `config/bot.json`.
#[derive(Deserialize, Clone)]
struct Config {
    #[serde(default = "default_timeout")]
    timeout: f32,
    /// Whether scripts may make outbound HTTP requests.
    #[serde(default = "default_net")]
    net: bool,
}

fn default_timeout() -> f32 {
    90.0
}

fn default_net() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            timeout: default_timeout(),
            net: default_net(),
        }
    }
}

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
        user_id,
        message_content,
        user_full_name,
        reply_to_content,
        reply_to_from,
        ..
    }: Context,
) -> BotResult<()> {
    let mut cn = pool.get()?;

    let config: Config = files::read(&mut cn, connected_chat_id, user_id, "config/bot.json")
        .ok()
        .and_then(|(_, v)| serde_json::from_slice(&v).ok())
        .unwrap_or_default();

    // A chat with no executable main.dog is simply inert.
    let Ok((_, source)) =
        files::read_for_execution(&mut cn, connected_chat_id, user_id, "main.dog")
    else {
        return Ok(());
    };
    let source = String::from_utf8_lossy(&source).into_owned();
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

    // dog4's Runtime is synchronous and not Send, so the whole run happens on a
    // blocking thread. Diesel is blocking too, which makes this the correct
    // place for the natives to reach the database.
    let script_pool = pool.clone();
    let ctx = ScriptContext {
        message: message_content,
        sender: user_full_name,
        reply: reply_to_content.unwrap_or_default(),
        reply_from: reply_to_from.unwrap_or_default(),
    };
    let (script_config, script_ctx) = (config.clone(), ctx.clone());
    let result = tokio::task::spawn_blocking(move || {
        let later = runtime::Later::default();
        let output = run_script(
            script_pool,
            connected_chat_id,
            user_id,
            &source,
            &script_config,
            &script_ctx,
            Some(later.clone()),
        );
        (output, later.take())
    })
    .await;

    typer.abort();

    let (result, later) = match result {
        Ok((output, later)) => (Ok(output), later),
        Err(err) => (Err(err), Vec::new()),
    };
    let text: Formatted = match result {
        Ok(Ok(output)) => raw(output),
        Ok(Err(err)) => code(err),
        Err(err) => {
            error!("Script task failed: {err}");
            code("The bot failed to run the script.")
        }
    };

    let sent = send_reply(&bot, &pool, chat_id, text).await;
    // Only now, with the reply out and recorded, so the scripts see it in
    // `history`. Queued even when the script failed after calling `later`: the
    // upkeep it asked for is usually independent of whatever went wrong.
    run_later(pool, connected_chat_id, user_id, later, config, ctx);
    sent
}

/// Sends a script's output, if there is any, and records it in the history.
async fn send_reply(bot: &Bot, pool: &PgPool, chat_id: ChatId, text: Formatted) -> BotResult<()> {
    if text.is_empty() {
        return Ok(());
    }

    match bot.send_formatted(chat_id, text).await {
        Ok(msg) => {
            info!("Sent message successfully");
            // Bot messages are recorded with a null user, so `history` can show them.
            let mut cn = pool.get()?;
            model::Message::insert(
                &mut cn,
                msg.id,
                chat_id,
                None,
                msg.text().unwrap_or_default(),
            )?;
        }
        Err(err) => {
            error!("Failed to send message: {err}");
            let _ = bot.send_formatted(chat_id, code(err.to_string())).await;
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
fn run_later(
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
                match run_script(pool.clone(), chat_id, user_id, &source, &config, &ctx, None) {
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

/// The four context values a script sees as globals.
#[derive(Clone)]
pub struct ScriptContext {
    pub message: String,
    pub sender: String,
    pub reply: String,
    pub reply_from: String,
}

/// Compiles and runs one script, returning what to reply with.
///
/// `Ok` is a normal result, `Err` an error report; both may carry buffered
/// output, because a script can print before it fails.
///
/// `later` collects the scripts the run queues with `bot.later`; `None` makes
/// that function raise, which is what a deferred run itself gets.
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
    later: Option<runtime::Later>,
) -> Result<String, String> {
    let out = OutputBuffer::default();
    let state = runtime::State {
        pool: pool.clone(),
        chat_id,
        user_id,
        later,
    };

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
