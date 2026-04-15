DROP INDEX files_unique_non_root;
DROP INDEX files_unique_root;

ALTER TABLE files
    ADD CONSTRAINT files_chat_id_parent_id_name_key
        UNIQUE (chat_id, parent_id, name);