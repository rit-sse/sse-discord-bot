use anyhow::{Context, Result};
use sqlx::{FromRow, PgPool};

use crate::domain::verification::{DisplayName, EmailAddress, VerificationCode};

const MAX_FAILED_ATTEMPTS: i16 = 5;

#[derive(Debug, FromRow)]
struct PendingVerificationRow {
    email: String,
    display_name: Option<String>,
    code_hash: String,
    failed_attempts: i16,
    expired: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartPendingVerificationResult {
    Created,
    Reused,
}

#[derive(Debug)]
pub enum CheckPendingVerificationResult {
    Accepted {
        email: EmailAddress,
        display_name: DisplayName,
    },
    Missing,
    Expired,
    Incorrect {
        attempts_remaining: u8,
        has_attempts_remaining: bool,
    },
}

pub async fn start_or_reuse(
    pool: &PgPool,
    user_id: u64,
    email: &EmailAddress,
    display_name: &DisplayName,
    code_hash: &str,
) -> Result<StartPendingVerificationResult> {
    let user_id = i64::try_from(user_id)?;

    let result = sqlx::query(
        r#"
        INSERT INTO pending_verification_attempts (
            discord_user_id,
            email,
            display_name,
            code_hash,
            expires_at
        )
        VALUES ($1, $2, $3, $4, NOW() + INTERVAL '1 hour')
        ON CONFLICT (discord_user_id)
        DO UPDATE SET
            email = EXCLUDED.email,
            display_name = EXCLUDED.display_name,
            code_hash = EXCLUDED.code_hash,
            expires_at = EXCLUDED.expires_at,
            failed_attempts = 0,
            created_at = NOW()
        WHERE pending_verification_attempts.expires_at <= NOW()
           OR pending_verification_attempts.display_name IS NULL
        "#,
    )
    .bind(user_id)
    .bind(email.to_string())
    .bind(display_name.as_str())
    .bind(code_hash)
    .execute(pool)
    .await
    .context("failed to start pending verification")?;

    if result.rows_affected() == 1 {
        Ok(StartPendingVerificationResult::Created)
    } else {
        Ok(StartPendingVerificationResult::Reused)
    }
}

pub async fn delete_if_code_matches(pool: &PgPool, user_id: u64, code_hash: &str) -> Result<()> {
    let user_id = i64::try_from(user_id)?;

    sqlx::query(
        r#"
        DELETE FROM pending_verification_attempts
        WHERE discord_user_id = $1 AND code_hash = $2
        "#,
    )
    .bind(user_id)
    .bind(code_hash)
    .execute(pool)
    .await
    .context("failed to delete pending verification")?;

    Ok(())
}

pub async fn check_code(
    pool: &PgPool,
    user_id: u64,
    submitted_code: &str,
) -> Result<CheckPendingVerificationResult> {
    let user_id = i64::try_from(user_id)?;
    let mut transaction = pool
        .begin()
        .await
        .context("failed to begin verification transaction")?;

    let row = sqlx::query_as::<_, PendingVerificationRow>(
        r#"
        SELECT
            email,
            display_name,
            code_hash,
            failed_attempts,
            expires_at <= NOW() AS expired
        FROM pending_verification_attempts
        WHERE discord_user_id = $1
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await
    .context("failed to retrieve pending verification")?;

    let Some(row) = row else {
        transaction
            .commit()
            .await
            .context("failed to finish missing verification check")?;
        return Ok(CheckPendingVerificationResult::Missing);
    };

    if row.expired {
        delete_attempt(&mut transaction, user_id).await?;
        transaction
            .commit()
            .await
            .context("failed to consume expired verification")?;
        return Ok(CheckPendingVerificationResult::Expired);
    }

    let Some(stored_display_name) = row.display_name else {
        delete_attempt(&mut transaction, user_id).await?;
        transaction
            .commit()
            .await
            .context("failed to consume legacy verification attempt")?;
        return Ok(CheckPendingVerificationResult::Missing);
    };

    if VerificationCode::verify_hash(&row.code_hash, submitted_code)? {
        let email =
            EmailAddress::parse(&row.email).context("stored verification email is invalid")?;
        let display_name = DisplayName::parse(&stored_display_name)
            .context("stored verification display name is invalid")?;

        sqlx::query(
            r#"
            INSERT INTO verified_identities (
                discord_user_id,
                email,
                display_name,
                display_name_confirmed
            )
            VALUES ($1, $2, $3, TRUE)
            ON CONFLICT (discord_user_id)
            DO UPDATE SET
                email = EXCLUDED.email,
                display_name = EXCLUDED.display_name,
                display_name_confirmed = TRUE,
                verified_at = NOW()
            "#,
        )
        .bind(user_id)
        .bind(email.to_string())
        .bind(display_name.as_str())
        .execute(&mut *transaction)
        .await
        .context("failed to persist accepted verified identity")?;

        delete_attempt(&mut transaction, user_id).await?;
        transaction
            .commit()
            .await
            .context("failed to commit accepted verification")?;

        return Ok(CheckPendingVerificationResult::Accepted {
            email,
            display_name,
        });
    }

    let failed_attempts = row.failed_attempts.saturating_add(1);
    let attempts_remaining = MAX_FAILED_ATTEMPTS.saturating_sub(failed_attempts) as u8;
    let has_attempts_remaining = failed_attempts < MAX_FAILED_ATTEMPTS;

    if has_attempts_remaining {
        sqlx::query(
            r#"
            UPDATE pending_verification_attempts
            SET failed_attempts = $2
            WHERE discord_user_id = $1
            "#,
        )
        .bind(user_id)
        .bind(failed_attempts)
        .execute(&mut *transaction)
        .await
        .context("failed to record unsuccessful verification attempt")?;
    } else {
        delete_attempt(&mut transaction, user_id).await?;
    }

    transaction
        .commit()
        .await
        .context("failed to commit unsuccessful verification attempt")?;

    Ok(CheckPendingVerificationResult::Incorrect {
        attempts_remaining,
        has_attempts_remaining,
    })
}

async fn delete_attempt(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: i64,
) -> Result<()> {
    sqlx::query(
        r#"
        DELETE FROM pending_verification_attempts
        WHERE discord_user_id = $1
        "#,
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await
    .context("failed to consume pending verification")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::verify_repository;

    fn email() -> EmailAddress {
        EmailAddress::parse("test@example.com").expect("test email should parse")
    }

    fn display_name() -> DisplayName {
        DisplayName::parse("Test Member").expect("test name should parse")
    }

    #[sqlx::test]
    async fn active_attempt_is_reused(pool: PgPool) -> Result<()> {
        let first_code = VerificationCode::generate();
        let first_hash = first_code.hash()?;
        let second_code = VerificationCode::generate();
        let second_hash = second_code.hash()?;

        let first = start_or_reuse(&pool, 1, &email(), &display_name(), &first_hash).await?;
        let second = start_or_reuse(&pool, 1, &email(), &display_name(), &second_hash).await?;
        let stored_hash = sqlx::query_scalar::<_, String>(
            "SELECT code_hash FROM pending_verification_attempts WHERE discord_user_id = 1",
        )
        .fetch_one(&pool)
        .await?;

        assert_eq!(first, StartPendingVerificationResult::Created);
        assert_eq!(second, StartPendingVerificationResult::Reused);
        assert_eq!(stored_hash, first_hash);
        Ok(())
    }

    #[sqlx::test]
    async fn accepted_code_records_identity_and_consumes_attempt(pool: PgPool) -> Result<()> {
        let code = VerificationCode::generate();
        let code_hash = code.hash()?;
        start_or_reuse(&pool, 1, &email(), &display_name(), &code_hash).await?;

        let result = check_code(&pool, 1, &code.to_string()).await?;
        let pending_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pending_verification_attempts WHERE discord_user_id = 1",
        )
        .fetch_one(&pool)
        .await?;
        let identity = verify_repository::find_user_by_id(&pool, 1).await?;

        assert!(matches!(
            result,
            CheckPendingVerificationResult::Accepted { .. }
        ));
        assert_eq!(pending_count, 0);
        let identity = identity.expect("identity should exist");
        assert_eq!(identity.email(), &email());
        assert_eq!(identity.display_name(), &display_name());
        assert!(identity.is_display_name_confirmed());
        Ok(())
    }

    #[sqlx::test]
    async fn failed_attempt_limit_survives_separate_checks(pool: PgPool) -> Result<()> {
        let code = VerificationCode::generate();
        let code_hash = code.hash()?;
        start_or_reuse(&pool, 1, &email(), &display_name(), &code_hash).await?;

        for attempts_remaining in (1..MAX_FAILED_ATTEMPTS as u8).rev() {
            let result = check_code(&pool, 1, "incorrect").await?;
            assert!(matches!(
                result,
                CheckPendingVerificationResult::Incorrect {
                    attempts_remaining: actual,
                    has_attempts_remaining: true,
                } if actual == attempts_remaining
            ));
        }

        let final_result = check_code(&pool, 1, "incorrect").await?;
        let missing_result = check_code(&pool, 1, "incorrect").await?;

        assert!(matches!(
            final_result,
            CheckPendingVerificationResult::Incorrect {
                attempts_remaining: 0,
                has_attempts_remaining: false,
            }
        ));
        assert!(matches!(
            missing_result,
            CheckPendingVerificationResult::Missing
        ));
        Ok(())
    }

    #[sqlx::test]
    async fn expired_attempt_is_consumed(pool: PgPool) -> Result<()> {
        let code = VerificationCode::generate();
        let code_hash = code.hash()?;
        start_or_reuse(&pool, 1, &email(), &display_name(), &code_hash).await?;
        sqlx::query(
            r#"
            UPDATE pending_verification_attempts
            SET expires_at = NOW() - INTERVAL '1 second'
            WHERE discord_user_id = 1
            "#,
        )
        .execute(&pool)
        .await?;

        let result = check_code(&pool, 1, &code.to_string()).await?;
        let pending_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pending_verification_attempts WHERE discord_user_id = 1",
        )
        .fetch_one(&pool)
        .await?;

        assert!(matches!(result, CheckPendingVerificationResult::Expired));
        assert_eq!(pending_count, 0);
        Ok(())
    }

    #[sqlx::test]
    async fn legacy_attempt_is_replaced_with_a_named_attempt(pool: PgPool) -> Result<()> {
        let legacy_code = VerificationCode::generate();
        let legacy_hash = legacy_code.hash()?;
        sqlx::query(
            r#"
            INSERT INTO pending_verification_attempts (
                discord_user_id,
                email,
                code_hash,
                expires_at
            )
            VALUES (1, 'test@example.com', $1, NOW() + INTERVAL '1 hour')
            "#,
        )
        .bind(legacy_hash)
        .execute(&pool)
        .await?;

        let replacement_code = VerificationCode::generate();
        let replacement_hash = replacement_code.hash()?;
        let result = start_or_reuse(&pool, 1, &email(), &display_name(), &replacement_hash).await?;
        let stored = sqlx::query_as::<_, (String, Option<String>)>(
            r#"
            SELECT code_hash, display_name
            FROM pending_verification_attempts
            WHERE discord_user_id = 1
            "#,
        )
        .fetch_one(&pool)
        .await?;

        assert_eq!(result, StartPendingVerificationResult::Created);
        assert_eq!(stored.0, replacement_hash);
        assert_eq!(stored.1.as_deref(), Some(display_name().as_str()));
        Ok(())
    }

    #[sqlx::test]
    async fn legacy_attempt_cannot_complete_without_a_display_name(pool: PgPool) -> Result<()> {
        let code = VerificationCode::generate();
        let code_hash = code.hash()?;
        sqlx::query(
            r#"
            INSERT INTO pending_verification_attempts (
                discord_user_id,
                email,
                code_hash,
                expires_at
            )
            VALUES (1, 'test@example.com', $1, NOW() + INTERVAL '1 hour')
            "#,
        )
        .bind(code_hash)
        .execute(&pool)
        .await?;

        let result = check_code(&pool, 1, &code.to_string()).await?;
        let pending_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pending_verification_attempts WHERE discord_user_id = 1",
        )
        .fetch_one(&pool)
        .await?;

        assert!(matches!(result, CheckPendingVerificationResult::Missing));
        assert_eq!(pending_count, 0);
        Ok(())
    }
}
