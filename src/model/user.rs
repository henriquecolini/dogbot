use diesel::*;
use teloxide::prelude::*;
use time::OffsetDateTime;

use crate::schema::users::dsl as users;

#[derive(HasQuery, Identifiable, Debug, PartialEq)]
#[diesel(table_name = crate::schema::users)]
pub struct User {
    pub id: i64,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
    pub current_connection: Option<i64>,
    pub created_at: OffsetDateTime,
}

impl User {
    pub fn upsert(
        cn: &mut PgConnection,
        id: UserId,
        first_name: &str,
        last_name: Option<&str>,
        username: Option<&str>,
    ) -> QueryResult<()> {
        insert_into(users::users)
            .values((
                users::id.eq(id.0 as i64),
                users::first_name.eq(&first_name),
                users::last_name.eq(last_name),
                users::username.eq(username),
            ))
            .on_conflict(users::id)
            .do_update()
            .set((
                users::first_name.eq(&first_name),
                users::last_name.eq(last_name),
                users::username.eq(username),
            ))
            .execute(cn)?;
        Ok(())
    }

    pub fn set_connection(
        cn: &mut PgConnection,
        user_id: UserId,
        chat_id: Option<ChatId>,
    ) -> QueryResult<()> {
        update(users::users.filter(users::id.eq(user_id.0 as i64)))
            .set(users::current_connection.eq(chat_id.map(|c| c.0)))
            .execute(cn)?;
        Ok(())
    }

    pub fn try_get(cn: &mut PgConnection, user_id: UserId) -> QueryResult<Option<User>> {
        Self::get(cn, user_id).optional()
    }

    pub fn get(cn: &mut PgConnection, user_id: UserId) -> QueryResult<User> {
        Self::query()
            .filter(users::id.eq(user_id.0 as i64))
            .first(cn)
    }
}
