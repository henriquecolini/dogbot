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
        [path] => path.value(),
        _ => return Err(ExecutionError::InternalError),
    };
    let mut cn = match pool.get() {
        Ok(cn) => cn,
        Err(err) => {
            error!("Error running `read` builtin: {err}");
            return Err(ExecutionError::InternalError);
        }
    };
    match crate::files::read(&mut cn, chat_id, user_id, path) {
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
        [path, content] => (path.value(), content.value().as_bytes()),
        _ => return Err(ExecutionError::InternalError),
    };
    let mut cn = match pool.get() {
        Ok(cn) => cn,
        Err(err) => {
            error!("Error running `write` builtin: {err}");
            return Err(ExecutionError::InternalError);
        }
    };
    match crate::files::write(&mut cn, chat_id, user_id, path, content) {
        Ok(_) => Ok(Output::new_truthy()),
        Err(files::FileError::QueryError(err)) => {
            error!("Error running `write` builtin: {err}");
            Err(ExecutionError::InternalError)
        }
        Err(err) => Ok(Output::new_falsy_with(err.to_string().into())),
    }
}

async fn ls(
    State {
        pool,
        chat_id,
        user_id,
        ..
    }: State,
    _: &FunctionLibrary,
    _: &mut ScopeStack<'_>,
    args: Vec<Output>,
) -> Result<Output, ExecutionError> {
    let (path, include_files, include_dirs) = match args.as_slice() {
        [path] => (path.value(), true, true),
        [path, include_files] => (path.value(), include_files.is_truthy(), true),
        [path, include_files, include_dirs] => (
            path.value(),
            include_files.is_truthy(),
            include_dirs.is_truthy(),
        ),
        _ => return Err(ExecutionError::InternalError),
    };
    let mut cn = match pool.get() {
        Ok(cn) => cn,
        Err(err) => {
            error!("Error running `ls` builtin: {err}");
            return Err(ExecutionError::InternalError);
        }
    };
    match files::list(&mut cn, chat_id, user_id, path, false) {
        Ok(list) => Ok(Output::new_truthy_with(
            list.into_iter()
                .filter(|f| (include_files && f.is_file()) || (include_dirs && f.is_dir()))
                .map(|f| f.name)
                .collect::<Vec<String>>()
                .join("\n")
                .into(),
        )),
        Err(files::FileError::QueryError(err)) => {
            error!("Error running `ls` builtin: {err}");
            Err(ExecutionError::InternalError)
        }
        Err(err) => Ok(Output::new_falsy_with(err.to_string().into())),
    }
}

async fn version(
    _: &FunctionLibrary,
    _: &mut ScopeStack<'_>,
    args: Vec<Output>,
) -> Result<Output, ExecutionError> {
    match args.as_slice() {
        [] => {}
        _ => return Err(ExecutionError::InternalError),
    };
    Ok(Output::new_truthy_with(
        option_env!("APP_VERSION").unwrap_or("dev").into(),
    ))
}

async fn history(
    State { pool, chat_id, .. }: State,
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
            let mut output = vec![];
            for (message, user) in messages {
                if !with_bot && user.is_none() {
                    continue;
                }
                let mut line = String::new();
                if with_users {
                    if let Some(user) = user {
                        line += &user.first_name;
                        if let Some(last_name) = user.last_name {
                            line += " ";
                            line += &last_name;
                        }
                    } else {
                        line += "<bot>"
                    }
                }
                if with_users && with_content {
                    line += ": ";
                }
                if with_content {
                    line += &message.content.replace("\n", " ");
                }
                output.push(line);
            }
            Ok(Output::new_truthy_with(output.join("\n").into()))
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
    builtin_state!(runtime.library, ls, state.clone(), "path");
    builtin_state!(runtime.library, ls, state.clone(), "path", "include_files");
    builtin_state!(
        runtime.library,
        ls,
        state.clone(),
        "path",
        "include_files",
        "include_dirs"
    );
    builtin_state!(runtime.library, history, state.clone());
    builtin_state!(runtime.library, history, state.clone(), "count");
    builtin_state!(runtime.library, history, state.clone(), "count", "offset");
    builtin_state!(
        runtime.library,
        history,
        state.clone(),
        "count",
        "offset",
        "with_content"
    );
    builtin_state!(
        runtime.library,
        history,
        state.clone(),
        "count",
        "offset",
        "with_content",
        "with_users"
    );
    builtin_state!(
        runtime.library,
        history,
        state.clone(),
        "count",
        "offset",
        "with_content",
        "with_users",
        "with_bot"
    );
    builtin!(runtime.library, version);
    runtime
}
