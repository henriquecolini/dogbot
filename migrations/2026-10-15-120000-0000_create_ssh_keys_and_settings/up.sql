-- SSH public keys, used to authenticate SFTP sessions. A key identifies the
-- user; the SSH username identifies which chat's filesystem to mount.
CREATE TABLE ssh_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    fingerprint TEXT NOT NULL UNIQUE,
    public_key TEXT NOT NULL,
    label TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX ssh_keys_user_id_idx ON ssh_keys (user_id);

-- Small key/value store for process-level state that must survive restarts.
-- Currently just the SSH host key, kept here so no extra volume is needed.
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
