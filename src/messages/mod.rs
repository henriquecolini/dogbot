mod provider;
mod runtime;

use crate::prelude::*;
use dog4::{Builder, Capability, Limits, OutputBuffer, Value};
use log::*;
use provider::DogbotProvider;
use serde::Deserialize;
use std::time::Duration;
use teloxide::prelude::*;
use tokio::time::interval;

/// Per-chat settings, read from `config/bot.json`.
#[derive(Deserialize)]
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
    let result = tokio::task::spawn_blocking(move || {
        run_script(script_pool, connected_chat_id, user_id, &source, &config, &ctx)
    })
    .await;

    typer.abort();

    let text: Formatted = match result {
        Ok(Ok(output)) => raw(output),
        Ok(Err(err)) => code(err),
        Err(err) => {
            error!("Script task failed: {err}");
            code("The bot failed to run the script.")
        }
    };

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

/// The four context values a script sees as globals.
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
/// Synchronous on purpose: dog4's `Runtime` is not `Send`, and Diesel blocks,
/// so this must run on a blocking thread.
fn run_script(
    pool: PgPool,
    chat_id: ChatId,
    user_id: UserId,
    source: &str,
    config: &Config,
    ctx: &ScriptContext,
) -> Result<String, String> {
    let out = OutputBuffer::default();
    let state = runtime::State {
        pool: pool.clone(),
        chat_id,
        user_id,
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
