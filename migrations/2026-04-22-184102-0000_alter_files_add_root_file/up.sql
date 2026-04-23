-- 1. Add root_id (temporary nullable for transition)
ALTER TABLE chats
    ADD COLUMN root_id UUID DEFAULT gen_random_uuid();

-- 2. Create root directories (one per chat, idempotent)
INSERT INTO files (id, chat_id, parent_id, name, user_read, user_write, user_execute, group_read, group_write,
                   group_execute, others_read, others_write, others_execute)
SELECT c.root_id, c.id, c.root_id, '', true, true, true, true, true, true, true, false, true
FROM chats c
ON CONFLICT (id) DO NOTHING;

-- 3. Reattach previous root-level files
UPDATE files f
SET parent_id = c.root_id
FROM chats c
WHERE f.parent_id IS NULL
  AND f.chat_id = c.id;

-- 4. Enforce exactly one root per chat
DROP INDEX IF EXISTS files_unique_root;
CREATE UNIQUE INDEX files_unique_root
    ON files (chat_id)
    WHERE parent_id = id;

-- 5. Disallow NULL parents (filesystem is now fully rooted)
ALTER TABLE files
    ALTER COLUMN parent_id SET NOT NULL;

-- 6. Add FK (must be DEFERRABLE for future inserts)
ALTER TABLE chats
    ADD CONSTRAINT chats_root_id_fkey
        FOREIGN KEY (root_id)
            REFERENCES files (id)
            ON DELETE RESTRICT
            ON UPDATE CASCADE
            DEFERRABLE INITIALLY DEFERRED;

-- 7. Enforce root_id presence
ALTER TABLE chats
    ALTER COLUMN root_id SET NOT NULL;
