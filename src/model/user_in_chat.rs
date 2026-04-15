use diesel::dsl::exists;
use diesel::*;
use time::OffsetDateTime;

use crate::schema::users_in_chats::dsl as uic;

pub struct UserInChat {
    user_id: i64,
    chat_id: i64,
    is_admin: bool,
    created_at: OffsetDateTime,
}

impl UserInChat {
    pub fn upsert(cn: &mut PgConnection, user_id: i64, chat_id: i64) -> QueryResult<()> {
        insert_into(uic::users_in_chats)
            .values((uic::user_id.eq(user_id), uic::chat_id.eq(chat_id)))
            .on_conflict((uic::user_id, uic::chat_id))
            .do_update()
            .set((uic::user_id.eq(user_id), uic::chat_id.eq(chat_id)))
            .execute(cn)?;
        Ok(())
    }

    pub fn exists(cn: &mut PgConnection, user_id: i64, chat_id: i64) -> QueryResult<bool> {
        select(exists(uic::users_in_chats.filter(
            uic::user_id.eq(user_id).and(uic::chat_id.eq(chat_id)),
        )))
        .get_result(cn)
    }
}
