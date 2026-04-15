CREATE TABLE files
(
    id           UUID PRIMARY KEY NOT NULL DEFAULT gen_random_uuid(),
    chat_id      BIGINT           NOT NULL REFERENCES chats (id),
    owner_id     BIGINT REFERENCES users (id),
    parent_id    UUID REFERENCES files (id),
    name         TEXT             NOT NULL,
    others_read  BOOLEAN          NOT NULL DEFAULT FALSE,
    others_write BOOLEAN          NOT NULL DEFAULT FALSE,
    content      BYTEA,
    created_at   TIMESTAMPTZ      NOT NULL DEFAULT NOW(),
    UNIQUE (chat_id, parent_id, name)
);