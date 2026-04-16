use crate::model::user::User;
use crate::schema::messages::dsl as messages;
use crate::schema::users::dsl as users;
use diesel::*;
use time::OffsetDateTime;

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
        id: i64,
        chat_id: i64,
        user_id: Option<i64>,
        content: &str,
    ) -> QueryResult<()> {
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
    pub fn list(
        cn: &mut PgConnection,
        chat_id: i64,
        count: i64,
        offset: i64,
    ) -> QueryResult<Vec<(Message, Option<User>)>> {
        crate::schema::messages::table
            .left_join(crate::schema::users::table.on(users::id.nullable().eq(messages::user_id)))
            .filter(messages::chat_id.eq(chat_id))
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
