use crate::prelude::*;
use diesel::prelude::*;
use teloxide::types::{ChatId, UserId};

use super::FileError;
use model::File;

#[derive(Debug, Clone)]
pub struct Path<'a> {
    chat_address: ChatAddress<'a>,
    parts: Vec<&'a str>,
}

#[derive(Debug)]
pub struct TraversedOptFile<'a> {
    file: Option<File>,
    file_name: &'a str,
    parent: File,
}

#[derive(Debug)]
pub struct TraversedFile {
    file: File,
    parent: File,
}

impl<'a> Path<'a> {
    pub fn parse(input: &'a str, default_chat: ChatId) -> Self {
        let (chat_address, path) = if let Some((chat, rest)) = input.split_once(':') {
            (ChatAddress::parse(chat), rest)
        } else {
            (ChatAddress::Id(default_chat), input)
        };

        let parts = path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        Self {
            chat_address,
            parts,
        }
    }

    pub fn traverse_create_parents(
        &self,
        cn: &mut PgConnection,
        user: UserId,
        create_parents: bool,
    ) -> Result<TraversedOptFile<'_>, FileError> {
        let Some(chat) = model::Chat::find(cn, self.chat_address)? else {
            return Err(FileError::ChatNotFound(self.chat_address.to_string()));
        };
        let chat_id = chat.id();
        let mut current = File::get_root_dir(cn, chat_id)?;

        if !current.is_dir() {
            return Err(FileError::NotADirectory(current.name));
        }

        if !current.can_execute(cn, user)? {
            return Err(FileError::NotEnoughPermissions(current.name));
        }

        let Some((last, parts)) = self.parts.split_last() else {
            return Ok(TraversedOptFile {
                file: Some(current.clone()),
                file_name: "",
                parent: current,
            });
        };

        for name in parts {
            let next = File::find_by_name(cn, current.id, name)?;

            let next = match next {
                Some(f) => f,
                None => {
                    if !create_parents {
                        return Err(FileError::FileNotFound(name.to_string()));
                    }
                    if !current.can_write(cn, user)? {
                        return Err(FileError::NotEnoughPermissions(current.name));
                    }
                    File::insert_dir(cn, Some(user), current.id, name)?
                }
            };
            current = next;
            if !current.is_dir() {
                return Err(FileError::NotADirectory(current.name));
            }

            if !current.can_execute(cn, user)? {
                return Err(FileError::NotEnoughPermissions(current.name));
            }
        }

        if let Some(file) = File::find_by_name(cn, current.id, last)? {
            Ok(TraversedOptFile {
                file: Some(file),
                file_name: last,
                parent: current,
            })
        } else {
            Ok(TraversedOptFile {
                file: None,
                file_name: last,
                parent: current,
            })
        }
    }

    pub fn traverse(
        &self,
        cn: &mut PgConnection,
        user: UserId,
    ) -> Result<TraversedOptFile<'_>, FileError> {
        Self::traverse_create_parents(self, cn, user, false)
    }
}

impl<'a> TraversedOptFile<'a> {
    pub fn parent(&self) -> &File {
        &self.parent
    }
    pub fn file(&self) -> Option<&File> {
        self.file.as_ref()
    }
    pub fn file_name(&self) -> &str {
        self.file_name
    }
    pub fn into_parent(self) -> File {
        self.parent
    }
    pub fn into_file(self) -> Option<File> {
        self.file
    }
    pub fn into_file_name(self) -> String {
        self.file
            .map(|f| f.name)
            .unwrap_or_else(|| self.file_name.to_string())
    }
    pub fn required(self) -> Result<TraversedFile, FileError> {
        if self.file.is_none() {
            return Err(FileError::FileNotFound(self.file_name.to_string()));
        }
        Ok(TraversedFile {
            file: self.file.unwrap(),
            parent: self.parent,
        })
    }
}

impl TraversedFile {
    pub fn parent(&self) -> &File {
        &self.parent
    }
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn file_name(&self) -> &str {
        &self.file.name
    }
    pub fn into_parent(self) -> File {
        self.parent
    }
    pub fn into_file(self) -> File {
        self.file
    }
    pub fn into_parent_and_file(self) -> (File, File) {
        (self.parent, self.file)
    }
    pub fn into_file_name(self) -> String {
        self.file.name
    }
}
