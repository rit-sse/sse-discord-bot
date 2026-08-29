use crate::{config::AuthentikConfig, domain::verification::EmailAddress};
use anyhow::{Context, Result, anyhow};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const AUTHENTIK_API_SCOPE: &str = "goauthentik.io/api";
const AUTHENTIK_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const AUTHENTIK_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RECOVERY_TOKEN_DURATION: &str = "minutes=30";
const TOKEN_REFRESH_BUFFER_SECONDS: u64 = 30;

#[derive(Debug, Clone, Deserialize)]
pub struct AuthentikUser {
    pub pk: u64,
    pub username: String,
    pub email: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PaginatedUsers {
    results: Vec<AuthentikUser>,
}

#[derive(Debug, Serialize)]
struct CreateUserRequest {
    username: String,
    name: String,
    email: String,
    path: String,
}

#[derive(Debug, Serialize)]
struct AddUserToGroupRequest {
    pk: u64,
}

#[derive(Debug, Serialize)]
struct CreateRecoveryLinkRequest {
    token_duration: &'static str,
}

#[derive(Debug, Deserialize)]
struct CreateRecoveryLinkResponse {
    link: String,
}

#[derive(Serialize)]
struct AccessTokenRequest<'a> {
    grant_type: &'static str,
    client_id: &'a str,
    username: &'a str,
    password: &'a str,
    scope: &'static str,
}

#[derive(Deserialize)]
struct AccessTokenResponse {
    access_token: String,
    expires_in: u64,
}

struct CachedAccessToken {
    value: secrecy::SecretString,
    refresh_at: Instant,
}

#[derive(Clone)]
pub struct AuthentikClient {
    config: AuthentikConfig,
    http: reqwest::Client,
    access_token: Arc<Mutex<Option<CachedAccessToken>>>,
}

impl AuthentikClient {
    pub fn new(config: AuthentikConfig) -> Result<Self> {
        if config.base_url.trim().is_empty() {
            return Err(anyhow!("AUTHENTIK_BASE_URL cannot be empty"));
        }

        tracing::debug!(
            base_url = %config.base_url,
            login_url = %config.login_url,
            "initialized authentik client"
        );

        let http = reqwest::Client::builder()
            .connect_timeout(AUTHENTIK_CONNECT_TIMEOUT)
            .timeout(AUTHENTIK_REQUEST_TIMEOUT)
            .build()
            .context("failed to build Authentik HTTP client")?;

        Ok(Self {
            config,
            http,
            access_token: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn find_user_by_email(&self, email: &EmailAddress) -> Result<Option<AuthentikUser>> {
        let users_url = self.url("/api/v3/core/users/");
        let email = email.to_string();

        tracing::info!(email = %email, "looking up authentik user by email");

        let response = self
            .send_authenticated(
                || {
                    self.http
                        .get(&users_url)
                        .query(&[("email", email.as_str())])
                },
                "failed to query Authentik users",
            )
            .await?;

        if !response.status().is_success() {
            tracing::warn!(
                email = %email,
                status = %response.status(),
                "authentik user lookup failed"
            );
            return Err(anyhow!(
                "Authentik user lookup failed with status {}",
                response.status()
            ));
        }

        let users: PaginatedUsers = response
            .json()
            .await
            .context("failed to parse Authentik user lookup response")?;
        tracing::debug!(
            email = %email,
            result_count = users.results.len(),
            "received authentik user lookup response"
        );

        Ok(users
            .results
            .into_iter()
            .find(|user| user.email.eq_ignore_ascii_case(&email)))
    }

    pub async fn create_user(
        &self,
        email: &EmailAddress,
        discord_user_id: u64,
        display_name: &str,
    ) -> Result<AuthentikUser> {
        let users_url = self.url("/api/v3/core/users/");
        let email = email.to_string();
        let username = username_from_email(&email, discord_user_id);
        let name = if display_name.trim().is_empty() {
            username.clone()
        } else {
            display_name.trim().to_owned()
        };

        tracing::info!(
            email = %email,
            username = %username,
            "creating authentik user"
        );

        let request = CreateUserRequest {
            username: username.clone(),
            name,
            email: email.clone(),
            path: "users".to_owned(),
        };
        let response = self
            .send_authenticated(
                || self.http.post(&users_url).json(&request),
                "failed to create Authentik user",
            )
            .await?;

        if !response.status().is_success() {
            tracing::warn!(
                email = %email,
                username = %username,
                status = %response.status(),
                "authentik user creation failed"
            );
            return Err(anyhow!(
                "Authentik user creation failed with status {}",
                response.status()
            ));
        }

        let user: AuthentikUser = response
            .json()
            .await
            .context("failed to parse Authentik user creation response")?;
        tracing::info!(
            authentik_user_id = user.pk,
            email = %user.email,
            username = %user.username,
            "created authentik user"
        );

        Ok(user)
    }

    pub async fn find_or_create_user(
        &self,
        email: &EmailAddress,
        discord_user_id: u64,
        display_name: &str,
    ) -> Result<AuthentikUser> {
        if let Some(user) = self.find_user_by_email(email).await? {
            tracing::info!(
                authentik_user_id = user.pk,
                email = %user.email,
                "found existing authentik user"
            );
            return Ok(user);
        }

        self.create_user(email, discord_user_id, display_name).await
    }

    pub async fn add_user_to_group(&self, user: &AuthentikUser, group_uuid: &str) -> Result<()> {
        let add_user_url = self.url(&format!("/api/v3/core/groups/{group_uuid}/add_user/"));

        tracing::info!(
            authentik_user_id = user.pk,
            group_uuid = %group_uuid,
            "adding authentik user to group"
        );

        let request = AddUserToGroupRequest { pk: user.pk };
        let response = self
            .send_authenticated(
                || self.http.post(&add_user_url).json(&request),
                "failed to add Authentik user to Headscale group",
            )
            .await?;

        if response.status().is_success() {
            tracing::info!(
                authentik_user_id = user.pk,
                group_uuid = %group_uuid,
                status = %response.status(),
                "authentik group membership ensured"
            );
            return Ok(());
        }

        tracing::warn!(
            authentik_user_id = user.pk,
            group_uuid = %group_uuid,
            status = %response.status(),
            "authentik group membership update failed"
        );

        Err(anyhow!(
            "Authentik group membership update failed with status {}",
            response.status()
        ))
    }

    pub fn login_url(&self) -> &str {
        &self.config.login_url
    }

    pub async fn create_recovery_link(&self, user: &AuthentikUser) -> Result<String> {
        let recovery_url = self.url(&format!("/api/v3/core/users/{}/recovery/", user.pk));

        tracing::info!(
            authentik_user_id = user.pk,
            token_duration = RECOVERY_TOKEN_DURATION,
            "creating authentik account recovery link"
        );

        let request = CreateRecoveryLinkRequest {
            token_duration: RECOVERY_TOKEN_DURATION,
        };
        let response = self
            .send_authenticated(
                || self.http.post(&recovery_url).json(&request),
                "failed to create Authentik account recovery link",
            )
            .await?;

        if !response.status().is_success() {
            tracing::warn!(
                authentik_user_id = user.pk,
                status = %response.status(),
                "authentik account recovery link creation failed"
            );
            return Err(anyhow!(
                "Authentik account recovery link creation failed with status {}",
                response.status()
            ));
        }

        let recovery: CreateRecoveryLinkResponse = response
            .json()
            .await
            .context("failed to parse Authentik account recovery link response")?;
        self.validate_recovery_link(&recovery.link)?;

        tracing::info!(
            authentik_user_id = user.pk,
            "created authentik account recovery link"
        );
        Ok(recovery.link)
    }

    pub async fn check_api_access(&self) -> Result<()> {
        let users_url = self.url("/api/v3/core/users/");
        let response = self
            .send_authenticated(
                || self.http.get(&users_url).query(&[("page_size", "1")]),
                "failed to check Authentik API access",
            )
            .await?;

        if response.status().is_success() {
            return Ok(());
        }

        Err(anyhow!(
            "Authentik API access check failed with status {}",
            response.status()
        ))
    }

    pub async fn check_group_access(&self, group_uuid: &str) -> Result<()> {
        let group_url = self.url(&format!("/api/v3/core/groups/{group_uuid}/"));
        let response = self
            .send_authenticated(
                || self.http.get(&group_url),
                "failed to check Authentik group access",
            )
            .await?;

        if response.status().is_success() {
            return Ok(());
        }

        Err(anyhow!(
            "Authentik group access check failed with status {}",
            response.status()
        ))
    }

    async fn send_authenticated<F>(
        &self,
        build_request: F,
        error_context: &'static str,
    ) -> Result<reqwest::Response>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let access_token = self.access_token().await?;
        let response = build_request()
            .bearer_auth(access_token.expose_secret())
            .send()
            .await
            .context(error_context)?;

        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }

        tracing::warn!("authentik access token was rejected; authenticating again");
        self.invalidate_access_token(&access_token).await;

        let access_token = self.access_token().await?;
        build_request()
            .bearer_auth(access_token.expose_secret())
            .send()
            .await
            .context(error_context)
    }

    async fn access_token(&self) -> Result<secrecy::SecretString> {
        let mut cached_access_token = self.access_token.lock().await;

        if let Some(cached) = cached_access_token
            .as_ref()
            .filter(|cached| Instant::now() < cached.refresh_at)
        {
            return Ok(cached.value.clone());
        }

        tracing::info!(
            username = %self.config.username,
            client_id = %self.config.client_id,
            "authenticating authentik service account"
        );

        let response = self
            .http
            .post(self.url("/application/o/token/"))
            .form(&AccessTokenRequest {
                grant_type: "client_credentials",
                client_id: &self.config.client_id,
                username: &self.config.username,
                password: self.config.password.expose_secret(),
                scope: AUTHENTIK_API_SCOPE,
            })
            .send()
            .await
            .context("failed to authenticate Authentik service account")?;

        if !response.status().is_success() {
            tracing::warn!(
                status = %response.status(),
                "authentik service account authentication failed"
            );
            return Err(anyhow!(
                "Authentik service account authentication failed with status {}",
                response.status()
            ));
        }

        let token: AccessTokenResponse = response
            .json()
            .await
            .context("failed to parse Authentik access token response")?;

        if token.access_token.is_empty() || token.expires_in == 0 {
            return Err(anyhow!("Authentik returned an invalid access token"));
        }

        let refresh_buffer =
            Duration::from_secs((token.expires_in / 10).min(TOKEN_REFRESH_BUFFER_SECONDS));
        let refresh_at =
            Instant::now() + Duration::from_secs(token.expires_in).saturating_sub(refresh_buffer);
        let access_token = secrecy::SecretString::from(token.access_token);

        *cached_access_token = Some(CachedAccessToken {
            value: access_token.clone(),
            refresh_at,
        });

        tracing::info!(
            expires_in = token.expires_in,
            "authenticated with authentik"
        );

        Ok(access_token)
    }

    async fn invalidate_access_token(&self, rejected_token: &secrecy::SecretString) {
        let mut cached_access_token = self.access_token.lock().await;
        let should_invalidate = cached_access_token
            .as_ref()
            .is_some_and(|cached| cached.value.expose_secret() == rejected_token.expose_secret());

        if should_invalidate {
            *cached_access_token = None;
        }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn validate_recovery_link(&self, link: &str) -> Result<()> {
        let base_url = reqwest::Url::parse(&self.config.base_url)
            .context("failed to parse configured Authentik base URL")?;
        let recovery_url = reqwest::Url::parse(link)
            .context("Authentik returned an invalid account recovery link")?;

        if base_url.origin() != recovery_url.origin() {
            return Err(anyhow!(
                "Authentik returned an account recovery link for an unexpected origin"
            ));
        }

        Ok(())
    }
}

fn username_from_email(email: &str, discord_user_id: u64) -> String {
    let local_part = email.split_once('@').map_or(email, |(local, _)| local);
    let sanitized: String = local_part
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();

    format!("{sanitized}-{discord_user_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, body_string_contains, header, method, path, query_param},
    };

    fn config(base_url: String) -> AuthentikConfig {
        AuthentikConfig {
            base_url,
            client_id: "test-client".to_owned(),
            username: "test-service-account".to_owned(),
            password: SecretString::from("test-app-password"),
            login_url: "https://authentik.example.com".to_owned(),
        }
    }

    async fn mount_authentication(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/application/o/token/"))
            .and(body_string_contains("grant_type=client_credentials"))
            .and(body_string_contains("client_id=test-client"))
            .and(body_string_contains("username=test-service-account"))
            .and(body_string_contains("password=test-app-password"))
            .and(body_string_contains("scope=goauthentik.io%2Fapi"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "test-access-token",
                "expires_in": 300,
                "token_type": "Bearer"
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    fn email() -> EmailAddress {
        EmailAddress::parse("test@example.com").expect("test email should parse")
    }

    #[test]
    fn username_from_email_is_safe_and_stable() {
        assert_eq!(
            username_from_email("test.user@example.com", 42),
            "test-user-42"
        );
    }

    #[tokio::test]
    async fn existing_authentik_user_is_found_by_exact_email() {
        let server = MockServer::start().await;
        mount_authentication(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/v3/core/users/"))
            .and(query_param("email", "test@example.com"))
            .and(header("authorization", "Bearer test-access-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {
                        "pk": 1,
                        "username": "test",
                        "email": "test@example.com",
                        "name": "Test User"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = AuthentikClient::new(config(server.uri())).unwrap();

        let user = client
            .find_user_by_email(&email())
            .await
            .expect("lookup should succeed")
            .expect("user should exist");

        assert_eq!(user.pk, 1);
        assert_eq!(user.email, "test@example.com");
    }

    #[tokio::test]
    async fn missing_authentik_user_is_created() {
        let server = MockServer::start().await;
        mount_authentication(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/v3/core/users/"))
            .and(query_param("email", "test@example.com"))
            .and(header("authorization", "Bearer test-access-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v3/core/users/"))
            .and(header("authorization", "Bearer test-access-token"))
            .and(body_json(json!({
                "username": "test-42",
                "name": "Test User",
                "email": "test@example.com",
                "path": "users"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "pk": 2,
                "username": "test-42",
                "email": "test@example.com",
                "name": "Test User"
            })))
            .mount(&server)
            .await;

        let client = AuthentikClient::new(config(server.uri())).unwrap();

        let user = client
            .find_or_create_user(&email(), 42, "Test User")
            .await
            .expect("missing user should be created");

        assert_eq!(user.pk, 2);
        assert_eq!(user.username, "test-42");
    }

    #[tokio::test]
    async fn authentik_user_is_added_to_headscale_group() {
        let server = MockServer::start().await;
        mount_authentication(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/v3/core/groups/group-uuid/add_user/"))
            .and(header("authorization", "Bearer test-access-token"))
            .and(body_json(json!({ "pk": 2 })))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let client = AuthentikClient::new(config(server.uri())).unwrap();
        let user = AuthentikUser {
            pk: 2,
            username: "test-42".to_owned(),
            email: "test@example.com".to_owned(),
            name: "Test User".to_owned(),
        };

        client
            .add_user_to_group(&user, "group-uuid")
            .await
            .expect("group update should succeed");
    }

    #[tokio::test]
    async fn temporary_recovery_link_is_created_for_the_user() {
        let server = MockServer::start().await;
        mount_authentication(&server).await;
        let recovery_link = format!(
            "{}/if/flow/sse-recovery-flow/?flow_token=temporary-secret",
            server.uri()
        );
        Mock::given(method("POST"))
            .and(path("/api/v3/core/users/2/recovery/"))
            .and(header("authorization", "Bearer test-access-token"))
            .and(body_json(json!({ "token_duration": "minutes=30" })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "link": recovery_link })),
            )
            .mount(&server)
            .await;

        let client = AuthentikClient::new(config(server.uri())).unwrap();
        let user = AuthentikUser {
            pk: 2,
            username: "test-42".to_owned(),
            email: "test@example.com".to_owned(),
            name: "Test User".to_owned(),
        };

        let link = client
            .create_recovery_link(&user)
            .await
            .expect("recovery link should be created");

        assert_eq!(link, recovery_link);
    }

    #[test]
    fn recovery_links_from_an_unexpected_origin_are_rejected() {
        let client = AuthentikClient::new(config("https://authentik.example.com".to_owned()))
            .expect("client config should be valid");

        let error = client
            .validate_recovery_link(
                "https://attacker.example.com/if/flow/recovery/?flow_token=secret",
            )
            .expect_err("cross-origin recovery link should be rejected");

        assert!(error.to_string().contains("unexpected origin"));
    }

    #[tokio::test]
    async fn service_account_and_group_access_can_be_checked() {
        let server = MockServer::start().await;
        mount_authentication(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/v3/core/users/"))
            .and(query_param("page_size", "1"))
            .and(header("authorization", "Bearer test-access-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/core/groups/group-uuid/"))
            .and(header("authorization", "Bearer test-access-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "pk": "group-uuid" })))
            .mount(&server)
            .await;
        let client = AuthentikClient::new(config(server.uri())).unwrap();

        client
            .check_api_access()
            .await
            .expect("service account check should succeed");
        client
            .check_group_access("group-uuid")
            .await
            .expect("group check should succeed");
    }

    #[tokio::test]
    async fn authentik_errors_are_returned_without_secret_values() {
        let server = MockServer::start().await;
        mount_authentication(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/v3/core/users/"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let client = AuthentikClient::new(config(server.uri())).unwrap();

        let err = client
            .find_user_by_email(&email())
            .await
            .expect_err("403 should fail")
            .to_string();

        assert!(err.contains("403"));
        assert!(!err.contains("test-access-token"));
        assert!(!err.contains("test-app-password"));
    }
}
