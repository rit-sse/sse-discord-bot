use std::env::VarError;

use secrecy::SecretString;

#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub bot_token: SecretString,
    pub guild_id: Option<u64>,
    pub verified_role_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_starttls: bool,
    pub smtp_username: SecretString,
    pub smtp_password: SecretString,
    pub from_address: String,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub discord: DiscordConfig,
    pub email: EmailConfig,
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
        let verified_role_id = match std::env::var("VERIFIED_ROLE_ID") {
            Ok(value) => Some(value.parse()?),
            Err(VarError::NotPresent) => None,
            Err(err) => return Err(err.into()),
        };
        let smtp_host =
            std::env::var("SMTP_HOST").map_err(|_| anyhow::anyhow!("missing SMTP_HOST"))?;
        let smtp_port = match std::env::var("SMTP_PORT") {
            Ok(value) => value.parse()?,
            Err(VarError::NotPresent) => 587,
            Err(err) => return Err(err.into()),
        };
        let smtp_starttls = match std::env::var("SMTP_STARTTLS") {
            Ok(value) => value.parse()?,
            Err(VarError::NotPresent) => true,
            Err(err) => return Err(err.into()),
        };
        let smtp_username = std::env::var("SMTP_USERNAME")
            .map(SecretString::from)
            .map_err(|_| anyhow::anyhow!("missing SMTP_USERNAME"))?;
        let smtp_password = std::env::var("SMTP_PASSWORD")
            .map(SecretString::from)
            .map_err(|_| anyhow::anyhow!("missing SMTP_PASSWORD"))?;
        let from_address = std::env::var("EMAIL_FROM_ADDRESS")
            .map_err(|_| anyhow::anyhow!("missing EMAIL_FROM_ADDRESS"))?;

        tracing::info!("Config loaded!");

        Ok(Self {
            discord: DiscordConfig {
                bot_token,
                guild_id,
                verified_role_id,
            },
            email: EmailConfig {
                smtp_host,
                smtp_port,
                smtp_starttls,
                smtp_username,
                smtp_password,
                from_address,
            },
        })
    }
}
