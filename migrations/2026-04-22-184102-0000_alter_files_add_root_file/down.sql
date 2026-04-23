-- 1. Drop FK constraint
ALTER TABLE chats
    DROP CONSTRAINT IF EXISTS chats_root_id_fkey;

-- 2. Allow NULL parents again
ALTER TABLE files
    ALTER COLUMN parent_id DROP NOT NULL;

-- 3. Move root children back to NULL (restore old "root-level" semantics)
UPDATE files f
SET parent_id = NULL
FROM chats c
WHERE f.parent_id = c.root_id
  AND f.id <> c.root_id;

-- 4. Delete root directories
DELETE
FROM files f
    USING chats c
WHERE f.id = c.root_id
  AND f.parent_id = f.id;

-- 5. Drop root uniqueness index
DROP INDEX IF EXISTS files_unique_root;

-- 6. Remove root_id constraint and column
ALTER TABLE chats
    ALTER COLUMN root_id DROP NOT NULL;

ALTER TABLE chats
    DROP COLUMN root_id;