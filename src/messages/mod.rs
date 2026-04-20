mod runtime;

use crate::prelude::*;
use dog3::parser::grammar::Execution;
use dog3::parser::parse;
use dog3::runtime::Runtime;
use log::*;
use std::time::Duration;
use teloxide::prelude::*;
use tokio::time::{interval, timeout};

fn command_execution(command: &str, args: &[&str]) -> Execution {
    use dog3::parser::format_string::FormatString;
    use dog3::parser::grammar::{
        ActualParameter, CommandStatement, Execution, OpenStatement, Value,
    };

    let mut parameters = vec![];
    for arg in args {
        parameters.push(ActualParameter {
            value: Value::String(FormatString::raw(arg)),
        })
    }
    Execution::OpenStatement(OpenStatement::CommandStmt(CommandStatement {
        name: command.to_string(),
        parameters,
    }))
}

pub async fn handle(
    bot: Bot,
    pool: PgPool,
    Context {
        connected_chat_id,
        chat_id,
        user_id,
        message_id,
        message_content,
        chat_is_private,
        chat_name,
        user_first_name,
        user_last_name,
        user_username,
        reply_to_content,
        reply_to_from,
        ..
    }: Context,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    model::Chat::upsert(&mut cn, chat_id, Some(&chat_name), !chat_is_private)?;
    model::User::upsert(
        &mut cn,
        user_id,
        &user_first_name,
        user_last_name.as_deref(),
        user_username.as_deref(),
    )?;
    model::UserInChat::upsert(&mut cn, user_id, chat_id)?;
    model::Message::insert(
        &mut cn,
        message_id,
        chat_id,
        Some(user_id),
        &message_content,
    )?;
    info!("Message received, {message_content} from {user_first_name}");
    let file = match crate::files::read(&mut cn, connected_chat_id, user_id, "main.dog") {
        Ok(file) => file,
        Err(_) => return Ok(()),
    };
    let text: Formatted = match parse(&String::from_utf8_lossy(&file)) {
        Ok(mut program) => {
            let mut runtime = Runtime::new();
            runtime =
                runtime::register_libraries(pool.clone(), connected_chat_id, user_id, runtime);
            match runtime.library.add_scripts(program.functions) {
                Ok(_) => {
                    program.executions.push(command_execution(
                        "main",
                        &[
                            &message_content,
                            &if let Some(user_last_name) = user_last_name {
                                format!("{} {}", user_first_name, user_last_name)
                            } else {
                                user_first_name.to_string()
                            },
                            reply_to_content.as_deref().unwrap_or_default(),
                            reply_to_from.as_deref().unwrap_or_default(),
                        ],
                    ));

                    let task = runtime.execute(&program.executions);

                    let bot = bot.clone();
                    let typer = tokio::task::spawn(async move {
                        let mut inter = interval(Duration::from_secs(1));
                        loop {
                            inter.tick().await;
                            bot.send_chat_action(chat_id, ChatAction::Typing).await.ok();
                        }
                    });

                    let result = timeout(Duration::from_secs(90), task).await;
                    typer.abort();

                    match result {
                        Ok(out) => match out {
                            Ok(out) => raw(out.value()),
                            Err(err) => code(err.to_string()),
                        },
                        Err(_) => code("The bot took too long to execute the code!"),
                    }
                }
                Err(err) => code(err.to_string()),
            }
        }
        Err(err) => code(err.to_string()),
    };
    if !text.is_empty() {
        match bot.send_formatted(chat_id, text).await {
            Ok(msg) => {
                info!("Sent message successfully");
                model::Message::insert(
                    &mut cn,
                    msg.id,
                    chat_id,
                    None,
                    msg.text().unwrap_or_default(),
                )?
            }
            Err(err) => {
                error!("Failed to send message: {err}");
                let _ = bot.send_formatted(chat_id, code(err.to_string())).await;
            }
        };
    }
    Ok(())
}
