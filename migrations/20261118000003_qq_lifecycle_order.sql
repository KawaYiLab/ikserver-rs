-- Official lifecycle events carry Unix-second timestamps. Missing and equal
-- timestamps continue to use receipt order; only provably older events lose.
ALTER TABLE qq_groups ADD COLUMN lifecycle_event_at TIMESTAMPTZ;
ALTER TABLE qq_identities ADD COLUMN lifecycle_event_at TIMESTAMPTZ;
