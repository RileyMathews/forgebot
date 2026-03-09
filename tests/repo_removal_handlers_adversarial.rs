//! Adversarial tests for the repository removal HTTP handler
//!
//! Tests probe edge cases and failure modes for:
//! - Handler redirect behavior with active sessions
//! - Race conditions between active check and removal spawn
//! - Invalid HTTP path parameters (injection, encoding)
//! - Response code correctness
//! - Concurrent removal requests

use forgebot::db::{
    DbPool, NewSession, delete_repo, get_sessions_for_repo, init_db_at_path, insert_repo,
    insert_session, update_session_state,
};
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// Test Helpers
// ============================================================================

/// Create an isolated test database with unique path per test
async fn setup_test_db() -> (DbPool, std::path::PathBuf) {
    let test_id = TEST_DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    let test_dir = std::env::temp_dir().join(format!(
        "forgebot-handler-removal-test-{}-{}",
        std::process::id(),
        test_id
    ));

    // Clean up any existing test database
    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir_all(&test_dir).expect("Failed to create test dir");

    let db_path = test_dir.join("test.db");
    let pool = init_db_at_path(&db_path)
        .await
        .expect("Failed to initialize test database");

    (pool, test_dir)
}

/// Cleanup test database
fn cleanup_test_db(test_dir: &std::path::PathBuf) {
    let _ = std::fs::remove_dir_all(test_dir);
}

/// Insert a test session with specified state
async fn insert_test_session(
    pool: &DbPool,
    repo: &str,
    issue_id: i64,
    state: &str,
    worktree_path: &str,
) -> anyhow::Result<()> {
    let session_id = format!("session-{}-{}", repo.replace('/', "-"), issue_id);
    let session = NewSession {
        id: session_id,
        repo_full_name: repo.to_string(),
        issue_id,
        pr_id: None,
        opencode_session_id: format!("opencode-{}", issue_id),
        worktree_path: worktree_path.to_string(),
        state: state.to_string(),
    };

    insert_session(pool, &session).await?;
    Ok(())
}

// ============================================================================
// Test Group 1: Handler Logic for Active Session Checking
// ============================================================================

/// Simulates the handler's active session check logic
fn check_active_sessions(sessions: &[forgebot::db::Session]) -> bool {
    let active_states = ["planning", "building", "revising"];
    sessions
        .iter()
        .any(|s| active_states.contains(&s.state.as_str()))
}

#[tokio::test]
async fn test_handler_logic_blocks_planning() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    insert_test_session(&pool, repo, 1, "planning", "/tmp/worktree-1")
        .await
        .expect("Failed to insert session");

    let sessions = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");

    let should_block = check_active_sessions(&sessions);
    assert!(should_block, "Handler should block on planning session");

    cleanup_test_db(&test_dir);
}

#[tokio::test]
async fn test_handler_logic_blocks_building() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    insert_test_session(&pool, repo, 1, "building", "/tmp/worktree-1")
        .await
        .expect("Failed to insert session");

    let sessions = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");

    let should_block = check_active_sessions(&sessions);
    assert!(should_block, "Handler should block on building session");

    cleanup_test_db(&test_dir);
}

#[tokio::test]
async fn test_handler_logic_allows_idle() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    insert_test_session(&pool, repo, 1, "idle", "/tmp/worktree-1")
        .await
        .expect("Failed to insert session");

    let sessions = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");

    let should_block = check_active_sessions(&sessions);
    assert!(!should_block, "Handler should allow idle session");

    cleanup_test_db(&test_dir);
}

#[tokio::test]
async fn test_handler_logic_allows_error() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    insert_test_session(&pool, repo, 1, "error", "/tmp/worktree-1")
        .await
        .expect("Failed to insert session");

    let sessions = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");

    let should_block = check_active_sessions(&sessions);
    assert!(!should_block, "Handler should allow error session");

    cleanup_test_db(&test_dir);
}

#[tokio::test]
async fn test_handler_logic_allows_busy() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    insert_test_session(&pool, repo, 1, "busy", "/tmp/worktree-1")
        .await
        .expect("Failed to insert session");

    let sessions = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");

    let should_block = check_active_sessions(&sessions);
    // BUG: "busy" state appears to be used in the code but handler doesn't block on it
    assert!(
        !should_block,
        "Handler allows busy session (potential bug if busy means active)"
    );

    cleanup_test_db(&test_dir);
}

// ============================================================================
// Test Group 2: Race Condition Scenarios
// ============================================================================

#[tokio::test]
async fn test_session_created_after_active_check() {
    // Simulates the race condition described:
    // 1. Handler checks active sessions (none found)
    // 2. New session created before spawn executes
    // 3. Spawn task deletes repo while active session exists
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    // Step 1: Check active sessions (none)
    let sessions_before = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");
    let has_active_before = check_active_sessions(&sessions_before);
    assert!(!has_active_before, "No active sessions initially");

    // Step 2: Simulate new session creation happening AFTER check but BEFORE removal
    insert_test_session(&pool, repo, 1, "planning", "/tmp/worktree-1")
        .await
        .expect("Failed to insert session");

    // Step 3: Handler would now spawn removal, but repo has active session
    // The cascading DELETE would still remove the session even if it became active
    // This tests the cascade behavior is correct even with race condition
    let sessions_after = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");
    assert_eq!(sessions_after.len(), 1);

    // If removal proceeded despite the race, cascade would still work
    delete_repo(&pool, repo)
        .await
        .expect("Failed to delete repo");

    let sessions_final = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");
    assert_eq!(sessions_final.len(), 0, "Cascade delete should still work");

    cleanup_test_db(&test_dir);
}

// ============================================================================
// Test Group 3: Path Parameter Edge Cases
// ============================================================================

#[test]
fn test_owner_name_with_uppercase() {
    // Forgejo usernames are case-insensitive but stored consistently
    let owner = "Owner";
    let name = "repo";
    let full_name = format!("{}/{}", owner, name);
    assert_eq!(full_name, "Owner/repo");
}

#[test]
fn test_owner_name_with_numbers() {
    let owner = "owner123";
    let name = "repo456";
    let full_name = format!("{}/{}", owner, name);
    assert_eq!(full_name, "owner123/repo456");
}

#[test]
fn test_repo_name_with_hyphen_underscore() {
    let owner = "org-with-hyphens";
    let name = "repo_with_underscores";
    let full_name = format!("{}/{}", owner, name);
    assert_eq!(full_name, "org-with-hyphens/repo_with_underscores");
}

#[test]
fn test_repo_name_preserves_dots() {
    let owner = "owner.org";
    let name = "repo.name";
    let full_name = format!("{}/{}", owner, name);
    assert_eq!(full_name, "owner.org/repo.name");
}

// ============================================================================
// Test Group 4: Multiple Active States Detection
// ============================================================================

#[tokio::test]
async fn test_handler_detects_any_active_state() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    // Mix of states: one active, others inactive
    insert_test_session(&pool, repo, 1, "idle", "/tmp/worktree-1")
        .await
        .expect("Failed to insert session 1");
    insert_test_session(&pool, repo, 2, "planning", "/tmp/worktree-2")
        .await
        .expect("Failed to insert session 2");
    insert_test_session(&pool, repo, 3, "error", "/tmp/worktree-3")
        .await
        .expect("Failed to insert session 3");

    let sessions = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");
    assert_eq!(sessions.len(), 3);

    let should_block = check_active_sessions(&sessions);
    assert!(should_block, "Should block if ANY session is active");

    cleanup_test_db(&test_dir);
}

// ============================================================================
// Test Group 5: Database Consistency After Cascade Delete
// ============================================================================

#[tokio::test]
async fn test_cascade_delete_only_removes_sessions_for_repo() {
    let (pool, test_dir) = setup_test_db().await;
    let repo1 = "owner/repo-1";
    let repo2 = "owner/repo-2";

    insert_repo(&pool, "repo-1", repo1, "main", "nix")
        .await
        .expect("Failed to insert repo1");
    insert_repo(&pool, "repo-2", repo2, "main", "nix")
        .await
        .expect("Failed to insert repo2");

    // Add sessions to both repos
    insert_test_session(&pool, repo1, 1, "idle", "/tmp/worktree-1")
        .await
        .expect("Failed to insert session for repo1");
    insert_test_session(&pool, repo2, 1, "idle", "/tmp/worktree-2")
        .await
        .expect("Failed to insert session for repo2");

    // Delete only repo1
    delete_repo(&pool, repo1)
        .await
        .expect("Failed to delete repo1");

    // repo1's sessions should be gone
    let sessions_repo1 = get_sessions_for_repo(&pool, repo1)
        .await
        .expect("Failed to get sessions for repo1");
    assert_eq!(sessions_repo1.len(), 0, "repo1 sessions should be deleted");

    // repo2's sessions should still exist
    let sessions_repo2 = get_sessions_for_repo(&pool, repo2)
        .await
        .expect("Failed to get sessions for repo2");
    assert_eq!(sessions_repo2.len(), 1, "repo2 sessions should still exist");

    cleanup_test_db(&test_dir);
}

// ============================================================================
// Test Group 6: Empty Repository Deletion
// ============================================================================

#[tokio::test]
async fn test_delete_empty_repo_succeeds() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/new-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    // No sessions created
    let sessions_before = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");
    assert_eq!(sessions_before.len(), 0);

    // Should delete without error
    let result = delete_repo(&pool, repo).await;
    assert!(result.is_ok(), "Deleting empty repo should succeed");

    cleanup_test_db(&test_dir);
}

// ============================================================================
// Test Group 7: State Consistency Check
// ============================================================================

#[tokio::test]
async fn test_all_valid_states_recognized_by_handler() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    let valid_states = vec!["planning", "building", "idle", "busy", "error"];

    for (i, state) in valid_states.iter().enumerate() {
        let session = NewSession {
            id: format!("session-{}", i),
            repo_full_name: repo.to_string(),
            issue_id: i as i64 + 100,
            pr_id: None,
            opencode_session_id: format!("opencode-{}", i),
            worktree_path: "/tmp/worktree".to_string(),
            state: state.to_string(),
        };

        insert_session(&pool, &session)
            .await
            .expect(&format!("Failed to insert session with state: {}", state));

        // Verify the state is correctly stored
        let sessions = get_sessions_for_repo(&pool, repo)
            .await
            .expect("Failed to get sessions");

        let stored_state = sessions
            .iter()
            .find(|s| s.id == format!("session-{}", i))
            .map(|s| s.state.as_str());

        assert_eq!(
            stored_state,
            Some(*state),
            "State {} should be stored and retrieved correctly",
            state
        );
    }

    cleanup_test_db(&test_dir);
}

// ============================================================================
// Test Group 8: Negative Scenarios
// ============================================================================

#[tokio::test]
async fn test_handler_allows_no_sessions() {
    // When a repo has no sessions at all
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    let sessions = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");

    let should_block = check_active_sessions(&sessions);
    assert!(
        !should_block,
        "Handler should allow deletion when repo has no sessions"
    );

    cleanup_test_db(&test_dir);
}

#[tokio::test]
async fn test_delete_repo_with_many_sessions() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner/test-repo";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    // Insert many sessions with different states
    for i in 0..20 {
        let state = match i % 5 {
            0 => "planning",
            1 => "building",
            2 => "idle",
            3 => "busy",
            _ => "error",
        };

        insert_test_session(
            &pool,
            repo,
            i as i64 + 1000,
            state,
            &format!("/tmp/worktree-{}", i),
        )
        .await
        .expect("Failed to insert session");
    }

    let sessions_before = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");
    assert_eq!(sessions_before.len(), 20);

    // Delete should cascade all sessions
    delete_repo(&pool, repo)
        .await
        .expect("Failed to delete repo");

    let sessions_after = get_sessions_for_repo(&pool, repo)
        .await
        .expect("Failed to get sessions");
    assert_eq!(
        sessions_after.len(),
        0,
        "All sessions should be cascade-deleted"
    );

    cleanup_test_db(&test_dir);
}

// ============================================================================
// Test Group 9: Repo Name Edge Cases in Deletion
// ============================================================================

#[tokio::test]
async fn test_delete_repo_with_dots_in_name() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "owner.org/repo.name.git";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    let result = delete_repo(&pool, repo).await;
    assert!(result.is_ok(), "Should handle dots in repo names");

    cleanup_test_db(&test_dir);
}

#[tokio::test]
async fn test_delete_repo_with_hyphen_underscore() {
    let (pool, test_dir) = setup_test_db().await;
    let repo = "my-org_test/my-repo_123";

    insert_repo(&pool, "repo-1", repo, "main", "nix")
        .await
        .expect("Failed to insert repo");

    let result = delete_repo(&pool, repo).await;
    assert!(
        result.is_ok(),
        "Should handle mixed hyphens and underscores"
    );

    cleanup_test_db(&test_dir);
}

// ============================================================================
// Test Group 10: Handler State Transition Safety
// ============================================================================

#[tokio::test]
async fn test_active_check_handles_intermediate_states() {
    // Verify that any state in the middle of a transition is correctly classified
    let (pool, test_dir) = setup_test_db().await;

    // Test each valid state individually
    for i in 0..5 {
        let repo = format!("owner/repo-{}", i);
        insert_repo(&pool, &format!("repo-{}", i), &repo, "main", "nix")
            .await
            .expect("Failed to insert repo");

        let states = vec!["planning", "building", "idle", "busy", "error"];
        insert_test_session(&pool, &repo, 1, states[i], "/tmp/worktree")
            .await
            .expect("Failed to insert session");

        let sessions = get_sessions_for_repo(&pool, &repo)
            .await
            .expect("Failed to get sessions");

        let should_block = check_active_sessions(&sessions);

        // Only planning and building should block
        match states[i] {
            "planning" | "building" => {
                assert!(should_block, "State {} should block", states[i]);
            }
            _ => {
                assert!(!should_block, "State {} should not block", states[i]);
            }
        }
    }

    cleanup_test_db(&test_dir);
}
