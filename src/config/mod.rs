use std::{
    collections::HashSet,
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
pub struct VerificationConfig {
    pub verified_role_id: u64,
    pub unverified_role_id: Option<u64>,
    pub log_channel_id: Option<u64>,
    pub allowed_email_domains: Vec<String>,
    pub email: EmailConfig,
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
    pub authentik: AuthentikConfig,
    pub headscale_login_url: String,
    pub review_channel_id: u64,
    pub targets: Vec<OnboardingTargetConfig>,
}

#[derive(Debug, Clone)]
pub struct OnboardingTargetConfig {
    pub key: String,
    pub label: String,
    pub authentik_group_uuid: String,
    pub completion_url: Option<String>,
    pub approver_role_ids: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub features: FeatureSet,
    pub discord: DiscordConfig,
    pub verification: Option<VerificationConfig>,
    pub onboarding: Option<OnboardingConfig>,
    pub db: DBConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Age,
    Verification,
    Onboarding,
}

#[derive(Debug, Clone)]
pub struct FeatureSet {
    enabled: HashSet<Feature>,
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

fn required_non_empty(key: &'static str) -> Result<String> {
    let value = required::<String>(key)?;
    if value.trim().is_empty() {
        anyhow::bail!("{key} cannot be empty");
    }
    Ok(value)
}

fn non_empty_secret(key: &'static str) -> Result<SecretString> {
    required_non_empty(key).map(SecretString::from)
}

fn required_web_url(key: &'static str) -> Result<String> {
    let value = required_non_empty(key)?;
    validate_web_url(&value).with_context(|| format!("invalid {key}"))?;
    Ok(value)
}

fn validate_web_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value)?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("expected an http or https URL");
    }
    Ok(())
}

impl DiscordConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            bot_token: secret("BOT_TOKEN")?,
            guild_id: optional("GUILD_ID")?,
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

impl VerificationConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            verified_role_id: required("VERIFIED_ROLE_ID")?,
            unverified_role_id: optional("UNVERIFIED_ROLE_ID")?,
            log_channel_id: optional("VERIFICATION_LOG_CHANNEL_ID")?,
            allowed_email_domains: parse_email_domains(&defaulted(
                "VERIFICATION_ALLOWED_EMAIL_DOMAINS",
                "rit.edu".to_owned(),
            )?)?,
            email: EmailConfig::from_env()?,
        })
    }
}

fn parse_email_domains(value: &str) -> Result<Vec<String>> {
    let mut domains = Vec::new();

    for entry in value.split(',') {
        let domain = entry.trim().to_ascii_lowercase();
        if domain.is_empty() || domain.contains('@') || domain.chars().any(char::is_whitespace) {
            anyhow::bail!(
                "invalid VERIFICATION_ALLOWED_EMAIL_DOMAINS entry `{entry}`; expected domain names such as rit.edu"
            );
        }

        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }

    if domains.is_empty() {
        anyhow::bail!("VERIFICATION_ALLOWED_EMAIL_DOMAINS cannot be empty");
    }

    Ok(domains)
}

impl AuthentikConfig {
    fn from_env() -> Result<Self> {
        Ok(Self {
            base_url: required_web_url("AUTHENTIK_BASE_URL")?,
            client_id: required_non_empty("AUTHENTIK_CLIENT_ID")?,
            username: required_non_empty("AUTHENTIK_USERNAME")?,
            password: non_empty_secret("AUTHENTIK_PASSWORD")?,
            login_url: required_web_url("AUTHENTIK_LOGIN_URL")?,
        })
    }
}

impl OnboardingConfig {
    fn from_env() -> Result<Self> {
        let headscale_login_url = required_web_url("HEADSCALE_LOGIN_URL")?;
        let targets = match optional::<String>("ONBOARDING_TARGETS")?
            .filter(|value| !value.trim().is_empty())
        {
            Some(value) => {
                tracing::debug!("loading onboarding targets from ONBOARDING_TARGETS");
                parse_onboarding_targets(&value)?
            }
            None => {
                tracing::debug!(
                    "ONBOARDING_TARGETS missing; using legacy single headscale target config"
                );
                let officer_role_id = required("ONBOARDING_OFFICER_ROLE_ID")?;
                vec![OnboardingTargetConfig {
                    key: "headscale".to_owned(),
                    label: "Headscale".to_owned(),
                    authentik_group_uuid: required_non_empty("AUTHENTIK_HEADSCALE_GROUP_UUID")?,
                    completion_url: None,
                    approver_role_ids: vec![officer_role_id],
                }]
            }
        };

        Ok(Self {
            authentik: AuthentikConfig::from_env()?,
            headscale_login_url,
            review_channel_id: required("ONBOARDING_REVIEW_CHANNEL_ID")?,
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

        let features = FeatureSet::from_env()?;
        let verification = features
            .contains(Feature::Verification)
            .then(VerificationConfig::from_env)
            .transpose()?;
        let onboarding = features
            .contains(Feature::Onboarding)
            .then(OnboardingConfig::from_env)
            .transpose()?;

        Ok(Self {
            features,
            discord: DiscordConfig::from_env()?,
            verification,
            onboarding,
            db: DBConfig::from_env()?,
        })
    }
}

impl FeatureSet {
    fn from_env() -> Result<Self> {
        required::<String>("BOT_FEATURES")?.parse()
    }

    pub fn contains(&self, feature: Feature) -> bool {
        self.enabled.contains(&feature)
    }

    pub fn names(&self) -> Vec<&'static str> {
        [Feature::Age, Feature::Verification, Feature::Onboarding]
            .into_iter()
            .filter(|feature| self.contains(*feature))
            .map(Feature::name)
            .collect()
    }
}

impl FromStr for FeatureSet {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let enabled = value
            .split(',')
            .map(str::trim)
            .filter(|feature| !feature.is_empty())
            .map(Feature::from_str)
            .collect::<Result<HashSet<_>>>()?;

        if enabled.is_empty() {
            return Err(anyhow::anyhow!(
                "BOT_FEATURES must enable at least one feature"
            ));
        }

        if enabled.contains(&Feature::Onboarding) && !enabled.contains(&Feature::Verification) {
            return Err(anyhow::anyhow!(
                "onboarding requires the verification feature"
            ));
        }

        Ok(Self { enabled })
    }
}

impl Feature {
    fn name(self) -> &'static str {
        match self {
            Self::Age => "age",
            Self::Verification => "verification",
            Self::Onboarding => "onboarding",
        }
    }
}

impl FromStr for Feature {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "age" => Ok(Self::Age),
            "verification" => Ok(Self::Verification),
            "onboarding" => Ok(Self::Onboarding),
            unknown => Err(anyhow::anyhow!(
                "unknown BOT_FEATURES entry `{unknown}`; expected age, verification, or onboarding"
            )),
        }
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

    let mut keys = HashSet::new();
    for target in &targets {
        if !keys.insert(target.key.to_ascii_lowercase()) {
            anyhow::bail!("ONBOARDING_TARGETS contains duplicate key `{}`", target.key);
        }
    }

    Ok(targets)
}

fn parse_onboarding_target(value: &str) -> anyhow::Result<OnboardingTargetConfig> {
    let parts = value.split('|').collect::<Vec<_>>();

    if parts.len() != 4 && parts.len() != 5 {
        return Err(anyhow::anyhow!(
            "invalid ONBOARDING_TARGETS entry; expected key|label|authentik_group_uuid|[completion_url|]manager_role_ids"
        ));
    }

    let key = parts[0].trim().to_owned();
    let label = parts[1].trim().to_owned();
    let authentik_group_uuid = parts[2].trim().to_owned();
    let completion_url = (parts.len() == 5)
        .then(|| parts[3].trim().to_owned())
        .filter(|url| !url.is_empty());
    let approver_role_ids_part = if parts.len() == 4 { parts[3] } else { parts[4] };

    if key.is_empty() || label.is_empty() || authentik_group_uuid.is_empty() {
        return Err(anyhow::anyhow!(
            "ONBOARDING_TARGETS key, label, and authentik group UUID cannot be empty"
        ));
    }
    if key.len() > 64
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        anyhow::bail!(
            "ONBOARDING_TARGETS key must be at most 64 ASCII letters, numbers, hyphens, or underscores"
        );
    }
    if label.len() > 100 || label.chars().any(char::is_control) {
        anyhow::bail!(
            "ONBOARDING_TARGETS label must be at most 100 characters without control characters"
        );
    }
    if authentik_group_uuid.len() > 200 || authentik_group_uuid.chars().any(char::is_whitespace) {
        anyhow::bail!(
            "ONBOARDING_TARGETS authentik group UUID must be at most 200 characters without whitespace"
        );
    }
    if let Some(completion_url) = completion_url.as_deref() {
        validate_web_url(completion_url).context("invalid ONBOARDING_TARGETS completion URL")?;
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
        completion_url,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enabled_features() {
        let features = "age, verification, onboarding"
            .parse::<FeatureSet>()
            .expect("feature set should parse");

        assert_eq!(features.names(), vec!["age", "verification", "onboarding"]);
    }

    #[test]
    fn rejects_unknown_feature() {
        let error = "verification,unknown"
            .parse::<FeatureSet>()
            .expect_err("unknown feature should be rejected");

        assert!(error.to_string().contains("unknown BOT_FEATURES entry"));
    }

    #[test]
    fn rejects_onboarding_without_verification() {
        let error = "onboarding"
            .parse::<FeatureSet>()
            .expect_err("onboarding dependency should be enforced");

        assert_eq!(
            error.to_string(),
            "onboarding requires the verification feature"
        );
    }

    #[test]
    fn rejects_empty_feature_set() {
        let error = "  ,  "
            .parse::<FeatureSet>()
            .expect_err("empty feature set should be rejected");

        assert_eq!(
            error.to_string(),
            "BOT_FEATURES must enable at least one feature"
        );
    }

    #[test]
    fn parses_verification_email_domain_allowlist() {
        let domains = parse_email_domains(" RIT.EDU,alumni.rit.edu,rit.edu ")
            .expect("email domains should parse");

        assert_eq!(domains, vec!["rit.edu", "alumni.rit.edu"]);
    }

    #[test]
    fn rejects_invalid_verification_email_domains() {
        assert!(parse_email_domains("").is_err());
        assert!(parse_email_domains("@rit.edu").is_err());
        assert!(parse_email_domains("rit.edu, ").is_err());
    }

    #[test]
    fn parses_target_specific_completion_url_and_roles() {
        let targets = parse_onboarding_targets(
            "headscale|Headscale|group-uuid|https://headscale.example.com|10,20;platform|Platform|platform-group|30",
        )
        .expect("onboarding targets should parse");

        assert_eq!(targets.len(), 2);
        assert_eq!(
            targets[0].completion_url.as_deref(),
            Some("https://headscale.example.com")
        );
        assert_eq!(targets[0].approver_role_ids, vec![10, 20]);
        assert_eq!(targets[1].completion_url, None);
        assert_eq!(targets[1].approver_role_ids, vec![30]);
    }

    #[test]
    fn rejects_duplicate_onboarding_target_keys() {
        let error = parse_onboarding_targets(
            "headscale|Headscale|group-one|10;HEADSCALE|Other|group-two|20",
        )
        .expect_err("duplicate target keys should be rejected");

        assert!(error.to_string().contains("duplicate key"));
    }

    #[test]
    fn rejects_unsafe_target_keys_and_completion_urls() {
        assert!(parse_onboarding_targets("bad key|Bad|group|10").is_err());
        assert!(
            parse_onboarding_targets("headscale|Headscale|group|ftp://example.com|10").is_err()
        );
    }
}
