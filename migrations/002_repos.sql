-- Migration 002: Create repos table
-- This is separate because sessions references repos via repo_full_name

-- Repos table: tracks registered repositories
CREATE TABLE IF NOT EXISTS repos (
    id TEXT PRIMARY KEY,
    full_name TEXT NOT NULL UNIQUE,
    default_branch TEXT NOT NULL,
    env_loader TEXT NOT NULL DEFAULT 'none' CHECK(env_loader IN ('nix', 'direnv', 'none')),
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP
);

-- Index for looking up repos by full_name
CREATE INDEX IF NOT EXISTS idx_repos_full_name ON repos(full_name);

-- Now add the foreign key constraint to sessions
-- SQLite doesn't support ALTER TABLE ADD CONSTRAINT, so we need to recreate the table

-- Create new sessions table with foreign key
CREATE TABLE sessions_new (
    id TEXT PRIMARY KEY,
    repo_full_name TEXT NOT NULL REFERENCES repos(full_name) ON DELETE CASCADE,
    issue_id INTEGER NOT NULL,
    pr_id INTEGER,
    opencode_session_id TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('planning', 'building', 'idle', 'busy', 'error')),
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
