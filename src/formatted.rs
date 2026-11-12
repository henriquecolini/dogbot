use crate::prelude::*;
use std::fmt::Display;
use teloxide::payloads::{EditMessageText, EditMessageTextSetters, SendMessage, SendMessageSetters};
use teloxide::requests::JsonRequest;
use teloxide::types::{MessageId, ParseMode};

/// Telegram rejects messages longer than 4096 UTF-16 units. Text is cut before
/// it is escaped or wrapped, so an entity can never be split in half; the
/// limits leave room for that expansion.
const MAX_RAW_CHARS: usize = 3900;
const MAX_CODE_CHARS: usize = 3000;
const TRUNCATION_MARK: &str = "\n… (truncated)";

/// Shortens `text` to `max` characters, marking it when something was cut.
fn truncate(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str(TRUNCATION_MARK);
    out
}

pub enum Formatted {
    Raw(String),
    Code(String),
    Html(String),
    /// Telegram's MarkdownV2, which is strict: a stray `.` or `-` outside an
    /// entity must be escaped with `\`, and Telegram refuses the message if not.
    Markdown(String),
}

impl Formatted {
    pub fn is_empty(&self) -> bool {
        match self {
            Formatted::Raw(text) => text.is_empty(),
            Formatted::Code(text) => text.is_empty(),
            Formatted::Html(text) => text.is_empty(),
            Formatted::Markdown(text) => text.is_empty(),
        }
    }

    /// The text to put on the wire and how Telegram should parse it.
    fn render(self) -> (String, Option<ParseMode>) {
        match self {
            Formatted::Raw(text) => (truncate(text, MAX_RAW_CHARS), None),
            Formatted::Code(text) => (
                format!("<code>{}</code>", escape_html(truncate(text, MAX_CODE_CHARS))),
                Some(ParseMode::Html),
            ),
            // Already-built markup is left alone: cutting it could split a tag.
            Formatted::Html(text) => (text, Some(ParseMode::Html)),
            Formatted::Markdown(text) => (text, Some(ParseMode::MarkdownV2)),
        }
    }
}

pub trait SendFormatted {
    fn send_formatted<C: Into<Recipient>>(
        &self,
        chat_id: C,
        text: Formatted,
    ) -> JsonRequest<SendMessage>;
    fn send_raw<C: Into<Recipient>>(
        &self,
        chat_id: C,
        text: impl AsRef<str>,
    ) -> JsonRequest<SendMessage> {
        self.send_formatted(chat_id, raw(text))
    }
    fn send_code<C: Into<Recipient>>(
        &self,
        chat_id: C,
        text: impl AsRef<str>,
    ) -> JsonRequest<SendMessage> {
        self.send_formatted(chat_id, code(text))
    }
    /// Sends a failure, the same way for every command: `cmd: message`.
    fn send_error<C: Into<Recipient>>(
        &self,
        chat_id: C,
        cmd: Option<&str>,
        message: impl Display,
    ) -> JsonRequest<SendMessage> {
        self.send_formatted(chat_id, error(cmd, message))
    }
    fn send_html<C: Into<Recipient>>(
        &self,
        chat_id: C,
        text: impl AsRef<str>,
    ) -> JsonRequest<SendMessage> {
        self.send_formatted(chat_id, html(text))
    }
}

impl SendFormatted for Bot {
    fn send_formatted<C: Into<Recipient>>(
        &self,
        chat_id: C,
        text: Formatted,
    ) -> JsonRequest<SendMessage> {
        let (text, mode) = text.render();
        let request = self.send_message(chat_id, text);
        match mode {
            Some(mode) => request.parse_mode(mode),
            None => request,
        }
    }
}

pub trait EditFormatted {
    /// Replaces the text of a message the bot sent earlier.
    fn edit_formatted<C: Into<Recipient>>(
        &self,
        chat_id: C,
        message_id: MessageId,
        text: Formatted,
    ) -> JsonRequest<EditMessageText>;
}

impl EditFormatted for Bot {
    fn edit_formatted<C: Into<Recipient>>(
        &self,
        chat_id: C,
        message_id: MessageId,
        text: Formatted,
    ) -> JsonRequest<EditMessageText> {
        let (text, mode) = text.render();
        let request = self.edit_message_text(chat_id, message_id, text);
        match mode {
            Some(mode) => request.parse_mode(mode),
            None => request,
        }
    }
}

pub fn raw(input: impl AsRef<str>) -> Formatted {
    Formatted::Raw(input.as_ref().to_owned())
}

pub fn code(input: impl AsRef<str>) -> Formatted {
    Formatted::Code(input.as_ref().to_owned())
}

/// How a failure looks in the chat: `cmd: message`, or just the message when it
/// does not come from a command.
pub fn error(cmd: Option<&str>, message: impl Display) -> Formatted {
    match cmd {
        Some(cmd) => Formatted::Code(format!("{cmd}: {message}")),
        None => Formatted::Code(message.to_string()),
    }
}

/// A failure, as [`error`] shows one, followed by a line of advice.
pub fn error_with_tip(message: impl Display, tip: impl AsRef<str>) -> Formatted {
    Formatted::Html(format!(
        "<code>{}</code>\n\n<i>{}</i>",
        escape_html(truncate(message.to_string(), MAX_CODE_CHARS)),
        escape_html(tip),
    ))
}

pub fn html(input: impl AsRef<str>) -> Formatted {
    Formatted::Html(input.as_ref().to_owned())
}

pub fn markdown(input: impl AsRef<str>) -> Formatted {
    Formatted::Markdown(input.as_ref().to_owned())
}

pub fn escape_html(input: impl AsRef<str>) -> String {
    input
        .as_ref()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
