use crate::{Context, Error};
use anyhow::{Result, anyhow};
use poise::serenity_prelude as serenity;
use std::fmt;

#[derive(Debug, Clone)]
pub struct EmailAddress {
    user: String,
    domain: String,
}

impl EmailAddress {
    pub fn parse(email: &str) -> Result<EmailAddress> {
        let (user, domain) = email
            .split_once('@')
            .ok_or(anyhow::anyhow!("Invalid email: missing '@' symbol"))?;

        if user.is_empty() || domain.is_empty() {
            return Err(anyhow!("Invalid Email: missing user or domain"));
        }

        Ok(Self {
            user: String::from(user),
            domain: String::from(domain),
        })
    }
}

impl fmt::Display for EmailAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}", self.user, self.domain)
    }
}

/// Verifies you by assigning the configured verified role.
#[poise::command(slash_command)]
pub async fn verify(ctx: Context<'_>, email: String) -> std::result::Result<(), Error> {
    let email = match EmailAddress::parse(&email) {
        Ok(email) => email,
        Err(err) => {
            ctx.say(format!("That does not look like a valid email: {err}"))
                .await?;
            return Ok(());
        }
    };

    let Some(guild_id) = ctx.guild_id() else {
        ctx.say("Verification only works inside a server.").await?;
        return Ok(());
    };

    let Some(verified_role_id) = ctx.data().config.discord.verified_role_id else {
        tracing::warn!("verification requested but VERIFIED_ROLE_ID is not configured");
        ctx.say("Verification is not configured yet.").await?;
        return Ok(());
    };

    let user = ctx.author();
    let role_id = serenity::RoleId::new(verified_role_id);

    if user
        .has_role(ctx.serenity_context(), guild_id, role_id)
        .await?
    {
        tracing::info!(user_id = %user.id, guild_id = %guild_id, "user is already verified");
        ctx.say("You are already verified.").await?;
        return Ok(());
    }

    let member = guild_id.member(ctx.serenity_context(), user.id).await?;
    member.add_role(ctx.serenity_context(), role_id).await?;

    tracing::info!(user_id = %user.id, guild_id = %guild_id, role_id = %role_id, email = %email, "verified user");
    ctx.say("You have been successfully verified.").await?;
    Ok(())
}
