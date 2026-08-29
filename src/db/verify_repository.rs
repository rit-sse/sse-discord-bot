use anyhow::{Context, Result};
use sqlx::{PgPool, prelude::FromRow, types::time::OffsetDateTime};

use crate::domain::verification::{DisplayName, EmailAddress, VerifiedIdentity};

#[derive(FromRow)]
pub struct VerifiedIdentityRow {
    discord_user_id: i64,
    email: String,
    display_name: Option<String>,
    display_name_confirmed: bool,
    verified_at: OffsetDateTime,
}

pub async fn find_user_by_id(pool: &PgPool, user_id: u64) -> Result<Option<VerifiedIdentity>> {
    let user_id = i64::try_from(user_id).context("user_id extends Postgres BIGINT range")?;

    let row = sqlx::query_as::<_, VerifiedIdentityRow>(
        r#"
        SELECT discord_user_id, email, display_name, display_name_confirmed, verified_at
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
    let (display_name, display_name_confirmed) = match row.display_name {
        Some(display_name) => (
            DisplayName::parse(&display_name).context("stored verified display name is invalid")?,
            row.display_name_confirmed,
        ),
        None => (
            DisplayName::parse("Unconfirmed member")
                .expect("the legacy display-name placeholder must remain valid"),
            false,
        ),
    };

    Ok(Some(VerifiedIdentity::from_persisted(
        user_id,
        email,
        display_name,
        display_name_confirmed,
        row.verified_at.into(),
    )))
}

pub async fn confirm_display_name(
    pool: &PgPool,
    user_id: u64,
    email: &EmailAddress,
    display_name: &DisplayName,
) -> Result<bool> {
    let user_id = i64::try_from(user_id).context("user_id extends Postgres BIGINT range")?;

    let result = sqlx::query(
        r#"
        UPDATE verified_identities
        SET display_name = $3,
            display_name_confirmed = TRUE
        WHERE discord_user_id = $1
          AND email = $2
        "#,
    )
    .bind(user_id)
    .bind(email.to_string())
    .bind(display_name.as_str())
    .execute(pool)
    .await
    .context("failed to confirm verified identity display name")?;

    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn legacy_identity_can_confirm_its_display_name(pool: PgPool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO verified_identities (discord_user_id, email)
            VALUES (1, 'test@example.com')
            "#,
        )
        .execute(&pool)
        .await?;
        let email = EmailAddress::parse("test@example.com")?;
        let display_name = DisplayName::parse("Test Member")?;

        let legacy_identity = find_user_by_id(&pool, 1)
            .await?
            .expect("legacy identity should exist");
        assert_eq!(legacy_identity.email(), &email);
        assert!(!legacy_identity.is_display_name_confirmed());

        assert!(confirm_display_name(&pool, 1, &email, &display_name).await?);
        let identity = find_user_by_id(&pool, 1)
            .await?
            .expect("identity should exist");

        assert_eq!(identity.display_name(), &display_name);
        assert!(identity.is_display_name_confirmed());
        Ok(())
    }
}
