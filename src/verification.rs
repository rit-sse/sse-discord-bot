use anyhow::{Result, anyhow};
use rand::RngExt;
use std::fmt;

#[derive(Debug, Clone)]
pub struct VerificationCode {
    code: String,
}

#[derive(Debug, Clone)]
pub struct VerificationAttempt {
    user_id: u64,
    email: EmailAddress,
    code: VerificationCode,
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
}
