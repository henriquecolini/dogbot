pub mod path;
pub mod perm;

#[cfg(test)]
mod tests;

use crate::prelude::*;
use diesel::PgConnection;
use model::File;
use path::*;
use perm::*;
use thiserror::Error;
use uuid::Uuid;

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
) -> Result<(String, Vec<u8>), FileError> {
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

        let content = file.read(cn)?;
        Ok((file.name, content))
    })
}

pub fn read_for_execution(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
) -> Result<(String, Vec<u8>), FileError> {
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

        let content = file.read(cn)?;
        Ok((file.name, content))
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

/// Metadata for a single path, without reading its content.
pub fn stat(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
) -> Result<File, FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        Ok(path.traverse(cn, user_id)?.required()?.into_file())
    })
}

/// Moves or renames a file or directory. Requires write on both the source and
/// the destination parent directory.
///
/// Editors save atomically by writing a temporary file and renaming it over the
/// target, so this is on the hot path for any mounted filesystem.
pub fn rename(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    from: &str,
    to: &str,
) -> Result<File, FileError> {
    cn.transaction(|cn| {
        let from_path = Path::parse(from, chat_id);
        let to_path = Path::parse(to, chat_id);

        let from_trav = from_path.traverse(cn, user_id)?.required()?;
        if !from_trav.parent().can_write(cn, user_id)? {
            return Err(FileError::NotEnoughPermissions(from_trav.into_parent().name));
        }
        let (from_parent, file) = from_trav.into_parent_and_file();

        // A chat root has parent_id == id; moving it would corrupt the tree.
        if file.id == from_parent.id {
            return Err(FileError::InvalidArguments);
        }

        let to_trav = to_path.traverse(cn, user_id)?;
        let new_name = validate_name(to_trav.file_name())?.to_string();
        let new_parent_id = to_trav.parent().id;

        if !to_trav.parent().can_write(cn, user_id)? {
            return Err(FileError::NotEnoughPermissions(to_trav.into_parent().name));
        }

        // Renaming a directory into itself or one of its own descendants would
        // detach that subtree from the root.
        if file.is_dir() && is_same_or_descendant(cn, new_parent_id, file.id)? {
            return Err(FileError::InvalidArguments);
        }

        let existing = to_trav.into_file();
        // Overwriting a regular file is how atomic saves land. Unlike `mv`, the
        // destination's permissions and owner are kept rather than the
        // source's: an editor writes a fresh temp file with default
        // permissions, and taking those would silently strip the execute bit
        // from main.dog on every save.
        let mut inherited = None;
        if let Some(existing) = existing {
            if existing.id == file.id {
                return Ok(file);
            }
            if existing.is_dir() {
                return Err(FileError::FileAlreadyExists(new_name));
            }
            inherited = Some(existing.clone());
            File::delete(cn, existing.id)?;
        }

        match File::set_parent_and_name(cn, file.id, new_parent_id, &new_name) {
            Ok(moved) => {
                if let Some(old) = inherited {
                    File::set_user_read(cn, moved.id, old.user_read)?;
                    File::set_user_write(cn, moved.id, old.user_write)?;
                    File::set_user_execute(cn, moved.id, old.user_execute)?;
                    File::set_group_read(cn, moved.id, old.group_read)?;
                    File::set_group_write(cn, moved.id, old.group_write)?;
                    File::set_group_execute(cn, moved.id, old.group_execute)?;
                    File::set_others_read(cn, moved.id, old.others_read)?;
                    File::set_others_write(cn, moved.id, old.others_write)?;
                    File::set_others_execute(cn, moved.id, old.others_execute)?;
                    File::set_owner(cn, moved.id, old.owner_id.map(|id| UserId(id as u64)))?;
                    return Ok(File::get(cn, moved.id)?);
                }
                Ok(moved)
            }
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => Err(FileError::FileAlreadyExists(new_name)),
            Err(err) => Err(err.into()),
        }
    })
}

/// Whether `candidate` is `ancestor` itself or sits underneath it.
fn is_same_or_descendant(
    cn: &mut PgConnection,
    candidate: Uuid,
    ancestor: Uuid,
) -> Result<bool, FileError> {
    let mut current = candidate;
    loop {
        if current == ancestor {
            return Ok(true);
        }
        let file = File::get(cn, current)?;
        // The chat root points at itself, so this terminates.
        if file.parent_id == current {
            return Ok(false);
        }
        current = file.parent_id;
    }
}

/// Applies a full Unix permission mode (the low 9 bits of `mode`) at once.
///
/// `set_permission` takes a symbolic clause, which can only describe one set of
/// flags; SFTP's `setstat` hands over a whole mode, so this sets all three
/// classes directly. Like `chmod`, only the owner may do it.
pub fn set_mode(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    mode: u32,
) -> Result<(), FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let file = path.traverse(cn, user_id)?.required()?.into_file();

        if file.owner_id != Some(user_id.0 as i64) {
            return Err(FileError::NotEnoughPermissions(file.name));
        }

        File::set_user_read(cn, file.id, mode & 0o400 != 0)?;
        File::set_user_write(cn, file.id, mode & 0o200 != 0)?;
        File::set_user_execute(cn, file.id, mode & 0o100 != 0)?;
        File::set_group_read(cn, file.id, mode & 0o040 != 0)?;
        File::set_group_write(cn, file.id, mode & 0o020 != 0)?;
        File::set_group_execute(cn, file.id, mode & 0o010 != 0)?;
        File::set_others_read(cn, file.id, mode & 0o004 != 0)?;
        File::set_others_write(cn, file.id, mode & 0o002 != 0)?;
        File::set_others_execute(cn, file.id, mode & 0o001 != 0)?;
        Ok(())
    })
}

/// Changes a file's owner. Only the current owner may give a file away.
pub fn set_owner(
    cn: &mut PgConnection,
    chat_id: ChatId,
    user_id: UserId,
    path: &str,
    new_owner: UserId,
) -> Result<(), FileError> {
    cn.transaction(|cn| {
        let path = Path::parse(path, chat_id);
        let file = path.traverse(cn, user_id)?.required()?.into_file();

        if file.owner_id != Some(user_id.0 as i64) {
            return Err(FileError::NotEnoughPermissions(file.name));
        }
        if !model::UserInChat::exists(cn, new_owner, ChatId(file.chat_id))? {
            return Err(FileError::InvalidArguments);
        }

        File::set_owner(cn, file.id, Some(new_owner))?;
        Ok(())
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
