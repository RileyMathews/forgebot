pub mod models;

use anyhow::{Context, Result};
use reqwest::Client;
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
    pub fn new(base_url: &str, token: &str, bot_username: &str) -> Result<Self> {
        let client = Client::builder()
            .user_agent("forgebot")
            .build()
            .context("Failed to build HTTP client")?;

        // Ensure base_url doesn't have trailing slash
        let base_url = base_url.trim_end_matches('/').to_string();

        Ok(Self {
            client,
            base_url,
            token: token.to_string(),
            bot_username: bot_username.to_string(),
        })
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

    /// GET an issue by ID
    pub async fn get_issue(&self, repo: &str, issue_id: u64) -> Result<Issue> {
        let url = self.api_url(&format!("/api/v1/repos/{}/issues/{}", repo, issue_id));
        debug!("Fetching issue from: {}", url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .with_context(|| {
                format!("Failed to send request to get issue {}/{}", repo, issue_id)
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read body>".to_string());
            anyhow::bail!(
                "Failed to get issue: {} {} - {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        let issue: Issue = response
            .json()
            .await
            .with_context(|| format!("Failed to parse issue response for {}/{}", repo, issue_id))?;

        Ok(issue)
    }

    /// List comments on an issue
    pub async fn list_issue_comments(
        &self,
        repo: &str,
        issue_id: u64,
    ) -> Result<Vec<IssueComment>> {
        let url = self.api_url(&format!(
            "/api/v1/repos/{}/issues/{}/comments",
            repo, issue_id
        ));
        debug!("Fetching issue comments from: {}", url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to send request to list issue comments {}/{}",
                    repo, issue_id
                )
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read body>".to_string());
            anyhow::bail!(
                "Failed to list issue comments: {} {} - {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        let comments: Vec<IssueComment> = response.json().await.with_context(|| {
            format!(
                "Failed to parse issue comments response for {}/{}",
                repo, issue_id
            )
        })?;

        Ok(comments)
    }

    /// List review comments on a pull request
    pub async fn list_pr_review_comments(
        &self,
        repo: &str,
        pr_id: u64,
    ) -> Result<Vec<PullRequestReviewComment>> {
        let url = self.api_url(&format!("/api/v1/repos/{}/pulls/{}/comments", repo, pr_id));
        debug!("Fetching PR review comments from: {}", url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to send request to list PR review comments {}/{}",
                    repo, pr_id
                )
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read body>".to_string());
            anyhow::bail!(
                "Failed to list PR review comments: {} {} - {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        let comments: Vec<PullRequestReviewComment> = response.json().await.with_context(|| {
            format!(
                "Failed to parse PR review comments response for {}/{}",
                repo, pr_id
            )
        })?;

        Ok(comments)
    }

    /// Post a comment on an issue
    pub async fn post_issue_comment(
        &self,
        repo: &str,
        issue_id: u64,
        body: &str,
    ) -> Result<IssueComment> {
        let url = self.api_url(&format!(
            "/api/v1/repos/{}/issues/{}/comments",
            repo, issue_id
        ));
        debug!("Posting issue comment to: {}", url);

        let payload = CommentPayload {
            body: body.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&payload)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to send request to post issue comment {}/{}",
                    repo, issue_id
                )
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read body>".to_string());
            anyhow::bail!(
                "Failed to post issue comment: {} {} - {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        let comment: IssueComment = response.json().await.with_context(|| {
            format!(
                "Failed to parse issue comment response for {}/{}",
                repo, issue_id
            )
        })?;

        Ok(comment)
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
        let url = self.api_url(&format!("/api/v1/repos/{}/hooks", repo));
        debug!("Fetching webhooks from: {}", url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .with_context(|| format!("Failed to send request to list webhooks for {}", repo))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read body>".to_string());
            anyhow::bail!(
                "Failed to list webhooks: {} {} - {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        let webhooks: Vec<Webhook> = response
            .json()
            .await
            .with_context(|| format!("Failed to parse webhooks response for {}", repo))?;

        Ok(webhooks)
    }

    /// Create a webhook for a repository
    pub async fn create_repo_webhook(
        &self,
        repo: &str,
        url: &str,
        secret: &str,
    ) -> Result<Webhook> {
        let api_url = self.api_url(&format!("/api/v1/repos/{}/hooks", repo));
        debug!("Creating webhook at: {}", api_url);

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

        let response = self
            .client
            .post(&api_url)
            .header("Authorization", self.auth_header())
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("Failed to send request to create webhook for {}", repo))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<could not read body>".to_string());
            anyhow::bail!(
                "Failed to create webhook: {} {} - {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                body
            );
        }

        let webhook: Webhook = response
            .json()
            .await
            .with_context(|| format!("Failed to parse webhook creation response for {}", repo))?;

        Ok(webhook)
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
