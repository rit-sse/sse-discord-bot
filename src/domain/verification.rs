use std::{fmt, time::SystemTime};

use anyhow::{Result, anyhow};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use rand::RngExt;

#[derive(Clone)]
pub struct VerificationCode {
    code: String,
}

#[derive(Debug, Clone)]
pub struct VerifiedIdentity {
    user_id: u64,
    email: EmailAddress,
    verified_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailAddress {
    user: String,
    domain: String,
}

impl EmailAddress {
    pub fn parse(email: &str) -> Result<Self> {
        let email = email.trim();
        let (user, domain) = email
            .split_once('@')
            .ok_or(anyhow!("Invalid email: missing '@' symbol"))?;

        if user.is_empty() || domain.is_empty() {
            return Err(anyhow!("Invalid email: missing user or domain"));
        }

        if domain.contains('@') {
            return Err(anyhow!("Invalid email: too many '@' symbols"));
        }

        if user.chars().any(char::is_whitespace) || domain.chars().any(char::is_whitespace) {
            return Err(anyhow!("Invalid email: whitespace is not allowed"));
        }

        Ok(Self {
            user: user.to_owned(),
            domain: domain.to_ascii_lowercase(),
        })
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }
}

impl fmt::Display for EmailAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}", self.user, self.domain)
    }
}

impl VerificationCode {
    pub fn generate() -> Self {
        let mut secure_rng = rand::rng();
        let code = format!("{:06}", secure_rng.random_range(0..=999_999));

        Self { code }
    }

    pub fn hash(&self) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);

        Argon2::default()
            .hash_password(self.code.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|error| anyhow!("failed to hash verification code: {error}"))
    }

    pub fn verify_hash(code_hash: &str, submitted_code: &str) -> Result<bool> {
        let parsed_hash = PasswordHash::new(code_hash)
            .map_err(|error| anyhow!("invalid stored verification code hash: {error}"))?;

        match Argon2::default().verify_password(submitted_code.trim().as_bytes(), &parsed_hash) {
            Ok(()) => Ok(true),
            Err(argon2::password_hash::Error::Password) => Ok(false),
            Err(error) => Err(anyhow!("failed to verify verification code: {error}")),
        }
    }

    #[cfg(test)]
    fn as_str(&self) -> &str {
        &self.code
    }
}

impl fmt::Display for VerificationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl VerifiedIdentity {
    pub(crate) fn from_persisted(
        user_id: u64,
        email: EmailAddress,
        verified_at: SystemTime,
    ) -> Self {
        Self {
            user_id,
            email,
            verified_at,
        }
    }

    pub fn user_id(&self) -> u64 {
        self.user_id
    }

    pub fn email(&self) -> &EmailAddress {
        &self.email
    }

    pub fn verified_at(&self) -> SystemTime {
        self.verified_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_email_address() {
        let parsed = EmailAddress::parse("test@EXAMPLE.COM").expect("email should parse");

        assert_eq!(parsed.to_string(), "test@example.com");
        assert_eq!(parsed.domain(), "example.com");
    }

    #[test]
    fn rejects_invalid_email_address() {
        assert!(EmailAddress::parse("not-an-email").is_err());
        assert!(EmailAddress::parse("@example.com").is_err());
        assert!(EmailAddress::parse("test@").is_err());
        assert!(EmailAddress::parse("test@example.com@evil.example").is_err());
        assert!(EmailAddress::parse("test @example.com").is_err());
    }

    #[test]
    fn generated_verification_code_is_six_digits() {
        let code = VerificationCode::generate();

        assert_eq!(code.as_str().len(), 6);
        assert!(
            code.as_str()
                .chars()
                .all(|character| character.is_ascii_digit())
        );
    }

    #[test]
    fn verification_code_hash_accepts_the_original_code() {
        let code = VerificationCode::generate();
        let hash = code.hash().expect("verification code should hash");

        assert!(
            VerificationCode::verify_hash(&hash, code.as_str())
                .expect("verification code hash should verify")
        );
    }

    #[test]
    fn verification_code_hash_rejects_a_different_code() {
        let code = VerificationCode::generate();
        let hash = code.hash().expect("verification code should hash");

        assert!(
            !VerificationCode::verify_hash(&hash, "not-the-code")
                .expect("verification code hash should verify")
        );
    }
}
