use std::{
    env::{self, VarError},
    error::Error,
    str::FromStr,
};

use anyhow::{Context, Result};
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
pub struct AuthentikConfig {
    pub base_url: String,
    pub client_id: String,
    pub username: String,
    pub password: SecretString,
    pub login_url: String,
}

#[derive(Debug, Clone)]
pub struct OnboardingConfig {
    pub review_channel_id: u64,
    pub officer_role_id: u64,
    pub headscale_login_url: String,
    pub targets: Vec<OnboardingTargetConfig>,
}

#[derive(Debug, Clone)]
pub struct OnboardingTargetConfig {
    pub key: String,
    pub label: String,
    pub authentik_group_uuid: String,
    pub approver_role_ids: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub discord: DiscordConfig,
    pub email: EmailConfig,
    pub authentik: AuthentikConfig,
    pub onboarding: OnboardingConfig,
    pub db: DBConfig,
}

#[derive(Debug, Clone)]
pub struct DBConfig {
    pub url: SecretString,
    pub max_connections: u32,
}

fn required<T>(key: &'static str) -> Result<T>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    env::var(key)
        .with_context(|| format!("missing {key}"))?
        .parse()
        .with_context(|| format!("invalid {key}"))
}

fn optional<T>(key: &'static str) -> Result<Option<T>>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    match env::var(key) {
        Ok(value) => value
            .parse()
            .with_context(|| format!("invalid {key}"))
            .map(Some),
        Err(VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed reading {key}")),
    }
}

fn defaulted<T>(key: &'static str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    Ok(optional(key)?.unwrap_or(default))
}

fn secret(key: &'static str) -> Result<SecretString> {
    required::<String>(key).map(SecretString::from)
}

impl DiscordConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            bot_token: secret("BOT_TOKEN")?,
            guild_id: optional("GUILD_ID")?,
            verified_role_id: optional("VERIFIED_ROLE_ID")?,
        })
    }
}

impl EmailConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            smtp_host: required("SMTP_HOST")?,
            smtp_port: defaulted("SMTP_PORT", 587)?,
            smtp_starttls: defaulted("SMTP_STARTTLS", true)?,
            smtp_username: secret("SMTP_USERNAME")?,
            smtp_password: secret("SMTP_PASSWORD")?,
            from_address: required("EMAIL_FROM_ADDRESS")?,
        })
    }
}

impl AuthentikConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            base_url: required("AUTHENTIK_BASE_URL")?,
            client_id: required("AUTHENTIK_CLIENT_ID")?,
            username: required("AUTHENTIK_USERNAME")?,
            password: secret("AUTHENTIK_PASSWORD")?,
            login_url: required("AUTHENTIK_LOGIN_URL")?,
        })
    }
}

impl OnboardingConfig {
    fn from_env() -> Result<Self> {
        let officer_role_id = required("ONBOARDING_OFFICER_ROLE_ID")?;
        let targets = match optional::<String>("ONBOARDING_TARGETS")? {
            Some(value) => {
                tracing::debug!("loading onboarding targets from ONBOARDING_TARGETS");
                parse_onboarding_targets(&value)?
            }
            None => {
                tracing::debug!(
                    "ONBOARDING_TARGETS missing; using legacy single headscale target config"
                );
                vec![OnboardingTargetConfig {
                    key: "headscale".to_owned(),
                    label: "Headscale".to_owned(),
                    authentik_group_uuid: required("AUTHENTIK_HEADSCALE_GROUP_UUID")?,
                    approver_role_ids: vec![officer_role_id],
                }]
            }
        };

        Ok(Self {
            review_channel_id: required("ONBOARDING_REVIEW_CHANNEL_ID")?,
            officer_role_id,
            headscale_login_url: required("HEADSCALE_LOGIN_URL")?,
            targets,
        })
    }
}

impl DBConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            url: secret("DATABASE_URL")?,
            max_connections: defaulted("DATABASE_MAX_CONNECTIONS", 10)?,
        })
    }
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        tracing::info!("loading config...");
        if cfg!(debug_assertions) {
            tracing::debug!("debug build detected; verbose development logging is enabled");
        }

        Ok(Self {
            discord: DiscordConfig::from_env()?,
            email: EmailConfig::from_env()?,
            authentik: AuthentikConfig::from_env()?,
            onboarding: OnboardingConfig::from_env()?,
            db: DBConfig::from_env()?,
        })
    }
}

fn parse_onboarding_targets(value: &str) -> anyhow::Result<Vec<OnboardingTargetConfig>> {
    let targets = value
        .split(';')
        .filter(|target| !target.trim().is_empty())
        .map(parse_onboarding_target)
        .collect::<anyhow::Result<Vec<_>>>()?;

    if targets.is_empty() {
        return Err(anyhow::anyhow!("ONBOARDING_TARGETS cannot be empty"));
    }

    Ok(targets)
}

fn parse_onboarding_target(value: &str) -> anyhow::Result<OnboardingTargetConfig> {
    let parts = value.split('|').collect::<Vec<_>>();

    if parts.len() != 4 && parts.len() != 5 {
        return Err(anyhow::anyhow!(
            "invalid ONBOARDING_TARGETS entry; expected key|label|authentik_group_uuid|manager_role_ids"
        ));
    }

    let key = parts[0].trim().to_owned();
    let label = parts[1].trim().to_owned();
    let authentik_group_uuid = parts[2].trim().to_owned();
    let approver_role_ids_part = if parts.len() == 4 { parts[3] } else { parts[4] };

    if key.is_empty() || label.is_empty() || authentik_group_uuid.is_empty() {
        return Err(anyhow::anyhow!(
            "ONBOARDING_TARGETS key, label, and authentik group UUID cannot be empty"
        ));
    }

    let approver_role_ids = parse_role_ids(approver_role_ids_part)?;

    if approver_role_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "ONBOARDING_TARGETS manager_role_ids cannot be empty"
        ));
    }

    tracing::debug!(
        target = %key,
        label = %label,
        manager_role_count = approver_role_ids.len(),
        "parsed onboarding target"
    );

    Ok(OnboardingTargetConfig {
        key,
        label,
        authentik_group_uuid,
        approver_role_ids,
    })
}

fn parse_role_ids(value: &str) -> anyhow::Result<Vec<u64>> {
    value
        .split(',')
        .filter(|role_id| !role_id.trim().is_empty())
        .map(|role_id| Ok(role_id.trim().parse()?))
        .collect()
}
