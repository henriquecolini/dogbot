use crate::prelude::*;
use teloxide::payloads::{SendMessage, SendMessageSetters};
use teloxide::requests::JsonRequest;

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
}

impl Formatted {
    pub fn is_empty(&self) -> bool {
        match self {
            Formatted::Raw(text) => text.is_empty(),
            Formatted::Code(text) => text.is_empty(),
            Formatted::Html(text) => text.is_empty(),
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
        match text {
            Formatted::Raw(text) => self.send_message(chat_id, truncate(text, MAX_RAW_CHARS)),
            Formatted::Code(text) => self
                .send_message(
                    chat_id,
                    format!("<code>{}</code>", escape_html(truncate(text, MAX_CODE_CHARS))),
                )
                .parse_mode(ParseMode::Html),
            // Already-built HTML is left alone: cutting it could split a tag.
            Formatted::Html(text) => self.send_message(chat_id, text).parse_mode(ParseMode::Html),
        }
    }
}

pub fn raw(input: impl AsRef<str>) -> Formatted {
    Formatted::Raw(input.as_ref().to_owned())
}

pub fn code(input: impl AsRef<str>) -> Formatted {
    Formatted::Code(input.as_ref().to_owned())
}

pub fn html(input: impl AsRef<str>) -> Formatted {
    Formatted::Html(input.as_ref().to_owned())
}

pub fn escape_html(input: impl AsRef<str>) -> String {
    input
        .as_ref()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
