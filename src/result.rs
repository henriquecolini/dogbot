use crate::files::FileError;
use crate::formatted::SendFormatted;
use log::error;
use teloxide::prelude::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BotError {
	#[error("{0}")]
	TelegramError(#[from] teloxide::RequestError),
	#[error("{0}")]
	QueryError(#[from] diesel::result::Error),
	#[error("{0}")]
	R2D2Error(#[from] r2d2::Error),
	#[error("{0}")]
	Other(#[from] anyhow::Error),
	/// Something the user did wrong, worded for them. It is shown as is; every
	/// other variant is internal and only gets logged.
	#[error("{0}")]
	User(String),
}

impl BotError {
	pub fn user(message: impl Into<String>) -> Self {
		BotError::User(message.into())
	}
}

impl From<FileError> for BotError {
	fn from(err: FileError) -> Self {
		match err {
			FileError::QueryError(err) => BotError::QueryError(err),
			err => BotError::User(err.to_string()),
		}
	}
}

pub type BotResult<T> = Result<T, BotError>;

/// Tells the chat that command `cmd` failed. This is the one place a failure
/// reaches the user: their own mistakes are shown as `cmd: message`, while
/// anything internal is logged and replaced by a generic message so no
/// database or network details leak into the chat.
pub async fn report_error(bot: &Bot, chat_id: ChatId, cmd: &str, err: &BotError) {
	let message = match err {
		BotError::User(message) => message.clone(),
		err => {
			error!("{cmd}: {err}");
			"internal error".to_owned()
		}
	};
	if let Err(err) = bot.send_error(chat_id, Some(cmd), message).await {
		error!("Failed to send error message: {err}");
	}
}
