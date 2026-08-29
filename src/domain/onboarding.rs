use crate::domain::verification::{DisplayName, EmailAddress};
use std::fmt;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OnboardingRequestId(i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingStatus {
    Pending,
    Approved,
    Denied,
    Provisioning,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct OnboardingRequest {
    id: OnboardingRequestId,
    user_id: u64,
    requested_by_user_id: u64,
    email: EmailAddress,
    display_name: DisplayName,
    target_key: String,
    target_label: String,
    requested_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    status: OnboardingStatus,
    decided_by_user_id: Option<u64>,
    decided_at: Option<OffsetDateTime>,
    review_channel_id: Option<u64>,
    review_message_id: Option<u64>,
    provisioning_attempts: u32,
    provisioning_started_at: Option<OffsetDateTime>,
    authentik_user_id: Option<u64>,
    last_error: Option<String>,
    completed_at: Option<OffsetDateTime>,
}

impl OnboardingRequestId {
    pub fn new(value: i64) -> Option<Self> {
        (value > 0).then_some(Self(value))
    }

    pub fn parse(value: &str) -> Option<Self> {
        value.parse().ok().and_then(Self::new)
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for OnboardingRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl OnboardingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Provisioning => "provisioning",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            "provisioning" => Some(Self::Provisioning),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

impl fmt::Display for OnboardingStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl OnboardingRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn from_persisted(
        id: OnboardingRequestId,
        user_id: u64,
        requested_by_user_id: u64,
        email: EmailAddress,
        display_name: DisplayName,
        target_key: String,
        target_label: String,
        requested_at: OffsetDateTime,
        updated_at: OffsetDateTime,
        status: OnboardingStatus,
        decided_by_user_id: Option<u64>,
        decided_at: Option<OffsetDateTime>,
        review_channel_id: Option<u64>,
        review_message_id: Option<u64>,
        provisioning_attempts: u32,
        provisioning_started_at: Option<OffsetDateTime>,
        authentik_user_id: Option<u64>,
        last_error: Option<String>,
        completed_at: Option<OffsetDateTime>,
    ) -> Self {
        Self {
            id,
            user_id,
            requested_by_user_id,
            email,
            display_name,
            target_key,
            target_label,
            requested_at,
            updated_at,
            status,
            decided_by_user_id,
            decided_at,
            review_channel_id,
            review_message_id,
            provisioning_attempts,
            provisioning_started_at,
            authentik_user_id,
            last_error,
            completed_at,
        }
    }

    pub fn id(&self) -> OnboardingRequestId {
        self.id
    }

    pub fn user_id(&self) -> u64 {
        self.user_id
    }

    pub fn requested_by_user_id(&self) -> u64 {
        self.requested_by_user_id
    }

    pub fn email(&self) -> &EmailAddress {
        &self.email
    }

    pub fn display_name(&self) -> &DisplayName {
        &self.display_name
    }

    pub fn target_key(&self) -> &str {
        &self.target_key
    }

    pub fn target_label(&self) -> &str {
        &self.target_label
    }

    pub fn requested_at(&self) -> OffsetDateTime {
        self.requested_at
    }

    pub fn updated_at(&self) -> OffsetDateTime {
        self.updated_at
    }

    pub fn status(&self) -> OnboardingStatus {
        self.status
    }

    pub fn decided_by_user_id(&self) -> Option<u64> {
        self.decided_by_user_id
    }

    pub fn decided_at(&self) -> Option<OffsetDateTime> {
        self.decided_at
    }

    pub fn review_location(&self) -> Option<(u64, u64)> {
        Some((self.review_channel_id?, self.review_message_id?))
    }

    pub fn provisioning_attempts(&self) -> u32 {
        self.provisioning_attempts
    }

    pub fn provisioning_started_at(&self) -> Option<OffsetDateTime> {
        self.provisioning_started_at
    }

    pub fn authentik_user_id(&self) -> Option<u64> {
        self.authentik_user_id
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn completed_at(&self) -> Option<OffsetDateTime> {
        self.completed_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_must_be_positive() {
        assert!(OnboardingRequestId::new(1).is_some());
        assert!(OnboardingRequestId::new(0).is_none());
        assert!(OnboardingRequestId::new(-1).is_none());
    }

    #[test]
    fn onboarding_status_round_trips() {
        for status in [
            OnboardingStatus::Pending,
            OnboardingStatus::Approved,
            OnboardingStatus::Denied,
            OnboardingStatus::Provisioning,
            OnboardingStatus::Completed,
            OnboardingStatus::Failed,
        ] {
            assert_eq!(OnboardingStatus::parse(status.as_str()), Some(status));
        }
    }
}
