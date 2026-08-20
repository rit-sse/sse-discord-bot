pub mod commands;
pub mod config;
pub mod db;
pub mod domain;
pub mod integrations;
pub mod logging;

use integrations::{authentik::AuthentikClient, email::EmailSender};
use poise::serenity_prelude as serenity;
use secrecy::ExposeSecret;
use sqlx::PgPool;
use std::sync::Mutex;

use config::{AppConfig, OnboardingConfig, VerificationConfig};
use domain::onboarding::OnboardingStore;

pub struct VerificationModule {
    pub config: VerificationConfig,
    pub email_sender: EmailSender,
}

pub struct OnboardingModule {
    pub config: OnboardingConfig,
    pub store: Mutex<OnboardingStore>,
    pub authentik_client: AuthentikClient,
}

#[derive(Default)]
pub struct EnabledModules {
    pub verification: Option<VerificationModule>,
    pub onboarding: Option<OnboardingModule>,
}

pub struct Data {
    pub modules: EnabledModules,
    pub db: PgPool,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

pub async fn data_from_config(config: &AppConfig) -> anyhow::Result<Data> {
    tracing::debug!("initializing application dependencies");
    let db = db::connect(&config.db).await?;
    let verification = config
        .verification
        .as_ref()
        .map(|verification_config| {
            Ok::<_, anyhow::Error>(VerificationModule {
                config: verification_config.clone(),
                email_sender: EmailSender::new(verification_config.email.clone())?,
            })
        })
        .transpose()?;
    let onboarding = config
        .onboarding
        .as_ref()
        .map(|onboarding_config| {
            Ok::<_, anyhow::Error>(OnboardingModule {
                config: onboarding_config.clone(),
                store: Mutex::new(OnboardingStore::new()),
                authentik_client: AuthentikClient::new(onboarding_config.authentik.clone())?,
            })
        })
        .transpose()?;

    tracing::debug!("application dependencies initialized");

    Ok(Data {
        modules: EnabledModules {
            verification,
            onboarding,
        },
        db,
    })
}

pub fn bot_token(config: &AppConfig) -> String {
    config.discord.bot_token.expose_secret().to_owned()
}

pub fn guild_id(config: &AppConfig) -> Option<serenity::GuildId> {
    config.discord.guild_id.map(serenity::GuildId::new)
}
