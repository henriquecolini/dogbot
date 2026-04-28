use crate::prelude::*;
use clap::Parser;
use teloxide::prelude::*;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct DownloadCommand {
	path: String,
}

pub async fn handle(
	bot: Bot,
	pool: PgPool,
	Context {
		chat_id,
		user_id,
		connected_chat_id,
		..
	}: Context,
	DownloadCommand { path }: DownloadCommand,
) -> BotResult<()> {
	let mut cn = pool.get()?;
	match files::read(&mut cn, connected_chat_id, user_id, &path) {
		Ok((name, content)) => {
			bot.send_document(chat_id, InputFile::memory(content).file_name(name)).await?;
		}
		Err(err) => {
			bot.send_code(chat_id, format!("read: {}", err))
				.await?;
		}
	}
	Ok(())
}
