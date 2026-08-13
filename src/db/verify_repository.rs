use anyhow::{Context, Result};
use sqlx::{PgPool, prelude::FromRow, types::time::OffsetDateTime};

use crate::domain::verification::{EmailAddress, VerifiedIdentity};

#[derive(FromRow)]
pub struct VerifiedIdentityRow {
    discord_user_id: i64,
    email: String,
    verified_at: OffsetDateTime,
}

pub async fn upsert(pool: &PgPool, user_id: u64, email: &EmailAddress) -> Result<()> {
    let user_id = i64::try_from(user_id).context("user_id is bigger than postgres BIGINT range")?;

    sqlx::query(
        r#"
        INSERT INTO verified_identities (discord_user_id, email) VALUES ($1, $2)
        ON CONFLICT (discord_user_id)
        DO UPDATE SET
            email = EXCLUDED.email,
            verified_at = NOW()
            "#,
    )
    .bind(user_id)
    .bind(email.to_string())
    .execute(pool)
    .await
    .context("failed to persist identity")?;

    Ok(())
}

pub async fn find_user_by_id(pool: &PgPool, user_id: u64) -> Result<Option<VerifiedIdentity>> {
    let user_id = i64::try_from(user_id).context("user_id extends Postgres BIGINT range")?;

    let row = sqlx::query_as::<_, VerifiedIdentityRow>(
        r#"
        SELECT discord_user_id, email, verified_at
        FROM verified_identities
        WHERE discord_user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("failed to retrieve verified identity")?;

    let Some(row) = row else {
        return Ok(None);
    };

    let user_id = u64::try_from(row.discord_user_id)
        .context("stored Discord user ID cannot be represented as u64")?;
    let email = EmailAddress::parse(&row.email).context("stored verified email is invalid")?;

    Ok(Some(VerifiedIdentity::from_persisted(
        user_id,
        email,
        row.verified_at.into(),
    )))
}
