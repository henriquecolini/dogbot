CREATE TABLE users_in_chats
(
    user_id  BIGINT  NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    chat_id  BIGINT  NOT NULL REFERENCES chats (id) ON DELETE CASCADE,
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, chat_id)
);