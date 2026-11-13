//! Delivery of the messages a script sends *while it runs*, as opposed to the
//! reply built from its output once it finishes.
//!
//! The script thread is blocking and the Telegram client is async, so a
//! `Courier` bridges the two. It is a trait so the native functions can be
//! tested against a fake instead of the network.

use crate::prelude::*;
use log::*;
use std::time::Duration;
use teloxide::net::Download;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::*;
use teloxide::types::{FileId, MessageId, ReplyParameters};

/// How a message is sent and what the history says of it.
#[derive(Clone, Debug, Default)]
pub struct Outgoing {
    /// Sent as a reply to this message.
    pub reply_to: Option<i32>,
    /// What the script called it (`bot.tag`).
    pub tag: Option<String>,
    /// The message whose run sent it.
    pub trigger: Option<i64>,
}

pub trait Courier: Send + Sync {
    /// Sends a new message and returns its id. `within` is how long the script
    /// can still afford to wait.
    fn send(&self, text: Formatted, how: &Outgoing, within: Option<Duration>) -> Result<i32, String>;
    /// Replaces the text of a message this run (or an earlier one) sent.
    fn edit(&self, id: i32, text: Formatted, within: Option<Duration>) -> Result<(), String>;
    /// Deletes a message the bot sent. Refuses anyone else's.
    fn delete(&self, id: i32, within: Option<Duration>) -> Result<(), String>;
    /// Downloads a file Telegram holds, refusing one over `max_bytes`.
    fn fetch(&self, _file_id: &str, _max_bytes: u64, _within: Option<Duration>) -> Result<Vec<u8>, String> {
        Err("files can't be downloaded from here".to_string())
    }
}

/// The real thing: sends through the bot and records what it sent.
pub struct TelegramCourier {
    pub bot: Bot,
    pub pool: PgPool,
    pub chat_id: ChatId,
    /// The runtime that owns `bot`; the script thread is not part of it.
    pub handle: tokio::runtime::Handle,
}

/// Stops one slow request from outliving the script's own deadline, without
/// letting a missing deadline mean "wait forever".
const DEFAULT_WAIT: Duration = Duration::from_secs(30);

impl TelegramCourier {
    fn wait<T>(
        &self,
        within: Option<Duration>,
        request: impl Future<Output = Result<T, teloxide::RequestError>>,
    ) -> Result<T, String> {
        let limit = within.unwrap_or(DEFAULT_WAIT).min(DEFAULT_WAIT);
        match self
            .handle
            .block_on(tokio::time::timeout(limit, request))
        {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(err.to_string()),
            Err(_) => Err("telegram did not answer in time".to_string()),
        }
    }

    /// History is a convenience; failing to record must not undo a send.
    fn record(&self, run: impl FnOnce(&mut PgConnection) -> diesel::QueryResult<()>) {
        match self.pool.get() {
            Ok(mut cn) => {
                if let Err(err) = run(&mut cn) {
                    warn!("could not record a sent message: {err}");
                }
            }
            Err(err) => warn!("could not record a sent message: {err}"),
        }
    }
}

impl Courier for TelegramCourier {
    fn send(&self, text: Formatted, how: &Outgoing, within: Option<Duration>) -> Result<i32, String> {
        let mut request = self.bot.send_formatted(self.chat_id, text);
        if let Some(reply_to) = how.reply_to {
            // If that message is gone, send anyway rather than fail.
            request = request.reply_parameters(
                ReplyParameters::new(MessageId(reply_to)).allow_sending_without_reply(),
            );
        }
        let sent = self.wait(within, request.into_future())?;
        let record = super::record::describe(&sent)
            .unwrap_or_else(|| super::record::Record::text(sent.text().unwrap_or_default()));
        // Bot messages are recorded with a null user, like the final reply.
        let origin = model::Origin {
            trigger: how.trigger.map(|t| MessageId(t as i32)),
            tag: how.tag.clone(),
            ..Default::default()
        };
        self.record(|cn| model::Message::insert(cn, sent.id, self.chat_id, &record, &origin));
        Ok(sent.id.0)
    }

    fn fetch(&self, file_id: &str, max_bytes: u64, within: Option<Duration>) -> Result<Vec<u8>, String> {
        let file = self.wait(within, self.bot.get_file(FileId(file_id.to_string())).into_future())?;
        // Telegram leaves the size out now and then (teloxide reads u32::MAX);
        // the download is checked again below either way.
        if file.meta.size != u32::MAX && file.meta.size as u64 > max_bytes {
            return Err(too_big(file.meta.size as u64, max_bytes));
        }
        let mut bytes = Vec::new();
        let limit = within.unwrap_or(DEFAULT_WAIT).min(DEFAULT_WAIT);
        let download = self.bot.download_file(&file.path, &mut bytes);
        match self.handle.block_on(tokio::time::timeout(limit, download)) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err.to_string()),
            Err(_) => return Err("telegram did not answer in time".to_string()),
        }
        if bytes.len() as u64 > max_bytes {
            return Err(too_big(bytes.len() as u64, max_bytes));
        }
        Ok(bytes)
    }

    fn edit(&self, id: i32, text: Formatted, within: Option<Duration>) -> Result<(), String> {
        let edited = match self.wait(
            within,
            self.bot
                .edit_formatted(self.chat_id, MessageId(id), text)
                .into_future(),
        ) {
            Ok(edited) => edited,
            // A progress loop that repeats itself is not an error worth stopping for.
            Err(err) if err.contains("message is not modified") => return Ok(()),
            Err(err) => return Err(err),
        };
        let content = edited.text().unwrap_or_default().to_owned();
        self.record(|cn| model::Message::update_content(cn, MessageId(id), self.chat_id, &content));
        Ok(())
    }

    fn delete(&self, id: i32, within: Option<Duration>) -> Result<(), String> {
        let message = MessageId(id);
        // Only what the bot recorded as its own: being a group admin must not
        // let a script remove people's messages.
        let ours = self
            .pool
            .get()
            .map_err(|err| err.to_string())
            .and_then(|mut cn| {
                model::Message::sent_by_bot(&mut cn, message, self.chat_id)
                    .map_err(|err| err.to_string())
            })?;
        if !ours {
            return Err("that is not a message the bot sent in this chat".to_string());
        }
        match self.wait(within, self.bot.delete_message(self.chat_id, message).into_future()) {
            Ok(_) => {}
            // Already gone (deleted by hand): the goal is met either way.
            Err(err) if err.contains("message to delete not found") => {}
            Err(err) => return Err(err),
        }
        self.record(|cn| model::Message::remove(cn, message, self.chat_id));
        Ok(())
    }
}

/// Why a file wasn't downloaded.
pub fn too_big(size: u64, max: u64) -> String {
    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    format!(
        "the file has {:.1} MiB, over the limit of {:.1} MiB (download_limit in config/bot.json)",
        mib(size),
        mib(max)
    )
}
