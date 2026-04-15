CREATE TABLE users
(
    id         BIGINT PRIMARY KEY NOT NULL,
    first_name TEXT               NOT NULL,
    last_name  TEXT,
    username   TEXT UNIQUE,
    created_at TIMESTAMPTZ        NOT NULL DEFAULT NOW()
);