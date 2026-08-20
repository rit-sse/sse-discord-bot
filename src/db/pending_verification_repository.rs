use anyhow::{Context, Result};
use sqlx::{FromRow, PgPool};

use crate::domain::verification::{EmailAddress, VerificationCode};

const MAX_FAILED_ATTEMPTS: i16 = 5;

#[derive(Debug, FromRow)]
struct PendingVerificationRow {
    email: String,
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
    code_hash: &str,
) -> Result<StartPendingVerificationResult> {
    let user_id = i64::try_from(user_id)?;

    let result = sqlx::query(
        r#"
        INSERT INTO pending_verification_attempts (
            discord_user_id,
            email,
            code_hash,
            expires_at
        )
        VALUES ($1, $2, $3, NOW() + INTERVAL '1 hour')
        ON CONFLICT (discord_user_id)
        DO UPDATE SET
            email = EXCLUDED.email,
            code_hash = EXCLUDED.code_hash,
            expires_at = EXCLUDED.expires_at,
            failed_attempts = 0,
            created_at = NOW()
        WHERE pending_verification_attempts.expires_at <= NOW()
        "#,
    )
    .bind(user_id)
    .bind(email.to_string())
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

    if VerificationCode::verify_hash(&row.code_hash, submitted_code)? {
        let email =
            EmailAddress::parse(&row.email).context("stored verification email is invalid")?;

        sqlx::query(
            r#"
            INSERT INTO verified_identities (discord_user_id, email)
            VALUES ($1, $2)
            ON CONFLICT (discord_user_id)
            DO UPDATE SET
                email = EXCLUDED.email,
                verified_at = NOW()
            "#,
        )
        .bind(user_id)
        .bind(email.to_string())
        .execute(&mut *transaction)
        .await
        .context("failed to persist accepted verified identity")?;

        delete_attempt(&mut transaction, user_id).await?;
        transaction
            .commit()
            .await
            .context("failed to commit accepted verification")?;

        return Ok(CheckPendingVerificationResult::Accepted { email });
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

    #[sqlx::test]
    async fn active_attempt_is_reused(pool: PgPool) -> Result<()> {
        let first_code = VerificationCode::generate();
        let first_hash = first_code.hash()?;
        let second_code = VerificationCode::generate();
        let second_hash = second_code.hash()?;

        let first = start_or_reuse(&pool, 1, &email(), &first_hash).await?;
        let second = start_or_reuse(&pool, 1, &email(), &second_hash).await?;
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
        start_or_reuse(&pool, 1, &email(), &code_hash).await?;

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
        assert_eq!(identity.expect("identity should exist").email(), &email());
        Ok(())
    }

    #[sqlx::test]
    async fn failed_attempt_limit_survives_separate_checks(pool: PgPool) -> Result<()> {
        let code = VerificationCode::generate();
        let code_hash = code.hash()?;
        start_or_reuse(&pool, 1, &email(), &code_hash).await?;

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
        start_or_reuse(&pool, 1, &email(), &code_hash).await?;
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
}
