use crate::{
    Data, Error,
    domain::verification::{CheckCodeResult, EmailAddress, StartVerificationResult},
};
use poise::{CreateReply, serenity_prelude as serenity};
use std::time::Duration;

type ApplicationContext<'a> = poise::ApplicationContext<'a, Data, Error>;

#[derive(Debug, poise::Modal)]
#[name = "Enter verification code"]
struct VerificationCodeModal {
    #[name = "Verification code"]
    #[placeholder = "123456"]
    #[min_length = 6]
    #[max_length = 6]
    code: String,
}

async fn ephemeral_reply(
    ctx: ApplicationContext<'_>,
    content: impl Into<String>,
) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(content).ephemeral(true))
        .await?;
    Ok(())
}

async fn prompt_retry_modal(
    ctx: ApplicationContext<'_>,
    user_id: u64,
    attempts_remaining: u8,
) -> Result<Option<VerificationCodeModal>, Error> {
    let custom_id = format!("verify_retry:{user_id}");
    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(&custom_id)
            .label("Try again")
            .style(serenity::ButtonStyle::Primary),
    ])];

    ctx.send(
        CreateReply::default()
            .content(format!(
                "That verification code is incorrect. You have {attempts_remaining} attempts remaining."
            ))
            .components(components)
            .ephemeral(true),
    )
    .await?;

    let Some(interaction) = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .timeout(Duration::from_secs(60 * 60))
        .filter(move |interaction| {
            interaction.data.custom_id == custom_id && interaction.user.id.get() == user_id
        })
        .await
    else {
        tracing::info!(user_id, "verification retry button timed out");
        return Ok(None);
    };

    let modal_data = poise::execute_modal_on_component_interaction::<VerificationCodeModal>(
        ctx,
        interaction,
        None,
        Some(Duration::from_secs(60 * 60)),
    )
    .await?;

    Ok(modal_data)
}

/// Starts email verification by generating a one-time code.
#[poise::command(slash_command)]
pub async fn verify(ctx: ApplicationContext<'_>, email: String) -> std::result::Result<(), Error> {
    let email = match EmailAddress::parse(&email) {
        Ok(email) => email,
        Err(err) => {
            ephemeral_reply(ctx, format!("That does not look like a valid email: {err}")).await?;
            return Ok(());
        }
    };

    let Some(guild_id) = ctx.guild_id() else {
        ephemeral_reply(ctx, "Verification only works inside a server.").await?;
        return Ok(());
    };

    let Some(verified_role_id) = ctx.data().config.discord.verified_role_id else {
        tracing::warn!("verification requested but VERIFIED_ROLE_ID is not configured");
        ephemeral_reply(ctx, "Verification is not configured yet.").await?;
        return Ok(());
    };

    let user = ctx.author();
    let role_id = serenity::RoleId::new(verified_role_id);

    if user
        .has_role(ctx.serenity_context(), guild_id, role_id)
        .await?
    {
        let has_recorded_identity = {
            let verified_identities = ctx
                .data()
                .verified_identities
                .lock()
                .map_err(|err| format!("verified identity store lock poisoned: {err}"))?;

            verified_identities.get(user.id.get()).is_some()
        };

        if has_recorded_identity {
            tracing::info!(user_id = %user.id, guild_id = %guild_id, "user is already verified");
            ephemeral_reply(ctx, "You are already verified.").await?;
            return Ok(());
        }

        tracing::info!(
            user_id = %user.id,
            guild_id = %guild_id,
            "user has verified role but no recorded verified identity; refreshing verification"
        );
    }

    let user_id = user.id.get();
    let start_result = {
        let mut verification_store = ctx
            .data()
            .verification_store
            .lock()
            .map_err(|err| format!("verification store lock poisoned: {err}"))?;

        verification_store.start_or_reuse(user_id, email)
    };

    let (attempt, should_send_email) = match start_result {
        StartVerificationResult::Created(attempt) => (attempt, true),
        StartVerificationResult::Reused(attempt) => {
            tracing::info!(
                user_id = %user.id,
                guild_id = %guild_id,
                email = %attempt.email(),
                "reusing pending verification attempt"
            );
            (attempt, false)
        }
    };

    if should_send_email {
        ctx.data()
            .email_sender
            .send_verification_code(attempt.email(), attempt.code())
            .await?;

        tracing::info!(
            user_id = attempt.user_id(),
            guild_id = %guild_id,
            role_id = %role_id,
            email = %attempt.email(),
            "created verification attempt and sent verification email"
        );
    }

    let Some(modal_data) = ({
        poise::execute_modal::<_, _, VerificationCodeModal>(
            ctx,
            None,
            Some(Duration::from_secs(60 * 60)),
        )
        .await?
    }) else {
        tracing::info!(user_id = %user.id, guild_id = %guild_id, "verification modal timed out or was dismissed");
        return Ok(());
    };

    let mut submitted_code = modal_data.code;
    let attempt = loop {
        let verification_result = {
            let mut verification_store = ctx
                .data()
                .verification_store
                .lock()
                .map_err(|err| format!("verification store lock poisoned: {err}"))?;

            verification_store.check_code(user_id, &submitted_code)
        };

        match verification_result {
            CheckCodeResult::Accepted(attempt) => break attempt,
            CheckCodeResult::Missing => {
                ephemeral_reply(
                    ctx,
                    "I could not find a pending verification attempt. Please run `/verify` again.",
                )
                .await?;
                return Ok(());
            }
            CheckCodeResult::Expired => {
                tracing::info!(user_id = %user.id, guild_id = %guild_id, "verification attempt expired");
                ephemeral_reply(
                    ctx,
                    "That verification code expired. Please run `/verify` again.",
                )
                .await?;
                return Ok(());
            }
            CheckCodeResult::Incorrect {
                attempts_remaining,
                has_attempts_remaining,
            } => {
                tracing::info!(
                    user_id = %user.id,
                    guild_id = %guild_id,
                    attempts_remaining,
                    "submitted incorrect verification code"
                );

                if has_attempts_remaining {
                    let Some(modal_data) =
                        prompt_retry_modal(ctx, user_id, attempts_remaining).await?
                    else {
                        return Ok(());
                    };

                    submitted_code = modal_data.code;
                } else {
                    ephemeral_reply(
                        ctx,
                        "That verification code is incorrect, and you have no attempts remaining. Please run `/verify` again to request a new code.",
                    )
                    .await?;
                    return Ok(());
                }
            }
        }
    };

    let member = guild_id.member(ctx.serenity_context(), user.id).await?;
    member.add_role(ctx.serenity_context(), role_id).await?;
    {
        let mut verified_identities = ctx
            .data()
            .verified_identities
            .lock()
            .map_err(|err| format!("verified identity store lock poisoned: {err}"))?;

        verified_identities.record_verified(user_id, attempt.email().clone());
    }

    tracing::info!(
        user_id = %user.id,
        guild_id = %guild_id,
        role_id = %role_id,
        email = %attempt.email(),
        "verified user"
    );
    ephemeral_reply(ctx, "You have been successfully verified.").await?;
    Ok(())
}
