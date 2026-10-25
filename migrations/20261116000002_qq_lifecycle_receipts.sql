-- Webhook timestamps have no age restriction. Keep lifecycle replay receipts
-- as long as the app exists, otherwise replaying an old ADD could undo DEL.
CREATE TABLE qq_lifecycle_receipts (
    app_id TEXT NOT NULL REFERENCES qq_bots(app_id) ON DELETE CASCADE,
    event_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(app_id, event_hash)
);
