DROP TRIGGER IF EXISTS trg_set_last_modified_at ON files;

DROP FUNCTION IF EXISTS set_files_last_modified_at();

ALTER TABLE files
    DROP COLUMN IF EXISTS last_modified_at;
