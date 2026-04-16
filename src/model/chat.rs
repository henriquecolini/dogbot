use crate::chat_address::ChatAddress;
use crate::schema::chats::dsl as chats;
use diesel::dsl::{exists, select};
use diesel::*;
use time::OffsetDateTime;

#[derive(HasQuery)]
#[diesel(table_name = crate::schema::chats)]
#[allow(unused)]
pub struct Chat {
    pub id: i64,
    pub name: Option<String>,
    pub alias: Option<String>,
    pub created_at: OffsetDateTime,
    pub is_group: bool,
}

impl Chat {
    pub fn upsert(
        cn: &mut PgConnection,
        id: i64,
        name: Option<&str>,
        is_group: bool,
    ) -> QueryResult<()> {
        insert_into(chats::chats)
            .values((
                chats::id.eq(id),
                chats::name.eq(&name),
                chats::is_group.eq(is_group),
            ))
            .on_conflict(chats::id)
            .do_update()
            .set((
                chats::id.eq(id),
                chats::name.eq(&name),
                chats::is_group.eq(is_group),
            ))
            .execute(cn)?;
        Ok(())
    }
    pub fn set_alias(cn: &mut PgConnection, id: i64, alias: Option<&str>) -> QueryResult<()> {
        update(chats::chats.filter(chats::id.eq(id)))
            .set(chats::alias.eq(&alias))
            .execute(cn)?;
        Ok(())
    }
    pub fn check_if_exists(cn: &mut PgConnection, id: i64) -> QueryResult<bool> {
        let exists =
            select(exists(chats::chats.filter(chats::id.eq(id)))).get_result::<bool>(cn)?;
        Ok(exists)
    }
    pub fn find(cn: &mut PgConnection, address: ChatAddress<'_>) -> QueryResult<Option<Chat>> {
        match address {
            ChatAddress::Id(id) => Chat::query()
                .filter(chats::id.eq(id))
                .get_result(cn)
                .optional(),
            ChatAddress::Alias(alias) => Chat::query()
                .filter(chats::alias.eq(alias))
                .get_result(cn)
                .optional(),
        }
    }
}

impl std::fmt::Display for Chat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(name) = &self.name {
            write!(f, "{}", name)
        } else if let Some(alias) = &self.alias {
            write!(f, "{}", alias)
        } else {
            write!(f, "{}", self.id)
        }
    }
}
