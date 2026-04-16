use crate::files::FileError;
use crate::result::*;
use crate::{PgPool, model::*, timeout};
use diesel::{PgConnection, QueryResult};
use dog3::parser::format_string::FormatString;
use dog3::parser::grammar::{ActualParameter, CommandStatement, Execution, OpenStatement, Value};
use dog3::parser::parse;
use dog3::runtime::functions::FunctionLibrary;
use dog3::runtime::output::Output;
use dog3::runtime::scope::ScopeStack;
use dog3::runtime::{ExecutionError, Runtime};
use dog3::{builtin, builtin_alias};
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

fn register_libraries(pool: PgPool, chat_id: i64, user_id: i64, mut runtime: Runtime) -> Runtime {
    let (pool1, pool2, pool3) = (pool.clone(), pool.clone(), pool);
    let read = move |_: &FunctionLibrary,
                     _: &mut ScopeStack,
                     args: &[Output]|
          -> Result<Output, ExecutionError> {
        let path = match args {
            [path] => path,
            _ => return Err(ExecutionError::InternalError),
        };
        let mut cn = match pool1.get() {
            Ok(cn) => cn,
            Err(err) => {
                error!("Error running `read` builtin: {err}");
                return Err(ExecutionError::InternalError);
            }
        };
        match crate::files::read(&mut cn, chat_id, user_id, path.value()) {
            Ok(file) => {
                let string = String::from_utf8_lossy(&file).into_owned();
                Ok(Output::new_truthy_with(string.into()))
            }
            Err(FileError::QueryError(err)) => {
                error!("Error running `read` builtin: {err}");
                Err(ExecutionError::InternalError)
            }
            Err(err) => Ok(Output::new_falsy_with(err.to_string().into())),
        }
    };
    let write = move |_: &FunctionLibrary,
                      _: &mut ScopeStack,
                      args: &[Output]|
          -> Result<Output, ExecutionError> {
        let (path, content) = match args {
            [path, content] => (path, content),
            _ => return Err(ExecutionError::InternalError),
        };
        let mut cn = match pool2.get() {
            Ok(cn) => cn,
            Err(err) => {
                error!("Error running `write` builtin: {err}");
                return Err(ExecutionError::InternalError);
            }
        };
        match crate::files::write(
            &mut cn,
            chat_id,
            user_id,
            path.value(),
            content.value().as_bytes(),
        ) {
            Ok(_) => Ok(Output::new_truthy()),
            Err(FileError::QueryError(err)) => {
                error!("Error running `write` builtin: {err}");
                Err(ExecutionError::InternalError)
            }
            Err(err) => Ok(Output::new_falsy_with(err.to_string().into())),
        }
    };
    let history_gen = || {
        let pool = pool3.clone();
        move |_: &FunctionLibrary,
              _: &mut ScopeStack,
              args: &[Output]|
              -> Result<Output, ExecutionError> {
            let (count, offset, with_content, with_users, with_bot) = match args {
                [] => (Ok(10), Ok(0), true, true, false),
                [count] => (count.try_into(), Ok(0), true, true, false),
                [count, offset] => (count.try_into(), offset.try_into(), true, true, false),
                [count, offset, with_content] => (
                    count.try_into(),
                    offset.try_into(),
                    with_content.is_truthy(),
                    true,
                    false,
                ),
                [count, offset, with_content, with_users] => (
                    count.try_into(),
                    offset.try_into(),
                    with_content.is_truthy(),
                    with_users.is_truthy(),
                    false,
                ),
                [count, offset, with_content, with_users, with_bot] => (
                    count.try_into(),
                    offset.try_into(),
                    with_content.is_truthy(),
                    with_users.is_truthy(),
                    with_bot.is_truthy(),
                ),
                _ => return Err(ExecutionError::InternalError),
            };
            let (count, offset) = match (count, offset) {
                (Ok(count), Ok(offset)) => (count, offset),
                _ => return Ok(Output::new_falsy()),
            };
            let mut cn = match pool.get() {
                Ok(cn) => cn,
                Err(err) => {
                    error!("Error running `write` builtin: {err}");
                    return Err(ExecutionError::InternalError);
                }
            };
            match message::Message::list(&mut cn, chat_id, count, offset) {
                Ok(messages) => {
                    let mut output = String::new();
                    for (message, user) in messages {
                        if !with_bot && user.is_none() {
                            continue;
                        }
                        if with_users {
                            if let Some(user) = user {
                                output += &user.first_name;
                                if let Some(last_name) = user.last_name {
                                    output += " ";
                                    output += &last_name;
                                }
                            } else {
                                output += "<bot>"
                            }
                        }
                        if with_users && with_content {
                            output += ": ";
                        }
                        if with_content {
                            output += &message.content.replace("\n", " ");
                        }
                        output += "\n";
                    }
                    Ok(Output::new_truthy_with(output.into()))
                }
                Err(err) => {
                    error!("Error running `history` builtin: {err}");
                    Err(ExecutionError::InternalError)
                }
            }
        }
    };
    let _ = runtime.library.merge(builtin::std::build());
    let _ = runtime.library.merge(builtin::str::build());
    let _ = runtime.library.merge(builtin::net::build());
    let _ = runtime.library.merge(builtin::iter::build());
    let _ = runtime.library.merge(builtin::math::build());
    let _ = runtime.library.merge(builtin::logic::build());
    let _ = runtime.library.merge(builtin::json::build());
    builtin!(runtime.library, read, "path");
    builtin!(runtime.library, write, "path", "content");
    builtin_alias!(runtime.library, history_gen(), "history",);
    builtin_alias!(runtime.library, history_gen(), "history", "count");
    builtin_alias!(runtime.library, history_gen(), "history", "count", "offset");
    builtin_alias!(
        runtime.library,
        history_gen(),
        "history",
        "count",
        "offset",
        "with_content"
    );
    builtin_alias!(
        runtime.library,
        history_gen(),
        "history",
        "count",
        "offset",
        "with_content",
        "with_users"
    );
    builtin_alias!(
        runtime.library,
        history_gen(),
        "history",
        "count",
        "offset",
        "with_content",
        "with_users",
        "with_bot"
    );
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
    message::Message::insert(&mut cn, message_id.0 as i64, chat_id, Some(user_id), &message)?;
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
    let text: Formatted =
        match parse(&String::from_utf8_lossy(&file)) {
            Ok(mut program) => {
                let mut runtime = Runtime::new();
                runtime = register_libraries(pool.clone(), chat_id, user_id, runtime);
                match runtime.library.add_scripts(program.functions) {
                    Ok(_) => {
                        program.executions.push(Execution::OpenStatement(
                            OpenStatement::CommandStmt(CommandStatement {
                                name: "main".to_string(),
                                parameters: vec![
                                    ActualParameter {
                                        value: Value::String(FormatString::raw(&message)),
                                    },
                                    ActualParameter {
                                        value: Value::String(FormatString::raw(&user.full_name())),
                                    },
                                    ActualParameter {
                                        value: Value::String(FormatString::raw(
                                            reply_to_text.unwrap_or_default(),
                                        )),
                                    },
                                    ActualParameter {
                                        value: Value::String(FormatString::raw(
                                            reply_to_from.as_deref().unwrap_or_default(),
                                        )),
                                    },
                                ],
                            }),
                        ));

                        let outcome = match timeout::timeout(
                            move || runtime.execute(&program.executions),
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
            Ok(msg) => {
                info!("Sent message successfully");
                message::Message::insert(
                    &mut cn,
                    msg.id.0 as i64,
                    chat_id,
                    None,
                    msg.text().unwrap_or_default(),
                )?
            }
            Err(err) => {
                error!("Failed to send message: {err}");
                let _ = bot.send_formatted(chat.id, pre(err.to_string())).await;
            }
        };
    }
    Ok(())
}
