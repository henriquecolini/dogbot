use crate::prelude::*;
use clap::Parser;
use teloxide::net::Download;
use teloxide::prelude::*;

#[derive(Clone, Debug, PartialEq, Parser)]
pub struct UploadCommand {
    path: String,
}

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        chat_id,
        user_id,
        connected_chat_id,
        document,
        ..
    }: Context,
    UploadCommand { path }: UploadCommand,
) -> BotResult<()> {
    let Some(document) = document else {
        bot.send_code(chat_id, "upload: missing document").await?;
        return Ok(());
    };
    let file = bot.get_file(document.file.id).await?;
    let mut bytes = Vec::new();
    bot.download_file(&file.path, &mut bytes)
        .await
        .map_err(anyhow::Error::from)?;
    let mut cn = pool.get()?;
    match crate::files::write(
        &mut cn,
        connected_chat_id,
        user_id,
        path.trim(),
        bytes.as_slice(),
    ) {
        Ok(_) => {}
        Err(err) => {
            bot.send_code(chat_id, format!("write: {}", err)).await?;
        }
    }
    Ok(())
}
