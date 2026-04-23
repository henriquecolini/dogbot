use crate::chat_address::ChatAddress;
use crate::schema::chats::dsl as chats;
use crate::schema::files::dsl as files;
use diesel::*;
use teloxide::prelude::*;
use time::OffsetDateTime;
use uuid::Uuid;

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
        id: ChatId,
        name: Option<&str>,
        is_group: bool,
    ) -> QueryResult<()> {
        cn.transaction(|cn| {
            let existing = chats::chats
                .filter(chats::id.eq(id.0))
                .select(chats::root_id)
                .first::<Uuid>(cn)
                .optional()?;

            match existing {
                Some(_) => {
                    update(chats::chats.filter(chats::id.eq(id.0)))
                        .set((chats::name.eq(name), chats::is_group.eq(is_group)))
                        .execute(cn)?;
                }
                None => {
                    let root_id = Uuid::new_v4();

                    insert_into(chats::chats)
                        .values((
                            chats::id.eq(id.0),
                            chats::name.eq(name),
                            chats::is_group.eq(is_group),
                            chats::root_id.eq(root_id),
                        ))
                        .execute(cn)?;

                    insert_into(files::files)
                        .values((
                            files::id.eq(root_id),
                            files::chat_id.eq(id.0),
                            files::parent_id.eq(root_id),
                            files::name.eq(""),
                            files::user_read.eq(true),
                            files::user_write.eq(true),
                            files::user_execute.eq(true),
                            files::group_read.eq(true),
                            files::group_write.eq(true),
                            files::group_execute.eq(true),
                            files::others_read.eq(true),
                            files::others_write.eq(false),
                            files::others_execute.eq(true),
                        ))
                        .execute(cn)?;
                }
            }

            Ok(())
        })
    }
    pub fn set_alias(cn: &mut PgConnection, id: ChatId, alias: Option<&str>) -> QueryResult<()> {
        update(chats::chats.filter(chats::id.eq(id.0)))
            .set(chats::alias.eq(&alias))
            .execute(cn)?;
        Ok(())
    }
    pub fn get(cn: &mut PgConnection, id: ChatId) -> QueryResult<Chat> {
        Chat::query().filter(chats::id.eq(id.0)).get_result(cn)
    }
    pub fn find(cn: &mut PgConnection, address: ChatAddress<'_>) -> QueryResult<Option<Chat>> {
        match address {
            ChatAddress::Id(id) => Chat::query()
                .filter(chats::id.eq(id.0))
                .get_result(cn)
                .optional(),
            ChatAddress::Alias(alias) => Chat::query()
                .filter(chats::alias.eq(alias))
                .get_result(cn)
                .optional(),
        }
    }
    pub fn id(&self) -> ChatId {
        ChatId(self.id)
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
