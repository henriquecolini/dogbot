ALTER TABLE files
    DROP COLUMN group_execute,
    DROP COLUMN user_read,
    DROP COLUMN user_write,
    DROP COLUMN user_execute,
    DROP COLUMN others_read,
    DROP COLUMN others_write,
    DROP COLUMN others_execute;

ALTER TABLE files
    RENAME COLUMN group_read TO others_read;
ALTER TABLE files
    RENAME COLUMN group_write TO others_write;
