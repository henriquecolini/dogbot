use super::User;
use diesel::*;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use time::OffsetDateTime;

use crate::schema::messages::dsl as messages;
use crate::schema::users::dsl as users;

#[derive(Queryable, Selectable, Identifiable, Associations, Debug, PartialEq)]
#[diesel(belongs_to(User))]
#[diesel(table_name = crate::schema::messages)]
pub struct Message {
    pub id: i64,
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub content: String,
    pub created_at: OffsetDateTime,
}

impl Message {
    pub fn insert(
        cn: &mut PgConnection,
        id: MessageId,
        chat_id: ChatId,
        user_id: Option<UserId>,
        content: &str,
    ) -> QueryResult<()> {
        insert_into(messages::messages)
            .values((
                messages::id.eq(id.0 as i64),
                messages::chat_id.eq(chat_id.0),
                messages::user_id.eq(user_id.map(|u| u.0 as i64)),
                messages::content.eq(content),
            ))
            .execute(cn)?;
        Ok(())
    }
    pub fn list(
        cn: &mut PgConnection,
        chat_id: ChatId,
        count: i64,
        offset: i64,
    ) -> QueryResult<Vec<(Message, Option<User>)>> {
        crate::schema::messages::table
            .left_join(crate::schema::users::table.on(users::id.nullable().eq(messages::user_id)))
            .filter(messages::chat_id.eq(chat_id.0))
            .select((Message::as_select(), Option::<User>::as_select()))
            .order_by(messages::created_at.desc())
            .limit(count)
            .offset(offset)
            .load::<(Message, Option<User>)>(cn)
            .and_then(|mut m| {
                m.reverse();
                Ok(m)
            })
    }
}
