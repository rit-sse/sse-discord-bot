use anyhow::{Result, anyhow};
use rand::RngExt;
use std::fmt;
use std::time::{Duration, Instant};

const VERIFICATION_CODE_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_FAILED_ATTEMPTS: u8 = 5;

#[derive(Debug, Clone)]
pub struct VerificationCode {
    code: String,
}

#[derive(Debug, Clone)]
pub struct VerificationAttempt {
    user_id: u64,
    email: EmailAddress,
    code: VerificationCode,
    expires_at: Instant,
    failed_attempts: u8,
}

#[derive(Debug, Clone)]
pub struct EmailAddress {
    user: String,
    domain: String,
}

impl EmailAddress {
    pub fn parse(email: &str) -> Result<EmailAddress> {
        let (user, domain) = email
            .split_once('@')
            .ok_or(anyhow!("Invalid email: missing '@' symbol"))?;

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

impl VerificationCode {
    pub fn generate() -> VerificationCode {
        let mut secure_rng = rand::rng();
        let code = format!("{:06}", secure_rng.random_range(0..=999_999));

        Self { code }
    }

    pub fn matches(&self, submitted_code: &str) -> bool {
        self.code == submitted_code.trim()
    }
}

impl fmt::Display for VerificationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.code)
    }
}

impl VerificationAttempt {
    pub fn new(user_id: u64, email: EmailAddress) -> Self {
        Self {
            user_id,
            email,
            code: VerificationCode::generate(),
            expires_at: Instant::now() + VERIFICATION_CODE_TTL,
            failed_attempts: 0,
        }
    }

    pub fn user_id(&self) -> u64 {
        self.user_id
    }

    pub fn email(&self) -> &EmailAddress {
        &self.email
    }

    pub fn code(&self) -> &VerificationCode {
        &self.code
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }

    pub fn code_matches(&self, submitted_code: &str) -> bool {
        self.code.matches(submitted_code)
    }

    pub fn register_failed_attempt(&mut self) {
        self.failed_attempts = self.failed_attempts.saturating_add(1);
    }

    pub fn attempts_remaining(&self) -> u8 {
        MAX_FAILED_ATTEMPTS.saturating_sub(self.failed_attempts)
    }

    pub fn has_attempts_remaining(&self) -> bool {
        self.failed_attempts < MAX_FAILED_ATTEMPTS
    }
}
