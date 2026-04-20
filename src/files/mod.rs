pub mod perm;

use crate::prelude::*;
use diesel::PgConnection;
use model::File;
use perm::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("{0}")]
    QueryError(#[from] diesel::result::Error),
    #[error("invalid arguments")]
    InvalidArguments,
    #[error("{0}: file not found")]
    FileNotFound(String),
    #[error("{0}: permission denied")]
    NotEnoughPermissions(String),
    #[error("{0}: file already exists")]
    FileAlreadyExists(String),
    #[error("{0}: not a directory")]
    NotADirectory(String),
    #[error("{0}: not a file")]
    NotAFile(String),
}

fn split_path(path: &str) -> Vec<&str> {
    path.strip_prefix("/")
        .unwrap_or(path)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect()
}

fn resolve_parts<'p>(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    parts: &[&'p str],
) -> Result<
    (
        Result<(File, &'p str), Option<&'p str>>,
        Result<(File, &'p str), Option<&'p str>>,
    ),
    FileError,
> {
    if parts.is_empty() {
        return Ok((Err(None), Err(None)));
    }

    let (last, rest) = parts.split_last().unwrap();

    let mut current: Result<(File, &str), Option<&str>> = Err(Some(parts[0]));

    for name in rest {
        let file_id = current.as_ref().map(|f| f.0.id);
        let file = File::find_by_name(cn, chat_id, file_id.ok(), name)?;

        let Some(file) = file else {
            return Err(FileError::FileNotFound(name.to_string()));
        };
        if !file.can_read(user_id) {
            return Err(FileError::NotEnoughPermissions(file.name));
        }
        if !file.is_dir() {
            return Err(FileError::NotADirectory(file.name));
        }
        current = Ok((file, name));
    }

    let file_id = current.as_ref().map(|f| f.0.id);
    let file = File::find_by_name(cn, chat_id, file_id.ok(), last)?;

    let parent = current;
    current = match file {
        None => Err(Some(last)),
        Some(file) => Ok((file, last)),
    };

    Ok((parent, current))
}

fn resolve_path<'p>(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &'p str,
) -> Result<
    (
        Result<(File, &'p str), Option<&'p str>>,
        Result<(File, &'p str), Option<&'p str>>,
    ),
    FileError,
> {
    resolve_parts(cn, chat_id, user_id, &split_path(path))
}

fn resolve_path_ensure_file<'p>(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &'p str,
) -> Result<
    (
        Result<(File, &'p str), Option<&'p str>>,
        (Option<File>, &'p str),
    ),
    FileError,
> {
    let (parent, file) = resolve_path(cn, chat_id, user_id, path)?;
    match file {
        Ok((file, name)) => Ok((parent, (Some(file), name))),
        Err(Some(name)) => Ok((parent, (None, name))),
        Err(None) => Err(FileError::InvalidArguments),
    }
}

pub fn mkdir(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
) -> Result<Option<File>, FileError> {
    let parts = split_path(path);
    if parts.is_empty() {
        return Ok(None);
    }
    let mut parent: Option<File> = None;

    for (idx, name) in parts.iter().enumerate() {
        let parent_id = parent.as_ref().map(|p| p.id);

        let file = File::find_by_name(cn, chat_id, parent_id, name)?;

        let current = match file {
            None => {
                // Permission: need write on parent to create
                if let Some(parent) = parent {
                    if !parent.can_write(user_id) {
                        return Err(FileError::NotEnoughPermissions(parent.name));
                    }
                }

                File::create_dir(cn, chat_id, Some(user_id), parent_id, name)?
            }

            Some(file) => {
                if !file.is_dir() {
                    return Err(FileError::NotADirectory(file.name));
                }

                // Permission: need read to traverse
                if !file.can_read(user_id) {
                    return Err(FileError::NotEnoughPermissions(file.name));
                }

                file
            }
        };

        // If this is the final component, return it
        if idx == parts.len() - 1 {
            return Ok(Some(current));
        }

        parent = Some(current);
    }

    // This should be unreachable logically
    Err(FileError::InvalidArguments)
}

pub fn write(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    content: &[u8],
) -> Result<File, FileError> {
    let (parent, (file, file_name)) = resolve_path_ensure_file(cn, chat_id, user_id, &path)?;

    if let Ok((parent, _)) = &parent {
        if !parent.can_write(user_id) {
            return Err(FileError::NotEnoughPermissions(parent.name.to_owned()));
        }
    }

    if let Some(file) = file {
        if file.is_dir() {
            return Err(FileError::NotAFile(file.name));
        }
        if !file.can_write(user_id) {
            return Err(FileError::NotEnoughPermissions(file.name));
        }
    }

    Ok(File::create_file(
        cn,
        chat_id,
        Some(user_id),
        parent.map(|p| p.0.id).ok(),
        file_name,
        content,
    )?)
}

pub fn remove(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    recursive: bool,
) -> Result<(), FileError> {
    let (parent, (file, file_name)) = resolve_path_ensure_file(cn, chat_id, user_id, &path)?;

    if let Ok((parent, _)) = &parent {
        if !parent.can_write(user_id) {
            return Err(FileError::NotEnoughPermissions(parent.name.to_owned()));
        }
    }

    let Some(file) = file else {
        return Err(FileError::FileNotFound(file_name.to_owned()));
    };

    if !file.can_write(user_id) {
        return Err(FileError::NotEnoughPermissions(file.name));
    }

    if file.is_dir() && !recursive {
        return Err(FileError::NotAFile(file.name));
    }

    if !file.can_write(user_id) {
        return Err(FileError::NotEnoughPermissions(file.name));
    }

    File::delete(cn, file.id)?;

    Ok(())
}

pub fn read(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
) -> Result<Vec<u8>, FileError> {
    let (_, (file, file_name)) = resolve_path_ensure_file(cn, chat_id, user_id, &path)?;

    let Some(file) = file else {
        return Err(FileError::FileNotFound(file_name.to_owned()));
    };

    if file.is_dir() {
        return Err(FileError::NotAFile(file.name));
    }

    if !file.can_read(user_id) {
        return Err(FileError::NotEnoughPermissions(file.name));
    }

    Ok(file.content.unwrap())
}

pub fn list(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
) -> Result<Vec<File>, FileError> {
    let (_, dir) = resolve_path(cn, chat_id, user_id, path)?;

    let Ok((dir, _)) = dir else {
        return Ok(File::list_children(cn, chat_id, None)?);
    };

    if !dir.is_dir() {
        return Err(FileError::NotADirectory(dir.name));
    }

    if !dir.can_read(user_id) {
        return Err(FileError::NotEnoughPermissions(dir.name));
    }

    Ok(File::list_children(cn, chat_id, Some(dir.id))?)
}

pub fn set_permission(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    perm: Clause,
) -> Result<(), FileError> {
    let (_, (file, file_name)) = resolve_path_ensure_file(cn, chat_id, user_id, &path)?;

    let Some(file) = file else {
        return Err(FileError::FileNotFound(file_name.to_owned()));
    };

    if file.owner_id != Some(user_id.0 as i64) {
        return Err(FileError::NotEnoughPermissions(file.name));
    }

    for who in perm.who {
        let set_read: fn(&mut _, _, _) -> _;
        let set_write: fn(&mut _, _, _) -> _;
        let set_execute: fn(&mut _, _, _) -> _;
        match who {
            Who::User => {
                set_read = File::set_user_read;
                set_write = File::set_user_write;
                set_execute = File::set_user_execute;
            }
            Who::Group => {
                set_read = File::set_group_read;
                set_write = File::set_group_write;
                set_execute = File::set_group_execute;
            }
            Who::Other => {
                set_read = File::set_others_read;
                set_write = File::set_others_write;
                set_execute = File::set_others_execute;
            }
            Who::All => {
                set_read = File::set_all_read;
                set_write = File::set_all_write;
                set_execute = File::set_all_execute;
            }
        }
        match perm.op {
            Op::Add => {
                if perm.perm.read {
                    set_read(cn, file.id, true).map(|_| ())?;
                }
                if perm.perm.write {
                    set_write(cn, file.id, true).map(|_| ())?;
                }
                if perm.perm.exec {
                    set_execute(cn, file.id, true).map(|_| ())?;
                }
            }
            Op::Remove => {
                if perm.perm.read {
                    set_read(cn, file.id, false).map(|_| ())?;
                }
                if perm.perm.write {
                    set_write(cn, file.id, false).map(|_| ())?;
                }
                if perm.perm.exec {
                    set_execute(cn, file.id, false).map(|_| ())?;
                }
            }
            Op::Set => {
                File::set_all_read(cn, file.id, false).map(|_| ())?;
                File::set_all_write(cn, file.id, false).map(|_| ())?;
                File::set_all_execute(cn, file.id, false).map(|_| ())?;
                if perm.perm.read {
                    set_read(cn, file.id, true).map(|_| ())?;
                }
                if perm.perm.write {
                    set_write(cn, file.id, true).map(|_| ())?;
                }
                if perm.perm.exec {
                    set_execute(cn, file.id, true).map(|_| ())?;
                }
            }
        }
    }
    Ok(())
}
