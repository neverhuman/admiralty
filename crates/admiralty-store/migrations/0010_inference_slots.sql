-- Storage only. No capacity, authority, holds or release are seeded.
CREATE TABLE IF NOT EXISTS inference_slot_commands (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    command_id TEXT NOT NULL UNIQUE,
    source_key TEXT NOT NULL UNIQUE,
    command_json TEXT NOT NULL,
    command_digest TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    receipt_digest TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS inference_slot_ledger (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision TEXT NOT NULL,
    ledger_json TEXT NOT NULL,
    ledger_digest TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS event_inference_slot_associations (
    event_seq INTEGER PRIMARY KEY REFERENCES domain_events(seq),
    command_id TEXT NOT NULL UNIQUE REFERENCES inference_slot_commands(command_id),
    command_digest TEXT NOT NULL,
    receipt_digest TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS inference_slot_commands_no_update
BEFORE UPDATE ON inference_slot_commands BEGIN
    SELECT RAISE(ABORT, 'immutable inference slot command');
END;
CREATE TRIGGER IF NOT EXISTS inference_slot_commands_no_delete
BEFORE DELETE ON inference_slot_commands BEGIN
    SELECT RAISE(ABORT, 'immutable inference slot command');
END;
CREATE TRIGGER IF NOT EXISTS event_inference_slot_associations_no_update
BEFORE UPDATE ON event_inference_slot_associations BEGIN
    SELECT RAISE(ABORT, 'immutable inference slot association');
END;
CREATE TRIGGER IF NOT EXISTS event_inference_slot_associations_no_delete
BEFORE DELETE ON event_inference_slot_associations BEGIN
    SELECT RAISE(ABORT, 'immutable inference slot association');
END;
