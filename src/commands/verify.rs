use crate::{
    Data, Error,
    db::{
        pending_verification_repository::{
            self, CheckPendingVerificationResult, StartPendingVerificationResult,
        },
        verify_repository,
    },
    domain::verification::{EmailAddress, VerificationCode},
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
    let user_id = user.id.get();
    let role_id = serenity::RoleId::new(verified_role_id);
    let has_verified_role = user
        .has_role(ctx.serenity_context(), guild_id, role_id)
        .await?;
    let persisted_identity = verify_repository::find_user_by_id(&ctx.data().db, user_id).await?;

    if let Some(identity) = persisted_identity {
        if !has_verified_role {
            let member = guild_id.member(ctx.serenity_context(), user.id).await?;
            member.add_role(ctx.serenity_context(), role_id).await?;
            tracing::info!(
                user_id = %user.id,
                guild_id = %guild_id,
                role_id = %role_id,
                email = %identity.email(),
                "restored verified role from persisted identity"
            );
        } else {
            tracing::info!(user_id = %user.id, guild_id = %guild_id, "user is already verified");
        }

        ephemeral_reply(ctx, "You are already verified.").await?;
        return Ok(());
    }

    if has_verified_role {
        tracing::info!(
            user_id = %user.id,
            guild_id = %guild_id,
            "user has verified role but no recorded verified identity; refreshing verification"
        );
    }

    let email = match EmailAddress::parse(&email) {
        Ok(email) => email,
        Err(err) => {
            ephemeral_reply(ctx, format!("That does not look like a valid email: {err}")).await?;
            return Ok(());
        }
    };

    let code = VerificationCode::generate();
    let code_hash = code.hash()?;
    let start_result = pending_verification_repository::start_or_reuse(
        &ctx.data().db,
        user_id,
        &email,
        &code_hash,
    )
    .await?;

    match start_result {
        StartPendingVerificationResult::Created => {
            if let Err(error) = ctx
                .data()
                .email_sender
                .send_verification_code(&email, &code)
                .await
            {
                if let Err(cleanup_error) = pending_verification_repository::delete_if_code_matches(
                    &ctx.data().db,
                    user_id,
                    &code_hash,
                )
                .await
                {
                    tracing::error!(
                        user_id = %user.id,
                        guild_id = %guild_id,
                        error = %cleanup_error,
                        "failed to clean up verification after email delivery failure"
                    );
                }

                return Err(error.into());
            }

            tracing::info!(
                user_id,
                guild_id = %guild_id,
                role_id = %role_id,
                email = %email,
                "created verification attempt and sent verification email"
            );
        }
        StartPendingVerificationResult::Reused => {
            tracing::info!(
                user_id = %user.id,
                guild_id = %guild_id,
                "reusing pending verification attempt"
            );
        }
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
    let verified_email = loop {
        let verification_result =
            pending_verification_repository::check_code(&ctx.data().db, user_id, &submitted_code)
                .await?;

        match verification_result {
            CheckPendingVerificationResult::Accepted { email } => break email,
            CheckPendingVerificationResult::Missing => {
                ephemeral_reply(
                    ctx,
                    "I could not find a pending verification attempt. Please run `/verify` again.",
                )
                .await?;
                return Ok(());
            }
            CheckPendingVerificationResult::Expired => {
                tracing::info!(user_id = %user.id, guild_id = %guild_id, "verification attempt expired");
                ephemeral_reply(
                    ctx,
                    "That verification code expired. Please run `/verify` again.",
                )
                .await?;
                return Ok(());
            }
            CheckPendingVerificationResult::Incorrect {
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

    tracing::info!(
        user_id = %user.id,
        guild_id = %guild_id,
        role_id = %role_id,
        email = %verified_email,
        "verified user"
    );
    ephemeral_reply(ctx, "You have been successfully verified.").await?;
    Ok(())
}
