-- Migration 001: Create sessions and pending_worktrees tables
-- Note: repos table will be created in migration 002 to handle foreign key ordering

-- Sessions table: tracks active opencode sessions
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    repo_full_name TEXT NOT NULL,
    issue_id INTEGER NOT NULL,
    pr_id INTEGER,
    opencode_session_id TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('planning', 'building', 'idle', 'busy', 'error')),
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(repo_full_name, issue_id)
);

-- Index for looking up sessions by repo + issue
CREATE INDEX IF NOT EXISTS idx_sessions_repo_issue ON sessions(repo_full_name, issue_id);

-- Index for looking up sessions by PR ID
CREATE INDEX IF NOT EXISTS idx_sessions_pr_id ON sessions(pr_id);

-- Index for filtering sessions by state
CREATE INDEX IF NOT EXISTS idx_sessions_state ON sessions(state);

-- Pending worktrees table: tracks worktrees scheduled for cleanup
CREATE TABLE IF NOT EXISTS pending_worktrees (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    worktree_path TEXT NOT NULL,
    scheduled_at TEXT DEFAULT CURRENT_TIMESTAMP
);

-- Index for finding pending worktrees by session
CREATE INDEX IF NOT EXISTS idx_pending_worktrees_session ON pending_worktrees(session_id);
