use crate::domain::{
    onboarding::{OnboardingRequest, OnboardingRequestId, OnboardingStatus},
    verification::{DisplayName, EmailAddress},
};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, FromRow, PgPool, Postgres, Transaction, types::time::OffsetDateTime};

const REQUEST_COLUMNS: &str = r#"
    id,
    discord_user_id,
    requested_by_user_id,
    verified_email,
    verified_display_name,
    target_key,
    target_label,
    status,
    decided_by_user_id,
    decided_at,
    review_channel_id,
    review_message_id,
    provisioning_attempts,
    provisioning_started_at,
    authentik_user_id,
    last_error,
    requested_at,
    updated_at,
    completed_at
"#;

#[derive(Debug, Clone)]
pub enum StartOnboardingResult {
    Created(OnboardingRequest),
    Reused(OnboardingRequest),
}

#[derive(Debug, Clone)]
pub enum DecisionResult {
    Updated(OnboardingRequest),
    Missing,
    AlreadyHandled(OnboardingRequest),
}

#[derive(Debug, Clone)]
pub enum ClaimProvisioningResult {
    Claimed(OnboardingRequest),
    Missing,
    Unavailable(OnboardingRequest),
}

#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub event_type: String,
    pub actor_user_id: Option<u64>,
    pub outcome: String,
    pub metadata: Value,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, FromRow)]
struct OnboardingRow {
    id: i64,
    discord_user_id: i64,
    requested_by_user_id: i64,
    verified_email: String,
    verified_display_name: String,
    target_key: String,
    target_label: String,
    status: String,
    decided_by_user_id: Option<i64>,
    decided_at: Option<OffsetDateTime>,
    review_channel_id: Option<i64>,
    review_message_id: Option<i64>,
    provisioning_attempts: i32,
    provisioning_started_at: Option<OffsetDateTime>,
    authentik_user_id: Option<i64>,
    last_error: Option<String>,
    requested_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
}

#[derive(Debug, FromRow)]
struct AuditEventRow {
    event_type: String,
    actor_user_id: Option<i64>,
    outcome: String,
    metadata: Value,
    created_at: OffsetDateTime,
}

pub async fn start_or_reuse(
    pool: &PgPool,
    user_id: u64,
    requested_by_user_id: u64,
    email: &EmailAddress,
    display_name: &DisplayName,
    target_key: &str,
    target_label: &str,
) -> Result<StartOnboardingResult> {
    let user_id = discord_id_to_db(user_id)?;
    let requested_by_user_id = discord_id_to_db(requested_by_user_id)?;
    let mut transaction = pool
        .begin()
        .await
        .context("failed to begin onboarding request transaction")?;
    let insert_query = format!(
        r#"
        INSERT INTO onboarding_requests (
            discord_user_id,
            requested_by_user_id,
            verified_email,
            verified_display_name,
            target_key,
            target_label
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (discord_user_id, target_key) WHERE status <> 'denied'
        DO NOTHING
        RETURNING {REQUEST_COLUMNS}
        "#
    );

    let inserted = sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(insert_query))
        .bind(user_id)
        .bind(requested_by_user_id)
        .bind(email.to_string())
        .bind(display_name.as_str())
        .bind(target_key)
        .bind(target_label)
        .fetch_optional(&mut *transaction)
        .await
        .context("failed to create onboarding request")?;

    let result = if let Some(row) = inserted {
        let request = row.into_domain()?;
        insert_audit(
            &mut transaction,
            "onboarding.request_created",
            Some(requested_by_user_id),
            &request,
            "created",
            json!({}),
        )
        .await?;
        StartOnboardingResult::Created(request)
    } else {
        let select_query = format!(
            r#"
            SELECT {REQUEST_COLUMNS}
            FROM onboarding_requests
            WHERE discord_user_id = $1
              AND target_key = $2
              AND status <> 'denied'
            "#
        );
        let request = sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(select_query))
            .bind(user_id)
            .bind(target_key)
            .fetch_one(&mut *transaction)
            .await
            .context("failed to load existing onboarding request")?
            .into_domain()?;
        StartOnboardingResult::Reused(request)
    };

    transaction
        .commit()
        .await
        .context("failed to commit onboarding request transaction")?;
    Ok(result)
}

pub async fn find_by_id(
    pool: &PgPool,
    request_id: OnboardingRequestId,
) -> Result<Option<OnboardingRequest>> {
    let query = format!("SELECT {REQUEST_COLUMNS} FROM onboarding_requests WHERE id = $1");

    sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(query))
        .bind(request_id.get())
        .fetch_optional(pool)
        .await
        .context("failed to load onboarding request")?
        .map(OnboardingRow::into_domain)
        .transpose()
}

pub async fn list_recent(pool: &PgPool, limit: i64) -> Result<Vec<OnboardingRequest>> {
    let query = format!(
        r#"
        SELECT {REQUEST_COLUMNS}
        FROM onboarding_requests
        ORDER BY updated_at DESC, id DESC
        LIMIT $1
        "#
    );

    sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(query))
        .bind(limit.clamp(1, 50))
        .fetch_all(pool)
        .await
        .context("failed to list onboarding requests")?
        .into_iter()
        .map(OnboardingRow::into_domain)
        .collect()
}

pub async fn set_review_message(
    pool: &PgPool,
    request_id: OnboardingRequestId,
    channel_id: u64,
    message_id: u64,
) -> Result<OnboardingRequest> {
    let channel_id = discord_id_to_db(channel_id)?;
    let message_id = discord_id_to_db(message_id)?;
    let mut transaction = pool
        .begin()
        .await
        .context("failed to begin onboarding review transaction")?;
    let query = format!(
        r#"
        UPDATE onboarding_requests
        SET review_channel_id = $2,
            review_message_id = $3,
            updated_at = NOW()
        WHERE id = $1
        RETURNING {REQUEST_COLUMNS}
        "#
    );
    let request = sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(query))
        .bind(request_id.get())
        .bind(channel_id)
        .bind(message_id)
        .fetch_one(&mut *transaction)
        .await
        .context("failed to persist onboarding review message")?
        .into_domain()?;

    insert_audit(
        &mut transaction,
        "onboarding.review_synchronized",
        None,
        &request,
        "posted",
        json!({ "channel_id": channel_id, "message_id": message_id }),
    )
    .await?;
    transaction
        .commit()
        .await
        .context("failed to commit onboarding review transaction")?;
    Ok(request)
}

pub async fn approve(
    pool: &PgPool,
    request_id: OnboardingRequestId,
    actor_user_id: u64,
) -> Result<DecisionResult> {
    decide(pool, request_id, actor_user_id, OnboardingStatus::Approved).await
}

pub async fn deny(
    pool: &PgPool,
    request_id: OnboardingRequestId,
    actor_user_id: u64,
) -> Result<DecisionResult> {
    decide(pool, request_id, actor_user_id, OnboardingStatus::Denied).await
}

async fn decide(
    pool: &PgPool,
    request_id: OnboardingRequestId,
    actor_user_id: u64,
    status: OnboardingStatus,
) -> Result<DecisionResult> {
    debug_assert!(matches!(
        status,
        OnboardingStatus::Approved | OnboardingStatus::Denied
    ));
    let actor_user_id = discord_id_to_db(actor_user_id)?;
    let mut transaction = pool
        .begin()
        .await
        .context("failed to begin onboarding decision transaction")?;
    let query = format!(
        r#"
        UPDATE onboarding_requests
        SET status = $2,
            decided_by_user_id = $3,
            decided_at = NOW(),
            updated_at = NOW(),
            last_error = NULL
        WHERE id = $1
          AND status = 'pending'
        RETURNING {REQUEST_COLUMNS}
        "#
    );
    let updated = sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(query))
        .bind(request_id.get())
        .bind(status.as_str())
        .bind(actor_user_id)
        .fetch_optional(&mut *transaction)
        .await
        .context("failed to record onboarding decision")?;

    let result = if let Some(row) = updated {
        let request = row.into_domain()?;
        insert_audit(
            &mut transaction,
            if status == OnboardingStatus::Approved {
                "onboarding.approved"
            } else {
                "onboarding.denied"
            },
            Some(actor_user_id),
            &request,
            status.as_str(),
            json!({}),
        )
        .await?;
        DecisionResult::Updated(request)
    } else {
        match find_by_id_in_transaction(&mut transaction, request_id).await? {
            Some(request) => DecisionResult::AlreadyHandled(request),
            None => DecisionResult::Missing,
        }
    };

    transaction
        .commit()
        .await
        .context("failed to commit onboarding decision transaction")?;
    Ok(result)
}

pub async fn claim_provisioning(
    pool: &PgPool,
    request_id: OnboardingRequestId,
    actor_user_id: u64,
) -> Result<ClaimProvisioningResult> {
    let actor_user_id = discord_id_to_db(actor_user_id)?;
    let mut transaction = pool
        .begin()
        .await
        .context("failed to begin onboarding provisioning transaction")?;
    let query = format!(
        r#"
        UPDATE onboarding_requests
        SET status = 'provisioning',
            provisioning_attempts = provisioning_attempts + 1,
            provisioning_started_at = NOW(),
            updated_at = NOW(),
            last_error = NULL
        WHERE id = $1
          AND (
              status IN ('approved', 'failed')
              OR (
                  status = 'provisioning'
                  AND provisioning_started_at <= NOW() - INTERVAL '15 minutes'
              )
          )
        RETURNING {REQUEST_COLUMNS}
        "#
    );
    let updated = sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(query))
        .bind(request_id.get())
        .fetch_optional(&mut *transaction)
        .await
        .context("failed to claim onboarding provisioning")?;

    let result = if let Some(row) = updated {
        let request = row.into_domain()?;
        insert_audit(
            &mut transaction,
            "onboarding.provisioning_started",
            Some(actor_user_id),
            &request,
            "started",
            json!({ "attempt": request.provisioning_attempts() }),
        )
        .await?;
        ClaimProvisioningResult::Claimed(request)
    } else {
        match find_by_id_in_transaction(&mut transaction, request_id).await? {
            Some(request) => ClaimProvisioningResult::Unavailable(request),
            None => ClaimProvisioningResult::Missing,
        }
    };

    transaction
        .commit()
        .await
        .context("failed to commit onboarding provisioning transaction")?;
    Ok(result)
}

pub async fn mark_completed(
    pool: &PgPool,
    request_id: OnboardingRequestId,
    authentik_user_id: u64,
    provisioning_attempt: u32,
) -> Result<OnboardingRequest> {
    let authentik_user_id = i64::try_from(authentik_user_id)
        .context("Authentik user ID is too large for Postgres BIGINT")?;
    let mut transaction = pool
        .begin()
        .await
        .context("failed to begin onboarding completion transaction")?;
    let query = format!(
        r#"
        UPDATE onboarding_requests
        SET status = 'completed',
            authentik_user_id = $2,
            last_error = NULL,
            completed_at = NOW(),
            updated_at = NOW()
        WHERE id = $1
          AND status = 'provisioning'
          AND provisioning_attempts = $3
        RETURNING {REQUEST_COLUMNS}
        "#
    );
    let request = sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(query))
        .bind(request_id.get())
        .bind(authentik_user_id)
        .bind(i32::try_from(provisioning_attempt).context("provisioning attempt is too large")?)
        .fetch_optional(&mut *transaction)
        .await
        .context("failed to complete onboarding provisioning")?
        .ok_or_else(|| anyhow!("onboarding request is not being provisioned"))?
        .into_domain()?;

    insert_audit(
        &mut transaction,
        "onboarding.provisioning_completed",
        None,
        &request,
        "completed",
        json!({ "authentik_user_id": authentik_user_id }),
    )
    .await?;
    transaction
        .commit()
        .await
        .context("failed to commit onboarding completion transaction")?;
    Ok(request)
}

pub async fn mark_failed(
    pool: &PgPool,
    request_id: OnboardingRequestId,
    safe_error: &str,
    provisioning_attempt: u32,
) -> Result<OnboardingRequest> {
    let safe_error = sanitize_error(safe_error);
    let mut transaction = pool
        .begin()
        .await
        .context("failed to begin onboarding failure transaction")?;
    let query = format!(
        r#"
        UPDATE onboarding_requests
        SET status = 'failed',
            last_error = $2,
            updated_at = NOW()
        WHERE id = $1
          AND status = 'provisioning'
          AND provisioning_attempts = $3
        RETURNING {REQUEST_COLUMNS}
        "#
    );
    let request = sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(query))
        .bind(request_id.get())
        .bind(&safe_error)
        .bind(i32::try_from(provisioning_attempt).context("provisioning attempt is too large")?)
        .fetch_optional(&mut *transaction)
        .await
        .context("failed to record onboarding provisioning failure")?
        .ok_or_else(|| anyhow!("onboarding request is not being provisioned"))?
        .into_domain()?;

    insert_audit(
        &mut transaction,
        "onboarding.provisioning_failed",
        None,
        &request,
        "failed",
        json!({ "error": safe_error }),
    )
    .await?;
    transaction
        .commit()
        .await
        .context("failed to commit onboarding failure transaction")?;
    Ok(request)
}

pub async fn list_audit_events(
    pool: &PgPool,
    request_id: OnboardingRequestId,
    limit: i64,
) -> Result<Vec<AuditEvent>> {
    sqlx::query_as::<_, AuditEventRow>(
        r#"
        SELECT event_type, actor_user_id, outcome, metadata, created_at
        FROM audit_events
        WHERE onboarding_request_id = $1
        ORDER BY created_at DESC, id DESC
        LIMIT $2
        "#,
    )
    .bind(request_id.get())
    .bind(limit.clamp(1, 20))
    .fetch_all(pool)
    .await
    .context("failed to load onboarding audit events")?
    .into_iter()
    .map(AuditEventRow::into_domain)
    .collect()
}

async fn find_by_id_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: OnboardingRequestId,
) -> Result<Option<OnboardingRequest>> {
    let query = format!("SELECT {REQUEST_COLUMNS} FROM onboarding_requests WHERE id = $1");
    sqlx::query_as::<_, OnboardingRow>(AssertSqlSafe(query))
        .bind(request_id.get())
        .fetch_optional(&mut **transaction)
        .await
        .context("failed to load onboarding request")?
        .map(OnboardingRow::into_domain)
        .transpose()
}

async fn insert_audit(
    transaction: &mut Transaction<'_, Postgres>,
    event_type: &str,
    actor_user_id: Option<i64>,
    request: &OnboardingRequest,
    outcome: &str,
    metadata: Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            event_type,
            actor_user_id,
            target_user_id,
            onboarding_request_id,
            target_key,
            outcome,
            metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(event_type)
    .bind(actor_user_id)
    .bind(discord_id_to_db(request.user_id())?)
    .bind(request.id().get())
    .bind(request.target_key())
    .bind(outcome)
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .context("failed to append onboarding audit event")?;
    Ok(())
}

fn discord_id_to_db(user_id: u64) -> Result<i64> {
    i64::try_from(user_id).context("Discord ID is too large for Postgres BIGINT")
}

fn db_id_to_discord(user_id: i64) -> Result<u64> {
    u64::try_from(user_id).context("stored Discord ID cannot be represented as u64")
}

fn optional_db_id_to_discord(user_id: Option<i64>) -> Result<Option<u64>> {
    user_id.map(db_id_to_discord).transpose()
}

fn sanitize_error(error: &str) -> String {
    error
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(500)
        .collect()
}

impl OnboardingRow {
    fn into_domain(self) -> Result<OnboardingRequest> {
        let id = OnboardingRequestId::new(self.id)
            .ok_or_else(|| anyhow!("stored onboarding request ID must be positive"))?;
        let status = OnboardingStatus::parse(&self.status)
            .ok_or_else(|| anyhow!("stored onboarding status is invalid"))?;

        Ok(OnboardingRequest::from_persisted(
            id,
            db_id_to_discord(self.discord_user_id)?,
            db_id_to_discord(self.requested_by_user_id)?,
            EmailAddress::parse(&self.verified_email)
                .context("stored onboarding email is invalid")?,
            DisplayName::parse(&self.verified_display_name)
                .context("stored onboarding display name is invalid")?,
            self.target_key,
            self.target_label,
            self.requested_at,
            self.updated_at,
            status,
            optional_db_id_to_discord(self.decided_by_user_id)?,
            self.decided_at,
            optional_db_id_to_discord(self.review_channel_id)?,
            optional_db_id_to_discord(self.review_message_id)?,
            u32::try_from(self.provisioning_attempts)
                .context("stored provisioning attempt count is invalid")?,
            self.provisioning_started_at,
            optional_db_id_to_discord(self.authentik_user_id)?,
            self.last_error,
            self.completed_at,
        ))
    }
}

impl AuditEventRow {
    fn into_domain(self) -> Result<AuditEvent> {
        Ok(AuditEvent {
            event_type: self.event_type,
            actor_user_id: optional_db_id_to_discord(self.actor_user_id)?,
            outcome: self.outcome,
            metadata: self.metadata,
            created_at: self.created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email() -> EmailAddress {
        EmailAddress::parse("test@example.com").expect("test email should parse")
    }

    fn display_name() -> DisplayName {
        DisplayName::parse("Test Member").expect("test name should parse")
    }

    #[sqlx::test]
    async fn pending_request_is_reused(pool: PgPool) -> Result<()> {
        let first = start_or_reuse(
            &pool,
            1,
            9,
            &email(),
            &display_name(),
            "headscale",
            "Headscale",
        )
        .await?;
        let second = start_or_reuse(
            &pool,
            1,
            10,
            &email(),
            &display_name(),
            "headscale",
            "Headscale",
        )
        .await?;

        assert!(matches!(first, StartOnboardingResult::Created(_)));
        assert!(matches!(second, StartOnboardingResult::Reused(_)));
        assert_eq!(list_recent(&pool, 10).await?.len(), 1);
        Ok(())
    }

    #[sqlx::test]
    async fn only_one_pending_decision_wins(pool: PgPool) -> Result<()> {
        let request = match start_or_reuse(
            &pool,
            1,
            9,
            &email(),
            &display_name(),
            "headscale",
            "Headscale",
        )
        .await?
        {
            StartOnboardingResult::Created(request) => request,
            StartOnboardingResult::Reused(_) => unreachable!(),
        };

        let (first, second) = tokio::join!(
            approve(&pool, request.id(), 11),
            deny(&pool, request.id(), 12)
        );
        let first = first?;
        let second = second?;

        let updated_count = [first, second]
            .iter()
            .filter(|result| matches!(result, DecisionResult::Updated(_)))
            .count();
        assert_eq!(updated_count, 1);
        Ok(())
    }

    #[sqlx::test]
    async fn failed_provisioning_can_be_retried(pool: PgPool) -> Result<()> {
        let request = match start_or_reuse(
            &pool,
            1,
            9,
            &email(),
            &display_name(),
            "headscale",
            "Headscale",
        )
        .await?
        {
            StartOnboardingResult::Created(request) => request,
            StartOnboardingResult::Reused(_) => unreachable!(),
        };
        let approved = match approve(&pool, request.id(), 11).await? {
            DecisionResult::Updated(request) => request,
            _ => unreachable!(),
        };
        let claimed = match claim_provisioning(&pool, approved.id(), 11).await? {
            ClaimProvisioningResult::Claimed(request) => request,
            _ => unreachable!(),
        };
        mark_failed(
            &pool,
            claimed.id(),
            "temporary failure",
            claimed.provisioning_attempts(),
        )
        .await?;

        let retried = claim_provisioning(&pool, claimed.id(), 12).await?;

        assert!(matches!(retried, ClaimProvisioningResult::Claimed(_)));
        Ok(())
    }

    #[sqlx::test]
    async fn denied_request_allows_a_new_request(pool: PgPool) -> Result<()> {
        let request = match start_or_reuse(
            &pool,
            1,
            9,
            &email(),
            &display_name(),
            "headscale",
            "Headscale",
        )
        .await?
        {
            StartOnboardingResult::Created(request) => request,
            StartOnboardingResult::Reused(_) => unreachable!(),
        };
        deny(&pool, request.id(), 11).await?;

        let next = start_or_reuse(
            &pool,
            1,
            9,
            &email(),
            &display_name(),
            "headscale",
            "Headscale",
        )
        .await?;

        assert!(matches!(next, StartOnboardingResult::Created(_)));
        Ok(())
    }

    #[sqlx::test]
    async fn stale_provisioning_attempt_cannot_overwrite_retry(pool: PgPool) -> Result<()> {
        let request = match start_or_reuse(
            &pool,
            1,
            9,
            &email(),
            &display_name(),
            "headscale",
            "Headscale",
        )
        .await?
        {
            StartOnboardingResult::Created(request) => request,
            StartOnboardingResult::Reused(_) => unreachable!(),
        };
        approve(&pool, request.id(), 11).await?;
        let first = match claim_provisioning(&pool, request.id(), 11).await? {
            ClaimProvisioningResult::Claimed(request) => request,
            _ => unreachable!(),
        };
        sqlx::query(
            "UPDATE onboarding_requests SET provisioning_started_at = NOW() - INTERVAL '16 minutes' WHERE id = $1",
        )
        .bind(request.id().get())
        .execute(&pool)
        .await?;
        let retry = match claim_provisioning(&pool, request.id(), 12).await? {
            ClaimProvisioningResult::Claimed(request) => request,
            _ => unreachable!(),
        };

        let stale_result = mark_failed(
            &pool,
            request.id(),
            "late failure",
            first.provisioning_attempts(),
        )
        .await;
        let completed =
            mark_completed(&pool, request.id(), 123, retry.provisioning_attempts()).await?;

        assert!(stale_result.is_err());
        assert_eq!(completed.status(), OnboardingStatus::Completed);
        Ok(())
    }

    #[sqlx::test]
    async fn workflow_transitions_append_audit_events(pool: PgPool) -> Result<()> {
        let request = match start_or_reuse(
            &pool,
            1,
            9,
            &email(),
            &display_name(),
            "headscale",
            "Headscale",
        )
        .await?
        {
            StartOnboardingResult::Created(request) => request,
            StartOnboardingResult::Reused(_) => unreachable!(),
        };
        approve(&pool, request.id(), 11).await?;
        let claimed = match claim_provisioning(&pool, request.id(), 11).await? {
            ClaimProvisioningResult::Claimed(request) => request,
            _ => unreachable!(),
        };
        mark_completed(&pool, request.id(), 123, claimed.provisioning_attempts()).await?;

        let event_types = list_audit_events(&pool, request.id(), 10)
            .await?
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>();

        assert_eq!(
            event_types,
            vec![
                "onboarding.provisioning_completed",
                "onboarding.provisioning_started",
                "onboarding.approved",
                "onboarding.request_created"
            ]
        );
        Ok(())
    }
}
