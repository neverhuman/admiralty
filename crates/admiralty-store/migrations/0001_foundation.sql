CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS domain_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  producer_id TEXT NOT NULL,
  incarnation TEXT NOT NULL,
  producer_seq INTEGER NOT NULL CHECK(producer_seq > 0),
  digest TEXT NOT NULL,
  observed_utc TEXT NOT NULL,
  payload TEXT NOT NULL CHECK(json_valid(payload)),
  UNIQUE(producer_id, incarnation, producer_seq)
) STRICT;
CREATE TABLE IF NOT EXISTS producer_watermarks (
  producer_id TEXT NOT NULL,
  incarnation TEXT NOT NULL,
  contiguous_seq INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(producer_id, incarnation)
) STRICT;
CREATE TABLE IF NOT EXISTS integrity_incidents (
  id INTEGER PRIMARY KEY,
  producer_id TEXT NOT NULL,
  incarnation TEXT NOT NULL,
  producer_seq INTEGER NOT NULL,
  original_digest TEXT NOT NULL,
  conflicting_digest TEXT NOT NULL,
  observed_utc TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS events_producer ON domain_events(producer_id, incarnation, producer_seq);
PRAGMA user_version = 1;
