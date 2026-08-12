pub mod commands;
pub mod config;
pub mod domain;
pub mod integrations;
pub mod logging;

use integrations::{authentik::AuthentikClient, email::EmailSender};
use poise::serenity_prelude as serenity;
use secrecy::ExposeSecret;
use std::sync::Mutex;

use config::AppConfig;
use domain::{
    onboarding::OnboardingStore,
    verification::{VerificationStore, VerifiedIdentityStore},
};

pub struct Data {
    pub config: AppConfig,
    pub email_sender: EmailSender,
    pub verification_store: Mutex<VerificationStore>,
    pub verified_identities: Mutex<VerifiedIdentityStore>,
    pub onboarding_store: Mutex<OnboardingStore>,
    pub authentik_client: AuthentikClient,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

pub fn data_from_config(config: AppConfig) -> anyhow::Result<Data> {
    let email_sender = EmailSender::new(config.email.clone())?;
    let authentik_client = AuthentikClient::new(config.authentik.clone())?;

    Ok(Data {
        config,
        email_sender,
        verification_store: Mutex::new(VerificationStore::new()),
        verified_identities: Mutex::new(VerifiedIdentityStore::new()),
        onboarding_store: Mutex::new(OnboardingStore::new()),
        authentik_client,
    })
}

pub fn bot_token(config: &AppConfig) -> String {
    config.discord.bot_token.expose_secret().to_owned()
}

pub fn guild_id(config: &AppConfig) -> Option<serenity::GuildId> {
    config.discord.guild_id.map(serenity::GuildId::new)
}
