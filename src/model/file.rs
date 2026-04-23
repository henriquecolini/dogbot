use crate::schema::chats::dsl as chats;
use crate::schema::files::dsl as files;
use diesel::*;
use teloxide::prelude::*;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Selectable, Queryable, Debug, Clone)]
#[diesel(table_name = crate::schema::files)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct File {
    pub id: Uuid,
    pub chat_id: i64,
    pub owner_id: Option<i64>,
    pub parent_id: Uuid,
    pub name: String,
    pub group_read: bool,
    pub group_write: bool,
    pub created_at: OffsetDateTime,
    pub group_execute: bool,
    pub user_read: bool,
    pub user_write: bool,
    pub user_execute: bool,
    pub others_read: bool,
    pub others_write: bool,
    pub others_execute: bool,
    pub last_modified_at: OffsetDateTime,
    pub is_dir: bool,
}

impl File {
    pub fn get_root_dir(cn: &mut PgConnection, chat_id: ChatId) -> QueryResult<File> {
        files::files
            .select(File::as_select())
            .inner_join(chats::chats.on(chats::root_id.eq(files::id)))
            .filter(chats::id.eq(chat_id.0))
            .first(cn)
    }
    pub fn get(cn: &mut PgConnection, id: Uuid) -> QueryResult<File> {
        files::files
            .select(File::as_select())
            .filter(files::id.eq(id))
            .first(cn)
    }
    pub fn get_chat_id(cn: &mut PgConnection, id: Uuid) -> QueryResult<ChatId> {
        files::files
            .select(files::chat_id)
            .filter(files::id.eq(id))
            .first(cn)
            .map(ChatId)
    }
    pub fn try_get(cn: &mut PgConnection, id: Uuid) -> QueryResult<Option<File>> {
        Self::get(cn, id).optional()
    }
    pub fn find_by_name(
        cn: &mut PgConnection,
        parent_id: Uuid,
        name: &str,
    ) -> QueryResult<Option<File>> {
        files::files
            .select(File::as_select())
            .filter(files::name.eq(name).and(files::parent_id.eq(parent_id)))
            .first(cn)
            .optional()
    }
    pub fn list_children(cn: &mut PgConnection, parent_id: Uuid) -> QueryResult<Vec<File>> {
        files::files
            .select(File::as_select())
            .filter(files::parent_id.eq(parent_id))
            .filter(files::parent_id.ne(files::id))
            .order(files::name)
            .load(cn)
    }
    pub fn insert_dir(
        cn: &mut PgConnection,
        owner_id: Option<UserId>,
        parent_id: Uuid,
        name: &str,
    ) -> QueryResult<File> {
        let chat_id = Self::get_chat_id(cn, parent_id)?;
        insert_into(files::files)
            .values((
                files::chat_id.eq(chat_id.0),
                files::owner_id.eq(owner_id.map(|id| id.0 as i64)),
                files::parent_id.eq(parent_id),
                files::name.eq(name),
                files::user_read.eq(true),
                files::user_write.eq(true),
                files::user_execute.eq(true),
                files::group_read.eq(true),
                files::group_write.eq(false),
                files::group_execute.eq(true),
                files::others_read.eq(true),
                files::others_write.eq(false),
                files::others_execute.eq(true),
            ))
            .returning(File::as_select())
            .get_result(cn)
    }
    pub fn upsert_file(
        cn: &mut PgConnection,
        owner_id: Option<UserId>,
        parent_id: Uuid,
        name: &str,
        content: &[u8],
    ) -> QueryResult<File> {
        let chat_id = Self::get_chat_id(cn, parent_id)?;
        if Self::find_by_name(cn, parent_id, name)?.is_some() {
            update(
                files::files
                    .filter(files::parent_id.eq(parent_id))
                    .filter(files::name.eq(name)),
            )
            .set(files::content.eq(content))
            .returning(File::as_select())
            .get_result(cn)
        } else {
            insert_into(files::files)
                .values((
                    files::chat_id.eq(chat_id.0),
                    files::owner_id.eq(owner_id.map(|id| id.0 as i64)),
                    files::parent_id.eq(parent_id),
                    files::name.eq(name),
                    files::content.eq(content),
                ))
                .returning(File::as_select())
                .get_result(cn)
        }
    }
    pub fn set_all_read(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set((
                files::user_read.eq(p),
                files::group_read.eq(p),
                files::others_read.eq(p),
            ))
            .execute(cn)
    }
    pub fn set_all_write(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set((
                files::user_write.eq(p),
                files::group_write.eq(p),
                files::others_write.eq(p),
            ))
            .execute(cn)
    }
    pub fn set_all_execute(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set((
                files::user_execute.eq(p),
                files::group_execute.eq(p),
                files::others_execute.eq(p),
            ))
            .execute(cn)
    }
    pub fn set_user_read(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::user_read.eq(p))
            .execute(cn)
    }
    pub fn set_user_write(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::user_write.eq(p))
            .execute(cn)
    }
    pub fn set_user_execute(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::user_execute.eq(p))
            .execute(cn)
    }
    pub fn set_group_read(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::group_read.eq(p))
            .execute(cn)
    }
    pub fn set_group_write(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::group_write.eq(p))
            .execute(cn)
    }
    pub fn set_group_execute(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::group_execute.eq(p))
            .execute(cn)
    }
    pub fn set_others_read(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::others_read.eq(p))
            .execute(cn)
    }
    pub fn set_others_write(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::others_write.eq(p))
            .execute(cn)
    }
    pub fn set_others_execute(cn: &mut PgConnection, id: Uuid, p: bool) -> QueryResult<usize> {
        update(files::files.filter(files::id.eq(id)))
            .set(files::others_execute.eq(p))
            .execute(cn)
    }
    pub fn delete(cn: &mut PgConnection, id: Uuid) -> QueryResult<usize> {
        delete(files::files.filter(files::id.eq(id))).execute(cn)
    }
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }
    pub fn is_file(&self) -> bool {
        !self.is_dir
    }
    pub fn can_read(&self, cn: &mut PgConnection, user_id: UserId) -> QueryResult<bool> {
        if self.owner_id == Some(user_id.0 as i64) {
            Ok(self.user_read)
        } else if super::UserInChat::exists(cn, user_id, ChatId(self.chat_id))? {
            Ok(self.group_read)
        } else {
            Ok(self.others_read)
        }
    }
    pub fn can_write(&self, cn: &mut PgConnection, user_id: UserId) -> QueryResult<bool> {
        if self.owner_id == Some(user_id.0 as i64) {
            Ok(self.user_write)
        } else if super::UserInChat::exists(cn, user_id, ChatId(self.chat_id))? {
            Ok(self.group_write)
        } else {
            Ok(self.others_write)
        }
    }
    pub fn can_execute(&self, cn: &mut PgConnection, user_id: UserId) -> QueryResult<bool> {
        if self.owner_id == Some(user_id.0 as i64) {
            Ok(self.user_execute)
        } else if super::UserInChat::exists(cn, user_id, ChatId(self.chat_id))? {
            Ok(self.group_execute)
        } else {
            Ok(self.others_execute)
        }
    }
    pub fn read(&self, cn: &mut PgConnection) -> QueryResult<Vec<u8>> {
        files::files
            .select(files::content)
            .filter(files::id.eq(self.id))
            .first(cn)
            .map(|opt: Option<Vec<u8>>| opt.unwrap_or_default())
    }
}
