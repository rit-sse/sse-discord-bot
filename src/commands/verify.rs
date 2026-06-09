use crate::{
    Data, Error,
    verification::{EmailAddress, VerificationAttempt},
};
use poise::{CreateReply, serenity_prelude as serenity};

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
        tracing::info!(user_id = %user.id, guild_id = %guild_id, "user is already verified");
        ephemeral_reply(ctx, "You are already verified.").await?;
        return Ok(());
    }

    let attempt = VerificationAttempt::new(user.id.get(), email);

    // Temporary local testing hook: remove the code field once email sending is wired in.
    tracing::info!(
        user_id = attempt.user_id(),
        guild_id = %guild_id,
        role_id = %role_id,
        email = %attempt.email(),
        verification_code = %attempt.code(),
        "created verification code"
    );

    {
        let mut pending_verifications = ctx
            .data()
            .pending_verifications
            .lock()
            .map_err(|err| format!("pending verification store lock poisoned: {err}"))?;
        pending_verifications.insert(attempt.user_id(), attempt);
    }

    let Some(modal_data) = ({
        use poise::Modal as _;
        VerificationCodeModal::execute(ctx).await?
    }) else {
        tracing::info!(user_id = %user.id, guild_id = %guild_id, "verification modal timed out or was dismissed");
        return Ok(());
    };

    let Some(attempt) = ctx
        .data()
        .pending_verifications
        .lock()
        .map_err(|err| format!("pending verification store lock poisoned: {err}"))?
        .remove(&user.id.get())
    else {
        ephemeral_reply(
            ctx,
            "I could not find a pending verification attempt. Please run `/verify` again.",
        )
        .await?;
        return Ok(());
    };

    if !attempt.code().matches(&modal_data.code) {
        tracing::info!(user_id = %user.id, guild_id = %guild_id, "submitted incorrect verification code");
        ephemeral_reply(
            ctx,
            "That verification code is incorrect. Please run `/verify` again.",
        )
        .await?;
        return Ok(());
    }

    let member = guild_id.member(ctx.serenity_context(), user.id).await?;
    member.add_role(ctx.serenity_context(), role_id).await?;

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
