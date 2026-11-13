//! What the history keeps of a Telegram message: one place that reads every
//! kind of message, so that a sticker, a poll or someone joining reaches the
//! scripts as what it is instead of being dropped.
//!
//! A reply is kept as the id of the message it answers, never its text: that
//! message can be edited afterwards, and the history has the current version.

use serde_json::{Map, Value, json};
use teloxide::types::{
    Chat, ExternalReplyInfoKind, MaybeInaccessibleMessage, MediaKind, Message, MessageKind,
    MessageOrigin, PhotoSize, Sticker, User,
};

/// A message as the history stores it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    /// `text`, `photo`, `sticker`, `document`, `video`, `animation`, `audio`,
    /// `voice`, `video_note`, `poll`, `location`, `venue`, `contact`, `dice`,
    /// `service` or `other`.
    pub kind: &'static str,
    /// The text, or a media message's caption. Empty when there is neither.
    pub content: String,
    /// What else there is to know, by kind: the file, the sticker's emoji, the
    /// poll's options, the service event.
    pub media: Option<Value>,
    /// The message this one replies to, in the same chat.
    pub reply_to_id: Option<i64>,
    /// The part of the replied message that was quoted, or the text of a
    /// message from another chat this one replies to.
    pub quote: Option<String>,
    /// Who wrote a forwarded message originally.
    pub forward_from: Option<String>,
}

impl Record {
    /// A service message (someone joined, a message was pinned) is history
    /// but not something anyone said: `main.dog` doesn't run for it.
    pub fn is_service(&self) -> bool {
        self.kind == "service"
    }

    /// A row for a message the bot sent with nothing more than its text.
    pub fn text(content: impl Into<String>) -> Record {
        Record {
            kind: "text",
            content: content.into(),
            ..Record::default()
        }
    }
}

/// Reads a message. `None` for the kinds history has no use for: payments,
/// forum topics, video chats and the like.
pub fn describe(msg: &Message) -> Option<Record> {
    match &msg.kind {
        MessageKind::Common(common) => {
            let (kind, content, media) = media(&common.media_kind);
            let reply_to_id = msg
                .reply_to_message()
                .filter(|replied| !is_topic_root(replied))
                .map(|replied| replied.id.0 as i64);
            let quote = common.quote.as_ref().map(|q| q.text.clone()).or_else(|| {
                // A reply to a message in another chat has no id here, so
                // what it answers is kept as text.
                common.external_reply.as_ref().map(|ext| {
                    let from = origin_name(&ext.origin);
                    let what = external_kind(&ext.kind);
                    match what {
                        Some(what) => format!("{from}: [{what}]"),
                        None => from,
                    }
                })
            });
            Some(Record {
                kind,
                content,
                media,
                reply_to_id,
                quote,
                forward_from: common.forward_origin.as_ref().map(origin_name),
            })
        }
        MessageKind::Dice(dice) => Some(Record {
            kind: "dice",
            media: Some(json!({
                "emoji": serde_json::to_value(dice.dice.emoji).unwrap_or(Value::Null),
                "value": dice.dice.value,
            })),
            ..Record::default()
        }),
        MessageKind::NewChatMembers(m) => Some(service(json!({
            "event": "join",
            "users": m.new_chat_members.iter().map(User::full_name).collect::<Vec<_>>(),
        }))),
        MessageKind::LeftChatMember(m) => Some(service(json!({
            "event": "leave",
            "user": m.left_chat_member.full_name(),
        }))),
        MessageKind::NewChatTitle(m) => Some(service(json!({
            "event": "title",
            "title": m.new_chat_title,
        }))),
        MessageKind::NewChatPhoto(_) => Some(service(json!({ "event": "photo" }))),
        MessageKind::DeleteChatPhoto(_) => Some(service(json!({ "event": "photo_removed" }))),
        MessageKind::Pinned(m) => {
            let pinned = match m.pinned.as_ref() {
                MaybeInaccessibleMessage::Regular(msg) => msg.id,
                MaybeInaccessibleMessage::Inaccessible(msg) => msg.message_id,
            };
            Some(Record {
                reply_to_id: Some(pinned.0 as i64),
                ..service(json!({ "event": "pin" }))
            })
        }
        _ => None,
    }
}

fn service(media: Value) -> Record {
    Record {
        kind: "service",
        media: Some(media),
        ..Record::default()
    }
}

/// In a forum, every message "replies" to the message that opened its topic.
/// That isn't a reply anyone made.
fn is_topic_root(msg: &Message) -> bool {
    matches!(msg.kind, MessageKind::ForumTopicCreated(_))
}

/// The name a forwarded message is credited to.
fn origin_name(origin: &MessageOrigin) -> String {
    match origin {
        MessageOrigin::User { sender_user, .. } => sender_user.full_name(),
        MessageOrigin::HiddenUser {
            sender_user_name, ..
        } => sender_user_name.clone(),
        MessageOrigin::Chat { sender_chat, .. } => chat_name(sender_chat),
        MessageOrigin::Channel { chat, .. } => chat_name(chat),
    }
}

fn chat_name(chat: &Chat) -> String {
    chat.title()
        .map(str::to_owned)
        .or_else(|| chat.username().map(|u| format!("@{u}")))
        .unwrap_or_default()
}

fn external_kind(kind: &ExternalReplyInfoKind) -> Option<&'static str> {
    use ExternalReplyInfoKind as K;
    Some(match kind {
        K::Photo(_) => "photo",
        K::Sticker(_) => "sticker",
        K::Video(_) => "video",
        K::Animation(_) => "animation",
        K::Audio(_) => "audio",
        K::Voice(_) => "voice",
        K::VideoNote(_) => "video_note",
        K::Document(_) => "document",
        K::Poll(_) => "poll",
        K::Location(_) | K::Venue(_) => "location",
        K::Contact(_) => "contact",
        _ => return None,
    })
}

/// A file's identifiers and size, the fields every attachment has.
fn file(meta: &teloxide::types::FileMeta) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("file_id".into(), json!(meta.id.0));
    m.insert("file_unique_id".into(), json!(meta.unique_id.0));
    // Telegram leaves the size out now and then; teloxide reads that as u32::MAX.
    if meta.size != u32::MAX {
        m.insert("file_size".into(), json!(meta.size));
    }
    m
}

fn put(m: &mut Map<String, Value>, key: &str, value: impl Into<Value>) {
    let value = value.into();
    if !value.is_null() {
        m.insert(key.into(), value);
    }
}

fn opt<T: Into<Value>>(value: Option<T>) -> Value {
    value.map(Into::into).unwrap_or(Value::Null)
}

/// The thumbnail's file id, which is what `bot.image` shows of a video or an
/// animated sticker.
fn thumb(m: &mut Map<String, Value>, thumb: Option<&PhotoSize>) {
    if let Some(t) = thumb {
        put(m, "thumb_id", t.file.id.0.clone());
    }
}

/// The size worth showing a vision model: the smallest whose longer side is at
/// least this, or the largest there is.
const PREVIEW_SIDE: u32 = 768;

fn photo(sizes: &[PhotoSize]) -> Map<String, Value> {
    let largest = sizes
        .iter()
        .max_by_key(|p| p.width as u64 * p.height as u64);
    let Some(largest) = largest else {
        return Map::new();
    };
    let mut m = file(&largest.file);
    put(&mut m, "width", largest.width);
    put(&mut m, "height", largest.height);
    let preview = sizes
        .iter()
        .filter(|p| p.width.max(p.height) >= PREVIEW_SIDE)
        .min_by_key(|p| p.width.max(p.height))
        .unwrap_or(largest);
    put(&mut m, "preview_id", preview.file.id.0.clone());
    m
}

fn sticker(s: &Sticker) -> Map<String, Value> {
    let mut m = file(&s.file);
    put(&mut m, "emoji", opt(s.emoji.clone()));
    put(&mut m, "set_name", opt(s.set_name.clone()));
    let format = if s.flags.is_animated {
        "animated"
    } else if s.flags.is_video {
        "video"
    } else {
        "static"
    };
    put(&mut m, "format", format);
    put(&mut m, "width", s.width);
    put(&mut m, "height", s.height);
    thumb(&mut m, s.thumbnail.as_ref());
    m
}

fn mime(m: &mut Map<String, Value>, mime: Option<&impl std::fmt::Display>) {
    if let Some(mime) = mime {
        put(m, "mime_type", mime.to_string());
    }
}

/// The kind, caption and details of a common message.
fn media(kind: &MediaKind) -> (&'static str, String, Option<Value>) {
    let caption = |c: &Option<String>| c.clone().unwrap_or_default();
    let done = |kind, content, m: Map<String, Value>| (kind, content, Some(Value::Object(m)));
    match kind {
        MediaKind::Text(t) => ("text", t.text.clone(), None),
        MediaKind::Photo(p) => done("photo", caption(&p.caption), photo(&p.photo)),
        MediaKind::Sticker(s) => done("sticker", String::new(), sticker(&s.sticker)),
        MediaKind::Document(d) => {
            let mut m = file(&d.document.file);
            put(&mut m, "file_name", opt(d.document.file_name.clone()));
            mime(&mut m, d.document.mime_type.as_ref());
            thumb(&mut m, d.document.thumbnail.as_ref());
            done("document", caption(&d.caption), m)
        }
        MediaKind::Video(v) => {
            let mut m = file(&v.video.file);
            put(&mut m, "width", v.video.width);
            put(&mut m, "height", v.video.height);
            put(&mut m, "duration", v.video.duration.seconds());
            put(&mut m, "file_name", opt(v.video.file_name.clone()));
            mime(&mut m, v.video.mime_type.as_ref());
            thumb(&mut m, v.video.thumbnail.as_ref());
            done("video", caption(&v.caption), m)
        }
        MediaKind::Animation(a) => {
            let mut m = file(&a.animation.file);
            put(&mut m, "width", a.animation.width);
            put(&mut m, "height", a.animation.height);
            put(&mut m, "duration", a.animation.duration.seconds());
            put(&mut m, "file_name", opt(a.animation.file_name.clone()));
            mime(&mut m, a.animation.mime_type.as_ref());
            thumb(&mut m, a.animation.thumbnail.as_ref());
            done("animation", caption(&a.caption), m)
        }
        MediaKind::Audio(a) => {
            let mut m = file(&a.audio.file);
            put(&mut m, "duration", a.audio.duration.seconds());
            put(&mut m, "performer", opt(a.audio.performer.clone()));
            put(&mut m, "title", opt(a.audio.title.clone()));
            put(&mut m, "file_name", opt(a.audio.file_name.clone()));
            mime(&mut m, a.audio.mime_type.as_ref());
            thumb(&mut m, a.audio.thumbnail.as_ref());
            done("audio", caption(&a.caption), m)
        }
        MediaKind::Voice(v) => {
            let mut m = file(&v.voice.file);
            put(&mut m, "duration", v.voice.duration.seconds());
            mime(&mut m, v.voice.mime_type.as_ref());
            done("voice", caption(&v.caption), m)
        }
        MediaKind::VideoNote(v) => {
            let mut m = file(&v.video_note.file);
            put(&mut m, "duration", v.video_note.duration.seconds());
            thumb(&mut m, v.video_note.thumbnail.as_ref());
            done("video_note", String::new(), m)
        }
        MediaKind::Poll(p) => {
            let poll = &p.poll;
            let mut m = Map::new();
            put(&mut m, "question", poll.question.clone());
            put(
                &mut m,
                "options",
                poll.options
                    .iter()
                    .map(|o| o.text.clone())
                    .collect::<Vec<_>>(),
            );
            put(&mut m, "multiple", poll.allows_multiple_answers);
            put(&mut m, "anonymous", poll.is_anonymous);
            put(
                &mut m,
                "quiz",
                matches!(poll.poll_type, teloxide::types::PollType::Quiz),
            );
            done("poll", String::new(), m)
        }
        MediaKind::Location(l) => {
            let mut m = Map::new();
            put(&mut m, "latitude", l.location.latitude);
            put(&mut m, "longitude", l.location.longitude);
            put(&mut m, "live", l.location.live_period.is_some());
            done("location", String::new(), m)
        }
        MediaKind::Venue(v) => {
            let mut m = Map::new();
            put(&mut m, "title", v.venue.title.clone());
            put(&mut m, "address", v.venue.address.clone());
            put(&mut m, "latitude", v.venue.location.latitude);
            put(&mut m, "longitude", v.venue.location.longitude);
            done("venue", String::new(), m)
        }
        MediaKind::Contact(c) => {
            let c = &c.contact;
            let mut m = Map::new();
            let name = match &c.last_name {
                Some(last) => format!("{} {last}", c.first_name),
                None => c.first_name.clone(),
            };
            put(&mut m, "name", name);
            put(&mut m, "phone", c.phone_number.clone());
            done("contact", String::new(), m)
        }
        MediaKind::Story(_) => done("other", String::new(), type_only("story")),
        MediaKind::Game(_) => done("other", String::new(), type_only("game")),
        MediaKind::PaidMedia(_) => done("other", String::new(), type_only("paid_media")),
        MediaKind::Checklist(_) => done("other", String::new(), type_only("checklist")),
        MediaKind::Migration(_) => (
            "service",
            String::new(),
            Some(json!({ "event": "migration" })),
        ),
    }
}

fn type_only(what: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".into(), json!(what));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message as the Bot API sends it, with `extra` merged into a plain
    /// group text message.
    fn message(extra: Value) -> Message {
        let mut base = json!({
            "message_id": 50,
            "date": 1_700_000_000,
            "chat": { "id": -100, "type": "supergroup", "title": "grupo" },
            "from": { "id": 7, "is_bot": false, "first_name": "Ana" },
        });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(base).expect("a valid message")
    }

    fn file(id: &str) -> Value {
        json!({ "file_id": id, "file_unique_id": format!("u{id}"), "file_size": 1234 })
    }

    #[test]
    fn text_with_a_reply_and_a_quote() {
        let r = describe(&message(json!({
            "text": "concordo",
            "reply_to_message": {
                "message_id": 40, "date": 1_699_999_000,
                "chat": { "id": -100, "type": "supergroup", "title": "grupo" },
                "from": { "id": 8, "is_bot": false, "first_name": "Bruno" },
                "text": "pizza é bom"
            },
            "quote": { "text": "pizza", "position": 0 }
        })))
        .unwrap();
        assert_eq!(r.kind, "text");
        assert_eq!(r.content, "concordo");
        assert_eq!(r.reply_to_id, Some(40));
        assert_eq!(r.quote.as_deref(), Some("pizza"));
        assert_eq!(r.media, None);
    }

    #[test]
    fn a_sticker_keeps_its_emoji() {
        let mut s = file("st");
        let o = s.as_object_mut().unwrap();
        o.insert("type".into(), json!("regular"));
        o.insert("width".into(), json!(512));
        o.insert("height".into(), json!(512));
        o.insert("is_animated".into(), json!(false));
        o.insert("is_video".into(), json!(false));
        o.insert("emoji".into(), json!("😂"));
        o.insert("set_name".into(), json!("gatos"));
        let r = describe(&message(json!({ "sticker": s }))).unwrap();
        assert_eq!(r.kind, "sticker");
        let m = r.media.unwrap();
        assert_eq!(m["emoji"], "😂");
        assert_eq!(m["format"], "static");
        assert_eq!(m["file_id"], "st");
    }

    #[test]
    fn a_photo_keeps_the_largest_size_and_a_preview() {
        let size = |id: &str, w: u32, h: u32| {
            let mut f = file(id);
            f["width"] = json!(w);
            f["height"] = json!(h);
            f
        };
        let r = describe(&message(json!({
            "photo": [size("s", 90, 60), size("m", 800, 533), size("l", 1280, 853)],
            "caption": "olha isso"
        })))
        .unwrap();
        assert_eq!(r.kind, "photo");
        assert_eq!(r.content, "olha isso");
        let m = r.media.unwrap();
        assert_eq!(m["file_id"], "l");
        assert_eq!(m["preview_id"], "m");
        assert_eq!(m["width"], 1280);
    }

    #[test]
    fn a_document_keeps_its_name() {
        let mut d = file("doc");
        d["file_name"] = json!("relatorio.pdf");
        d["mime_type"] = json!("application/pdf");
        let r = describe(&message(json!({ "document": d }))).unwrap();
        assert_eq!(r.kind, "document");
        let m = r.media.unwrap();
        assert_eq!(m["file_name"], "relatorio.pdf");
        assert_eq!(m["mime_type"], "application/pdf");
        assert_eq!(m["file_size"], 1234);
    }

    #[test]
    fn a_poll_keeps_its_question_and_options() {
        let r = describe(&message(json!({ "poll": {
            "id": "p", "question": "pizza?", "options": [
                { "text": "sim", "voter_count": 0 }, { "text": "não", "voter_count": 0 }
            ],
            "total_voter_count": 0, "is_closed": false, "is_anonymous": true,
            "type": "regular", "allows_multiple_answers": false
        }})))
        .unwrap();
        assert_eq!(r.kind, "poll");
        let m = r.media.unwrap();
        assert_eq!(m["question"], "pizza?");
        assert_eq!(m["options"], json!(["sim", "não"]));
    }

    #[test]
    fn dice_keep_their_value() {
        let r = describe(&message(json!({ "dice": { "emoji": "🎲", "value": 5 } }))).unwrap();
        assert_eq!(r.kind, "dice");
        let m = r.media.unwrap();
        assert_eq!(m["emoji"], "🎲");
        assert_eq!(m["value"], 5);
    }

    #[test]
    fn a_forward_names_its_origin() {
        let r = describe(&message(json!({
            "text": "notícia",
            "forward_origin": { "type": "hidden_user", "date": 1_600_000_000, "sender_user_name": "Fulano" }
        })))
        .unwrap();
        assert_eq!(r.forward_from.as_deref(), Some("Fulano"));
    }

    #[test]
    fn service_messages() {
        let join = describe(&message(json!({ "new_chat_members": [
            { "id": 9, "is_bot": false, "first_name": "Carla", "last_name": "Dias" }
        ]})))
        .unwrap();
        assert!(join.is_service());
        assert_eq!(
            join.media.unwrap(),
            json!({ "event": "join", "users": ["Carla Dias"] })
        );

        let left = describe(&message(json!({ "left_chat_member":
            { "id": 9, "is_bot": false, "first_name": "Carla" }
        })))
        .unwrap();
        assert_eq!(left.media.unwrap()["event"], "leave");

        let title = describe(&message(json!({ "new_chat_title": "grupo novo" }))).unwrap();
        assert_eq!(title.media.unwrap()["title"], "grupo novo");

        let pin = describe(&message(json!({ "pinned_message": {
            "message_id": 33, "date": 1_699_000_000,
            "chat": { "id": -100, "type": "supergroup", "title": "grupo" },
            "from": { "id": 8, "is_bot": false, "first_name": "Bruno" },
            "text": "regras"
        }})))
        .unwrap();
        assert!(pin.is_service());
        assert_eq!(pin.reply_to_id, Some(33));
        assert_eq!(pin.media.unwrap()["event"], "pin");
    }
}
