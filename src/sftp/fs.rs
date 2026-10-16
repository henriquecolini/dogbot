//! The SFTP protocol handler, backed by the emulated filesystem.
//!
//! One instance serves one authenticated session: a fixed `(user_id, chat_id)`
//! pair established during SSH authentication. Every operation runs through the
//! ordinary `crate::files` API, so the mount is exactly as privileged as that
//! user already is in Telegram — there is no separate authorization path here.

use crate::prelude::*;
use russh_sftp::protocol::{
    Attrs, Data, File as SftpFile, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
    Version,
};
use std::collections::HashMap;
use teloxide::prelude::*;

/// Largest file the mount will hold in memory. Content lives in a single
/// `BYTEA` column, so reads and writes are whole-file operations.
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

/// An open handle. Content is buffered because the database stores each file as
/// one blob while SFTP reads and writes ranges of it.
enum Open {
    Dir { entries: Vec<SftpFile>, sent: bool },
    File { path: String, buf: Vec<u8>, dirty: bool },
}

pub struct DogFs {
    pool: PgPool,
    user_id: UserId,
    chat_id: ChatId,
    handles: HashMap<String, Open>,
    next_handle: u64,
}

impl DogFs {
    pub fn new(pool: PgPool, user_id: UserId, chat_id: ChatId) -> Self {
        Self {
            pool,
            user_id,
            chat_id,
            handles: HashMap::new(),
            next_handle: 0,
        }
    }

    fn fresh_handle(&mut self) -> String {
        self.next_handle += 1;
        self.next_handle.to_string()
    }

    /// Runs a blocking database closure off the async runtime.
    async fn db<T, F>(&self, f: F) -> Result<T, StatusCode>
    where
        T: Send + 'static,
        F: FnOnce(&mut diesel::PgConnection) -> Result<T, files::FileError> + Send + 'static,
    {
        let pool = self.pool.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let mut cn = pool.get().map_err(|err| {
                log::error!("sftp: no database connection: {err}");
                files::FileError::InvalidArguments
            })?;
            f(&mut cn)
        })
        .await;

        match joined {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(status_for(&err)),
            Err(err) => {
                log::error!("sftp: blocking task failed: {err}");
                Err(StatusCode::Failure)
            }
        }
    }

    fn ok(&self, id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        }
    }
}

/// Maps a filesystem error onto the closest SFTP status code.
fn status_for(err: &files::FileError) -> StatusCode {
    use files::FileError::*;
    match err {
        FileNotFound(_) | ChatNotFound(_) => StatusCode::NoSuchFile,
        NotEnoughPermissions(_) => StatusCode::PermissionDenied,
        FileAlreadyExists(_) => StatusCode::Failure,
        NotADirectory(_) | NotAFile(_) | InvalidFileName(_) | InvalidArguments => {
            StatusCode::Failure
        }
        QueryError(err) => {
            log::error!("sftp: database error: {err}");
            StatusCode::Failure
        }
    }
}

/// Normalises a client-supplied path to one the `files` API accepts.
///
/// Resolves `.` and `..` (which `Path::parse` does not) and rejects `:`, which
/// would otherwise be read as a cross-chat host prefix and escape the mount.
fn normalize(path: &str) -> Result<String, StatusCode> {
    if path.contains(':') {
        return Err(StatusCode::NoSuchFile);
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

/// Renders a file row as SFTP attributes. `size` is passed separately so
/// listings can avoid fetching content.
fn attrs_for(file: &model::File, size: i64, owner: Option<&str>) -> FileAttributes {
    let mut mode = 0u32;
    if file.user_read {
        mode |= 0o400;
    }
    if file.user_write {
        mode |= 0o200;
    }
    if file.user_execute {
        mode |= 0o100;
    }
    if file.group_read {
        mode |= 0o040;
    }
    if file.group_write {
        mode |= 0o020;
    }
    if file.group_execute {
        mode |= 0o010;
    }
    if file.others_read {
        mode |= 0o004;
    }
    if file.others_write {
        mode |= 0o002;
    }
    if file.others_execute {
        mode |= 0o001;
    }
    mode |= if file.is_dir() { 0o040000 } else { 0o100000 };

    FileAttributes {
        size: Some(if file.is_dir() { 0 } else { size.max(0) as u64 }),
        uid: Some(file.owner_id.unwrap_or(0) as u32),
        user: owner.map(str::to_string),
        gid: Some(0),
        group: None,
        permissions: Some(mode),
        atime: Some(file.last_modified_at.unix_timestamp() as u32),
        mtime: Some(file.last_modified_at.unix_timestamp() as u32),
    }
}

impl russh_sftp::server::Handler for DogFs {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let path = normalize(&path)?;
        Ok(Name {
            id,
            files: vec![SftpFile::dummy(format!("/{path}"))],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = normalize(&path)?;
        let (chat, user) = (self.chat_id, self.user_id);
        let (file, size) = self
            .db(move |cn| {
                let file = files::stat(cn, chat, user, &path)?;
                let size = model::File::size(cn, file.id)?;
                Ok((file, size))
            })
            .await?;
        Ok(Attrs {
            id,
            attrs: attrs_for(&file, size, None),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        // There are no symlinks, so lstat and stat are the same.
        self.stat(id, path).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let path = match self.handles.get(&handle) {
            Some(Open::File { path, .. }) => path.clone(),
            _ => return Err(StatusCode::Failure),
        };
        self.stat(id, path).await
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let path = normalize(&path)?;
        let (chat, user) = (self.chat_id, self.user_id);
        let listing = self
            .db(move |cn| {
                let dir = files::stat(cn, chat, user, &path)?;
                if !dir.is_dir() {
                    return Err(files::FileError::NotADirectory(dir.name));
                }
                if !dir.can_read(cn, user)? || !dir.can_execute(cn, user)? {
                    return Err(files::FileError::NotEnoughPermissions(dir.name));
                }
                let children = model::File::list_children_with_size(cn, dir.id)?;
                let mut entries = Vec::with_capacity(children.len());
                for (child, size) in children {
                    let owner = match child.owner_id {
                        Some(owner_id) => model::User::try_get(cn, UserId(owner_id as u64))?
                            .map(|u| u.first_name),
                        None => Some("root".to_string()),
                    };
                    let attrs = attrs_for(&child, size, owner.as_deref());
                    entries.push(SftpFile::new(child.name.clone(), attrs));
                }
                Ok(entries)
            })
            .await?;

        let handle = self.fresh_handle();
        self.handles.insert(
            handle.clone(),
            Open::Dir {
                entries: listing,
                sent: false,
            },
        );
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir { entries, sent }) if !*sent => {
                *sent = true;
                Ok(Name {
                    id,
                    files: std::mem::take(entries),
                })
            }
            // Everything was delivered in one batch; signal end of directory.
            Some(Open::Dir { .. }) => Err(StatusCode::Eof),
            _ => Err(StatusCode::Failure),
        }
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = normalize(&filename)?;
        let (chat, user) = (self.chat_id, self.user_id);
        let truncate = pflags.contains(OpenFlags::TRUNCATE);
        let create = pflags.contains(OpenFlags::CREATE);
        let exclude = pflags.contains(OpenFlags::EXCLUDE);
        let writing = pflags.contains(OpenFlags::WRITE) || pflags.contains(OpenFlags::APPEND);

        let probe = path.clone();
        let (buf, dirty, existed) = self
            .db(move |cn| {
                let existing = match files::stat(cn, chat, user, &probe) {
                    Ok(file) => Some(file),
                    Err(files::FileError::FileNotFound(_)) => None,
                    Err(err) => return Err(err),
                };

                match existing {
                    Some(file) if file.is_dir() => Err(files::FileError::NotAFile(file.name)),
                    Some(_) if exclude => Err(files::FileError::FileAlreadyExists(probe)),
                    // Truncating discards the old content, so don't fetch it.
                    Some(_) if truncate => Ok((Vec::new(), true, true)),
                    Some(file) => {
                        if !file.can_read(cn, user)? {
                            return Err(files::FileError::NotEnoughPermissions(file.name));
                        }
                        Ok((file.read(cn)?, false, true))
                    }
                    // A new file must exist by the time `close` runs, even if
                    // nothing is ever written to it.
                    None if create => Ok((Vec::new(), true, false)),
                    None => Err(files::FileError::FileNotFound(probe)),
                }
            })
            .await?;

        if !writing && !existed {
            // A read-only open may not conjure a file into existence.
            return Err(StatusCode::NoSuchFile);
        }

        let handle = self.fresh_handle();
        self.handles
            .insert(handle.clone(), Open::File { path, buf, dirty });
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(Open::File { buf, .. }) = self.handles.get(&handle) else {
            return Err(StatusCode::Failure);
        };
        let offset = offset as usize;
        if offset >= buf.len() {
            return Err(StatusCode::Eof);
        }
        let end = (offset + len as usize).min(buf.len());
        Ok(Data {
            id,
            data: buf[offset..end].to_vec(),
        })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(Open::File { buf, dirty, .. }) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        let offset = offset as usize;
        if offset + data.len() > MAX_FILE_BYTES {
            return Err(StatusCode::Failure);
        }
        if offset > buf.len() {
            buf.resize(offset, 0);
        }
        let end = offset + data.len();
        if end > buf.len() {
            buf.resize(end, 0);
        }
        buf[offset..end].copy_from_slice(&data);
        *dirty = true;
        Ok(self.ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        let Some(open) = self.handles.remove(&handle) else {
            return Err(StatusCode::Failure);
        };
        if let Open::File { path, buf, dirty } = open
            && dirty
        {
            let (chat, user) = (self.chat_id, self.user_id);
            self.db(move |cn| files::write(cn, chat, user, &path, &buf).map(|_| ()))
                .await?;
        }
        Ok(self.ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = normalize(&path)?;
        let (chat, user) = (self.chat_id, self.user_id);
        self.db(move |cn| files::mkdir(cn, chat, user, &path, false).map(|_| ()))
            .await?;
        Ok(self.ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let path = normalize(&path)?;
        let (chat, user) = (self.chat_id, self.user_id);
        self.db(move |cn| {
            let dir = files::stat(cn, chat, user, &path)?;
            if !dir.is_dir() {
                return Err(files::FileError::NotADirectory(dir.name));
            }
            // rmdir only removes empty directories; `rm -r` is a separate request.
            if !model::File::list_children(cn, dir.id)?.is_empty() {
                return Err(files::FileError::InvalidArguments);
            }
            files::remove(cn, chat, user, &path, true)
        })
        .await?;
        Ok(self.ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let path = normalize(&filename)?;
        let (chat, user) = (self.chat_id, self.user_id);
        self.db(move |cn| files::remove(cn, chat, user, &path, false))
            .await?;
        Ok(self.ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let from = normalize(&oldpath)?;
        let to = normalize(&newpath)?;
        let (chat, user) = (self.chat_id, self.user_id);
        self.db(move |cn| files::rename(cn, chat, user, &from, &to).map(|_| ()))
            .await?;
        Ok(self.ok(id))
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = normalize(&path)?;
        let (chat, user) = (self.chat_id, self.user_id);
        let mode = attrs.permissions.map(|m| m & 0o777);
        let size = attrs.size;

        self.db(move |cn| {
            if let Some(mode) = mode {
                files::set_mode(cn, chat, user, &path, mode)?;
            }
            // `truncate` arrives as a setstat with a size.
            if let Some(size) = size {
                let current = files::read(cn, chat, user, &path)?.1;
                if (size as usize) < current.len() {
                    files::write(cn, chat, user, &path, &current[..size as usize])?;
                }
            }
            Ok(())
        })
        .await?;
        Ok(self.ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = match self.handles.get(&handle) {
            Some(Open::File { path, .. }) => path.clone(),
            _ => return Err(StatusCode::Failure),
        };
        self.setstat(id, path, attrs).await
    }
}
