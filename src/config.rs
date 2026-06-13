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
pub struct AuthentikConfig {
    pub base_url: String,
    pub api_token: SecretString,
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
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        tracing::info!("loading config");
        if cfg!(debug_assertions) {
            tracing::debug!("debug build detected; verbose development logging is enabled");
        }

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
        let authentik_base_url = std::env::var("AUTHENTIK_BASE_URL")
            .map_err(|_| anyhow::anyhow!("missing AUTHENTIK_BASE_URL"))?;
        let authentik_api_token = std::env::var("AUTHENTIK_API_TOKEN")
            .map(SecretString::from)
            .map_err(|_| anyhow::anyhow!("missing AUTHENTIK_API_TOKEN"))?;
        let authentik_login_url = std::env::var("AUTHENTIK_LOGIN_URL")
            .map_err(|_| anyhow::anyhow!("missing AUTHENTIK_LOGIN_URL"))?;
        let onboarding_review_channel_id = std::env::var("ONBOARDING_REVIEW_CHANNEL_ID")
            .map_err(|_| anyhow::anyhow!("missing ONBOARDING_REVIEW_CHANNEL_ID"))?
            .parse()?;
        let onboarding_officer_role_id = std::env::var("ONBOARDING_OFFICER_ROLE_ID")
            .map_err(|_| anyhow::anyhow!("missing ONBOARDING_OFFICER_ROLE_ID"))?
            .parse()?;
        let headscale_login_url = std::env::var("HEADSCALE_LOGIN_URL")
            .map_err(|_| anyhow::anyhow!("missing HEADSCALE_LOGIN_URL"))?;
        let onboarding_targets = match std::env::var("ONBOARDING_TARGETS") {
            Ok(value) => {
                tracing::debug!("loading onboarding targets from ONBOARDING_TARGETS");
                parse_onboarding_targets(&value)?
            }
            Err(VarError::NotPresent) => {
                tracing::debug!(
                    "ONBOARDING_TARGETS missing; using legacy single headscale target config"
                );
                let authentik_headscale_group_uuid =
                    std::env::var("AUTHENTIK_HEADSCALE_GROUP_UUID")
                        .map_err(|_| anyhow::anyhow!("missing AUTHENTIK_HEADSCALE_GROUP_UUID"))?;

                vec![OnboardingTargetConfig {
                    key: "headscale".to_owned(),
                    label: "Headscale".to_owned(),
                    authentik_group_uuid: authentik_headscale_group_uuid,
                    approver_role_ids: vec![onboarding_officer_role_id],
                }]
            }
            Err(err) => return Err(err.into()),
        };

        tracing::info!(
            guild_id = ?guild_id,
            verified_role_id = ?verified_role_id,
            smtp_host = %smtp_host,
            smtp_port,
            smtp_starttls,
            onboarding_review_channel_id,
            onboarding_target_count = onboarding_targets.len(),
            "config loaded"
        );
        for target in &onboarding_targets {
            tracing::debug!(
                target = %target.key,
                label = %target.label,
                authentik_group_uuid = %target.authentik_group_uuid,
                manager_role_count = target.approver_role_ids.len(),
                "loaded onboarding target"
            );
        }

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
            authentik: AuthentikConfig {
                base_url: authentik_base_url,
                api_token: authentik_api_token,
                login_url: authentik_login_url,
            },
            onboarding: OnboardingConfig {
                review_channel_id: onboarding_review_channel_id,
                officer_role_id: onboarding_officer_role_id,
                headscale_login_url,
                targets: onboarding_targets,
            },
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
