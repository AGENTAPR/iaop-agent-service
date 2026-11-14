-- Extend outbox_events for envelope fields + retry scheduling.
-- Expand-only change: existing rows remain valid (new columns nullable).

ALTER TABLE outbox_events
    ADD COLUMN IF NOT EXISTS source          TEXT,
    ADD COLUMN IF NOT EXISTS correlation_id  UUID,
    ADD COLUMN IF NOT EXISTS causation_id    UUID,
    ADD COLUMN IF NOT EXISTS next_attempt_at TIMESTAMPTZ;

-- Publisher claim scan: unpublished rows due for (re)delivery.
CREATE INDEX IF NOT EXISTS idx_outbox_due
    ON outbox_events (next_attempt_at NULLS FIRST, created_at)
    WHERE published_at IS NULL;
