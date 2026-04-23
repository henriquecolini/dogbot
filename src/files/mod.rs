pub mod path;
pub mod perm;

use crate::prelude::*;
use diesel::PgConnection;
use model::File;
use path::*;
use perm::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("{0}")]
    QueryError(#[from] diesel::result::Error),
    #[error("invalid arguments")]
    InvalidArguments,
    #[error("{0}: chat not found")]
    ChatNotFound(String),
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
    #[error("{0}: invalid filename")]
    InvalidFileName(String),
}

fn validate_name(name: &str) -> Result<&str, FileError> {
    if name.is_empty() || name.contains(['/', ':', '\n']) {
        Err(FileError::InvalidFileName(name.to_string()))
    } else {
        Ok(name)
    }
}

pub fn mkdir(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    create_parents: bool,
) -> Result<File, FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let trav = path.traverse_create_parents(cn, user_id, create_parents)?;

        if let Some(file) = trav.file() {
            if !create_parents {
                return Err(FileError::FileAlreadyExists(trav.into_file_name()));
            }
            if file.is_file() {
                return Err(FileError::FileAlreadyExists(trav.into_file_name()));
            }
            return Ok(trav.into_file().unwrap());
        }

        let parent = trav.parent();

        if !parent.can_write(cn, user_id)? {
            return Err(FileError::NotEnoughPermissions(trav.into_parent().name));
        }

        Ok(File::insert_dir(
            cn,
            Some(user_id),
            parent.id,
            validate_name(trav.file_name())?,
        )?)
    })
}

pub fn write(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    content: &[u8],
) -> Result<File, FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let trav = path.traverse(cn, user_id)?;

        if let Some(file) = trav.file() {
            if file.is_dir() {
                return Err(FileError::NotAFile(trav.into_file_name()));
            }
            if !file.can_write(cn, user_id)? {
                return Err(FileError::NotEnoughPermissions(trav.into_file_name()));
            }
        } else {
            if !trav.parent().can_write(cn, user_id)? {
                return Err(FileError::NotEnoughPermissions(trav.into_parent().name));
            }
        }

        Ok(File::upsert_file(
            cn,
            Some(user_id),
            trav.parent().id,
            validate_name(trav.file_name())?,
            content,
        )?)
    })
}

pub fn remove(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    recursive: bool,
) -> Result<(), FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let trav = path.traverse(cn, user_id)?.required()?;

        if !trav.parent().can_write(cn, user_id)? {
            return Err(FileError::NotEnoughPermissions(trav.into_parent().name));
        }

        let file = trav.into_file();

        if file.is_dir() && !recursive {
            return Err(FileError::NotAFile(file.name));
        }

        File::delete(cn, file.id)?;

        Ok(())
    })
}

pub fn read(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
) -> Result<Vec<u8>, FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let trav = path.traverse(cn, user_id)?.required()?;

        let file = trav.into_file();

        if file.is_dir() {
            return Err(FileError::NotAFile(file.name));
        }

        if !file.can_read(cn, user_id)? {
            return Err(FileError::NotEnoughPermissions(file.name));
        }

        Ok(file.read(cn)?)
    })
}

pub fn read_for_execution(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
) -> Result<Vec<u8>, FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let trav = path.traverse(cn, user_id)?.required()?;

        let file = trav.into_file();

        if file.is_dir() {
            return Err(FileError::NotAFile(file.name));
        }

        if !file.can_read(cn, user_id)? || !file.can_execute(cn, user_id)? {
            return Err(FileError::NotEnoughPermissions(file.name));
        }

        Ok(file.read(cn)?)
    })
}

pub fn list(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    all: bool,
) -> Result<Vec<File>, FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let (parent, file) = path
            .traverse(cn, user_id)?
            .required()?
            .into_parent_and_file();

        let mut children;

        if file.is_dir() {
            if !file.can_read(cn, user_id)? || !file.can_execute(cn, user_id)? {
                return Err(FileError::NotEnoughPermissions(file.name));
            }
            children = File::list_children(cn, file.id)?;
            if all {
                children.insert(0, parent);
                children[0].name.replace_range(.., "..");
                children.insert(0, file);
                children[0].name.replace_range(.., ".");
            }
        } else {
            children = vec![file];
        }

        Ok(children)
    })
}

pub fn set_permission(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    perm: Clause,
) -> Result<(), FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let file = path.traverse(cn, user_id)?.required()?.into_file();

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
    })
}
