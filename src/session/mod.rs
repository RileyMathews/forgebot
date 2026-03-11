pub mod clone;
pub mod env_loader;
pub mod opencode;
pub mod opencode_api;
pub mod repo_cleanup;
pub mod worktree;

use base64::Engine;

use crate::forgejo::models::{Issue, IssueComment, PullRequestReviewComment};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Plan,
    Build,
    Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Collab,
    Build,
}

impl SessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Collab => "collab",
            Self::Build => "build",
        }
    }

    pub fn action(self) -> SessionAction {
        match self {
            Self::Collab => SessionAction::Plan,
            Self::Build => SessionAction::Build,
        }
    }
}

impl std::str::FromStr for SessionMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "collab" => Ok(Self::Collab),
            "build" => Ok(Self::Build),
            _ => anyhow::bail!("Unknown session mode: {}", value),
        }
    }
}

impl SessionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Build => "build",
            Self::Revision => "revision",
        }
    }

    pub fn state(self) -> SessionState {
        match self {
            Self::Plan => SessionState::Planning,
            Self::Build => SessionState::Building,
            Self::Revision => SessionState::Revising,
        }
    }

    pub fn agent_mode(self) -> &'static str {
        "forgebot"
    }

    pub fn session_mode(self) -> SessionMode {
        match self {
            Self::Plan => SessionMode::Collab,
            Self::Build | Self::Revision => SessionMode::Build,
        }
    }
}

impl std::str::FromStr for SessionAction {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "plan" => Ok(Self::Plan),
            "build" => Ok(Self::Build),
            "revision" => Ok(Self::Revision),
            _ => anyhow::bail!("Unknown session action: {}", value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Planning,
    Building,
    Revising,
    Idle,
    Busy,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneStatus {
    Pending,
    Cloning,
    Ready,
    Failed,
}

impl CloneStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Cloning => "cloning",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for CloneStatus {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "cloning" => Ok(Self::Cloning),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            _ => anyhow::bail!("Unknown clone status: {}", value),
        }
    }
}

impl std::fmt::Display for CloneStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl PartialEq<&str> for CloneStatus {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Building => "building",
            Self::Revising => "revising",
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Error => "error",
        }
    }

    pub fn is_busy(self) -> bool {
        matches!(self, Self::Planning | Self::Building | Self::Revising)
    }
}

impl std::str::FromStr for SessionState {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "planning" => Ok(Self::Planning),
            "building" => Ok(Self::Building),
            "revising" => Ok(Self::Revising),
            "idle" => Ok(Self::Idle),
            "busy" => Ok(Self::Busy),
            "error" => Ok(Self::Error),
            _ => anyhow::bail!("Unknown session state: {}", value),
        }
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl PartialEq<&str> for SessionState {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

pub const SESSION_ACTIVE_STATES: &[SessionState] = &[
    SessionState::Planning,
    SessionState::Building,
    SessionState::Revising,
    SessionState::Idle,
    SessionState::Busy,
    SessionState::Error,
];

pub const SESSION_BUSY_STATES: &[SessionState] = &[
    SessionState::Planning,
    SessionState::Building,
    SessionState::Revising,
];

/// Trigger information for starting a session
#[derive(Debug, Clone)]
pub struct SessionTrigger {
    pub repo_full_name: String,
    pub issue_id: u64,
    pub pr_id: Option<u64>,
    pub action: SessionAction,
}

#[derive(Debug, Clone)]
pub struct PromptContext<'a> {
    pub repo_full_name: &'a str,
    pub issue_id: u64,
    pub pr_id: Option<u64>,
    pub base_branch: &'a str,
    pub work_branch: &'a str,
}

/// Comment text helpers for consistent bot messaging
pub fn comment_text_thinking() -> String {
    "🤖 forgebot is thinking...".to_string()
}

pub fn comment_text_working() -> String {
    "🤖 forgebot is working on it...".to_string()
}

pub fn comment_text_busy() -> String {
    "🤖 forgebot is currently working on this issue. Please wait for the current operation to complete.".to_string()
}

pub fn comment_text_error(err: &str) -> String {
    format!("❌ Error: {}", err)
}

pub fn comment_text_no_context() -> String {
    "❌ I don't have context for this PR. Please ensure this PR was created through forgebot."
        .to_string()
}

pub fn opencode_session_web_url(
    web_host: &str,
    worktree_path: &str,
    opencode_session_id: &str,
) -> String {
    let host = web_host.trim_end_matches('/');
    let encoded_dir = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(worktree_path);
    format!("{}/{}/session/{}", host, encoded_dir, opencode_session_id)
}

/// Derive a deterministic session ID from repository and issue
///
/// Format: `ses_{issue_id}_{sanitized_owner}_{sanitized_repo}`
/// Sanitization: lowercase, strip non-alphanumeric except underscore
///
/// Example: `derive_session_id("Alice/My-Repo", 42)` → `"ses_42_alice_my_repo"`
pub fn derive_session_id(repo_full_name: &str, issue_id: u64) -> String {
    let parts: Vec<&str> = repo_full_name.split('/').collect();
    let owner = parts
        .first()
        .map(|s| sanitize_for_session_id(s))
        .unwrap_or_default();
    let repo = parts
        .get(1)
        .map(|s| sanitize_for_session_id(s))
        .unwrap_or_default();

    format!("ses_{}_{}_{}", issue_id, owner, repo)
}

/// Sanitize a string for use in a session ID
/// - Lowercase
/// - Keep only alphanumeric and underscores
/// - Replace invalid chars with underscores
fn sanitize_for_session_id(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Build the prompt for opencode based on the session action
///
/// # Arguments
/// * `phase` - The action: collab (`plan`), `build`, or `revision`
/// * `issue` - The issue being worked on
/// * `issue_comments` - All comments on the issue
/// * `pr_review_comments` - PR review comments (for revision phase)
/// * `pr_id` - PR ID (for revision phase)
///
/// # Returns
/// The full prompt string for opencode
pub fn build_prompt(
    phase: SessionAction,
    context: &PromptContext<'_>,
    issue: &Issue,
    issue_comments: &[IssueComment],
    pr_review_comments: &[PullRequestReviewComment],
) -> String {
    match phase {
        SessionAction::Plan => build_plan_prompt(context, issue, issue_comments),
        SessionAction::Build => build_build_prompt(context, issue, issue_comments),
        SessionAction::Revision => build_revision_prompt(context, issue, pr_review_comments),
    }
}

fn build_plan_prompt(
    context: &PromptContext<'_>,
    issue: &Issue,
    issue_comments: &[IssueComment],
) -> String {
    let comments_text = format_issue_comments(issue_comments);

    format!(
        r#"You are working on issue #{issue_number}.

Explicit context (use these exact values for Forgejo tool arguments):
- repo: {repo}
- issue_id: {issue_id}
- pr_id: {pr_id}
- base_branch: {base_branch}
- work_branch: {work_branch}

Issue Title: {title}

Issue Body:
{body}

Issue Comments:
{comments}

You are operating in a single issue-to-PR workflow.

Your task:
1. Start by posting at least one concrete planning comment on this issue (approach, assumptions, tradeoffs)
2. If you need guidance or have questions while planning, ask them in an issue comment and wait for a user reply
3. After posting a planning comment, stop and wait for a new user comment before starting implementation
4. On a later trigger, if requirements are clear, post a short issue comment that you are starting implementation
5. Implement the solution, commit changes, and open a pull request linked to this issue
6. Post a final issue comment with what was done and the PR link

This run is headless (no interactive human chat).
Do not provide your deliverable as a plain assistant response.

Post your response as a comment on this issue using Forgejo MCP tools.
Split repo={repo} into owner/repo and use issue index={issue_id}."#,
        repo = context.repo_full_name,
        issue_id = context.issue_id,
        pr_id = context
            .pr_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "(none)".to_string()),
        base_branch = context.base_branch,
        work_branch = context.work_branch,
        issue_number = issue.number,
        title = issue.title,
        body = issue.body.as_deref().unwrap_or("(no body)"),
        comments = if comments_text.is_empty() {
            "(no comments)".to_string()
        } else {
            comments_text
        },
    )
}

fn build_build_prompt(
    context: &PromptContext<'_>,
    issue: &Issue,
    issue_comments: &[IssueComment],
) -> String {
    // For now, include all comments. In the future, we may filter by session creation time.
    let comments_text = format_issue_comments(issue_comments);

    format!(
        r#"You are continuing work on issue #{issue_number}.

Explicit context (use these exact values for Forgejo tool arguments):
- repo: {repo}
- issue_id: {issue_id}
- pr_id: {pr_id}
- base_branch: {base_branch}
- work_branch: {work_branch}

Issue Title: {title}

Issue Body:
{body}

Issue Comments:
{comments}

Build mode is active. Your task: Implement the solution and open a pull request.

1. Review the issue and any comments for context
2. Make the necessary code changes in the worktree
3. Commit your changes with a meaningful commit message
4. Create a pull request using Forgejo MCP tools
5. Link the PR to this issue in the description

Use explicit tool arguments for every Forgejo operation.

This run is headless (no interactive human chat).
Do not provide your deliverable as a plain assistant response.

When creating the PR via Forgejo MCP, set arguments to:
- owner/repo parsed from repo={repo}
- head={work_branch}
- base={base_branch}

The PR body must include `Closes #{issue_id}` on its own line."#,
        repo = context.repo_full_name,
        issue_id = context.issue_id,
        pr_id = context
            .pr_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "(none)".to_string()),
        base_branch = context.base_branch,
        work_branch = context.work_branch,
        issue_number = issue.number,
        title = issue.title,
        body = issue.body.as_deref().unwrap_or("(no body)"),
        comments = if comments_text.is_empty() {
            "(no comments)".to_string()
        } else {
            comments_text
        },
    )
}

fn build_revision_prompt(
    context: &PromptContext<'_>,
    issue: &Issue,
    pr_review_comments: &[PullRequestReviewComment],
) -> String {
    let pr_num = context.pr_id.unwrap_or(0);
    let review_comments_text = format_pr_review_comments(pr_review_comments);

    format!(
        r#"Your PR #{pr_number} on issue #{issue_number} has received review comments.

Explicit context (use these exact values for Forgejo tool arguments):
- repo: {repo}
- issue_id: {issue_id}
- pr_id: {pr_id}
- base_branch: {base_branch}
- work_branch: {work_branch}

Issue Title: {title}

Issue Body:
{body}

Review Comments:
{review_comments}

Your task: Address these review comments and force-push an updated commit.

1. Review each comment carefully
2. Make the necessary changes to address the feedback
3. Commit your changes
4. Force-push the updated branch
5. Verify all comments are addressed

This run is headless (no interactive human chat).
Do not provide your deliverable as a plain assistant response.

Use explicit tool arguments for every Forgejo operation (repo/pr_id must match the context block)."#,
        repo = context.repo_full_name,
        issue_id = context.issue_id,
        pr_id = context
            .pr_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "(none)".to_string()),
        base_branch = context.base_branch,
        work_branch = context.work_branch,
        pr_number = pr_num,
        issue_number = issue.number,
        title = issue.title,
        body = issue.body.as_deref().unwrap_or("(no body)"),
        review_comments = if review_comments_text.is_empty() {
            "(no review comments)".to_string()
        } else {
            review_comments_text
        },
    )
}

fn format_issue_comments(comments: &[IssueComment]) -> String {
    comments
        .iter()
        .map(|c| {
            format!(
                "{} ({}): {}",
                c.user.login,
                format_timestamp(&c.created_at),
                c.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_pr_review_comments(comments: &[PullRequestReviewComment]) -> String {
    comments
        .iter()
        .map(|c| {
            let line_info = c.line.map(|l| format!(":{}", l)).unwrap_or_default();
            format!(
                "{} on {}{} ({}): {}",
                c.user.login,
                c.path,
                line_info,
                format_timestamp(&c.created_at),
                c.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_timestamp(ts: &str) -> String {
    // Try to parse the timestamp and format it nicely
    // If parsing fails, return the original string
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        dt.format("%Y-%m-%d %H:%M UTC").to_string()
    } else {
        ts.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forgejo::models::{Issue, IssueComment, PullRequestReviewComment, User};

    #[test]
    fn test_derive_session_id() {
        // Basic case
        let id = derive_session_id("Alice/My-Repo", 42);
        assert_eq!(id, "ses_42_alice_my_repo");

        // Already lowercase
        let id = derive_session_id("alice/myrepo", 123);
        assert_eq!(id, "ses_123_alice_myrepo");

        // With dots (should become underscores)
        let id = derive_session_id("user/repo.name", 1);
        assert_eq!(id, "ses_1_user_repo_name");

        // With multiple special chars
        let id = derive_session_id("My-Org/Some_Repo", 99);
        assert_eq!(id, "ses_99_my_org_some_repo");

        // With numbers
        let id = derive_session_id("org2/repo-v1.0", 7);
        assert_eq!(id, "ses_7_org2_repo_v1_0");

        // Edge case: missing slash
        let id = derive_session_id("just-owner", 5);
        assert_eq!(id, "ses_5_just_owner_");
    }

    fn test_issue() -> Issue {
        Issue {
            id: 1,
            number: 42,
            title: "Test Issue".to_string(),
            body: Some("This is a test issue body.".to_string()),
            state: "open".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-02T00:00:00Z".to_string(),
        }
    }

    fn test_user() -> User {
        User {
            id: 1,
            login: "testuser".to_string(),
        }
    }

    fn test_comments() -> Vec<IssueComment> {
        vec![
            IssueComment {
                id: 1,
                body: "First comment".to_string(),
                user: test_user(),
                created_at: "2024-01-01T12:00:00Z".to_string(),
                updated_at: "2024-01-01T12:00:00Z".to_string(),
            },
            IssueComment {
                id: 2,
                body: "Second comment with more context.".to_string(),
                user: User {
                    id: 2,
                    login: "anotheruser".to_string(),
                },
                created_at: "2024-01-02T08:30:00Z".to_string(),
                updated_at: "2024-01-02T08:30:00Z".to_string(),
            },
        ]
    }

    #[test]
    fn test_build_plan_prompt() {
        let issue = test_issue();
        let comments = test_comments();
        let context = PromptContext {
            repo_full_name: "alice/project",
            issue_id: 42,
            pr_id: None,
            base_branch: "main",
            work_branch: "agent/issue-42",
        };
        let prompt = build_prompt(SessionAction::Plan, &context, &issue, &comments, &[]);

        // Check for key components
        assert!(prompt.contains("issue #42"));
        assert!(prompt.contains("Test Issue"));
        assert!(prompt.contains("This is a test issue body"));
        assert!(prompt.contains("First comment"));
        assert!(prompt.contains("Second comment with more context"));
        assert!(prompt.contains("testuser"));
        assert!(prompt.contains("anotheruser"));
        assert!(prompt.contains("single issue-to-PR workflow"));
        assert!(prompt.contains("planning comment"));
        assert!(prompt.contains("starting implementation"));
        assert!(prompt.contains("open a pull request"));
        assert!(prompt.contains("repo: alice/project"));
        assert!(prompt.contains("issue_id: 42"));
    }

    #[test]
    fn test_build_build_prompt() {
        let issue = test_issue();
        let comments = test_comments();
        let context = PromptContext {
            repo_full_name: "alice/project",
            issue_id: 42,
            pr_id: None,
            base_branch: "main",
            work_branch: "agent/issue-42",
        };
        let prompt = build_prompt(SessionAction::Build, &context, &issue, &comments, &[]);

        // Check for key components
        assert!(prompt.contains("issue #42"));
        assert!(prompt.contains("Build mode is active"));
        assert!(prompt.contains("Your task: Implement the solution"));
        assert!(prompt.contains("open a pull request"));
        assert!(prompt.contains("head=agent/issue-42"));
        assert!(prompt.contains("base=main"));
    }

    #[test]
    fn test_build_revision_prompt() {
        let issue = test_issue();
        let review_comments = vec![PullRequestReviewComment {
            id: 1,
            body: "Please fix this function name.".to_string(),
            user: test_user(),
            path: "src/main.rs".to_string(),
            line: Some(42),
            created_at: "2024-01-03T10:00:00Z".to_string(),
            updated_at: "2024-01-03T10:00:00Z".to_string(),
        }];
        let context = PromptContext {
            repo_full_name: "alice/project",
            issue_id: 42,
            pr_id: Some(123),
            base_branch: "main",
            work_branch: "agent/issue-42",
        };
        let prompt = build_prompt(
            SessionAction::Revision,
            &context,
            &issue,
            &[],
            &review_comments,
        );

        // Check for key components
        assert!(prompt.contains("Your PR #123"));
        assert!(prompt.contains("issue #42"));
        assert!(prompt.contains("Please fix this function name"));
        assert!(prompt.contains("testuser on src/main.rs:42"));
        assert!(prompt.contains("Your task: Address these review comments"));
        assert!(prompt.contains("force-push an updated commit"));
        assert!(prompt.contains("pr_id: 123"));
    }

    #[test]
    fn test_build_prompt_empty_comments() {
        let issue = test_issue();
        let context = PromptContext {
            repo_full_name: "alice/project",
            issue_id: 42,
            pr_id: None,
            base_branch: "main",
            work_branch: "agent/issue-42",
        };
        let prompt = build_prompt(SessionAction::Plan, &context, &issue, &[], &[]);

        assert!(prompt.contains("(no comments)"));
    }

    #[test]
    fn test_format_timestamp() {
        // RFC 3339 format should be parsed
        let formatted = format_timestamp("2024-01-15T14:30:45Z");
        assert_eq!(formatted, "2024-01-15 14:30 UTC");

        // Non-RFC 3339 should pass through
        let formatted = format_timestamp("some random string");
        assert_eq!(formatted, "some random string");
    }

    #[test]
    fn test_sanitize_for_session_id() {
        assert_eq!(sanitize_for_session_id("My-Repo"), "my_repo");
        assert_eq!(sanitize_for_session_id("repo.name"), "repo_name");
        assert_eq!(sanitize_for_session_id("UPPER"), "upper");
        assert_eq!(sanitize_for_session_id("already_lower"), "already_lower");
        assert_eq!(sanitize_for_session_id("123abc"), "123abc");
        assert_eq!(sanitize_for_session_id("!@#$%"), "_____");
    }

    #[test]
    fn test_session_action_mappings() {
        assert_eq!(SessionAction::Plan.as_str(), "plan");
        assert_eq!(SessionAction::Build.as_str(), "build");
        assert_eq!(SessionAction::Revision.as_str(), "revision");

        assert_eq!(SessionAction::Plan.state(), SessionState::Planning);
        assert_eq!(SessionAction::Build.state(), SessionState::Building);
        assert_eq!(SessionAction::Revision.state(), SessionState::Revising);

        assert_eq!(SessionAction::Plan.agent_mode(), "forgebot");
        assert_eq!(SessionAction::Build.agent_mode(), "forgebot");
        assert_eq!(SessionAction::Revision.agent_mode(), "forgebot");
    }

    #[test]
    fn test_session_mode_mappings() {
        assert_eq!(SessionMode::Collab.as_str(), "collab");
        assert_eq!(SessionMode::Build.as_str(), "build");

        assert_eq!(SessionMode::Collab.action(), SessionAction::Plan);
        assert_eq!(SessionMode::Build.action(), SessionAction::Build);

        assert_eq!(
            "collab".parse::<SessionMode>().unwrap(),
            SessionMode::Collab
        );
        assert_eq!("build".parse::<SessionMode>().unwrap(), SessionMode::Build);
    }

    #[test]
    fn test_session_state_busy_policy() {
        assert!(SessionState::Planning.is_busy());
        assert!(SessionState::Building.is_busy());
        assert!(SessionState::Revising.is_busy());
        assert!(!SessionState::Idle.is_busy());
        assert!(!SessionState::Error.is_busy());
    }

    #[test]
    fn test_clone_status_mappings() {
        assert_eq!(CloneStatus::Pending.as_str(), "pending");
        assert_eq!(CloneStatus::Cloning.as_str(), "cloning");
        assert_eq!(CloneStatus::Ready.as_str(), "ready");
        assert_eq!(CloneStatus::Failed.as_str(), "failed");

        assert_eq!(
            "pending".parse::<CloneStatus>().unwrap(),
            CloneStatus::Pending
        );
        assert_eq!(
            "cloning".parse::<CloneStatus>().unwrap(),
            CloneStatus::Cloning
        );
        assert_eq!("ready".parse::<CloneStatus>().unwrap(), CloneStatus::Ready);
        assert_eq!(
            "failed".parse::<CloneStatus>().unwrap(),
            CloneStatus::Failed
        );
    }

    #[test]
    fn test_opencode_session_web_url() {
        let url = opencode_session_web_url(
            "http://localhost:4096/",
            "/var/lib/forgebot/worktrees/_worktrees/alice_repo/42",
            "oc_abc123",
        );

        assert_eq!(
            url,
            "http://localhost:4096/L3Zhci9saWIvZm9yZ2Vib3Qvd29ya3RyZWVzL193b3JrdHJlZXMvYWxpY2VfcmVwby80Mg/session/oc_abc123"
        );
    }
}
