CREATE TABLE pending_verification_attempts (
    discord_user_id BIGINT PRIMARY KEY,
    email TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    failed_attempts SMALLINT NOT NULL DEFAULT 0
        CHECK (failed_attempts >= 0 AND failed_attempts <= 5),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX pending_verification_attempts_expires_at_idx
    ON pending_verification_attempts (expires_at);
