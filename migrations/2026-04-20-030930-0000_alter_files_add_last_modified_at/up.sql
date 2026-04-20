ALTER TABLE files
    ADD COLUMN last_modified_at timestamptz NOT NULL DEFAULT now();

UPDATE files
SET last_modified_at = created_at;

CREATE OR REPLACE FUNCTION set_files_last_modified_at()
    RETURNS trigger AS
$$
BEGIN
    NEW.last_modified_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_set_last_modified_at
    BEFORE UPDATE
    ON files
    FOR EACH ROW
EXECUTE FUNCTION set_files_last_modified_at();