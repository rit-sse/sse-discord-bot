use anyhow::{Result, anyhow};
use rand::RngExt;
use std::collections::HashMap;
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
pub enum StartVerificationResult {
    Created(VerificationAttempt),
    Reused(VerificationAttempt),
}

#[derive(Debug, Clone)]
pub enum CheckCodeResult {
    Accepted(VerificationAttempt),
    Missing,
    Expired,
    Incorrect {
        attempts_remaining: u8,
        has_attempts_remaining: bool,
    },
}

#[derive(Debug, Default)]
pub struct VerificationStore {
    pending_attempts: HashMap<u64, VerificationAttempt>,
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

    #[cfg(test)]
    fn new_for_test(code: &str) -> VerificationCode {
        Self {
            code: code.to_owned(),
        }
    }

    #[cfg(test)]
    fn as_str(&self) -> &str {
        &self.code
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

    #[cfg(test)]
    fn new_for_test(
        user_id: u64,
        email: EmailAddress,
        code: VerificationCode,
        expires_at: Instant,
    ) -> Self {
        Self {
            user_id,
            email,
            code,
            expires_at,
            failed_attempts: 0,
        }
    }
}

impl VerificationStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_or_reuse(&mut self, user_id: u64, email: EmailAddress) -> StartVerificationResult {
        if self
            .pending_attempts
            .get(&user_id)
            .is_some_and(VerificationAttempt::is_expired)
        {
            self.pending_attempts.remove(&user_id);
        }

        if let Some(existing_attempt) = self.pending_attempts.get(&user_id) {
            return StartVerificationResult::Reused(existing_attempt.clone());
        }

        let attempt = VerificationAttempt::new(user_id, email);
        self.pending_attempts.insert(user_id, attempt.clone());
        StartVerificationResult::Created(attempt)
    }

    pub fn check_code(&mut self, user_id: u64, submitted_code: &str) -> CheckCodeResult {
        match self.pending_attempts.get_mut(&user_id) {
            None => CheckCodeResult::Missing,
            Some(attempt) if attempt.is_expired() => {
                self.pending_attempts.remove(&user_id);
                CheckCodeResult::Expired
            }
            Some(attempt) if attempt.code_matches(submitted_code) => {
                match self.pending_attempts.remove(&user_id) {
                    Some(attempt) => CheckCodeResult::Accepted(attempt),
                    None => CheckCodeResult::Missing,
                }
            }
            Some(attempt) => {
                attempt.register_failed_attempt();
                let attempts_remaining = attempt.attempts_remaining();
                let has_attempts_remaining = attempt.has_attempts_remaining();

                if !has_attempts_remaining {
                    self.pending_attempts.remove(&user_id);
                }

                CheckCodeResult::Incorrect {
                    attempts_remaining,
                    has_attempts_remaining,
                }
            }
        }
    }

    #[cfg(test)]
    fn insert_attempt_for_test(&mut self, attempt: VerificationAttempt) {
        self.pending_attempts.insert(attempt.user_id(), attempt);
    }

    #[cfg(test)]
    fn has_pending_attempt(&self, user_id: u64) -> bool {
        self.pending_attempts.contains_key(&user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email() -> EmailAddress {
        EmailAddress::parse("test@example.com").expect("test email should parse")
    }

    fn attempt_with_code(user_id: u64, code: &str) -> VerificationAttempt {
        VerificationAttempt::new_for_test(
            user_id,
            email(),
            VerificationCode::new_for_test(code),
            Instant::now() + VERIFICATION_CODE_TTL,
        )
    }

    fn expired_attempt(user_id: u64) -> VerificationAttempt {
        VerificationAttempt::new_for_test(
            user_id,
            email(),
            VerificationCode::new_for_test("123456"),
            Instant::now() - Duration::from_secs(1),
        )
    }

    #[test]
    fn parses_valid_email_address() {
        let parsed = EmailAddress::parse("test@example.com").expect("email should parse");

        assert_eq!(parsed.to_string(), "test@example.com");
    }

    #[test]
    fn rejects_invalid_email_address() {
        assert!(EmailAddress::parse("not-an-email").is_err());
        assert!(EmailAddress::parse("@example.com").is_err());
        assert!(EmailAddress::parse("test@").is_err());
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
    fn starting_verification_creates_new_attempt() {
        let mut store = VerificationStore::new();

        let result = store.start_or_reuse(1, email());

        assert!(matches!(result, StartVerificationResult::Created(_)));
        assert!(store.has_pending_attempt(1));
    }

    #[test]
    fn starting_verification_reuses_active_attempt() {
        let mut store = VerificationStore::new();

        let first = store.start_or_reuse(1, email());
        let second = store.start_or_reuse(1, EmailAddress::parse("other@example.com").unwrap());

        assert!(matches!(first, StartVerificationResult::Created(_)));
        assert!(matches!(second, StartVerificationResult::Reused(_)));
    }

    #[test]
    fn correct_code_accepts_and_removes_pending_attempt() {
        let mut store = VerificationStore::new();
        store.insert_attempt_for_test(attempt_with_code(1, "123456"));

        let result = store.check_code(1, "123456");

        assert!(matches!(result, CheckCodeResult::Accepted(_)));
        assert!(!store.has_pending_attempt(1));
    }

    #[test]
    fn incorrect_code_keeps_pending_attempt_while_attempts_remain() {
        let mut store = VerificationStore::new();
        store.insert_attempt_for_test(attempt_with_code(1, "123456"));

        let result = store.check_code(1, "000000");

        assert!(matches!(
            result,
            CheckCodeResult::Incorrect {
                attempts_remaining: 4,
                has_attempts_remaining: true
            }
        ));
        assert!(store.has_pending_attempt(1));
    }

    #[test]
    fn exhausted_attempts_remove_pending_attempt() {
        let mut store = VerificationStore::new();
        store.insert_attempt_for_test(attempt_with_code(1, "123456"));

        for _ in 0..4 {
            let result = store.check_code(1, "000000");
            assert!(matches!(
                result,
                CheckCodeResult::Incorrect {
                    has_attempts_remaining: true,
                    ..
                }
            ));
        }

        let result = store.check_code(1, "000000");

        assert!(matches!(
            result,
            CheckCodeResult::Incorrect {
                attempts_remaining: 0,
                has_attempts_remaining: false
            }
        ));
        assert!(!store.has_pending_attempt(1));
    }

    #[test]
    fn expired_attempt_is_removed_and_reported_expired() {
        let mut store = VerificationStore::new();
        store.insert_attempt_for_test(expired_attempt(1));

        let result = store.check_code(1, "123456");

        assert!(matches!(result, CheckCodeResult::Expired));
        assert!(!store.has_pending_attempt(1));
    }
}
