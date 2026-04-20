use crate::prelude::*;
use teloxide::prelude::*;
use teloxide::utils::command::ParseError;

#[derive(Clone, Debug, PartialEq)]
pub struct WriteCommand {
    path: String,
    content: String,
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
    WriteCommand { path, content }: WriteCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    match crate::files::write(
        &mut cn,
        connected_chat_id,
        user_id,
        path.trim(),
        content.as_bytes(),
    ) {
        Ok(_) => {}
        Err(err) => {
            bot.send_code(chat_id, format!("write: {}", err))
                .await?;
        }
    }
    Ok(())
}

pub fn parse(input: String) -> Result<(WriteCommand,), ParseError> {
    let input = input.as_str();
    let mut shlex = shlex::Shlex::new(input);
    let (Some(path), (iter, _, false)) = (shlex.next(), shlex.into_inner()) else {
        return Err(ParseError::IncorrectFormat(
            anyhow::anyhow!("Incorrect args format").into_boxed_dyn_error(),
        ));
    };
    let rest = iter.copied().collect::<Vec<u8>>();
    let rest = match String::from_utf8(rest) {
        Ok(rest) => rest,
        Err(err) => return Err(ParseError::Custom(err.into())),
    };
    let content = rest.trim().to_string();
    Ok((WriteCommand { path, content },))
}
