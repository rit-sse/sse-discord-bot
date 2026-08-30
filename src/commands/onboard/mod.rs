use crate::{
    Data, Error,
    config::OnboardingTargetConfig,
    db::{
        onboarding_repository::{
            self, ClaimProvisioningResult, DecisionResult, StartOnboardingResult,
        },
        verify_repository,
    },
    domain::onboarding::{OnboardingRequest, OnboardingRequestId, OnboardingStatus},
};
use poise::{CreateReply, serenity_prelude as serenity};

type ApplicationContext<'a> = poise::ApplicationContext<'a, Data, Error>;

const APPROVE_PREFIX: &str = "onboard:approve:";
const DENY_PREFIX: &str = "onboard:deny:";
const TAILSCALE_DOWNLOAD_URL: &str = "https://tailscale.com/download";

async fn ephemeral_reply(
    ctx: ApplicationContext<'_>,
    content: impl Into<String>,
) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(content).ephemeral(true))
        .await?;
    Ok(())
}

async fn autocomplete_target(
    ctx: crate::Context<'_>,
    partial: &str,
) -> Vec<serenity::AutocompleteChoice> {
    if ctx.guild_id().is_none() {
        return vec![];
    }
    let Some(onboarding) = ctx.data().modules.onboarding.as_ref() else {
        return vec![];
    };
    let Some(member) = ctx.author_member().await else {
        return vec![];
    };
    let partial = partial.trim().to_ascii_lowercase();

    onboarding
        .config
        .targets
        .iter()
        .filter(|target| can_approve_target(&member, target))
        .filter(|target| {
            partial.is_empty()
                || target.key.to_ascii_lowercase().contains(&partial)
                || target.label.to_ascii_lowercase().contains(&partial)
        })
        .take(25)
        .map(|target| {
            serenity::AutocompleteChoice::new(
                format!("{} ({})", target.label, target.key),
                target.key.clone(),
            )
        })
        .collect()
}

/// Requests infrastructure onboarding for a verified Discord member.
#[poise::command(slash_command)]
pub async fn onboard(
    ctx: ApplicationContext<'_>,
    #[description = "Verified member to onboard"] user: serenity::User,
    #[description = "Configured onboarding target"]
    #[autocomplete = "autocomplete_target"]
    target: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        ephemeral_reply(ctx, "Onboarding only works inside a server.").await?;
        return Ok(());
    };
    let onboarding = onboarding_module(ctx.data())?;
    let verification = verification_module(ctx.data())?;
    ctx.defer_ephemeral().await?;
    let requester_member = guild_id
        .member(ctx.serenity_context(), ctx.author().id)
        .await?;
    let Some(target_config) = find_target(&onboarding.config.targets, &target) else {
        ephemeral_reply(
            ctx,
            format!(
                "I do not recognize that onboarding target. Available targets: {}",
                available_targets_for_member(&onboarding.config.targets, &requester_member)
            ),
        )
        .await?;
        return Ok(());
    };

    if !can_approve_target(&requester_member, target_config) {
        tracing::warn!(
            requested_by_user_id = ctx.author().id.get(),
            target_user_id = user.id.get(),
            target = %target_config.key,
            "onboarding requester lacks target manager role"
        );
        ephemeral_reply(
            ctx,
            format!(
                "You do not have the Discord role needed to onboard users into {}.",
                target_config.label
            ),
        )
        .await?;
        return Ok(());
    }

    let target_member = guild_id.member(ctx.serenity_context(), user.id).await?;
    if !target_member
        .roles
        .contains(&serenity::RoleId::new(verification.config.verified_role_id))
    {
        ephemeral_reply(
            ctx,
            format!(
                "{} must complete Discord email verification before onboarding.",
                user.name
            ),
        )
        .await?;
        return Ok(());
    }

    let Some(verified_identity) =
        verify_repository::find_user_by_id(&ctx.data().db, user.id.get()).await?
    else {
        ephemeral_reply(
            ctx,
            format!(
                "{} has the verified role but no persisted identity. Ask them to verify again.",
                user.name
            ),
        )
        .await?;
        return Ok(());
    };
    if !verified_identity.is_display_name_confirmed() {
        ephemeral_reply(
            ctx,
            format!(
                "{} verified before preferred names were collected. Ask them to press the verification button once to confirm their preferred name before onboarding.",
                user.name
            ),
        )
        .await?;
        return Ok(());
    }

    let start_result = onboarding_repository::start_or_reuse(
        &ctx.data().db,
        user.id.get(),
        ctx.author().id.get(),
        verified_identity.email(),
        verified_identity.display_name(),
        &target_config.key,
        &target_config.label,
    )
    .await?;
    let (request, created) = match start_result {
        StartOnboardingResult::Created(request) => (request, true),
        StartOnboardingResult::Reused(request) => (request, false),
    };

    let request = ensure_review_message(ctx.serenity_context(), ctx.data(), &request).await?;
    tracing::info!(
        request_id = request.id().get(),
        user_id = request.user_id(),
        requested_by_user_id = ctx.author().id.get(),
        target = %request.target_key(),
        status = %request.status(),
        created,
        "onboarding request ready for review"
    );

    let response = match request.status() {
        OnboardingStatus::Completed => format!(
            "{} already has completed onboarding for {} (request `{}`).",
            user.name,
            request.target_label(),
            request.id()
        ),
        OnboardingStatus::Failed => format!(
            "{}'s onboarding request `{}` needs an officer retry.",
            user.name,
            request.id()
        ),
        OnboardingStatus::Provisioning => format!(
            "{}'s onboarding request `{}` is currently provisioning.",
            user.name,
            request.id()
        ),
        _ => format!(
            "{}'s onboarding request `{}` is waiting for approval.",
            user.name,
            request.id()
        ),
    };
    ephemeral_reply(ctx, response).await?;
    Ok(())
}

/// Shows an authorized officer onboarding state and recent audit history.
#[poise::command(slash_command)]
pub async fn onboarding_status(
    ctx: ApplicationContext<'_>,
    #[description = "Request ID; omit to list recent requests"] request_id: Option<i64>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        ephemeral_reply(ctx, "Onboarding status only works inside a server.").await?;
        return Ok(());
    };
    let onboarding = onboarding_module(ctx.data())?;
    ctx.defer_ephemeral().await?;
    let member = guild_id
        .member(ctx.serenity_context(), ctx.author().id)
        .await?;

    if let Some(raw_request_id) = request_id {
        let Some(request_id) = OnboardingRequestId::new(raw_request_id) else {
            ephemeral_reply(ctx, "Request IDs must be positive numbers.").await?;
            return Ok(());
        };
        let Some(request) = onboarding_repository::find_by_id(&ctx.data().db, request_id).await?
        else {
            ephemeral_reply(ctx, "That onboarding request does not exist.").await?;
            return Ok(());
        };
        let Some(target) = find_target(&onboarding.config.targets, request.target_key()) else {
            tracing::warn!(
                request_id = request.id().get(),
                target = %request.target_key(),
                "status requested for an unconfigured onboarding target"
            );
            ephemeral_reply(ctx, "That request's target is no longer configured.").await?;
            return Ok(());
        };
        if !can_approve_target(&member, target) {
            ephemeral_reply(ctx, "You do not have permission to inspect that request.").await?;
            return Ok(());
        }

        let request = ensure_review_message(ctx.serenity_context(), ctx.data(), &request).await?;
        let events =
            onboarding_repository::list_audit_events(&ctx.data().db, request.id(), 8).await?;
        let audit = events
            .iter()
            .map(|event| {
                let actor = event
                    .actor_user_id
                    .map_or_else(|| "system".to_owned(), |id| format!("<@{id}>"));
                format!(
                    "- <t:{}:R> `{}` → `{}` by {}",
                    event.created_at.unix_timestamp(),
                    event.event_type,
                    event.outcome,
                    actor
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        ephemeral_reply(
            ctx,
            format!(
                "{}\n\n**Recent audit events**\n{}",
                request_status_content(&request),
                if audit.is_empty() { "None" } else { &audit }
            ),
        )
        .await?;
        return Ok(());
    }

    let visible = onboarding_repository::list_recent(&ctx.data().db, 25)
        .await?
        .into_iter()
        .filter(|request| {
            find_target(&onboarding.config.targets, request.target_key())
                .is_some_and(|target| can_approve_target(&member, target))
        })
        .take(10)
        .map(|request| {
            format!(
                "- `{}` • {} • <@{}> • `{}`",
                request.id(),
                request.target_label(),
                request.user_id(),
                request.status()
            )
        })
        .collect::<Vec<_>>();

    ephemeral_reply(
        ctx,
        if visible.is_empty() {
            "No onboarding requests are visible to your configured roles.".to_owned()
        } else {
            format!("**Recent onboarding requests**\n{}", visible.join("\n"))
        },
    )
    .await?;
    Ok(())
}

/// Retries failed or abandoned onboarding provisioning.
#[poise::command(slash_command)]
pub async fn onboarding_retry(
    ctx: ApplicationContext<'_>,
    #[description = "Onboarding request ID"] request_id: i64,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        ephemeral_reply(ctx, "Onboarding retry only works inside a server.").await?;
        return Ok(());
    };
    let Some(request_id) = OnboardingRequestId::new(request_id) else {
        ephemeral_reply(ctx, "Request IDs must be positive numbers.").await?;
        return Ok(());
    };
    let onboarding = onboarding_module(ctx.data())?;
    let member = guild_id
        .member(ctx.serenity_context(), ctx.author().id)
        .await?;
    let Some(request) = onboarding_repository::find_by_id(&ctx.data().db, request_id).await? else {
        ephemeral_reply(ctx, "That onboarding request does not exist.").await?;
        return Ok(());
    };
    let Some(target) = find_target(&onboarding.config.targets, request.target_key()) else {
        ephemeral_reply(ctx, "That request's target is no longer configured.").await?;
        return Ok(());
    };
    if !can_approve_target(&member, target) {
        ephemeral_reply(ctx, "You do not have permission to retry that request.").await?;
        return Ok(());
    }

    ctx.defer_ephemeral().await?;
    let outcome = provision_request(
        ctx.serenity_context(),
        ctx.data(),
        guild_id,
        request.id(),
        ctx.author().id.get(),
    )
    .await?;
    ctx.send(
        CreateReply::default()
            .content(outcome.message())
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Checks the Authentik service account and configured groups available to an officer.
#[poise::command(slash_command)]
pub async fn onboarding_check(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        ephemeral_reply(ctx, "Onboarding checks only work inside a server.").await?;
        return Ok(());
    };
    let onboarding = onboarding_module(ctx.data())?;
    let member = guild_id
        .member(ctx.serenity_context(), ctx.author().id)
        .await?;
    let visible_targets = onboarding
        .config
        .targets
        .iter()
        .filter(|target| can_approve_target(&member, target))
        .collect::<Vec<_>>();
    if visible_targets.is_empty() {
        ephemeral_reply(ctx, "You do not manage any configured onboarding targets.").await?;
        return Ok(());
    }

    ctx.defer_ephemeral().await?;
    let mut results = Vec::new();
    match onboarding.authentik_client.check_api_access().await {
        Ok(()) => results.push("- Authentik service account: ready".to_owned()),
        Err(error) => {
            tracing::error!(
                actor_user_id = ctx.author().id.get(),
                error = %error,
                "authentik service account validation failed"
            );
            results.push(format!("- Authentik service account: failed (`{error}`)"));
        }
    }

    for target in visible_targets {
        match onboarding
            .authentik_client
            .check_group_access(&target.authentik_group_uuid)
            .await
        {
            Ok(()) => results.push(format!("- {} group: ready", target.label)),
            Err(error) => {
                tracing::error!(
                    actor_user_id = ctx.author().id.get(),
                    target = %target.key,
                    error = %error,
                    "authentik target validation failed"
                );
                results.push(format!("- {} group: failed (`{error}`)", target.label));
            }
        }
    }

    ctx.send(
        CreateReply::default()
            .content(format!(
                "**Onboarding dependency check**\n{}",
                results.join("\n")
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

pub async fn handle_component_interaction(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    data: &Data,
) -> Result<(), Error> {
    let Some((action, request_id)) = parse_onboarding_custom_id(&interaction.data.custom_id) else {
        return Ok(());
    };
    if data.modules.onboarding.is_none() {
        tracing::warn!(
            request_id = request_id.get(),
            "ignored onboarding interaction while disabled"
        );
        return Ok(());
    }

    interaction
        .create_response(
            serenity_ctx,
            serenity::CreateInteractionResponse::Defer(
                serenity::CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;

    let Some(request) = onboarding_repository::find_by_id(&data.db, request_id).await? else {
        edit_interaction_response(
            serenity_ctx,
            interaction,
            "That onboarding request does not exist.",
        )
        .await?;
        return Ok(());
    };
    let onboarding = onboarding_module(data)?;
    let Some(target) = find_target(&onboarding.config.targets, request.target_key()) else {
        edit_interaction_response(
            serenity_ctx,
            interaction,
            "That onboarding target is no longer configured.",
        )
        .await?;
        return Ok(());
    };
    let Some(guild_id) = interaction.guild_id else {
        edit_interaction_response(
            serenity_ctx,
            interaction,
            "Onboarding decisions only work inside the configured server.",
        )
        .await?;
        return Ok(());
    };
    let actor_member = guild_id.member(serenity_ctx, interaction.user.id).await?;
    if !can_approve_target(&actor_member, target) {
        tracing::warn!(
            request_id = request.id().get(),
            actor_user_id = interaction.user.id.get(),
            target = %request.target_key(),
            "onboarding decision denied by runtime authorization"
        );
        edit_interaction_response(
            serenity_ctx,
            interaction,
            "You do not have permission to handle this onboarding target.",
        )
        .await?;
        return Ok(());
    }

    match action {
        OnboardingAction::Approve => {
            handle_approval(serenity_ctx, interaction, data, request).await?
        }
        OnboardingAction::Deny => handle_denial(serenity_ctx, interaction, data, request).await?,
    }
    Ok(())
}

async fn handle_approval(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    data: &Data,
    request: OnboardingRequest,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id else {
        edit_interaction_response(
            serenity_ctx,
            interaction,
            "Onboarding decisions only work inside the configured server.",
        )
        .await?;
        return Ok(());
    };
    if !target_remains_eligible(serenity_ctx, data, guild_id, &request).await? {
        edit_interaction_response(
            serenity_ctx,
            interaction,
            "The target user is no longer eligible for onboarding. Confirm their Discord verification and persisted email before trying again.",
        )
        .await?;
        return Ok(());
    }

    let request =
        match onboarding_repository::approve(&data.db, request.id(), interaction.user.id.get())
            .await?
        {
            DecisionResult::Updated(request) => request,
            DecisionResult::Missing => {
                edit_interaction_response(
                    serenity_ctx,
                    interaction,
                    "That onboarding request does not exist.",
                )
                .await?;
                return Ok(());
            }
            DecisionResult::AlreadyHandled(request) => {
                synchronize_review_message(serenity_ctx, data, &request).await;
                edit_interaction_response(
                    serenity_ctx,
                    interaction,
                    format!("That request is already `{}`.", request.status()),
                )
                .await?;
                return Ok(());
            }
        };

    synchronize_review_message(serenity_ctx, data, &request).await;
    let outcome = provision_request(
        serenity_ctx,
        data,
        guild_id,
        request.id(),
        interaction.user.id.get(),
    )
    .await?;
    edit_interaction_response(serenity_ctx, interaction, outcome.message()).await?;
    Ok(())
}

async fn handle_denial(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    data: &Data,
    request: OnboardingRequest,
) -> Result<(), Error> {
    match onboarding_repository::deny(&data.db, request.id(), interaction.user.id.get()).await? {
        DecisionResult::Updated(request) => {
            synchronize_review_message(serenity_ctx, data, &request).await;
            if let Err(error) = serenity::UserId::new(request.user_id())
                .direct_message(
                    serenity_ctx,
                    serenity::CreateMessage::new().content(format!(
                        "Your SSE onboarding request for {} was denied. Please contact an officer if you need clarification.",
                        request.target_label()
                    )),
                )
                .await
            {
                tracing::warn!(
                    request_id = request.id().get(),
                    user_id = request.user_id(),
                    error = %error,
                    "failed to DM onboarding denial"
                );
            }
            edit_interaction_response(serenity_ctx, interaction, "Onboarding denied.").await?;
        }
        DecisionResult::Missing => {
            edit_interaction_response(
                serenity_ctx,
                interaction,
                "That onboarding request does not exist.",
            )
            .await?;
        }
        DecisionResult::AlreadyHandled(request) => {
            synchronize_review_message(serenity_ctx, data, &request).await;
            edit_interaction_response(
                serenity_ctx,
                interaction,
                format!("That request is already `{}`.", request.status()),
            )
            .await?;
        }
    }
    Ok(())
}

async fn provision_request(
    serenity_ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    request_id: OnboardingRequestId,
    actor_user_id: u64,
) -> Result<ProvisioningOutcome, Error> {
    let Some(current_request) = onboarding_repository::find_by_id(&data.db, request_id).await?
    else {
        return Ok(ProvisioningOutcome::Missing);
    };
    let onboarding = onboarding_module(data)?;
    let Some(target_config) = find_target(&onboarding.config.targets, current_request.target_key())
    else {
        return Ok(ProvisioningOutcome::Unavailable(current_request.status()));
    };
    let actor_member = match guild_id
        .member(serenity_ctx, serenity::UserId::new(actor_user_id))
        .await
    {
        Ok(member) => member,
        Err(error) => {
            tracing::warn!(
                request_id = current_request.id().get(),
                actor_user_id,
                error = %error,
                "could not confirm provisioning actor guild membership"
            );
            return Ok(ProvisioningOutcome::Unauthorized);
        }
    };
    if !can_approve_target(&actor_member, target_config) {
        tracing::warn!(
            request_id = current_request.id().get(),
            actor_user_id,
            target = %current_request.target_key(),
            "provisioning denied after execution-time authorization check"
        );
        return Ok(ProvisioningOutcome::Unauthorized);
    }
    if !target_remains_eligible(serenity_ctx, data, guild_id, &current_request).await? {
        return Ok(ProvisioningOutcome::Ineligible);
    }

    let request =
        match onboarding_repository::claim_provisioning(&data.db, request_id, actor_user_id).await?
        {
            ClaimProvisioningResult::Claimed(request) => request,
            ClaimProvisioningResult::Missing => return Ok(ProvisioningOutcome::Missing),
            ClaimProvisioningResult::Unavailable(request) => {
                return Ok(ProvisioningOutcome::Unavailable(request.status()));
            }
        };
    synchronize_review_message(serenity_ctx, data, &request).await;

    let Some(target) = find_target(&onboarding.config.targets, request.target_key()).cloned()
    else {
        let failed = onboarding_repository::mark_failed(
            &data.db,
            request.id(),
            "onboarding target is no longer configured",
            request.provisioning_attempts(),
        )
        .await?;
        synchronize_review_message(serenity_ctx, data, &failed).await;
        return Ok(ProvisioningOutcome::Failed(failed.id()));
    };

    tracing::info!(
        request_id = request.id().get(),
        user_id = request.user_id(),
        target = %request.target_key(),
        attempt = request.provisioning_attempts(),
        "provisioning onboarding request"
    );
    let provisioned = async {
        let authentik_user = onboarding
            .authentik_client
            .find_or_create_user(request.email(), request.display_name().as_str())
            .await?;
        onboarding
            .authentik_client
            .add_user_to_group(&authentik_user, &target.authentik_group_uuid)
            .await?;
        let recovery_link = onboarding
            .authentik_client
            .create_recovery_link(&authentik_user)
            .await?;
        Ok::<_, anyhow::Error>((authentik_user, recovery_link))
    }
    .await;

    let (authentik_user, recovery_link) = match provisioned {
        Ok(provisioned) => provisioned,
        Err(error) => {
            tracing::error!(
                request_id = request.id().get(),
                user_id = request.user_id(),
                target = %request.target_key(),
                error = %error,
                "onboarding provisioning failed"
            );
            let failed = onboarding_repository::mark_failed(
                &data.db,
                request.id(),
                &safe_provisioning_error(&error),
                request.provisioning_attempts(),
            )
            .await?;
            synchronize_review_message(serenity_ctx, data, &failed).await;
            return Ok(ProvisioningOutcome::Failed(failed.id()));
        }
    };

    let completion_url = target
        .completion_url
        .as_deref()
        .unwrap_or(&onboarding.config.headscale_login_url);
    let user_message = completion_message(
        request.target_label(),
        &request.email().to_string(),
        &authentik_user.username,
        onboarding.authentik_client.login_url(),
        &recovery_link,
        &onboarding.config.headscale_login_url,
        completion_url,
    );
    if let Err(error) = serenity::UserId::new(request.user_id())
        .direct_message(
            serenity_ctx,
            serenity::CreateMessage::new().content(user_message),
        )
        .await
    {
        tracing::error!(
            request_id = request.id().get(),
            user_id = request.user_id(),
            error = %error,
            "failed to DM completed onboarding"
        );
        let failed = onboarding_repository::mark_failed(
            &data.db,
            request.id(),
            "failed to deliver onboarding completion message",
            request.provisioning_attempts(),
        )
        .await?;
        synchronize_review_message(serenity_ctx, data, &failed).await;
        return Ok(ProvisioningOutcome::Failed(failed.id()));
    }

    let completed = onboarding_repository::mark_completed(
        &data.db,
        request.id(),
        authentik_user.pk,
        request.provisioning_attempts(),
    )
    .await?;
    synchronize_review_message(serenity_ctx, data, &completed).await;

    tracing::info!(
        request_id = completed.id().get(),
        user_id = completed.user_id(),
        target = %completed.target_key(),
        authentik_user_id = authentik_user.pk,
        "completed onboarding request"
    );
    Ok(ProvisioningOutcome::Completed(completed.id()))
}

fn completion_message(
    target_label: &str,
    verified_email: &str,
    authentik_username: &str,
    authentik_login_url: &str,
    recovery_link: &str,
    headscale_login_url: &str,
    completion_url: &str,
) -> String {
    let mut message = format!(
        "Your SSE onboarding for {target_label} is complete.\n\n\
         **Authentik account**\n\
         Verified email: `{verified_email}`\n\
         Username: `{authentik_username}`\n\
         Create your password within 30 minutes: <{recovery_link}>\n\
         Sign in: {authentik_login_url}\n\n\
         **Connect with Tailscale**\n\
         1. Download and install Tailscale: {TAILSCALE_DOWNLOAD_URL}\n\
         2. Run this command:\n```sh\n\
         tailscale login --login-server {headscale_login_url}\n\
         ```\n\
         3. In the browser, sign in using the verified email above."
    );

    if completion_url != headscale_login_url {
        message.push_str(&format!("\n\nAdditional access: {completion_url}"));
    }

    message
}

async fn ensure_review_message(
    serenity_ctx: &serenity::Context,
    data: &Data,
    request: &OnboardingRequest,
) -> Result<OnboardingRequest, Error> {
    if let Some((channel_id, message_id)) = request.review_location() {
        let edit_result = serenity::ChannelId::new(channel_id)
            .edit_message(
                serenity_ctx,
                serenity::MessageId::new(message_id),
                review_message_update(request),
            )
            .await;
        if edit_result.is_ok() {
            return Ok(request.clone());
        }
        tracing::warn!(
            request_id = request.id().get(),
            channel_id,
            message_id,
            error = %edit_result.expect_err("checked error"),
            "failed to reconcile onboarding review message; posting replacement"
        );
    }

    post_review_message(serenity_ctx, data, request).await
}

async fn post_review_message(
    serenity_ctx: &serenity::Context,
    data: &Data,
    request: &OnboardingRequest,
) -> Result<OnboardingRequest, Error> {
    let channel_id = serenity::ChannelId::new(onboarding_module(data)?.config.review_channel_id);
    let message = channel_id
        .send_message(
            serenity_ctx,
            serenity::CreateMessage::new()
                .content(review_message_content(request))
                .components(review_components(request)),
        )
        .await?;
    let request = onboarding_repository::set_review_message(
        &data.db,
        request.id(),
        channel_id.get(),
        message.id.get(),
    )
    .await?;
    tracing::info!(
        request_id = request.id().get(),
        channel_id = channel_id.get(),
        message_id = message.id.get(),
        "persisted onboarding review message"
    );
    Ok(request)
}

async fn synchronize_review_message(
    serenity_ctx: &serenity::Context,
    data: &Data,
    request: &OnboardingRequest,
) {
    if let Err(error) = ensure_review_message(serenity_ctx, data, request).await {
        tracing::error!(
            request_id = request.id().get(),
            status = %request.status(),
            error = %error,
            "failed to synchronize onboarding review message"
        );
    }
}

fn review_message_update(request: &OnboardingRequest) -> serenity::EditMessage {
    serenity::EditMessage::new()
        .content(review_message_content(request))
        .components(review_components(request))
}

fn review_components(request: &OnboardingRequest) -> Vec<serenity::CreateActionRow> {
    if request.status() != OnboardingStatus::Pending {
        return vec![];
    }

    vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{APPROVE_PREFIX}{}", request.id()))
            .label("Approve")
            .style(serenity::ButtonStyle::Success),
        serenity::CreateButton::new(format!("{DENY_PREFIX}{}", request.id()))
            .label("Deny")
            .style(serenity::ButtonStyle::Danger),
    ])]
}

fn review_message_content(request: &OnboardingRequest) -> String {
    format!(
        "**Infrastructure onboarding request**\n{}",
        request_status_content(request)
    )
}

fn request_status_content(request: &OnboardingRequest) -> String {
    let actor = request
        .decided_by_user_id()
        .map(|id| format!("\nDecision by: <@{id}>"))
        .unwrap_or_default();
    let error = request
        .last_error()
        .map(|error| format!("\nLast error: `{error}`"))
        .unwrap_or_default();
    format!(
        "Status: `{}`\nTarget: {}\nUser: <@{}>\nPreferred name: `{}`\nRequested by: <@{}>\nVerified email: `{}`\nRequest ID: `{}`\nProvisioning attempts: {}{}{}",
        request.status(),
        request.target_label(),
        request.user_id(),
        request.display_name(),
        request.requested_by_user_id(),
        request.email(),
        request.id(),
        request.provisioning_attempts(),
        actor,
        error
    )
}

async fn edit_interaction_response(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    content: impl Into<String>,
) -> Result<(), Error> {
    interaction
        .edit_response(
            serenity_ctx,
            serenity::EditInteractionResponse::new().content(content),
        )
        .await?;
    Ok(())
}

fn find_target<'a>(
    targets: &'a [OnboardingTargetConfig],
    key: &str,
) -> Option<&'a OnboardingTargetConfig> {
    targets
        .iter()
        .find(|target| target.key.eq_ignore_ascii_case(key.trim()))
}

fn can_approve_target(member: &serenity::Member, target: &OnboardingTargetConfig) -> bool {
    target
        .approver_role_ids
        .iter()
        .map(|role_id| serenity::RoleId::new(*role_id))
        .any(|role_id| member.roles.contains(&role_id))
}

async fn target_remains_eligible(
    serenity_ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    request: &OnboardingRequest,
) -> Result<bool, Error> {
    let verification = verification_module(data)?;
    let member = match guild_id
        .member(serenity_ctx, serenity::UserId::new(request.user_id()))
        .await
    {
        Ok(member) => member,
        Err(error) => {
            tracing::warn!(
                request_id = request.id().get(),
                user_id = request.user_id(),
                error = %error,
                "could not confirm onboarding target guild membership"
            );
            return Ok(false);
        }
    };
    if !member
        .roles
        .contains(&serenity::RoleId::new(verification.config.verified_role_id))
    {
        tracing::warn!(
            request_id = request.id().get(),
            user_id = request.user_id(),
            "onboarding target no longer has the verified role"
        );
        return Ok(false);
    }

    let identity = verify_repository::find_user_by_id(&data.db, request.user_id()).await?;
    let is_current = identity.as_ref().is_some_and(|identity| {
        identity.is_display_name_confirmed()
            && identity.email() == request.email()
            && identity.display_name() == request.display_name()
    });
    if !is_current {
        tracing::warn!(
            request_id = request.id().get(),
            user_id = request.user_id(),
            "onboarding request identity no longer matches persisted verification"
        );
    }
    Ok(is_current)
}

fn available_targets_for_member(
    targets: &[OnboardingTargetConfig],
    member: &serenity::Member,
) -> String {
    let available = targets
        .iter()
        .filter(|target| can_approve_target(member, target))
        .map(|target| format!("`{}`", target.key))
        .collect::<Vec<_>>();
    if available.is_empty() {
        "none".to_owned()
    } else {
        available.join(", ")
    }
}

fn onboarding_module(data: &Data) -> Result<&crate::OnboardingModule, Error> {
    data.modules
        .onboarding
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("onboarding module is disabled").into())
}

fn verification_module(data: &Data) -> Result<&crate::VerificationModule, Error> {
    data.modules
        .verification
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("verification module is disabled").into())
}

fn safe_provisioning_error(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

#[derive(Debug, Clone, Copy)]
enum OnboardingAction {
    Approve,
    Deny,
}

fn parse_onboarding_custom_id(custom_id: &str) -> Option<(OnboardingAction, OnboardingRequestId)> {
    if let Some(request_id) = custom_id.strip_prefix(APPROVE_PREFIX) {
        return OnboardingRequestId::parse(request_id)
            .map(|request_id| (OnboardingAction::Approve, request_id));
    }
    custom_id
        .strip_prefix(DENY_PREFIX)
        .and_then(OnboardingRequestId::parse)
        .map(|request_id| (OnboardingAction::Deny, request_id))
}

enum ProvisioningOutcome {
    Completed(OnboardingRequestId),
    Failed(OnboardingRequestId),
    Missing,
    Ineligible,
    Unauthorized,
    Unavailable(OnboardingStatus),
}

impl ProvisioningOutcome {
    fn message(&self) -> String {
        match self {
            Self::Completed(request_id) => {
                format!("Onboarding request `{request_id}` completed successfully.")
            }
            Self::Failed(request_id) => format!(
                "Onboarding request `{request_id}` was approved, but provisioning failed. Inspect it with `/onboarding_status` and retry it with `/onboarding_retry`."
            ),
            Self::Missing => "That onboarding request does not exist.".to_owned(),
            Self::Ineligible => "The target user is no longer eligible for this onboarding request. Confirm their Discord verification and persisted email before retrying.".to_owned(),
            Self::Unauthorized => {
                "You no longer have permission to provision that onboarding target.".to_owned()
            }
            Self::Unavailable(status) => {
                format!("That onboarding request cannot be provisioned while it is `{status}`.")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_positive_custom_ids() {
        assert!(matches!(
            parse_onboarding_custom_id("onboard:approve:123"),
            Some((OnboardingAction::Approve, request_id)) if request_id.get() == 123
        ));
        assert!(matches!(
            parse_onboarding_custom_id("onboard:deny:123"),
            Some((OnboardingAction::Deny, request_id)) if request_id.get() == 123
        ));
    }

    #[test]
    fn rejects_invalid_custom_ids() {
        assert!(parse_onboarding_custom_id("onboard:approve:0").is_none());
        assert!(parse_onboarding_custom_id("onboard:deny:-1").is_none());
        assert!(parse_onboarding_custom_id("other:123").is_none());
    }

    #[test]
    fn completion_message_contains_account_and_tailscale_setup() {
        let message = completion_message(
            "Officers",
            "member@example.com",
            "member-42",
            "https://authentik.example.com",
            "https://authentik.example.com/if/flow/sse-recovery-flow/?flow_token=temporary",
            "https://headscale.example.com",
            "https://headscale.example.com",
        );

        assert!(message.contains("Verified email: `member@example.com`"));
        assert!(message.contains("Username: `member-42`"));
        assert!(message.contains("Create your password within 30 minutes"));
        assert!(message.contains("flow_token=temporary"));
        assert!(message.contains("https://tailscale.com/download"));
        assert!(message.contains("tailscale login --login-server https://headscale.example.com"));
        assert!(!message.contains("Additional access:"));
    }
}
