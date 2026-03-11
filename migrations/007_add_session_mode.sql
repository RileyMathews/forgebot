-- Migration 007: Add persistent session mode (collab/build)
--
-- This mode controls which agent workflow is used for issue comments:
-- - collab: discussion/brainstorming mode
-- - build: implementation/PR mode (sticky after --build)

CREATE TABLE sessions_new (
    id TEXT PRIMARY KEY,
    repo_full_name TEXT NOT NULL REFERENCES repos(full_name) ON DELETE CASCADE,
    issue_id INTEGER NOT NULL,
    pr_id INTEGER,
    opencode_session_id TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('planning', 'building', 'idle', 'busy', 'error', 'revising')),
    mode TEXT NOT NULL DEFAULT 'collab' CHECK(mode IN ('collab', 'build')),
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(repo_full_name, issue_id)
);

INSERT INTO sessions_new (
    id,
    repo_full_name,
    issue_id,
    pr_id,
    opencode_session_id,
    worktree_path,
    state,
    created_at,
    updated_at
)
SELECT
    id,
    repo_full_name,
    issue_id,
    pr_id,
    opencode_session_id,
    worktree_path,
    state,
    created_at,
    updated_at
FROM sessions;

DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;

CREATE INDEX IF NOT EXISTS idx_sessions_repo_issue ON sessions(repo_full_name, issue_id);
CREATE INDEX IF NOT EXISTS idx_sessions_pr_id ON sessions(pr_id);
CREATE INDEX IF NOT EXISTS idx_sessions_state ON sessions(state);
