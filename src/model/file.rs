use crate::schema::files::dsl as files;
use diesel::*;
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
    pub others_read: bool,
    pub others_write: bool,
    pub content: Option<Vec<u8>>,
    pub created_at: OffsetDateTime,
}

impl File {
    pub fn find_by_name(
        cn: &mut PgConnection,
        chat_id: i64,
        parent_id: Option<Uuid>,
        name: &str,
    ) -> QueryResult<Option<File>> {
        File::query()
            .filter(
                files::chat_id.eq(chat_id).and(
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
    pub fn list_children(cn: &mut PgConnection, parent_id: Option<Uuid>) -> QueryResult<Vec<File>> {
        File::query()
            .filter(files::parent_id.is_not_distinct_from(parent_id))
            .load(cn)
    }
    pub fn create_dir(
        cn: &mut PgConnection,
        chat_id: i64,
        owner_id: Option<i64>,
        parent_id: Option<Uuid>,
        name: &str,
    ) -> QueryResult<File> {
        if let Some(file) = Self::find_by_name(cn, chat_id, parent_id, name)? {
            return Ok(file);
        }
        insert_into(files::files)
            .values((
                files::chat_id.eq(chat_id),
                files::owner_id.eq(owner_id),
                files::parent_id.eq(parent_id),
                files::name.eq(name),
            ))
            .get_result(cn)
    }
    pub fn create_file(
        cn: &mut PgConnection,
        chat_id: i64,
        owner_id: Option<i64>,
        parent_id: Option<Uuid>,
        name: &str,
        content: &[u8],
    ) -> QueryResult<File> {
        if Self::find_by_name(cn, chat_id, parent_id, name)?.is_some() {
            update(files::files.filter(files::name.eq(name)))
                .set(files::content.eq(content))
                .get_result(cn)
        } else {
            insert_into(files::files)
                .values((
                    files::chat_id.eq(chat_id),
                    files::owner_id.eq(owner_id),
                    files::parent_id.eq(parent_id),
                    files::name.eq(name),
                    files::content.eq(content),
                ))
                .get_result(cn)
        }
    }
    pub fn is_dir(&self) -> bool {
        self.content.is_none()
    }
    pub fn is_file(&self) -> bool {
        self.content.is_some()
    }
    pub fn can_read(&self, user_id: Option<i64>) -> bool {
        self.others_read || self.owner_id == user_id
    }
    pub fn can_write(&self, user_id: Option<i64>) -> bool {
        self.others_write || self.owner_id == user_id
    }
}
