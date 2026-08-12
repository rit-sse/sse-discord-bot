use crate::domain::verification::EmailAddress;
use rand::RngExt;
use std::{collections::HashMap, time::SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OnboardingRequestId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingStatus {
    Pending,
    Denied { approver_id: u64 },
    Approved { approver_id: u64 },
}

#[derive(Debug, Clone)]
pub struct OnboardingRequest {
    id: OnboardingRequestId,
    user_id: u64,
    requested_by_user_id: u64,
    email: EmailAddress,
    target_key: String,
    target_label: String,
    requested_at: SystemTime,
    status: OnboardingStatus,
}

#[derive(Debug, Clone)]
pub enum StartOnboardingResult {
    Created(OnboardingRequest),
    Reused(OnboardingRequest),
}

#[derive(Debug, Clone)]
pub enum ApproveOnboardingResult {
    Approved(OnboardingRequest),
    Missing,
    AlreadyHandled(OnboardingRequest),
}

#[derive(Debug, Clone)]
pub enum DenyOnboardingResult {
    Denied(OnboardingRequest),
    Missing,
    AlreadyHandled(OnboardingRequest),
}

#[derive(Debug, Default)]
pub struct OnboardingStore {
    pending_requests_by_user_and_target: HashMap<(u64, String), OnboardingRequestId>,
    requests: HashMap<OnboardingRequestId, OnboardingRequest>,
}

impl OnboardingRequestId {
    pub fn generate() -> Self {
        Self(rand::rng().random())
    }

    pub fn parse(value: &str) -> Option<Self> {
        value.parse().ok().map(Self)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for OnboardingRequestId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl OnboardingRequest {
    fn new(
        user_id: u64,
        requested_by_user_id: u64,
        email: EmailAddress,
        target_key: impl Into<String>,
        target_label: impl Into<String>,
    ) -> Self {
        Self {
            id: OnboardingRequestId::generate(),
            user_id,
            requested_by_user_id,
            email,
            target_key: target_key.into(),
            target_label: target_label.into(),
            requested_at: SystemTime::now(),
            status: OnboardingStatus::Pending,
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

    pub fn target_key(&self) -> &str {
        &self.target_key
    }

    pub fn target_label(&self) -> &str {
        &self.target_label
    }

    pub fn requested_at(&self) -> SystemTime {
        self.requested_at
    }

    pub fn status(&self) -> &OnboardingStatus {
        &self.status
    }

    fn is_pending(&self) -> bool {
        self.status == OnboardingStatus::Pending
    }

    #[cfg(test)]
    fn new_for_test(id: u64, user_id: u64, email: EmailAddress, status: OnboardingStatus) -> Self {
        Self {
            id: OnboardingRequestId(id),
            user_id,
            requested_by_user_id: 99,
            email,
            target_key: "headscale".to_owned(),
            target_label: "Headscale".to_owned(),
            requested_at: SystemTime::now(),
            status,
        }
    }
}

impl OnboardingStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_or_reuse(
        &mut self,
        user_id: u64,
        requested_by_user_id: u64,
        email: EmailAddress,
        target_key: impl Into<String>,
        target_label: impl Into<String>,
    ) -> StartOnboardingResult {
        let target_key = target_key.into();
        let target_label = target_label.into();
        let pending_key = (user_id, target_key.clone());

        if let Some(existing_id) = self.pending_requests_by_user_and_target.get(&pending_key)
            && let Some(existing_request) = self.requests.get(existing_id)
            && existing_request.is_pending()
        {
            return StartOnboardingResult::Reused(existing_request.clone());
        }

        let request = OnboardingRequest::new(
            user_id,
            requested_by_user_id,
            email,
            target_key,
            target_label,
        );
        self.pending_requests_by_user_and_target
            .insert(pending_key, request.id());
        self.requests.insert(request.id(), request.clone());
        StartOnboardingResult::Created(request)
    }

    pub fn approve(
        &mut self,
        request_id: OnboardingRequestId,
        approver_id: u64,
    ) -> ApproveOnboardingResult {
        let Some(request) = self.requests.get_mut(&request_id) else {
            return ApproveOnboardingResult::Missing;
        };

        if !request.is_pending() {
            return ApproveOnboardingResult::AlreadyHandled(request.clone());
        }

        request.status = OnboardingStatus::Approved { approver_id };
        self.pending_requests_by_user_and_target
            .remove(&(request.user_id, request.target_key.clone()));

        ApproveOnboardingResult::Approved(request.clone())
    }

    pub fn deny(
        &mut self,
        request_id: OnboardingRequestId,
        approver_id: u64,
    ) -> DenyOnboardingResult {
        let Some(request) = self.requests.get_mut(&request_id) else {
            return DenyOnboardingResult::Missing;
        };

        if !request.is_pending() {
            return DenyOnboardingResult::AlreadyHandled(request.clone());
        }

        request.status = OnboardingStatus::Denied { approver_id };
        self.pending_requests_by_user_and_target
            .remove(&(request.user_id, request.target_key.clone()));

        DenyOnboardingResult::Denied(request.clone())
    }

    pub fn get(&self, request_id: OnboardingRequestId) -> Option<&OnboardingRequest> {
        self.requests.get(&request_id)
    }

    #[cfg(test)]
    fn insert_request_for_test(&mut self, request: OnboardingRequest) {
        if request.is_pending() {
            self.pending_requests_by_user_and_target.insert(
                (request.user_id(), request.target_key().to_owned()),
                request.id(),
            );
        }

        self.requests.insert(request.id(), request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email() -> EmailAddress {
        EmailAddress::parse("test@example.com").expect("test email should parse")
    }

    #[test]
    fn starting_onboarding_creates_new_request() {
        let mut store = OnboardingStore::new();

        let result = store.start_or_reuse(1, 99, email(), "headscale", "Headscale");

        assert!(matches!(result, StartOnboardingResult::Created(_)));
    }

    #[test]
    fn starting_onboarding_reuses_pending_request() {
        let mut store = OnboardingStore::new();

        let first = store.start_or_reuse(1, 99, email(), "headscale", "Headscale");
        let second = store.start_or_reuse(
            1,
            100,
            EmailAddress::parse("other@example.com").unwrap(),
            "headscale",
            "Headscale",
        );

        assert!(matches!(first, StartOnboardingResult::Created(_)));
        assert!(matches!(second, StartOnboardingResult::Reused(_)));
    }

    #[test]
    fn starting_different_targets_creates_separate_requests() {
        let mut store = OnboardingStore::new();

        let first = store.start_or_reuse(1, 99, email(), "headscale", "Headscale");
        let second = store.start_or_reuse(1, 99, email(), "officers", "Officers");

        assert!(matches!(first, StartOnboardingResult::Created(_)));
        assert!(matches!(second, StartOnboardingResult::Created(_)));
    }

    #[test]
    fn approving_request_marks_it_approved() {
        let mut store = OnboardingStore::new();
        store.insert_request_for_test(OnboardingRequest::new_for_test(
            100,
            1,
            email(),
            OnboardingStatus::Pending,
        ));

        let result = store.approve(OnboardingRequestId(100), 7);

        assert!(matches!(
            result,
            ApproveOnboardingResult::Approved(OnboardingRequest {
                status: OnboardingStatus::Approved { approver_id: 7 },
                ..
            })
        ));
    }

    #[test]
    fn denied_request_cannot_be_approved_later() {
        let mut store = OnboardingStore::new();
        store.insert_request_for_test(OnboardingRequest::new_for_test(
            100,
            1,
            email(),
            OnboardingStatus::Denied { approver_id: 7 },
        ));

        let result = store.approve(OnboardingRequestId(100), 8);

        assert!(matches!(result, ApproveOnboardingResult::AlreadyHandled(_)));
    }

    #[test]
    fn approving_missing_request_reports_missing() {
        let mut store = OnboardingStore::new();

        let result = store.approve(OnboardingRequestId(100), 7);

        assert!(matches!(result, ApproveOnboardingResult::Missing));
    }
}
