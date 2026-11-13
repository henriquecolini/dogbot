-- What a member sees of a message, beyond its text: what kind it is, its
-- attachment, what it replies to, and whether it was edited. Also who caused a
-- bot message, and the tag a script gave it.
ALTER TABLE messages
    ADD COLUMN kind         TEXT        NOT NULL DEFAULT 'text',
    ADD COLUMN media        JSONB,
    ADD COLUMN reply_to_id  BIGINT,
    ADD COLUMN quote        TEXT,
    ADD COLUMN forward_from TEXT,
    ADD COLUMN edited_at    TIMESTAMPTZ,
    ADD COLUMN tag          TEXT,
    ADD COLUMN trigger_id   BIGINT;

-- History is read newest first, and pruned by id, per chat.
CREATE INDEX messages_chat_id_id ON messages (chat_id, id DESC);

-- One row per person per emoji on a message.
CREATE TABLE message_reactions
(
    chat_id    BIGINT      NOT NULL,
    message_id BIGINT      NOT NULL,
    user_id    BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    emoji      TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (chat_id, message_id, user_id, emoji),
    FOREIGN KEY (message_id, chat_id) REFERENCES messages (id, chat_id) ON DELETE CASCADE
);
