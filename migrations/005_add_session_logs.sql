-- Migration 005: Create session_logs table to store opencode output
-- This allows debugging by viewing the full stdout/stderr from opencode sessions

-- Session logs table: stores the complete output from opencode subprocesses
CREATE TABLE IF NOT EXISTS session_logs (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    exit_code INTEGER,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

-- Index for finding logs by session
CREATE INDEX IF NOT EXISTS idx_session_logs_session_id ON session_logs(session_id);

-- Index for ordering logs by creation time
CREATE INDEX IF NOT EXISTS idx_session_logs_created_at ON session_logs(created_at);
