use crate::{
    Data, Error,
    config::OnboardingTargetConfig,
    domain::onboarding::{
        ApproveOnboardingResult, DenyOnboardingResult, OnboardingRequest, OnboardingRequestId,
        StartOnboardingResult,
    },
};
use poise::{CreateReply, serenity_prelude as serenity};

type ApplicationContext<'a> = poise::ApplicationContext<'a, Data, Error>;

const APPROVE_PREFIX: &str = "onboard:approve:";
const DENY_PREFIX: &str = "onboard:deny:";

async fn ephemeral_reply(
    ctx: ApplicationContext<'_>,
    content: impl Into<String>,
) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(content).ephemeral(true))
        .await?;
    Ok(())
}

/// Requests Authentik and Headscale onboarding after Discord verification.
#[poise::command(slash_command)]
pub async fn onboard(
    ctx: ApplicationContext<'_>,
    user: serenity::User,
    target: String,
) -> Result<(), Error> {
    tracing::debug!(
        requested_by_user_id = ctx.author().id.get(),
        target_user_id = user.id.get(),
        target = %target,
        "received onboarding command"
    );

    let Some(guild_id) = ctx.guild_id() else {
        tracing::warn!(
            requested_by_user_id = ctx.author().id.get(),
            target_user_id = user.id.get(),
            target = %target,
            "onboarding requested outside a guild"
        );
        ephemeral_reply(ctx, "Onboarding only works inside a server.").await?;
        return Ok(());
    };

    let Some(verified_role_id) = ctx.data().config.discord.verified_role_id else {
        tracing::warn!("onboarding requested but VERIFIED_ROLE_ID is not configured");
        ephemeral_reply(ctx, "Onboarding is not configured yet.").await?;
        return Ok(());
    };

    let role_id = serenity::RoleId::new(verified_role_id);
    let requester = ctx.author();
    let requester_member = guild_id
        .member(ctx.serenity_context(), requester.id)
        .await?;
    let target_member = guild_id.member(ctx.serenity_context(), user.id).await?;
    let Some(target_config) = find_target(&ctx.data().config.onboarding.targets, &target) else {
        tracing::warn!(
            requested_by_user_id = requester.id.get(),
            target_user_id = user.id.get(),
            target = %target,
            "onboarding requested for unknown target"
        );
        ephemeral_reply(
            ctx,
            format!(
                "I do not recognize that onboarding target. Available targets: {}",
                available_targets(&ctx.data().config.onboarding.targets)
            ),
        )
        .await?;
        return Ok(());
    };

    if !can_approve_target(&requester_member, target_config) {
        tracing::warn!(
            requested_by_user_id = requester.id.get(),
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

    if !target_member.roles.contains(&role_id) {
        tracing::info!(
            requested_by_user_id = requester.id.get(),
            target_user_id = user.id.get(),
            target = %target_config.key,
            "target user is not verified"
        );
        ephemeral_reply(
            ctx,
            format!(
                "{} needs to run `/verify` before they can be onboarded.",
                user.name
            ),
        )
        .await?;
        return Ok(());
    }

    let verified_identity = {
        let verified_identities = ctx
            .data()
            .verified_identities
            .lock()
            .map_err(|err| format!("verified identity store lock poisoned: {err}"))?;

        verified_identities.get(user.id.get()).cloned()
    };

    let Some(verified_identity) = verified_identity else {
        tracing::info!(
            requested_by_user_id = requester.id.get(),
            target_user_id = user.id.get(),
            target = %target_config.key,
            "target user has verified role but no recorded verified identity"
        );
        ephemeral_reply(
            ctx,
            format!(
                "I need {} to refresh their verified email before onboarding. Ask them to run `/verify` again.",
                user.name
            ),
        )
        .await?;
        return Ok(());
    };

    let start_result = {
        let mut onboarding_store = ctx
            .data()
            .onboarding_store
            .lock()
            .map_err(|err| format!("onboarding store lock poisoned: {err}"))?;

        onboarding_store.start_or_reuse(
            user.id.get(),
            requester.id.get(),
            verified_identity.email().clone(),
            target_config.key.clone(),
            target_config.label.clone(),
        )
    };

    let (request, created) = match start_result {
        StartOnboardingResult::Created(request) => (request, true),
        StartOnboardingResult::Reused(request) => (request, false),
    };

    if created {
        send_review_request(ctx, &request).await?;
        tracing::info!(
            request_id = request.id().get(),
            user_id = user.id.get(),
            requested_by_user_id = requester.id.get(),
            target = %request.target_key(),
            email = %request.email(),
            "created onboarding request"
        );
    } else {
        tracing::info!(
            request_id = request.id().get(),
            user_id = user.id.get(),
            requested_by_user_id = requester.id.get(),
            target = %request.target_key(),
            email = %request.email(),
            "reusing pending onboarding request"
        );
    }

    ephemeral_reply(
        ctx,
        format!(
            "{}'s onboarding request is waiting for approval.",
            user.name
        ),
    )
    .await?;
    Ok(())
}

pub async fn handle_component_interaction(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    data: &Data,
) -> Result<(), Error> {
    let custom_id = interaction.data.custom_id.as_str();
    let Some((action, request_id)) = parse_onboarding_custom_id(custom_id) else {
        return Ok(());
    };
    tracing::debug!(
        request_id = request_id.get(),
        user_id = interaction.user.id.get(),
        action = ?action,
        "received onboarding component interaction"
    );

    interaction
        .create_response(
            serenity_ctx,
            serenity::CreateInteractionResponse::Defer(
                serenity::CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;

    if !interaction_is_from_officer(interaction, data).await? {
        tracing::warn!(
            request_id = request_id.get(),
            user_id = interaction.user.id.get(),
            "onboarding component interaction denied for missing manager role"
        );
        interaction
            .edit_response(
                serenity_ctx,
                serenity::EditInteractionResponse::new()
                    .content("You do not have permission to handle onboarding requests."),
            )
            .await?;
        return Ok(());
    }

    match action {
        OnboardingAction::Approve => {
            approve_request(serenity_ctx, interaction, data, request_id).await?
        }
        OnboardingAction::Deny => deny_request(serenity_ctx, interaction, data, request_id).await?,
    }

    Ok(())
}

async fn send_review_request(
    ctx: ApplicationContext<'_>,
    request: &OnboardingRequest,
) -> Result<(), Error> {
    let review_channel_id =
        serenity::ChannelId::new(ctx.data().config.onboarding.review_channel_id);
    let approve_custom_id = format!("{APPROVE_PREFIX}{}", request.id());
    let deny_custom_id = format!("{DENY_PREFIX}{}", request.id());
    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(approve_custom_id)
            .label("Approve")
            .style(serenity::ButtonStyle::Success),
        serenity::CreateButton::new(deny_custom_id)
            .label("Deny")
            .style(serenity::ButtonStyle::Danger),
    ])];

    review_channel_id
        .send_message(
            ctx.serenity_context(),
            serenity::CreateMessage::new()
                .content(review_message_content(request, "Pending officer review"))
                .components(components),
        )
        .await?;
    tracing::info!(
        request_id = request.id().get(),
        user_id = request.user_id(),
        requested_by_user_id = request.requested_by_user_id(),
        target = %request.target_key(),
        review_channel_id = review_channel_id.get(),
        "posted onboarding review request"
    );

    Ok(())
}

async fn approve_request(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    data: &Data,
    request_id: OnboardingRequestId,
) -> Result<(), Error> {
    let approval_result = {
        let mut onboarding_store = data
            .onboarding_store
            .lock()
            .map_err(|err| format!("onboarding store lock poisoned: {err}"))?;

        onboarding_store.approve(request_id, interaction.user.id.get())
    };

    let request = match approval_result {
        ApproveOnboardingResult::Approved(request) => request,
        ApproveOnboardingResult::Missing => {
            tracing::warn!(
                request_id = request_id.get(),
                approver_id = interaction.user.id.get(),
                "onboarding approval requested for missing request"
            );
            interaction
                .edit_response(
                    serenity_ctx,
                    serenity::EditInteractionResponse::new()
                        .content("That onboarding request no longer exists."),
                )
                .await?;
            return Ok(());
        }
        ApproveOnboardingResult::AlreadyHandled(request) => {
            tracing::info!(
                request_id = request.id().get(),
                approver_id = interaction.user.id.get(),
                "onboarding approval requested for already handled request"
            );
            interaction
                .edit_response(
                    serenity_ctx,
                    serenity::EditInteractionResponse::new()
                        .content("That onboarding request has already been handled."),
                )
                .await?;
            update_review_message(serenity_ctx, interaction, &request).await?;
            return Ok(());
        }
    };
    let Some(target_config) = target_for_request(data, &request) else {
        tracing::error!(
            request_id = request.id().get(),
            target = %request.target_key(),
            "onboarding target is no longer configured"
        );
        interaction
            .edit_response(
                serenity_ctx,
                serenity::EditInteractionResponse::new()
                    .content("That onboarding target is no longer configured."),
            )
            .await?;
        return Ok(());
    };

    tracing::info!(
        request_id = request.id().get(),
        user_id = request.user_id(),
        target = %request.target_key(),
        authentik_group_uuid = %target_config.authentik_group_uuid,
        "provisioning approved onboarding request"
    );
    let authentik_user = data
        .authentik_client
        .find_or_create_user(
            request.email(),
            request.user_id(),
            &request.email().to_string(),
        )
        .await?;
    data.authentik_client
        .add_user_to_group(&authentik_user, &target_config.authentik_group_uuid)
        .await?;

    update_review_message(serenity_ctx, interaction, &request).await?;

    if let Err(err) = serenity::UserId::new(request.user_id())
        .direct_message(
            serenity_ctx,
            serenity::CreateMessage::new().content(format!(
                "Your SSE infra onboarding was approved.\n\nAuthentik: {}\nHeadscale: {}",
                data.authentik_client.login_url(),
                data.config.onboarding.headscale_login_url
            )),
        )
        .await
    {
        tracing::warn!(
            request_id = request.id().get(),
            user_id = request.user_id(),
            error = %err,
            "failed to DM onboarding approval"
        );
    }

    interaction
        .edit_response(
            serenity_ctx,
            serenity::EditInteractionResponse::new().content("Onboarding approved."),
        )
        .await?;

    tracing::info!(
        request_id = request.id().get(),
        user_id = request.user_id(),
        requested_by_user_id = request.requested_by_user_id(),
        target = %request.target_key(),
        approver_id = interaction.user.id.get(),
        authentik_user_id = authentik_user.pk,
        "approved onboarding request"
    );

    Ok(())
}

async fn deny_request(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    data: &Data,
    request_id: OnboardingRequestId,
) -> Result<(), Error> {
    let deny_result = {
        let mut onboarding_store = data
            .onboarding_store
            .lock()
            .map_err(|err| format!("onboarding store lock poisoned: {err}"))?;

        onboarding_store.deny(request_id, interaction.user.id.get())
    };

    match deny_result {
        DenyOnboardingResult::Denied(request) => {
            update_review_message(serenity_ctx, interaction, &request).await?;
            interaction
                .edit_response(
                    serenity_ctx,
                    serenity::EditInteractionResponse::new().content("Onboarding denied."),
                )
                .await?;

            tracing::info!(
                request_id = request.id().get(),
                user_id = request.user_id(),
                requested_by_user_id = request.requested_by_user_id(),
                target = %request.target_key(),
                approver_id = interaction.user.id.get(),
                "denied onboarding request"
            );
        }
        DenyOnboardingResult::Missing => {
            tracing::warn!(
                request_id = request_id.get(),
                approver_id = interaction.user.id.get(),
                "onboarding denial requested for missing request"
            );
            interaction
                .edit_response(
                    serenity_ctx,
                    serenity::EditInteractionResponse::new()
                        .content("That onboarding request no longer exists."),
                )
                .await?;
        }
        DenyOnboardingResult::AlreadyHandled(request) => {
            tracing::info!(
                request_id = request.id().get(),
                approver_id = interaction.user.id.get(),
                "onboarding denial requested for already handled request"
            );
            update_review_message(serenity_ctx, interaction, &request).await?;
            interaction
                .edit_response(
                    serenity_ctx,
                    serenity::EditInteractionResponse::new()
                        .content("That onboarding request has already been handled."),
                )
                .await?;
        }
    }

    Ok(())
}

async fn interaction_is_from_officer(
    interaction: &serenity::ComponentInteraction,
    data: &Data,
) -> Result<bool, Error> {
    let custom_id = interaction.data.custom_id.as_str();
    let Some((_, request_id)) = parse_onboarding_custom_id(custom_id) else {
        return Ok(false);
    };
    let request = {
        let onboarding_store = data
            .onboarding_store
            .lock()
            .map_err(|err| format!("onboarding store lock poisoned: {err}"))?;

        onboarding_store.get(request_id).cloned()
    };
    let Some(request) = request else {
        return Ok(false);
    };
    let Some(target_config) = target_for_request(data, &request) else {
        return Ok(false);
    };

    Ok(interaction
        .member
        .as_ref()
        .is_some_and(|member| has_any_role(member, &target_config.approver_role_ids)))
}

async fn update_review_message(
    serenity_ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    request: &OnboardingRequest,
) -> Result<(), Error> {
    interaction
        .channel_id
        .edit_message(
            serenity_ctx,
            interaction.message.id,
            serenity::EditMessage::new()
                .content(review_message_content(request, "Handled"))
                .components(vec![]),
        )
        .await?;

    Ok(())
}

fn review_message_content(request: &OnboardingRequest, fallback_status: &str) -> String {
    let status = match request.status() {
        crate::domain::onboarding::OnboardingStatus::Pending => "Pending officer review".to_owned(),
        crate::domain::onboarding::OnboardingStatus::Denied { approver_id } => {
            format!("Denied by <@{approver_id}>")
        }
        crate::domain::onboarding::OnboardingStatus::Approved { approver_id } => {
            format!("Approved by <@{approver_id}>")
        }
    };

    format!(
        "**Infra onboarding request**\nStatus: {}\nTarget: {}\nUser: <@{}>\nRequested by: <@{}>\nVerified email: `{}`\nRequest ID: `{}`",
        if status.is_empty() {
            fallback_status.to_owned()
        } else {
            status
        },
        request.target_label(),
        request.user_id(),
        request.requested_by_user_id(),
        request.email(),
        request.id()
    )
}

fn find_target<'a>(
    targets: &'a [OnboardingTargetConfig],
    key: &str,
) -> Option<&'a OnboardingTargetConfig> {
    targets
        .iter()
        .find(|target| target.key.eq_ignore_ascii_case(key.trim()))
}

fn target_for_request<'a>(
    data: &'a Data,
    request: &OnboardingRequest,
) -> Option<&'a OnboardingTargetConfig> {
    find_target(&data.config.onboarding.targets, request.target_key())
}

fn can_approve_target(member: &serenity::Member, target_config: &OnboardingTargetConfig) -> bool {
    has_any_role(member, &target_config.approver_role_ids)
}

fn has_any_role(member: &serenity::Member, role_ids: &[u64]) -> bool {
    role_ids
        .iter()
        .map(|role_id| serenity::RoleId::new(*role_id))
        .any(|role_id| member.roles.contains(&role_id))
}

fn available_targets(targets: &[OnboardingTargetConfig]) -> String {
    targets
        .iter()
        .map(|target| format!("`{}`", target.key))
        .collect::<Vec<_>>()
        .join(", ")
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

    if let Some(request_id) = custom_id.strip_prefix(DENY_PREFIX) {
        return OnboardingRequestId::parse(request_id)
            .map(|request_id| (OnboardingAction::Deny, request_id));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_approve_custom_id() {
        let parsed = parse_onboarding_custom_id("onboard:approve:123");

        assert!(matches!(
            parsed,
            Some((OnboardingAction::Approve, request_id)) if request_id.get() == 123
        ));
    }

    #[test]
    fn parses_deny_custom_id() {
        let parsed = parse_onboarding_custom_id("onboard:deny:123");

        assert!(matches!(
            parsed,
            Some((OnboardingAction::Deny, request_id)) if request_id.get() == 123
        ));
    }
}
