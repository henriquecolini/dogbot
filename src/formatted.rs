use crate::prelude::*;
use teloxide::payloads::{SendMessage, SendMessageSetters};
use teloxide::requests::JsonRequest;

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
            Formatted::Raw(text) => self.send_message(chat_id, text),
            Formatted::Code(text) => self
                .send_message(chat_id, format!("<code>{}</code>", escape_html(text)))
                .parse_mode(ParseMode::Html),
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
