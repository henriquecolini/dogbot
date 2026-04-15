CREATE TABLE chats
(
    id         BIGINT PRIMARY KEY NOT NULL,
    name       TEXT,
    alias      TEXT UNIQUE,
    created_at TIMESTAMPTZ        NOT NULL DEFAULT NOW(),
    is_group   BOOLEAN            NOT NULL DEFAULT FALSE
);