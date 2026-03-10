-- Migration 006: Remove debug session logs table

DROP INDEX IF EXISTS idx_session_logs_session_id;
DROP INDEX IF EXISTS idx_session_logs_created_at;
DROP TABLE IF EXISTS session_logs;
