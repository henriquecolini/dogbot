use super::User;
use crate::messages::record::Record;
use diesel::*;
use std::collections::HashMap;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use time::OffsetDateTime;

use crate::schema::message_reactions::dsl as reactions;
use crate::schema::messages::dsl as messages;
use crate::schema::users::dsl as users;

#[derive(Queryable, Selectable, Identifiable, Associations, Debug, PartialEq, Clone)]
#[diesel(belongs_to(User))]
#[diesel(table_name = crate::schema::messages)]
pub struct Message {
    pub id: i64,
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub content: String,
    pub created_at: OffsetDateTime,
    pub kind: String,
    pub media: Option<serde_json::Value>,
    pub reply_to_id: Option<i64>,
    pub quote: Option<String>,
    pub forward_from: Option<String>,
    pub edited_at: Option<OffsetDateTime>,
    pub tag: Option<String>,
    pub trigger_id: Option<i64>,
}

/// Who sent a message and why, beyond what Telegram says of it.
#[derive(Clone, Debug, Default)]
pub struct Origin {
    /// `None` for the bot's own messages.
    pub user_id: Option<UserId>,
    /// When it was sent; now, if unknown.
    pub date: Option<OffsetDateTime>,
    /// For a bot message: the message whose run produced it.
    pub trigger: Option<MessageId>,
    /// For a bot message: what the script that sent it called it (`bot.tag`).
    pub tag: Option<String>,
}

/// A message with its author (`None` for the bot) and its reactions, each
/// emoji with how many people chose it, most chosen first.
pub type Row = (Message, Option<User>, Vec<(String, i64)>);

impl Message {
    pub fn insert(
        cn: &mut PgConnection,
        id: MessageId,
        chat_id: ChatId,
        record: &Record,
        origin: &Origin,
    ) -> QueryResult<()> {
        insert_into(messages::messages)
            .values((
                messages::id.eq(id.0 as i64),
                messages::chat_id.eq(chat_id.0),
                messages::user_id.eq(origin.user_id.map(|u| u.0 as i64)),
                messages::content.eq(&record.content),
                messages::created_at.eq(origin.date.unwrap_or_else(OffsetDateTime::now_utc)),
                messages::kind.eq(record.kind),
                messages::media.eq(&record.media),
                messages::reply_to_id.eq(record.reply_to_id),
                messages::quote.eq(&record.quote),
                messages::forward_from.eq(&record.forward_from),
                messages::tag.eq(&origin.tag),
                messages::trigger_id.eq(origin.trigger.map(|t| t.0 as i64)),
            ))
            // Telegram redelivers an update the bot didn't confirm before a
            // restart; the second copy is the same message.
            .on_conflict_do_nothing()
            .execute(cn)?;
        Ok(())
    }

    /// Whether the bot itself sent this message (bot messages have no user).
    pub fn sent_by_bot(cn: &mut PgConnection, id: MessageId, chat_id: ChatId) -> QueryResult<bool> {
        let found = messages::messages
            .find((id.0 as i64, chat_id.0))
            .select(messages::user_id)
            .first::<Option<i64>>(cn)
            .optional()?;
        Ok(matches!(found, Some(None)))
    }

    /// Forgets a message, so `history` stops listing one that was deleted.
    pub fn remove(cn: &mut PgConnection, id: MessageId, chat_id: ChatId) -> QueryResult<()> {
        delete(messages::messages.find((id.0 as i64, chat_id.0))).execute(cn)?;
        Ok(())
    }

    /// Records that the bot edited one of its messages, so `history` shows its
    /// current text. Not marked as edited: the bot rewriting its own progress
    /// message is not news to anyone.
    pub fn update_content(
        cn: &mut PgConnection,
        id: MessageId,
        chat_id: ChatId,
        content: &str,
    ) -> QueryResult<()> {
        update(messages::messages.find((id.0 as i64, chat_id.0)))
            .set(messages::content.eq(content))
            .execute(cn)?;
        Ok(())
    }

    /// Applies someone's edit: the new text or caption, and the new attachment
    /// if they replaced it. A message the history doesn't have (older than it
    /// reaches) is left alone.
    pub fn apply_edit(
        cn: &mut PgConnection,
        id: MessageId,
        chat_id: ChatId,
        record: &Record,
        edited_at: OffsetDateTime,
    ) -> QueryResult<usize> {
        update(messages::messages.find((id.0 as i64, chat_id.0)))
            .set((
                messages::content.eq(&record.content),
                messages::kind.eq(record.kind),
                messages::media.eq(&record.media),
                messages::edited_at.eq(Some(edited_at)),
            ))
            .execute(cn)
    }

    /// Keeps only the newest `limit` messages of a chat. Their reactions go
    /// with them.
    pub fn prune(cn: &mut PgConnection, chat_id: ChatId, limit: i64) -> QueryResult<usize> {
        let newest_dropped = messages::messages
            .filter(messages::chat_id.eq(chat_id.0))
            .select(messages::id)
            .order_by(messages::id.desc())
            .offset(limit.max(0))
            .limit(1)
            .first::<i64>(cn)
            .optional()?;
        let Some(cut) = newest_dropped else {
            return Ok(0);
        };
        delete(
            messages::messages
                .filter(messages::chat_id.eq(chat_id.0))
                .filter(messages::id.le(cut)),
        )
        .execute(cn)
    }

    /// One message, as `list` gives them.
    pub fn get(cn: &mut PgConnection, chat_id: ChatId, id: i64) -> QueryResult<Option<Row>> {
        let found = crate::schema::messages::table
            .left_join(crate::schema::users::table.on(users::id.nullable().eq(messages::user_id)))
            .filter(messages::chat_id.eq(chat_id.0))
            .filter(messages::id.eq(id))
            .select((Message::as_select(), Option::<User>::as_select()))
            .first::<(Message, Option<User>)>(cn)
            .optional()?;
        let Some((message, user)) = found else {
            return Ok(None);
        };
        let mut counts = Reaction::counts(cn, chat_id, &[message.id])?;
        let reactions = counts.remove(&message.id).unwrap_or_default();
        Ok(Some((message, user, reactions)))
    }

    /// The chat's messages, oldest first: the latest `count`, skipping the
    /// `offset` newest. In message order, which in a chat is the order they
    /// were sent, the bot's included.
    pub fn list(
        cn: &mut PgConnection,
        chat_id: ChatId,
        count: i64,
        offset: i64,
    ) -> QueryResult<Vec<Row>> {
        let mut found = crate::schema::messages::table
            .left_join(crate::schema::users::table.on(users::id.nullable().eq(messages::user_id)))
            .filter(messages::chat_id.eq(chat_id.0))
            .select((Message::as_select(), Option::<User>::as_select()))
            .order_by(messages::id.desc())
            .limit(count)
            .offset(offset)
            .load::<(Message, Option<User>)>(cn)?;
        found.reverse();
        let ids = found.iter().map(|(m, _)| m.id).collect::<Vec<_>>();
        let mut counts = Reaction::counts(cn, chat_id, &ids)?;
        Ok(found
            .into_iter()
            .map(|(m, u)| {
                let r = counts.remove(&m.id).unwrap_or_default();
                (m, u, r)
            })
            .collect())
    }
}

pub struct Reaction;

impl Reaction {
    /// Sets what `user_id` is reacting to a message with, replacing what they
    /// had. Nothing happens for a message the history doesn't have.
    pub fn replace(
        cn: &mut PgConnection,
        chat_id: ChatId,
        message_id: MessageId,
        user_id: UserId,
        emojis: &[String],
    ) -> QueryResult<()> {
        cn.transaction(|cn| {
            let known = messages::messages
                .find((message_id.0 as i64, chat_id.0))
                .select(messages::id)
                .first::<i64>(cn)
                .optional()?
                .is_some();
            if !known {
                return Ok(());
            }
            delete(
                reactions::message_reactions
                    .filter(reactions::chat_id.eq(chat_id.0))
                    .filter(reactions::message_id.eq(message_id.0 as i64))
                    .filter(reactions::user_id.eq(user_id.0 as i64)),
            )
            .execute(cn)?;
            let rows = emojis
                .iter()
                .map(|emoji| {
                    (
                        reactions::chat_id.eq(chat_id.0),
                        reactions::message_id.eq(message_id.0 as i64),
                        reactions::user_id.eq(user_id.0 as i64),
                        reactions::emoji.eq(emoji),
                    )
                })
                .collect::<Vec<_>>();
            if !rows.is_empty() {
                insert_into(reactions::message_reactions)
                    .values(rows)
                    .on_conflict_do_nothing()
                    .execute(cn)?;
            }
            Ok(())
        })
    }

    /// Each message's reactions, as emoji and how many chose it, most chosen
    /// first, then in the order they first appeared.
    pub fn counts(
        cn: &mut PgConnection,
        chat_id: ChatId,
        ids: &[i64],
    ) -> QueryResult<HashMap<i64, Vec<(String, i64)>>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let found = reactions::message_reactions
            .filter(reactions::chat_id.eq(chat_id.0))
            .filter(reactions::message_id.eq_any(ids))
            .group_by((reactions::message_id, reactions::emoji))
            .select((
                reactions::message_id,
                reactions::emoji,
                dsl::count_star(),
                dsl::min(reactions::created_at),
            ))
            .load::<(i64, String, i64, Option<OffsetDateTime>)>(cn)?;
        let mut by_message: HashMap<i64, Vec<(String, i64, Option<OffsetDateTime>)>> =
            HashMap::new();
        for (id, emoji, n, first) in found {
            by_message.entry(id).or_default().push((emoji, n, first));
        }
        Ok(by_message
            .into_iter()
            .map(|(id, mut list)| {
                list.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
                (id, list.into_iter().map(|(e, n, _)| (e, n)).collect())
            })
            .collect())
    }
}
