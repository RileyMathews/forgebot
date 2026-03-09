-- Migration 004: Add 'revising' state to sessions table
-- The code uses 'revising' state for revision actions but the CHECK constraint
-- in migration 002 doesn't include it.

-- SQLite doesn't support ALTER TABLE to modify CHECK constraints directly.
-- We need to recreate the table with the updated constraint.

-- Create new sessions table with updated CHECK constraint
CREATE TABLE sessions_new (
    id TEXT PRIMARY KEY,
    repo_full_name TEXT NOT NULL REFERENCES repos(full_name) ON DELETE CASCADE,
    issue_id INTEGER NOT NULL,
    pr_id INTEGER,
    opencode_session_id TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('planning', 'building', 'idle', 'busy', 'error', 'revising')),
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(repo_full_name, issue_id)
);

-- Copy data from old table
INSERT INTO sessions_new SELECT * FROM sessions;

-- Drop old table
DROP TABLE sessions;

-- Rename new table
ALTER TABLE sessions_new RENAME TO sessions;

-- Recreate indexes
CREATE INDEX IF NOT EXISTS idx_sessions_repo_issue ON sessions(repo_full_name, issue_id);
CREATE INDEX IF NOT EXISTS idx_sessions_pr_id ON sessions(pr_id);
CREATE INDEX IF NOT EXISTS idx_sessions_state ON sessions(state);
