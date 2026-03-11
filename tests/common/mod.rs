#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use forgebot::db::{
    DbPool, NewSession, init_db_at_path, insert_repo, insert_session, update_repo_clone_status,
};

static TEST_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

pub async fn setup_test_db() -> (DbPool, PathBuf) {
    let test_id = TEST_DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    let test_dir =
        std::env::temp_dir().join(format!("forgebot-test-{}-{}", std::process::id(), test_id));

    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir_all(&test_dir).expect("Failed to create test dir");

    let db_path = test_dir.join("test.db");
    let pool = init_db_at_path(&db_path)
        .await
        .expect("Failed to initialize test database");

    (pool, test_dir)
}

pub fn cleanup_test_db(test_dir: &Path) {
    let _ = std::fs::remove_dir_all(test_dir);
}

pub async fn insert_test_session(
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
        mode: "collab".to_string(),
    };

    insert_session(pool, &session).await?;
    Ok(())
}

pub async fn insert_test_repo(
    pool: &DbPool,
    full_name: &str,
    clone_status: &str,
) -> anyhow::Result<()> {
    let repo_id = format!("repo-{}", full_name.replace('/', "-"));
    insert_repo(pool, &repo_id, full_name, "main", "nix").await?;

    if clone_status != "pending" {
        update_repo_clone_status(pool, full_name, clone_status, None).await?;
    }

    Ok(())
}
