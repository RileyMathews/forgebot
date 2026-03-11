pub mod models;

use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use tracing::{debug, error};

use models::*;

/// HTTP client for the Forgejo API
#[derive(Debug, Clone)]
pub struct ForgejoClient {
    client: Client,
    base_url: String,
    token: String,
    bot_username: String,
}

impl ForgejoClient {
    /// Create a new Forgejo client
    pub fn new(base_url: &str, token: &str, bot_username: &str) -> Self {
        let client = Client::builder()
            .user_agent("forgebot")
            .build()
            .context("Failed to build HTTP client").expect("should be able to build forgejo client");

        // Ensure base_url doesn't have trailing slash
        let base_url = base_url.trim_end_matches('/').to_string();

        Self {
            client,
            base_url,
            token: token.to_string(),
            bot_username: bot_username.to_string(),
        }
    }

    /// Get authorization header value
    fn auth_header(&self) -> String {
        format!("token {}", self.token)
    }

    /// Build full API URL
    fn api_url(&self, path: &str) -> String {
        // path should start with /
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };
        format!("{}{}", self.base_url, path)
    }

    fn request_builder(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, self.api_url(path))
            .header("Authorization", self.auth_header())
    }

    async fn send_json<T>(
        &self,
        request: RequestBuilder,
        send_context: impl FnOnce() -> String,
        parse_context: impl FnOnce() -> String,
        error_prefix: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let response = request.send().await.with_context(send_context)?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read body>".to_string());
            anyhow::bail!(
                "{}: {} {} - {}",
                error_prefix,
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        response.json().await.with_context(parse_context)
    }

    async fn send_empty_ok(
        &self,
        request: RequestBuilder,
        send_context: impl FnOnce() -> String,
        ok_statuses: &[StatusCode],
        error_prefix: &str,
    ) -> Result<()> {
        let response = request.send().await.with_context(send_context)?;
        let status = response.status();

        if ok_statuses.contains(&status) {
            return Ok(());
        }

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read body>".to_string());
            anyhow::bail!(
                "{}: {} {} - {}",
                error_prefix,
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        Ok(())
    }

    /// GET an issue by ID
    pub async fn get_issue(&self, repo: &str, issue_id: u64) -> Result<Issue> {
        let path = format!("/api/v1/repos/{}/issues/{}", repo, issue_id);
        debug!("Fetching issue from: {}", self.api_url(&path));

        self.send_json(
            self.request_builder(Method::GET, &path),
            || format!("Failed to send request to get issue {}/{}", repo, issue_id),
            || format!("Failed to parse issue response for {}/{}", repo, issue_id),
            "Failed to get issue",
        )
        .await
    }

    /// Get the authenticated user for the configured token
    pub async fn get_authenticated_user(&self) -> Result<User> {
        let path = "/api/v1/user";
        debug!("Fetching authenticated user from: {}", self.api_url(path));

        self.send_json(
            self.request_builder(Method::GET, path),
            || "Failed to send request to get authenticated user".to_string(),
            || "Failed to parse authenticated user response".to_string(),
            "Failed to get authenticated user",
        )
        .await
    }

    /// List comments on an issue
    pub async fn list_issue_comments(
        &self,
        repo: &str,
        issue_id: u64,
    ) -> Result<Vec<IssueComment>> {
        let path = format!("/api/v1/repos/{}/issues/{}/comments", repo, issue_id);
        debug!("Fetching issue comments from: {}", self.api_url(&path));

        self.send_json(
            self.request_builder(Method::GET, &path),
            || {
                format!(
                    "Failed to send request to list issue comments {}/{}",
                    repo, issue_id
                )
            },
            || {
                format!(
                    "Failed to parse issue comments response for {}/{}",
                    repo, issue_id
                )
            },
            "Failed to list issue comments",
        )
        .await
    }

    /// List review comments on a pull request
    pub async fn list_pr_review_comments(
        &self,
        repo: &str,
        pr_id: u64,
    ) -> Result<Vec<PullRequestReviewComment>> {
        let path = format!("/api/v1/repos/{}/pulls/{}/comments", repo, pr_id);
        debug!("Fetching PR review comments from: {}", self.api_url(&path));

        self.send_json(
            self.request_builder(Method::GET, &path),
            || {
                format!(
                    "Failed to send request to list PR review comments {}/{}",
                    repo, pr_id
                )
            },
            || {
                format!(
                    "Failed to parse PR review comments response for {}/{}",
                    repo, pr_id
                )
            },
            "Failed to list PR review comments",
        )
        .await
    }

    /// Post a comment on an issue
    pub async fn post_issue_comment(
        &self,
        repo: &str,
        issue_id: u64,
        body: &str,
    ) -> Result<IssueComment> {
        let path = format!("/api/v1/repos/{}/issues/{}/comments", repo, issue_id);
        debug!("Posting issue comment to: {}", self.api_url(&path));

        let payload = CommentPayload {
            body: body.to_string(),
        };

        self.send_json(
            self.request_builder(Method::POST, &path).json(&payload),
            || {
                format!(
                    "Failed to send request to post issue comment {}/{}",
                    repo, issue_id
                )
            },
            || {
                format!(
                    "Failed to parse issue comment response for {}/{}",
                    repo, issue_id
                )
            },
            "Failed to post issue comment",
        )
        .await
    }

    /// Post a comment on a pull request (uses same endpoint as issue comments)
    pub async fn post_pr_comment(
        &self,
        repo: &str,
        pr_id: u64,
        body: &str,
    ) -> Result<IssueComment> {
        // PR comments use the same endpoint as issue comments in Forgejo
        self.post_issue_comment(repo, pr_id, body).await
    }

    /// List webhooks for a repository
    pub async fn list_repo_webhooks(&self, repo: &str) -> Result<Vec<Webhook>> {
        let path = format!("/api/v1/repos/{}/hooks", repo);
        debug!("Fetching webhooks from: {}", self.api_url(&path));

        self.send_json(
            self.request_builder(Method::GET, &path),
            || format!("Failed to send request to list webhooks for {}", repo),
            || format!("Failed to parse webhooks response for {}", repo),
            "Failed to list webhooks",
        )
        .await
    }

    /// Create a webhook for a repository
    pub async fn create_repo_webhook(
        &self,
        repo: &str,
        url: &str,
        secret: &str,
    ) -> Result<Webhook> {
        let path = format!("/api/v1/repos/{}/hooks", repo);
        debug!("Creating webhook at: {}", self.api_url(&path));

        let payload = WebhookPayload {
            hook_type: "gitea".to_string(),
            config: WebhookConfig {
                url: url.to_string(),
                content_type: "json".to_string(),
                secret: secret.to_string(),
            },
            events: vec![
                "issues".to_string(),
                "issue_comment".to_string(),
                "pull_request".to_string(),
            ],
            active: true,
        };

        self.send_json(
            self.request_builder(Method::POST, &path).json(&payload),
            || format!("Failed to send request to create webhook for {}", repo),
            || format!("Failed to parse webhook creation response for {}", repo),
            "Failed to create webhook",
        )
        .await
    }

    /// Delete a webhook for a repository
    pub async fn delete_repo_webhook(&self, repo: &str, hook_id: u64) -> Result<()> {
        let path = format!("/api/v1/repos/{}/hooks/{}", repo, hook_id);
        debug!("Deleting webhook at: {}", self.api_url(&path));

        self.send_empty_ok(
            self.request_builder(Method::DELETE, &path),
            || {
                format!(
                    "Failed to send request to delete webhook {} for {}",
                    hook_id, repo
                )
            },
            &[StatusCode::NO_CONTENT, StatusCode::NOT_FOUND],
            "Failed to delete webhook",
        )
        .await
    }

    /// Check if the token has permissions by attempting to list collaborators
    pub async fn check_token_permissions(&self, repo: &str) -> Result<bool> {
        let url = self.api_url(&format!("/api/v1/repos/{}/collaborators", repo));
        debug!("Checking token permissions at: {}", url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await;

        match response {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    debug!("Token permissions check succeeded for {}", repo);
                    Ok(true)
                } else {
                    debug!("Token permissions check failed for {}: {}", repo, status);
                    Ok(false)
                }
            }
            Err(e) => {
                error!("Token permissions check failed for {}: {}", repo, e);
                Ok(false)
            }
        }
    }

    /// Get the bot username
    pub fn bot_username(&self) -> &str {
        &self.bot_username
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}
