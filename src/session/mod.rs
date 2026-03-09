pub mod env_loader;
pub mod opencode;
pub mod worktree;

use crate::forgejo::models::{Issue, IssueComment, PullRequestReviewComment};

/// Trigger information for starting a session
#[derive(Debug, Clone)]
pub struct SessionTrigger {
    pub repo_full_name: String,
    pub issue_id: u64,
    pub pr_id: Option<u64>,
    pub action: String, // "plan", "build", "revision"
    pub comment_body: String, // for revision phase context
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
    "❌ I don't have context for this PR. Please ensure this PR was created through forgebot.".to_string()
}

/// Derive a deterministic session ID from repository and issue
///
/// Format: `ses_{issue_id}_{sanitized_owner}_{sanitized_repo}`
/// Sanitization: lowercase, strip non-alphanumeric except underscore
///
/// Example: `derive_session_id("Alice/My-Repo", 42)` → `"ses_42_alice_my_repo"`
pub fn derive_session_id(repo_full_name: &str, issue_id: u64) -> String {
    let parts: Vec<&str> = repo_full_name.split('/').collect();
    let owner = parts.get(0).map(|s| sanitize_for_session_id(s)).unwrap_or_default();
    let repo = parts.get(1).map(|s| sanitize_for_session_id(s)).unwrap_or_default();

    format!("ses_{}_{}_{}", issue_id, owner, repo)
}

/// Sanitize a string for use in a session ID
/// - Lowercase
/// - Keep only alphanumeric and underscores
/// - Replace invalid chars with underscores
fn sanitize_for_session_id(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

/// Build the prompt for opencode based on the phase
///
/// # Arguments
/// * `phase` - The phase: "plan", "build", or "revision"
/// * `issue` - The issue being worked on
/// * `issue_comments` - All comments on the issue
/// * `pr_review_comments` - PR review comments (for revision phase)
/// * `pr_id` - PR ID (for revision phase)
///
/// # Returns
/// The full prompt string for opencode
pub fn build_prompt(
    phase: &str,
    issue: &Issue,
    issue_comments: &[IssueComment],
    pr_review_comments: &[PullRequestReviewComment],
    pr_id: Option<u64>,
) -> String {
    match phase {
        "plan" => build_plan_prompt(issue, issue_comments),
        "build" => build_build_prompt(issue, issue_comments),
        "revision" => build_revision_prompt(issue, pr_review_comments, pr_id),
        _ => format!("Unknown phase: {}", phase),
    }
}

fn build_plan_prompt(issue: &Issue, issue_comments: &[IssueComment]) -> String {
    let comments_text = format_issue_comments(issue_comments);

    format!(
        r#"You are working on issue #{issue_number} in repository.

Issue Title: {title}

Issue Body:
{body}

Issue Comments:
{comments}

Your task: Analyze this issue and post a plan as a comment. The plan should outline:
1. Understanding of the problem or feature request
2. Proposed approach to solve/implement
3. Any questions or clarifications needed
4. Estimated complexity or effort

Post your plan as a comment on this issue using the comment-issue tool."#,
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

fn build_build_prompt(issue: &Issue, issue_comments: &[IssueComment]) -> String {
    // For now, include all comments. In the future, we may filter by session creation time.
    let comments_text = format_issue_comments(issue_comments);

    format!(
        r#"You are continuing work on issue #{issue_number}.

Issue Title: {title}

Issue Body:
{body}

Issue Comments:
{comments}

You have a plan for this issue. Your task: Implement the solution and open a pull request.

1. Review the issue and any comments for context
2. Make the necessary code changes in the worktree
3. Commit your changes with a meaningful commit message
4. Create a pull request using the create-pr tool
5. Link the PR to this issue in the description

Use the available tools to interact with the repository and create the PR."#,
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
    issue: &Issue,
    pr_review_comments: &[PullRequestReviewComment],
    pr_id: Option<u64>,
) -> String {
    let pr_num = pr_id.unwrap_or(0);
    let review_comments_text = format_pr_review_comments(pr_review_comments);

    format!(
        r#"Your PR #{pr_number} on issue #{issue_number} has received review comments.

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

Use the available tools to make changes and update the PR."#,
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
        let prompt = build_prompt("plan", &issue, &comments, &[], None);

        // Check for key components
        assert!(prompt.contains("issue #42"));
        assert!(prompt.contains("Test Issue"));
        assert!(prompt.contains("This is a test issue body"));
        assert!(prompt.contains("First comment"));
        assert!(prompt.contains("Second comment with more context"));
        assert!(prompt.contains("testuser"));
        assert!(prompt.contains("anotheruser"));
        assert!(prompt.contains("Your task: Analyze this issue and post a plan"));
    }

    #[test]
    fn test_build_build_prompt() {
        let issue = test_issue();
        let comments = test_comments();
        let prompt = build_prompt("build", &issue, &comments, &[], None);

        // Check for key components
        assert!(prompt.contains("issue #42"));
        assert!(prompt.contains("You have a plan for this issue"));
        assert!(prompt.contains("Your task: Implement the solution"));
        assert!(prompt.contains("open a pull request"));
    }

    #[test]
    fn test_build_revision_prompt() {
        let issue = test_issue();
        let review_comments = vec![
            PullRequestReviewComment {
                id: 1,
                body: "Please fix this function name.".to_string(),
                user: test_user(),
                path: "src/main.rs".to_string(),
                line: Some(42),
                created_at: "2024-01-03T10:00:00Z".to_string(),
                updated_at: "2024-01-03T10:00:00Z".to_string(),
            },
        ];
        let prompt = build_prompt("revision", &issue, &[], &review_comments, Some(123));

        // Check for key components
        assert!(prompt.contains("Your PR #123"));
        assert!(prompt.contains("issue #42"));
        assert!(prompt.contains("Please fix this function name"));
        assert!(prompt.contains("testuser on src/main.rs:42"));
        assert!(prompt.contains("Your task: Address these review comments"));
        assert!(prompt.contains("force-push an updated commit"));
    }

    #[test]
    fn test_build_prompt_empty_comments() {
        let issue = test_issue();
        let prompt = build_prompt("plan", &issue, &[], &[], None);

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
}
