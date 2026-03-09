-- Migration 003: Add clone status tracking to repos table

-- Add clone status columns to repos table
ALTER TABLE repos ADD COLUMN clone_status TEXT NOT NULL DEFAULT 'pending' CHECK(clone_status IN ('pending', 'cloning', 'ready', 'failed'));
ALTER TABLE repos ADD COLUMN clone_error TEXT;
ALTER TABLE repos ADD COLUMN clone_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE repos ADD COLUMN last_clone_attempt_at TEXT;

-- Index for dashboard queries filtering by clone status
CREATE INDEX IF NOT EXISTS idx_repos_clone_status ON repos(clone_status);
