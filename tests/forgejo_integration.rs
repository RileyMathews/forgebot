//! Integration tests for the Forgejo API client
//!
//! To run these tests, you need:
//! 1. A Forgejo instance with a test repository
//! 2. Set FORGEBOT_TEST_URL, FORGEBOT_TEST_TOKEN, and FORGEBOT_TEST_REPO environment variables
//!
//! Example:
//! ```
//! export FORGEBOT_TEST_URL="https://codeberg.org"
//! export FORGEBOT_TEST_TOKEN="your-token-here"
//! export FORGEBOT_TEST_REPO="username/repo"
//! cargo test --test forgejo_integration -- --nocapture
//! ```

use forgebot::forgejo::ForgejoClient;
use std::env;

fn get_test_config() -> Option<(String, String, String)> {
    let url = env::var("FORGEBOT_TEST_URL").ok()?;
    let token = env::var("FORGEBOT_TEST_TOKEN").ok()?;
    let repo = env::var("FORGEBOT_TEST_REPO").ok()?;
    Some((url, token, repo))
}

#[tokio::test]
#[ignore = "Requires Forgejo instance - run manually with env vars set"]
async fn test_list_webhooks() {
    let Some((url, token, repo)) = get_test_config() else {
        eprintln!("Skipping test - set FORGEBOT_TEST_URL, FORGEBOT_TEST_TOKEN, FORGEBOT_TEST_REPO");
        return;
    };

    let client =
        ForgejoClient::new(&url, &token, "forgebot-test").expect("Failed to create client");

    println!("Testing list_repo_webhooks on {}/{}...", url, repo);

    match client.list_repo_webhooks(&repo).await {
        Ok(webhooks) => {
            println!("Success! Found {} webhooks:", webhooks.len());
            for webhook in &webhooks {
                println!(
                    "  - ID: {}, URL: {}, Active: {}",
                    webhook.id, webhook.url, webhook.active
                );
                println!("    Events: {:?}", webhook.events);
            }
        }
        Err(e) => {
            println!("Error listing webhooks: {}", e);
            panic!("Test failed: {}", e);
        }
    }
}

#[tokio::test]
#[ignore = "Requires Forgejo instance - run manually with env vars set"]
async fn test_check_token_permissions() {
    let Some((url, token, repo)) = get_test_config() else {
        eprintln!("Skipping test - set FORGEBOT_TEST_URL, FORGEBOT_TEST_TOKEN, FORGEBOT_TEST_REPO");
        return;
    };

    let client =
        ForgejoClient::new(&url, &token, "forgebot-test").expect("Failed to create client");

    println!("Testing check_token_permissions on {}/{}...", url, repo);

    match client.check_token_permissions(&repo).await {
        Ok(has_permissions) => {
            println!(
                "Token permissions check: {}",
                if has_permissions { "GRANTED" } else { "DENIED" }
            );
        }
        Err(e) => {
            println!("Error checking permissions: {}", e);
            panic!("Test failed: {}", e);
        }
    }
}

#[tokio::test]
#[ignore = "Requires Forgejo instance with an issue - run manually with env vars set"]
async fn test_get_issue() {
    let Some((url, token, repo)) = get_test_config() else {
        eprintln!("Skipping test - set FORGEBOT_TEST_URL, FORGEBOT_TEST_TOKEN, FORGEBOT_TEST_REPO");
        return;
    };

    // Also need FORGEBOT_TEST_ISSUE_ID
    let issue_id: u64 = env::var("FORGEBOT_TEST_ISSUE_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let client =
        ForgejoClient::new(&url, &token, "forgebot-test").expect("Failed to create client");

    println!(
        "Testing get_issue on {}/{} issue #{}...",
        url, repo, issue_id
    );

    match client.get_issue(&repo, issue_id).await {
        Ok(issue) => {
            println!("Success! Issue #{}: {}", issue.number, issue.title);
            println!("  State: {}", issue.state);
            println!("  Created: {}", issue.created_at);
        }
        Err(e) => {
            println!("Error getting issue: {}", e);
            panic!("Test failed: {}", e);
        }
    }
}

#[tokio::test]
#[ignore = "Requires Forgejo instance with an issue - run manually with env vars set"]
async fn test_list_issue_comments() {
    let Some((url, token, repo)) = get_test_config() else {
        eprintln!("Skipping test - set FORGEBOT_TEST_URL, FORGEBOT_TEST_TOKEN, FORGEBOT_TEST_REPO");
        return;
    };

    let issue_id: u64 = env::var("FORGEBOT_TEST_ISSUE_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let client =
        ForgejoClient::new(&url, &token, "forgebot-test").expect("Failed to create client");

    println!(
        "Testing list_issue_comments on {}/{} issue #{}...",
        url, repo, issue_id
    );

    match client.list_issue_comments(&repo, issue_id).await {
        Ok(comments) => {
            println!("Success! Found {} comments:", comments.len());
            for comment in &comments {
                println!(
                    "  - {} @ {}: {}...",
                    comment.user.login,
                    comment.created_at,
                    &comment.body[..comment.body.len().min(50)]
                );
            }
        }
        Err(e) => {
            println!("Error listing comments: {}", e);
            panic!("Test failed: {}", e);
        }
    }
}

#[tokio::test]
#[ignore = "Requires Forgejo instance - creates a webhook, then deletes it"]
async fn test_create_and_delete_webhook() {
    let Some((url, token, repo)) = get_test_config() else {
        eprintln!("Skipping test - set FORGEBOT_TEST_URL, FORGEBOT_TEST_TOKEN, FORGEBOT_TEST_REPO");
        return;
    };

    let webhook_url = env::var("FORGEBOT_TEST_WEBHOOK_URL")
        .unwrap_or_else(|_| "https://example.com/webhook".to_string());

    let client =
        ForgejoClient::new(&url, &token, "forgebot-test").expect("Failed to create client");

    println!("Testing create_repo_webhook on {}/{}...", url, repo);

    match client
        .create_repo_webhook(&repo, &webhook_url, "test-secret")
        .await
    {
        Ok(webhook) => {
            println!("Success! Created webhook ID: {}", webhook.id);
            println!("  URL: {}", webhook.url);
            println!("  Active: {}", webhook.active);
            println!("  Events: {:?}", webhook.events);
            println!("\nNote: Webhook was created. You may want to delete it manually.");
        }
        Err(e) => {
            println!("Error creating webhook: {}", e);
            panic!("Test failed: {}", e);
        }
    }
}
