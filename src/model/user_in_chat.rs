use diesel::dsl::exists;
use diesel::*;
use teloxide::prelude::*;
use time::OffsetDateTime;

use crate::schema::users_in_chats::dsl as uic;

#[allow(unused)]
pub struct UserInChat {
    user_id: i64,
    chat_id: i64,
    is_admin: bool,
    created_at: OffsetDateTime,
}

impl UserInChat {
    pub fn upsert(cn: &mut PgConnection, user_id: UserId, chat_id: ChatId) -> QueryResult<()> {
        insert_into(uic::users_in_chats)
            .values((
                uic::user_id.eq(user_id.0 as i64),
                uic::chat_id.eq(chat_id.0),
            ))
            .on_conflict((uic::user_id, uic::chat_id))
            .do_update()
            .set((
                uic::user_id.eq(user_id.0 as i64),
                uic::chat_id.eq(chat_id.0),
            ))
            .execute(cn)?;
        Ok(())
    }

    pub fn exists(cn: &mut PgConnection, user_id: UserId, chat_id: ChatId) -> QueryResult<bool> {
        select(exists(
            uic::users_in_chats.filter(
                uic::user_id
                    .eq(user_id.0 as i64)
                    .and(uic::chat_id.eq(chat_id.0)),
            ),
        ))
        .get_result(cn)
    }
}
