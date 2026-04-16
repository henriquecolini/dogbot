ALTER TABLE files
    DROP CONSTRAINT files_parent_id_fkey;

ALTER TABLE files
    ADD CONSTRAINT files_parent_id_fkey
        FOREIGN KEY (parent_id)
            REFERENCES files(id)
            ON DELETE NO ACTION;