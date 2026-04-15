ALTER TABLE files
    DROP CONSTRAINT files_chat_id_parent_id_name_key;

CREATE UNIQUE INDEX files_unique_non_root
    ON files (chat_id, parent_id, name)
    WHERE parent_id IS NOT NULL;

CREATE UNIQUE INDEX files_unique_root
    ON files (chat_id, name)
    WHERE parent_id IS NULL;