use crate::result::*;
use crate::{PgPool, model::*, timeout};
use diesel::{PgConnection, QueryResult};
use dog3::builtin;
use dog3::parser::format_string::FormatString;
use dog3::parser::grammar::{ActualParameter, CommandStatement, Execution, OpenStatement, Value};
use dog3::parser::parse;
use dog3::runtime::Runtime;
use dog3::runtime::functions::RegisterError;
use log::*;
use std::time::Duration;
use teloxide::payloads::SendMessage;
use teloxide::prelude::*;
use teloxide::requests::JsonRequest;
use teloxide::types::*;

enum Formatted {
    Raw(String),
    Html(String),
}

trait SendFormatted {
    fn send_formatted<C: Into<Recipient>>(
        &self,
        chat_id: C,
        text: Formatted,
    ) -> JsonRequest<SendMessage>;
}

impl SendFormatted for Bot {
    fn send_formatted<C: Into<Recipient>>(
        &self,
        chat_id: C,
        text: Formatted,
    ) -> JsonRequest<SendMessage> {
        match text {
            Formatted::Raw(text) => self.send_message(chat_id, text),
            Formatted::Html(text) => self.send_message(chat_id, text).parse_mode(ParseMode::Html),
        }
    }
}

impl Formatted {
    fn is_empty(&self) -> bool {
        match self {
            Formatted::Raw(text) => text.is_empty(),
            Formatted::Html(text) => text.is_empty(),
        }
    }
}

fn raw(input: impl AsRef<str>) -> Formatted {
    Formatted::Raw(input.as_ref().to_owned())
}

fn pre(input: impl AsRef<str>) -> Formatted {
    Formatted::Html(format!("<pre>{}</pre>", input.as_ref()))
}

fn register_libraries(mut runtime: Runtime) -> Runtime {
    let _ = runtime.library.merge(builtin::std::build());
    let _ = runtime.library.merge(builtin::str::build());
    let _ = runtime.library.merge(builtin::net::build());
    let _ = runtime.library.merge(builtin::iter::build());
    let _ = runtime.library.merge(builtin::math::build());
    let _ = runtime.library.merge(builtin::logic::build());
    let _ = runtime.library.merge(builtin::json::build());
    runtime
}

fn get_connected_chat(cn: &mut PgConnection, user_id: i64, chat_id: i64) -> QueryResult<i64> {
    let user = user::User::get(cn, user_id)?;
    Ok(user.current_connection.unwrap_or(chat_id))
}

pub async fn handle_message(
    bot: Bot,
    pool: PgPool,
    chat: Chat,
    user: User,
    message_id: MessageId,
    message: String,
    reply_to: Option<Message>,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    let (chat_name, is_group) = match chat.kind {
        ChatKind::Public(chat) => (chat.title, true),
        ChatKind::Private(chat) => (chat.first_name, false),
    };
    let chat_id = chat.id.0;
    let user_id = user.id.0 as i64;
    chat::Chat::upsert(&mut cn, chat_id, chat_name.as_deref(), is_group)?;
    user::User::upsert(
        &mut cn,
        user_id,
        &user.first_name,
        user.last_name.as_deref(),
        user.username.as_deref(),
    )?;
    user_in_chat::UserInChat::upsert(&mut cn, user_id, chat_id)?;
    message::Message::insert(&mut cn, message_id.0 as i64, chat_id, user_id, &message)?;
    let (reply_to_text, reply_to_from) = if let Some(reply_to) = &reply_to {
        (
            reply_to.text(),
            reply_to.from.as_ref().map(|u| u.full_name()),
        )
    } else {
        (None, None)
    };
    let chat_id = get_connected_chat(&mut cn, user_id, chat_id)?;
    let file = match crate::files::read(&mut cn, chat_id, user_id, "main.dog") {
        Ok(file) => file,
        Err(_) => return Ok(()),
    };
    let text: Formatted = match parse(&String::from_utf8_lossy(&file)) {
        Ok(mut program) => {
            let mut runtime = Runtime::new();
            runtime = register_libraries(runtime);
            match runtime.library.add_scripts(program.functions) {
                Ok(_) => {
                    let executions = vec![Execution::OpenStatement(OpenStatement::CommandStmt(
                        CommandStatement {
                            name: "main".to_string(),
                            parameters: vec![
                                ActualParameter {
                                    value: Value::String(FormatString::raw(&message)),
                                },
                                ActualParameter {
                                    value: Value::String(FormatString::raw(&user.full_name())),
                                },
                            ],
                        },
                    ))];

                    let outcome = match timeout::timeout(
                        move || runtime.execute(&executions),
                        Duration::from_secs(90),
                    ) {
                        Ok(res) => match res {
                            Ok(out) => raw(out.value()),
                            Err(err) => pre(err.to_string()),
                        },
                        Err(err) => match err {
                            timeout::TimeoutError::Panic => {
                                pre("The bot panicked while executing the code!")
                            }
                            timeout::TimeoutError::Timeout => {
                                pre("The bot took too long to execute the code!")
                            }
                        },
                    };
                    outcome
                }
                Err(err) => pre(err.to_string()),
            }
        }
        Err(err) => pre(err.to_string()),
    };
    if !text.is_empty() {
        match bot.send_formatted(chat.id, text).await {
            Ok(_) => {
                info!("Sent message successfully")
            }
            Err(err) => {
                error!("Failed to send message: {err}");
                let _ = bot.send_formatted(chat.id, pre(err.to_string())).await;
            }
        };
    }
    Ok(())
}
