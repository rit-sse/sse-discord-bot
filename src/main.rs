use email::EmailSender;
use poise::serenity_prelude as serenity;
use secrecy::ExposeSecret;
use std::{collections::HashMap, sync::Mutex};
use verification::VerificationAttempt;

pub mod commands;
mod config;
pub mod email;
mod logging;
pub mod verification;
pub struct Data {
    config: config::AppConfig,
    pub email_sender: EmailSender,
    pending_verifications: Mutex<HashMap<u64, VerificationAttempt>>,
} // User data, which is stored and accessible in all command invocations
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init();

    let config = config::AppConfig::from_env()?;
    let intents = serenity::GatewayIntents::non_privileged();
    let bot_token = config.discord.bot_token.expose_secret().to_owned();
    let guild_id = config.discord.guild_id.map(serenity::GuildId::new);
    let email_sender = EmailSender::new(config.email.clone())?;
    let data_config = config.clone();

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                if let Some(guild_id) = guild_id {
                    poise::builtins::register_in_guild(
                        ctx,
                        &framework.options().commands,
                        guild_id,
                    )
                    .await?;
                    tracing::info!(%guild_id, "registered guild slash commands");
                } else {
                    poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                    tracing::info!("registered global slash commands");
                }

                Ok(Data {
                    config: data_config,
                    email_sender,
                    pending_verifications: Mutex::new(HashMap::new()),
                })
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(bot_token, intents)
        .framework(framework)
        .await?;

    client.start().await?;
    Ok(())
}
