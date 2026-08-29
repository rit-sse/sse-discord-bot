-- Keep the schema writable by the previously deployed verification image.
-- A later contract migration can restore these constraints once that image is
-- no longer a supported rollback target.
ALTER TABLE pending_verification_attempts
    ALTER COLUMN display_name DROP NOT NULL;

ALTER TABLE verified_identities
    ALTER COLUMN display_name DROP NOT NULL;
