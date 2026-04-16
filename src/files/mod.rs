use crate::model::file::File;
use diesel::PgConnection;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("{0}")]
    QueryError(#[from] diesel::result::Error),
    #[error("Arquivo não encontrado")]
    FileNotFound,
    #[error("Você não possui permissões suficientes")]
    NotEnoughPermissions,
    #[error("Já existe um arquivo com esse nome na mesma pasta")]
    FileAlreadyExists,
    #[error("Não é uma pasta")]
    NotADirectory,
    #[error("Não é um arquivo")]
    NotAFile,
}

fn split_path(path: &str) -> Vec<&str> {
    path.strip_prefix("/")
        .unwrap_or(path)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect()
}

fn resolve_parts(
    cn: &mut PgConnection,
    chat_id: i64,
    user_id: i64,
    parts: &[&str],
) -> Result<Option<File>, FileError> {
    if parts.is_empty() {
        return Ok(None);
    }

    let mut current: Option<File> = None;

    for (idx, name) in parts.iter().enumerate() {
        let parent_id = current.as_ref().map(|f| f.id);

        let file =
            File::find_by_name(cn, chat_id, parent_id, name)?.ok_or(FileError::FileNotFound)?;

        if !file.can_read(Some(user_id)) {
            return Err(FileError::NotEnoughPermissions);
        }

        if idx < parts.len() - 1 && !file.is_dir() {
            return Err(FileError::NotADirectory);
        }

        current = Some(file);
    }

    Ok(Some(current.unwrap()))
}

fn resolve_path(
    cn: &mut PgConnection,
    chat_id: i64,
    user_id: i64,
    path: &str,
) -> Result<Option<File>, FileError> {
    resolve_parts(cn, chat_id, user_id, &split_path(path))
}

pub fn mkdir(
    cn: &mut PgConnection,
    chat_id: i64,
    user_id: i64,
    path: &str,
) -> Result<Option<File>, FileError> {
    let path = split_path(path);
    if path.is_empty() {
        return Ok(None);
    }
    let mut parent: Option<File> = None;

    for (idx, name) in path.iter().enumerate() {
        let parent_id = parent.as_ref().map(|p| p.id);

        let file = File::find_by_name(cn, chat_id, parent_id, name)?;

        let current = match file {
            None => {
                // Permission: need write on parent to create
                if let Some(ref parent) = parent {
                    if !parent.can_write(Some(user_id)) {
                        return Err(FileError::NotEnoughPermissions);
                    }
                }

                File::create_dir(cn, chat_id, Some(user_id), parent_id, name)?
            }

            Some(file) => {
                if !file.is_dir() {
                    return Err(FileError::NotADirectory);
                }

                // Permission: need read to traverse
                if !file.can_read(Some(user_id)) {
                    return Err(FileError::NotEnoughPermissions);
                }

                file
            }
        };

        // If this is the final component, return it
        if idx == path.len() - 1 {
            return Ok(Some(current));
        }

        parent = Some(current);
    }

    // This should be unreachable logically
    Err(FileError::FileNotFound)
}

pub fn write(
    cn: &mut PgConnection,
    chat_id: i64,
    user_id: i64,
    path: &str,
    content: &[u8],
) -> Result<File, FileError> {
    let parts = split_path(path);

    if parts.is_empty() {
        return Err(FileError::FileNotFound);
    }

    let parent_parts = &parts[..parts.len() - 1];
    let parent = resolve_parts(cn, chat_id, user_id, parent_parts)?;

    if let Some(parent) = &parent {
        if !parent.is_dir() {
            return Err(FileError::NotADirectory);
        }
        if !parent.can_write(Some(user_id)) {
            return Err(FileError::NotEnoughPermissions);
        }
    }

    let parent_id = parent.map(|f| f.id);
    let name = parts.last().unwrap();

    if let Some(existing) = File::find_by_name(cn, chat_id, parent_id, name)? {
        if existing.is_dir() {
            return Err(FileError::NotAFile);
        }
        if !existing.can_write(Some(user_id)) {
            return Err(FileError::NotEnoughPermissions);
        }
    }

    Ok(File::create_file(
        cn,
        chat_id,
        Some(user_id),
        parent_id,
        name,
        content,
    )?)
}

pub fn remove(
    cn: &mut PgConnection,
    chat_id: i64,
    user_id: i64,
    path: &str,
    recursive: bool,
) -> Result<(), FileError> {
    let file = resolve_path(cn, chat_id, user_id, path)?;

    let Some(file) = file else {
        return Err(FileError::FileNotFound);
    };

    if file.is_dir() && !recursive {
        return Err(FileError::NotAFile);
    }

    if !file.can_write(Some(user_id)) {
        return Err(FileError::NotEnoughPermissions);
    }
    
    File::delete(cn, file.id)?;

    Ok(())
}

pub fn read(
    cn: &mut PgConnection,
    chat_id: i64,
    user_id: i64,
    path: &str,
) -> Result<Vec<u8>, FileError> {
    let file = resolve_path(cn, chat_id, user_id, path)?;

    let Some(file) = file else {
        return Err(FileError::FileNotFound);
    };

    if file.is_dir() {
        return Err(FileError::NotAFile);
    }

    if !file.can_read(Some(user_id)) {
        return Err(FileError::NotEnoughPermissions);
    }

    Ok(file.content.unwrap())
}

pub fn list(
    cn: &mut PgConnection,
    chat_id: i64,
    user_id: i64,
    path: &str,
) -> Result<Vec<File>, FileError> {
    let dir = resolve_path(cn, chat_id, user_id, path)?;

    let Some(dir) = dir else {
        return Ok(File::list_children(cn, chat_id, None)?);
    };

    if !dir.is_dir() {
        return Err(FileError::NotADirectory);
    }

    if !dir.can_read(Some(user_id)) {
        return Err(FileError::NotEnoughPermissions);
    }

    Ok(File::list_children(cn, chat_id, Some(dir.id))?)
}
