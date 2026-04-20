use crate::schema::files::dsl as files;
use diesel::*;
use teloxide::prelude::*;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(HasQuery, Debug)]
#[diesel(table_name = crate::schema::files)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct File {
    pub id: Uuid,
    pub chat_id: i64,
    pub owner_id: Option<i64>,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub group_read: bool,
    pub group_write: bool,
    pub content: Option<Vec<u8>>,
    pub created_at: OffsetDateTime,
    pub group_execute: bool,
    pub user_read: bool,
    pub user_write: bool,
    pub user_execute: bool,
    pub others_read: bool,
    pub others_write: bool,
    pub others_execute: bool,
    pub last_modified_at: OffsetDateTime,
}

impl File {
    pub fn find_by_name(
        cn: &mut PgConnection,
        chat_id: ChatId,
        parent_id: Option<Uuid>,
        name: &str,
    ) -> QueryResult<Option<File>> {
        File::query()
            .filter(
                files::chat_id.eq(chat_id.0).and(
                    files::name
                        .eq(name)
                        .and(files::parent_id.is_not_distinct_from(parent_id)),
                ),
            )
            .first(cn)
            .optional()
    }
    pub fn find_by_id(cn: &mut PgConnection, id: Uuid) -> QueryResult<Option<File>> {
        File::query().filter(files::id.eq(id)).first(cn).optional()
    }
    pub fn list_children(
        cn: &mut PgConnection,
        chat_id: ChatId,
        parent_id: Option<Uuid>,
    ) -> QueryResult<Vec<File>> {
        File::query()
            .filter(
                files::chat_id
                    .eq(chat_id.0)
                    .and(files::parent_id.is_not_distinct_from(parent_id)),
            )
            .load(cn)
    }
    pub fn create_dir(
        cn: &mut PgConnection,
        chat_id: ChatId,
        owner_id: Option<UserId>,
        parent_id: Option<Uuid>,
        name: &str,
    ) -> QueryResult<File> {
        if let Some(file) = Self::find_by_name(cn, chat_id, parent_id, name)? {
            return Ok(file);
        }
        insert_into(files::files)
            .values((
                files::chat_id.eq(chat_id.0),
                files::owner_id.eq(owner_id.map(|id| id.0 as i64)),
                files::parent_id.eq(parent_id),
                files::name.eq(name),
            ))
            .get_result(cn)
    }
    pub fn create_file(
        cn: &mut PgConnection,
        chat_id: ChatId,
        owner_id: Option<UserId>,
        parent_id: Option<Uuid>,
        name: &str,
        content: &[u8],
    ) -> QueryResult<File> {
        if Self::find_by_name(cn, chat_id, parent_id, name)?.is_some() {
            update(
                files::files
                    .filter(files::chat_id.eq(chat_id.0))
                    .filter(files::parent_id.is_not_distinct_from(parent_id))
                    .filter(files::name.eq(name)),
            )
            .set(files::content.eq(content))
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
        self.content.is_none()
    }
    pub fn is_file(&self) -> bool {
        self.content.is_some()
    }
    pub fn can_read(&self, user_id: UserId) -> bool {
        self.others_read || self.owner_id == Some(user_id.0 as i64)
    }
    pub fn can_write(&self, user_id: UserId) -> bool {
        self.others_write || self.owner_id == Some(user_id.0 as i64)
    }
}
