-- Add explicit terminal failure status to outbox_events.
-- Replaces synthetic 100-year backoff with operational terminal poison flag.

ALTER TABLE outbox_events
    ADD COLUMN IF NOT EXISTS is_terminal BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_outbox_terminal
    ON outbox_events (created_at)
    WHERE is_terminal = TRUE;
