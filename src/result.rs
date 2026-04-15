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
}

pub type BotResult<T> = Result<T, BotError>;