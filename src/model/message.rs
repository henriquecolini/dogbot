use diesel::*;
use time::OffsetDateTime;

use crate::schema::messages::dsl as messages;

pub struct Message {
    id: i64,
    chat_id: i64,
    user_id: i64,
    content: String,
    created_at: OffsetDateTime,
}

impl Message {
    pub fn insert(cn: &mut PgConnection, id: i64, chat_id: i64, user_id: i64, content: &str) -> QueryResult<()> {
        insert_into(messages::messages)
            .values((
                messages::id.eq(id),
                messages::chat_id.eq(chat_id),
                messages::user_id.eq(user_id),
                messages::content.eq(content),
            ))
            .execute(cn)?;
        Ok(())
    }
}