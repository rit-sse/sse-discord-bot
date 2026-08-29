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
    display_name: DisplayName,
    display_name_confirmed: bool,
    verified_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailAddress {
    user: String,
    domain: String,
}

impl DisplayName {
    const DISCORD_NICKNAME_MAX_LENGTH: usize = 32;

    pub fn parse(display_name: &str) -> Result<Self> {
        let display_name = display_name.trim();

        if display_name.is_empty() {
            return Err(anyhow!("Preferred name cannot be empty"));
        }

        if display_name.chars().count() > 100 {
            return Err(anyhow!(
                "Preferred name cannot be longer than 100 characters"
            ));
        }

        if display_name.chars().any(char::is_control) {
            return Err(anyhow!("Preferred name cannot contain control characters"));
        }

        Ok(Self(display_name.to_owned()))
    }

    pub fn parse_discord_nickname(display_name: &str) -> Result<Self> {
        let display_name = Self::parse(display_name)?;

        if display_name.0.chars().count() > Self::DISCORD_NICKNAME_MAX_LENGTH {
            return Err(anyhow!(
                "Preferred name cannot be longer than {} characters because it is also used as your Discord nickname",
                Self::DISCORD_NICKNAME_MAX_LENGTH
            ));
        }

        Ok(display_name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DisplayName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
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
        display_name: DisplayName,
        display_name_confirmed: bool,
        verified_at: SystemTime,
    ) -> Self {
        Self {
            user_id,
            email,
            display_name,
            display_name_confirmed,
            verified_at,
        }
    }

    pub fn user_id(&self) -> u64 {
        self.user_id
    }

    pub fn email(&self) -> &EmailAddress {
        &self.email
    }

    pub fn display_name(&self) -> &DisplayName {
        &self.display_name
    }

    pub fn is_display_name_confirmed(&self) -> bool {
        self.display_name_confirmed
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
    fn parses_and_trims_display_name() {
        let parsed = DisplayName::parse("  Ada Lovelace  ").expect("name should parse");

        assert_eq!(parsed.as_str(), "Ada Lovelace");
    }

    #[test]
    fn rejects_invalid_display_name() {
        assert!(DisplayName::parse("   ").is_err());
        assert!(DisplayName::parse(&"a".repeat(101)).is_err());
        assert!(DisplayName::parse("Ada\nLovelace").is_err());
    }

    #[test]
    fn rejects_display_name_too_long_for_discord_nickname() {
        assert!(DisplayName::parse_discord_nickname(&"a".repeat(32)).is_ok());
        assert!(DisplayName::parse_discord_nickname(&"a".repeat(33)).is_err());
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
