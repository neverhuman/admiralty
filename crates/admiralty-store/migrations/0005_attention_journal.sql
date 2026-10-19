-- Storage component only. No delivery, authority, or global user_version change.
CREATE TABLE IF NOT EXISTS attention_journals (
  journal_id TEXT PRIMARY KEY CHECK(length(CAST(journal_id AS BLOB)) BETWEEN 1 AND 128),
  definition_json TEXT NOT NULL CHECK(length(CAST(definition_json AS BLOB))<=8192 AND json_valid(definition_json)),
  definition_digest TEXT NOT NULL CHECK(length(definition_digest)=71),
  head_sequence INTEGER NOT NULL DEFAULT 0 CHECK(head_sequence>=0),
  head_digest TEXT CHECK(head_digest IS NULL OR length(head_digest)=71),
  CHECK((head_sequence=0 AND head_digest IS NULL) OR (head_sequence>0 AND head_digest IS NOT NULL))
) STRICT;
CREATE TABLE IF NOT EXISTS attention_commands (
  journal_id TEXT NOT NULL REFERENCES attention_journals(journal_id),
  command_id TEXT NOT NULL CHECK(length(CAST(command_id AS BLOB)) BETWEEN 1 AND 128),
  sequence INTEGER NOT NULL CHECK(sequence>0),
  command_json TEXT NOT NULL CHECK(length(CAST(command_json AS BLOB))<=32768 AND json_valid(command_json)),
  command_digest TEXT NOT NULL CHECK(length(command_digest)=71),
  receipt_json TEXT NOT NULL CHECK(length(CAST(receipt_json AS BLOB))<=32768 AND json_valid(receipt_json)),
  receipt_digest TEXT NOT NULL CHECK(length(receipt_digest)=71),
  outbox_id INTEGER UNIQUE REFERENCES outbox_intents(id),
  PRIMARY KEY(journal_id,command_id), UNIQUE(journal_id,sequence)
) STRICT;
CREATE TRIGGER IF NOT EXISTS attention_definition_immutable_update
BEFORE UPDATE OF journal_id,definition_json,definition_digest ON attention_journals
BEGIN SELECT RAISE(ABORT,'attention definition is immutable'); END;
CREATE TRIGGER IF NOT EXISTS attention_definition_immutable_delete BEFORE DELETE ON attention_journals
BEGIN SELECT RAISE(ABORT,'attention definition is immutable'); END;
CREATE TRIGGER IF NOT EXISTS attention_command_immutable_update BEFORE UPDATE ON attention_commands
BEGIN SELECT RAISE(ABORT,'attention command is immutable'); END;
CREATE TRIGGER IF NOT EXISTS attention_command_immutable_delete BEFORE DELETE ON attention_commands
BEGIN SELECT RAISE(ABORT,'attention command is immutable'); END;
