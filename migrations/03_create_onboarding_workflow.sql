CREATE TABLE onboarding_requests (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    discord_user_id BIGINT NOT NULL,
    requested_by_user_id BIGINT NOT NULL,
    verified_email TEXT NOT NULL,
    target_key TEXT NOT NULL,
    target_label TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'denied', 'provisioning', 'completed', 'failed')),
    decided_by_user_id BIGINT,
    decided_at TIMESTAMPTZ,
    review_channel_id BIGINT,
    review_message_id BIGINT,
    provisioning_attempts INTEGER NOT NULL DEFAULT 0
        CHECK (provisioning_attempts >= 0),
    provisioning_started_at TIMESTAMPTZ,
    authentik_user_id BIGINT,
    last_error TEXT,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    CHECK (
        (review_channel_id IS NULL AND review_message_id IS NULL)
        OR (review_channel_id IS NOT NULL AND review_message_id IS NOT NULL)
    ),
    CHECK (
        (status = 'pending' AND decided_by_user_id IS NULL AND decided_at IS NULL)
        OR (status <> 'pending' AND decided_by_user_id IS NOT NULL AND decided_at IS NOT NULL)
    ),
    CHECK (
        status <> 'provisioning'
        OR (provisioning_attempts > 0 AND provisioning_started_at IS NOT NULL)
    ),
    CHECK (
        status <> 'completed'
        OR (authentik_user_id IS NOT NULL AND completed_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX onboarding_requests_active_user_target_idx
    ON onboarding_requests (discord_user_id, target_key)
    WHERE status <> 'denied';

CREATE INDEX onboarding_requests_status_updated_at_idx
    ON onboarding_requests (status, updated_at DESC);

CREATE TABLE audit_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_type TEXT NOT NULL,
    actor_user_id BIGINT,
    target_user_id BIGINT,
    onboarding_request_id BIGINT REFERENCES onboarding_requests (id),
    target_key TEXT,
    outcome TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX audit_events_onboarding_request_created_at_idx
    ON audit_events (onboarding_request_id, created_at DESC);

CREATE INDEX audit_events_target_user_created_at_idx
    ON audit_events (target_user_id, created_at DESC);
