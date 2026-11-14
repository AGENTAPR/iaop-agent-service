-- Add atomic leasing columns to outbox_events to prevent multi-instance race conditions.
-- locked_until: timestamp until which the lease is held by locked_by
-- locked_by: identifier of the publisher instance currently holding the lease

ALTER TABLE outbox_events
    ADD COLUMN IF NOT EXISTS locked_until TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS locked_by    TEXT;

CREATE INDEX IF NOT EXISTS idx_outbox_lease_claim
    ON outbox_events (locked_until NULLS FIRST, next_attempt_at NULLS FIRST, created_at)
    WHERE published_at IS NULL;
