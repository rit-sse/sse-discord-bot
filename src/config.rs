use std::env::VarError;

use secrecy::SecretString;

#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub bot_token: SecretString,
    pub guild_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub discord: DiscordConfig,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();
        tracing::info!("Loading config...");
        let bot_token = std::env::var("BOT_TOKEN")
            .map(SecretString::from)
            .map_err(|_| anyhow::anyhow!("missing BOT_TOKEN"))?;
        let guild_id = match std::env::var("GUILD_ID") {
            Ok(value) => Some(value.parse()?),
            Err(VarError::NotPresent) => None,
            Err(err) => return Err(err.into()),
        };

        tracing::info!("Config loaded!");

        Ok(Self {
            discord: DiscordConfig {
                bot_token,
                guild_id,
            },
        })
    }
}
