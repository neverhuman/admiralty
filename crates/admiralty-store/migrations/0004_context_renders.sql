-- Additive component tables; global user_version remains integration-owned.
CREATE TABLE IF NOT EXISTS context_render_records (
  scope_digest TEXT NOT NULL CHECK(length(scope_digest)=71),
  render_id TEXT NOT NULL CHECK(length(CAST(render_id AS BLOB)) BETWEEN 1 AND 128),
  version INTEGER NOT NULL CHECK(version BETWEEN 1 AND 4294967295),
  record_json TEXT NOT NULL CHECK(length(CAST(record_json AS BLOB))<=196608 AND json_valid(record_json)),
  record_digest TEXT NOT NULL CHECK(length(record_digest)=71),
  PRIMARY KEY(scope_digest,render_id,version)
) STRICT;
CREATE TABLE IF NOT EXISTS context_render_commands (
  scope_digest TEXT NOT NULL CHECK(length(scope_digest)=71),
  command_id TEXT NOT NULL CHECK(length(CAST(command_id AS BLOB)) BETWEEN 1 AND 128),
  sequence INTEGER NOT NULL CHECK(sequence>0),
  request_json TEXT NOT NULL CHECK(length(CAST(request_json AS BLOB))<=16384 AND json_valid(request_json)),
  request_digest TEXT NOT NULL CHECK(length(request_digest)=71),
  receipt_json TEXT NOT NULL CHECK(length(CAST(receipt_json AS BLOB))<=16384 AND json_valid(receipt_json)),
  receipt_digest TEXT NOT NULL CHECK(length(receipt_digest)=71),
  PRIMARY KEY(scope_digest,command_id), UNIQUE(scope_digest,sequence)
) STRICT;
CREATE TABLE IF NOT EXISTS context_render_baselines (
  scope_digest TEXT PRIMARY KEY CHECK(length(scope_digest)=71),
  current_sequence INTEGER NOT NULL CHECK(current_sequence>0),
  FOREIGN KEY(scope_digest,current_sequence) REFERENCES context_render_commands(scope_digest,sequence)
) STRICT;
CREATE TRIGGER IF NOT EXISTS context_render_immutable_update BEFORE UPDATE ON context_render_records
BEGIN SELECT RAISE(ABORT,'context render is immutable'); END;
CREATE TRIGGER IF NOT EXISTS context_render_immutable_delete BEFORE DELETE ON context_render_records
BEGIN SELECT RAISE(ABORT,'context render is immutable'); END;
CREATE TRIGGER IF NOT EXISTS context_render_command_immutable_update BEFORE UPDATE ON context_render_commands
BEGIN SELECT RAISE(ABORT,'context baseline command is immutable'); END;
CREATE TRIGGER IF NOT EXISTS context_render_command_immutable_delete BEFORE DELETE ON context_render_commands
BEGIN SELECT RAISE(ABORT,'context baseline command is immutable'); END;
