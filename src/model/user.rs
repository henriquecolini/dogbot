use diesel::*;
use time::OffsetDateTime;

use crate::schema::users::dsl as users;

#[derive(HasQuery, Debug)]
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
        id: i64,
        first_name: &str,
        last_name: Option<&str>,
        username: Option<&str>,
    ) -> QueryResult<()> {
        insert_into(users::users)
            .values((
                users::id.eq(id),
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

    pub fn set_connection(cn: &mut PgConnection, user_id: i64, chat_id: Option<i64>) -> QueryResult<()> {
        update(users::users.filter(users::id.eq(user_id)))
            .set(users::current_connection.eq(chat_id))
            .execute(cn)?;
        Ok(())
    }
    
    pub fn try_get(cn: &mut PgConnection, user_id: i64) -> QueryResult<Option<User>> {
        Self::get(cn, user_id).optional()
    }
    
    pub fn get(cn: &mut PgConnection, user_id: i64) -> QueryResult<User> {
        Self::query().filter(users::id.eq(user_id)).first(cn)        
    }
}
