use serde::{Deserialize, Serialize};

/// X-Gitea-Event header values
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GiteaEvent {
    Issues,
    IssueComment,
    PullRequest,
    PullRequestReviewComment,
    #[serde(other)]
    Other,
}

impl std::fmt::Display for GiteaEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GiteaEvent::Issues => write!(f, "issues"),
            GiteaEvent::IssueComment => write!(f, "issue_comment"),
            GiteaEvent::PullRequest => write!(f, "pull_request"),
            GiteaEvent::PullRequestReviewComment => write!(f, "pull_request_review_comment"),
            GiteaEvent::Other => write!(f, "other"),
        }
    }
}

/// User object in webhook payloads
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct User {
    pub id: u64,
    pub login: String,
}

/// Repository object in webhook payloads
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Repository {
    pub id: u64,
    pub full_name: String,
}

/// Issue or PR object in webhook payloads
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IssueOrPR {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
}

/// Git reference object (head or base)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct GitRef {
    #[serde(rename = "ref")]
    pub ref_field: String,
    pub sha: String,
}

/// Comment object in issue_comment webhooks
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IssueCommentWebhook {
    pub id: u64,
    pub body: String,
    pub user: User,
}

/// Issue object in issue_comment webhooks (simplified)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IssueWebhook {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
}

/// Pull request object in webhook payloads
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullRequestWebhook {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub head: GitRef,
    pub base: GitRef,
    pub user: User,
    pub state: String,
}

/// Review comment object in webhook payloads
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ReviewCommentWebhook {
    pub id: u64,
    pub body: String,
    pub user: User,
    pub path: String,
    pub line: Option<u64>,
}

/// Payload for issue_comment events
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IssueCommentPayload {
    pub action: String,
    pub issue: IssueWebhook,
    pub comment: IssueCommentWebhook,
    pub repository: Repository,
    pub sender: User,
}

/// Payload for pull_request events
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullRequestPayload {
    pub action: String,
    pub pull_request: PullRequestWebhook,
    pub repository: Repository,
    pub sender: User,
}

/// Payload for pull_request_review_comment events
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PullRequestReviewCommentPayload {
    pub action: String,
    pub pull_request: PullRequestWebhook,
    pub review_comment: ReviewCommentWebhook,
    pub repository: Repository,
    pub sender: User,
}
