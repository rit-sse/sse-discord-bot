ALTER TABLE pending_verification_attempts
    ADD COLUMN display_name TEXT;

DELETE FROM pending_verification_attempts;

ALTER TABLE pending_verification_attempts
    ALTER COLUMN display_name SET NOT NULL;

ALTER TABLE verified_identities
    ADD COLUMN display_name TEXT;

UPDATE verified_identities
SET display_name = LEFT(email, 100);

ALTER TABLE verified_identities
    ALTER COLUMN display_name SET NOT NULL;

ALTER TABLE verified_identities
    ADD COLUMN display_name_confirmed BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE onboarding_requests
    ADD COLUMN verified_display_name TEXT;

UPDATE onboarding_requests
SET verified_display_name = LEFT(verified_email, 100);

ALTER TABLE onboarding_requests
    ALTER COLUMN verified_display_name SET NOT NULL;
