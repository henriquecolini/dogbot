use crate::prelude::*;
use clap::Parser;
use std::collections::HashMap;
use teloxide::prelude::*;
use time::macros::format_description;

#[derive(Parser, Clone, Debug, PartialEq)]
pub struct LsCommand {
    #[clap(short, long)]
    long: bool,
    path: Option<String>,
}

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        user_id,
        chat_id,
        connected_chat_id,
        ..
    }: Context,
    LsCommand { long, path }: LsCommand,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    match files::list(
        &mut cn,
        connected_chat_id,
        user_id,
        path.as_deref().unwrap_or_default(),
    ) {
        Ok(mut files) => {
            if files.is_empty() {
                return Ok(());
            }
            files.sort_by(|a, b| b.is_dir().cmp(&a.is_dir()).then(a.name.cmp(&b.name)));
            if long {
                let mut user_names = HashMap::new();
                let mut longest_length = 0;
                for file in files.iter_mut() {
                    let Some(owner_id) = file.owner_id else {
                        continue;
                    };
                    let user = model::User::try_get(&mut cn, UserId(owner_id as u64))?;
                    let Some(user) = user else {
                        user_names.insert(owner_id, "".to_owned());
                        continue;
                    };
                    let user_name = user.username.unwrap_or(user.first_name);
                    longest_length = longest_length.max(user_name.len());
                    user_names.insert(owner_id, user_name);
                }
                let date_format =
                    format_description!("[month repr:short] [day] [year] [hour]:[minute]");
                bot.send_html(
                    chat_id,
                    files
                        .iter()
                        .map(|f| {
                            format!(
                                "<code>{}{}{}{}{}{}{}{}{}{} {:<longest_length$} {}</code> {}",
                                if f.is_dir() { 'd' } else { '-' },
                                if f.user_read { 'r' } else { '-' },
                                if f.user_write { 'w' } else { '-' },
                                if f.user_execute { 'x' } else { '-' },
                                if f.group_read { 'r' } else { '-' },
                                if f.group_write { 'w' } else { '-' },
                                if f.group_execute { 'x' } else { '-' },
                                if f.others_read { 'r' } else { '-' },
                                if f.others_write { 'w' } else { '-' },
                                if f.others_execute { 'x' } else { '-' },
                                f.owner_id
                                    .map(|id| user_names[&id].to_owned())
                                    .unwrap_or_default(),
                                f.last_modified_at.format(&date_format).unwrap_or_default(),
                                if f.is_dir() {
                                    format!("<strong>{}</strong>", f.name)
                                } else {
                                    format!("<code>{}</code>", f.name)
                                },
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
                .await?;
            } else {
                bot.send_html(
                    chat_id,
                    files
                        .iter()
                        .map(|f| {
                            if f.is_dir() {
                                format!("<strong>{}</strong>", escape_html(&f.name))
                            } else {
                                format!("<code>{}</code>", escape_html(&f.name))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                )
                .await?;
            }
        }
        Err(err) => {
            bot.send_code(chat_id, format!("ls: {}", err)).await?;
        }
    }
    Ok(())
}
