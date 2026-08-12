use crate::{config::AuthentikConfig, domain::verification::EmailAddress};
use anyhow::{Context, Result, anyhow};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};

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

#[derive(Clone)]
pub struct AuthentikClient {
    config: AuthentikConfig,
    http: reqwest::Client,
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

        Ok(Self {
            config,
            http: reqwest::Client::new(),
        })
    }

    pub async fn find_user_by_email(&self, email: &EmailAddress) -> Result<Option<AuthentikUser>> {
        let users_url = self.url("/api/v3/core/users/");
        let email = email.to_string();

        tracing::info!(email = %email, "looking up authentik user by email");

        let response = self
            .http
            .get(users_url)
            .bearer_auth(self.config.api_token.expose_secret())
            .query(&[("search", email.as_str())])
            .send()
            .await
            .context("failed to query Authentik users")?;

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

        let response = self
            .http
            .post(users_url)
            .bearer_auth(self.config.api_token.expose_secret())
            .json(&CreateUserRequest {
                username: username.clone(),
                name,
                email: email.clone(),
                path: "users".to_owned(),
            })
            .send()
            .await
            .context("failed to create Authentik user")?;

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

        let response = self
            .http
            .post(add_user_url)
            .bearer_auth(self.config.api_token.expose_secret())
            .json(&AddUserToGroupRequest { pk: user.pk })
            .send()
            .await
            .context("failed to add Authentik user to Headscale group")?;

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

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
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
        matchers::{body_json, header, method, path, query_param},
    };

    fn config(base_url: String) -> AuthentikConfig {
        AuthentikConfig {
            base_url,
            api_token: SecretString::from("test-token"),
            login_url: "https://authentik.example.com".to_owned(),
        }
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
        Mock::given(method("GET"))
            .and(path("/api/v3/core/users/"))
            .and(query_param("search", "test@example.com"))
            .and(header("authorization", "Bearer test-token"))
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
        Mock::given(method("GET"))
            .and(path("/api/v3/core/users/"))
            .and(query_param("search", "test@example.com"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v3/core/users/"))
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
        Mock::given(method("POST"))
            .and(path("/api/v3/core/groups/group-uuid/add_user/"))
            .and(header("authorization", "Bearer test-token"))
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
    async fn authentik_errors_are_returned_without_secret_values() {
        let server = MockServer::start().await;
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
        assert!(!err.contains("test-token"));
    }
}
