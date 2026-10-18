-- Additive component migration. The integration owner owns the global
-- PRAGMA user_version transition; this migration intentionally keeps version 1.
CREATE TABLE IF NOT EXISTS event_outbox_associations (
  event_seq INTEGER PRIMARY KEY REFERENCES domain_events(seq) ON DELETE RESTRICT,
  outbox_intent_id INTEGER NOT NULL UNIQUE REFERENCES outbox_intents(id) ON DELETE RESTRICT,
  idempotency_key TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 256),
  payload_digest TEXT NOT NULL CHECK(length(payload_digest) = 71 AND substr(payload_digest,1,7) = 'sha256:'),
  not_before INTEGER NOT NULL CHECK(not_before >= 0),
  intent_digest TEXT NOT NULL CHECK(length(intent_digest) = 71 AND substr(intent_digest,1,7) = 'sha256:')
) STRICT;

CREATE TRIGGER IF NOT EXISTS event_outbox_association_immutable_update
BEFORE UPDATE ON event_outbox_associations
BEGIN SELECT RAISE(ABORT, 'event/outbox association is immutable'); END;

CREATE TRIGGER IF NOT EXISTS event_outbox_association_immutable_delete
BEFORE DELETE ON event_outbox_associations
BEGIN SELECT RAISE(ABORT, 'event/outbox association is immutable'); END;
