use serde::{Deserialize, Serialize};

/// User object returned by Forgejo API
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct User {
    pub id: u64,
    pub login: String,
}

/// Issue returned by Forgejo API
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Issue {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Comment on an issue returned by Forgejo API
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IssueComment {
    pub id: u64,
    pub body: String,
    pub user: User,
    pub created_at: String,
    pub updated_at: String,
}

/// Git reference object (head or base)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct GitRef {
    pub ref_field: String,
    pub sha: String,
}

/// Pull request returned by Forgejo API
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullRequest {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub head: GitRef,
    pub base: GitRef,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Review comment on a pull request returned by Forgejo API
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullRequestReviewComment {
    pub id: u64,
    pub body: String,
    pub user: User,
    pub path: String,
    pub line: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Webhook returned by Forgejo API
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Webhook {
    pub id: u64,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
}

/// Payload for creating a webhook
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WebhookPayload {
    #[serde(rename = "type")]
    pub hook_type: String,
    pub config: WebhookConfig,
    pub events: Vec<String>,
    pub active: bool,
}

/// Webhook configuration
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WebhookConfig {
    pub url: String,
    pub content_type: String,
    pub secret: String,
}

/// Payload for creating a comment
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CommentPayload {
    pub body: String,
}
