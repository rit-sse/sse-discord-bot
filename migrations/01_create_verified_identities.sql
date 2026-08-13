CREATE TABLE verified_identities (
    discord_user_id BIGINT PRIMARY KEY,
    email TEXT NOT NULL,
    verified_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
