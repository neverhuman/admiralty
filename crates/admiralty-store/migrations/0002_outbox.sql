-- Prototype storage only. Initializing or replaying these rows performs no effect.
-- The integration owner owns the global database schema-version transition.
CREATE TABLE IF NOT EXISTS outbox_clock (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  last_observed INTEGER NOT NULL CHECK(last_observed >= 0)
) STRICT;
CREATE TABLE IF NOT EXISTS outbox_intents (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  idempotency_key TEXT NOT NULL UNIQUE CHECK(length(idempotency_key) BETWEEN 1 AND 256),
  payload TEXT NOT NULL CHECK(json_valid(payload) AND length(CAST(payload AS BLOB)) <= 16384),
  payload_digest TEXT NOT NULL,
  not_before INTEGER NOT NULL CHECK(not_before >= 0),
  state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','claimed','unknown','completed')),
  fence INTEGER NOT NULL DEFAULT 0 CHECK(fence >= 0),
  claim_nonce TEXT UNIQUE,
  claim_owner TEXT,
  lease_deadline INTEGER,
  receipt_ref TEXT,
  unknown_reason TEXT,
  CHECK((state = 'pending' AND fence = 0 AND claim_nonce IS NULL AND claim_owner IS NULL AND lease_deadline IS NULL AND receipt_ref IS NULL AND unknown_reason IS NULL)
    OR (state = 'claimed' AND fence > 0 AND claim_nonce IS NOT NULL AND claim_owner IS NOT NULL AND lease_deadline IS NOT NULL AND receipt_ref IS NULL AND unknown_reason IS NULL)
    OR (state = 'unknown' AND fence > 0 AND claim_nonce IS NOT NULL AND claim_owner IS NOT NULL AND lease_deadline IS NOT NULL AND receipt_ref IS NULL AND unknown_reason IS NOT NULL)
    OR (state = 'completed' AND fence > 0 AND claim_nonce IS NOT NULL AND claim_owner IS NOT NULL AND lease_deadline IS NOT NULL AND receipt_ref IS NOT NULL AND unknown_reason IS NULL))
) STRICT;
CREATE INDEX IF NOT EXISTS outbox_pending_fifo ON outbox_intents(state, id, not_before);
CREATE INDEX IF NOT EXISTS outbox_expiring_claims ON outbox_intents(state, lease_deadline);
CREATE TABLE IF NOT EXISTS outbox_integrity_incidents (
  id INTEGER PRIMARY KEY,
  idempotency_key TEXT NOT NULL,
  original_digest TEXT NOT NULL,
  conflicting_digest TEXT NOT NULL
) STRICT;
CREATE TRIGGER IF NOT EXISTS outbox_immutable_intent
BEFORE UPDATE OF idempotency_key,payload,payload_digest,not_before ON outbox_intents
WHEN NEW.idempotency_key != OLD.idempotency_key OR NEW.payload != OLD.payload OR NEW.payload_digest != OLD.payload_digest OR NEW.not_before != OLD.not_before
BEGIN SELECT RAISE(ABORT, 'outbox intent identity is immutable'); END;
CREATE TRIGGER IF NOT EXISTS outbox_monotonic_states
BEFORE UPDATE OF state ON outbox_intents
WHEN NOT ((OLD.state = 'pending' AND NEW.state = 'claimed') OR (OLD.state = 'claimed' AND NEW.state IN ('unknown','completed')) OR OLD.state = NEW.state)
BEGIN SELECT RAISE(ABORT, 'outbox state cannot be retried or reversed'); END;
