use crate::{Context, Error};
use poise::serenity_prelude as serenity;
#[poise::command(slash_command)]
pub async fn verify(ctx: Context<'_>, user: Option<serenity::User>) -> Result<(), Error> {
    let user = user.as_ref().unwrap_or_else(|| ctx.author());
    let res = format!("Verifying: {}", user.name);
    tracing::info!("{} ran command verify", user.name);
    ctx.say(res).await?;
    Ok(())
}
