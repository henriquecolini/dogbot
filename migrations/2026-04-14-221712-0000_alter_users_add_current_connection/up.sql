ALTER TABLE users
    ADD COLUMN current_connection BIGINT;

ALTER TABLE users
    ADD CONSTRAINT users_connection_fkey
        FOREIGN KEY (id, current_connection)
            REFERENCES users_in_chats (user_id, chat_id)
            ON DELETE SET NULL ON UPDATE CASCADE;