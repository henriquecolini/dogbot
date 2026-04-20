use crate::prelude::*;
use dog3::{
    builtin, builtin_state,
    runtime::{
        ExecutionError, Runtime, functions::FunctionLibrary, output::Output, scope::ScopeStack,
    },
};
use log::*;
use teloxide::prelude::*;

#[derive(Clone)]
struct State {
    pool: PgPool,
    chat_id: ChatId,
    user_id: UserId,
}

async fn read(
    State {
        pool,
        chat_id,
        user_id,
    }: State,
    _: &FunctionLibrary,
    _: &mut ScopeStack<'_>,
    args: Vec<Output>,
) -> Result<Output, ExecutionError> {
    let path = match args.as_slice() {
        [path] => path,
        _ => return Err(ExecutionError::InternalError),
    };
    let mut cn = match pool.get() {
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
        Err(files::FileError::QueryError(err)) => {
            error!("Error running `read` builtin: {err}");
            Err(ExecutionError::InternalError)
        }
        Err(err) => Ok(Output::new_falsy_with(err.to_string().into())),
    }
}

async fn write(
    State {
        pool,
        chat_id,
        user_id,
    }: State,
    _: &FunctionLibrary,
    _: &mut ScopeStack<'_>,
    args: Vec<Output>,
) -> Result<Output, ExecutionError> {
    let (path, content) = match args.as_slice() {
        [path, content] => (path, content),
        _ => return Err(ExecutionError::InternalError),
    };
    let mut cn = match pool.get() {
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
        Err(files::FileError::QueryError(err)) => {
            error!("Error running `write` builtin: {err}");
            Err(ExecutionError::InternalError)
        }
        Err(err) => Ok(Output::new_falsy_with(err.to_string().into())),
    }
}

async fn history(
    State {
        pool,
        chat_id,
        ..
    }: State,
    _: &FunctionLibrary,
    _: &mut ScopeStack<'_>,
    args: Vec<Output>,
) -> Result<Output, ExecutionError> {
    let (count, offset, with_content, with_users, with_bot) = match args.as_slice() {
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
    match model::Message::list(&mut cn, chat_id, count, offset) {
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

pub fn register_libraries(
    pool: PgPool,
    chat_id: ChatId,
    user_id: UserId,
    mut runtime: Runtime,
) -> Runtime {
    let state = State {
        pool,
        chat_id,
        user_id,
    };
    let _ = runtime.library.merge(builtin::std::build());
    let _ = runtime.library.merge(builtin::str::build());
    let _ = runtime.library.merge(builtin::net::build());
    let _ = runtime.library.merge(builtin::iter::build());
    let _ = runtime.library.merge(builtin::math::build());
    let _ = runtime.library.merge(builtin::logic::build());
    let _ = runtime.library.merge(builtin::json::build());
    builtin_state!(runtime.library, read, state.clone(), "path");
    builtin_state!(runtime.library, write, state.clone(), "path", "content");
    builtin_state!(runtime.library, history, state.clone());
    builtin_state!(runtime.library, history, state.clone(), "history");
    builtin_state!(runtime.library, history, state.clone(), "history", "count");
    builtin_state!(
        runtime.library,
        history,
        state.clone(),
        "history",
        "count",
        "offset"
    );
    builtin_state!(
        runtime.library,
        history,
        state.clone(),
        "history",
        "count",
        "offset",
        "with_content"
    );
    builtin_state!(
        runtime.library,
        history,
        state.clone(),
        "history",
        "count",
        "offset",
        "with_content",
        "with_users"
    );
    builtin_state!(
        runtime.library,
        history,
        state.clone(),
        "history",
        "count",
        "offset",
        "with_content",
        "with_users",
        "with_bot"
    );
    runtime
}
