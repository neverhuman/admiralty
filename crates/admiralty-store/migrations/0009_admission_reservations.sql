-- Storage only. No limits, grants, reservations or settlement are seeded.
CREATE TABLE IF NOT EXISTS admission_reservation_commands (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    command_id TEXT NOT NULL UNIQUE,
    source_key TEXT NOT NULL UNIQUE,
    command_json TEXT NOT NULL,
    command_digest TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    receipt_digest TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS admission_reservation_ledger (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision TEXT NOT NULL,
    ledger_json TEXT NOT NULL,
    ledger_digest TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS event_reservation_associations (
    event_seq INTEGER PRIMARY KEY REFERENCES domain_events(seq),
    command_id TEXT NOT NULL UNIQUE REFERENCES admission_reservation_commands(command_id),
    command_digest TEXT NOT NULL,
    receipt_digest TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS admission_reservation_commands_no_update
BEFORE UPDATE ON admission_reservation_commands BEGIN
    SELECT RAISE(ABORT, 'immutable reservation command');
END;
CREATE TRIGGER IF NOT EXISTS admission_reservation_commands_no_delete
BEFORE DELETE ON admission_reservation_commands BEGIN
    SELECT RAISE(ABORT, 'immutable reservation command');
END;
CREATE TRIGGER IF NOT EXISTS event_reservation_associations_no_update
BEFORE UPDATE ON event_reservation_associations BEGIN
    SELECT RAISE(ABORT, 'immutable reservation association');
END;
CREATE TRIGGER IF NOT EXISTS event_reservation_associations_no_delete
BEFORE DELETE ON event_reservation_associations BEGIN
    SELECT RAISE(ABORT, 'immutable reservation association');
END;
