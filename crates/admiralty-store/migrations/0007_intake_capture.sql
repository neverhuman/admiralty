-- Original request custody only: no work orders, tasks, plans or outbox effects.
CREATE TABLE IF NOT EXISTS main.intake_capture_heads (
 project_id TEXT NOT NULL,root_id TEXT NOT NULL,capture_id TEXT NOT NULL,
 current_revision INTEGER NOT NULL CHECK(current_revision BETWEEN 1 AND 32),
 PRIMARY KEY(project_id,root_id,capture_id),
 FOREIGN KEY(project_id,root_id,capture_id,current_revision)
 REFERENCES intake_capture_revisions(project_id,root_id,capture_id,revision)
 DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE IF NOT EXISTS main.intake_capture_sources (
 source_key TEXT PRIMARY KEY CHECK(length(source_key)=71),
 project_id TEXT NOT NULL,root_id TEXT NOT NULL,capture_id TEXT NOT NULL,revision INTEGER NOT NULL,
 provenance_digest TEXT NOT NULL CHECK(length(provenance_digest)=71),
 raw_digest TEXT NOT NULL CHECK(length(raw_digest)=71),
 UNIQUE(project_id,root_id,capture_id,revision),
 FOREIGN KEY(project_id,root_id,capture_id,revision)
 REFERENCES intake_capture_revisions(project_id,root_id,capture_id,revision)
 DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE IF NOT EXISTS main.intake_capture_revisions (
 project_id TEXT NOT NULL,root_id TEXT NOT NULL,capture_id TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 32),
 command_id TEXT NOT NULL UNIQUE,
 principal_id TEXT NOT NULL,authority_ref TEXT NOT NULL,authority_generation TEXT NOT NULL,
 source_key TEXT NOT NULL UNIQUE CHECK(length(source_key)=71),
 request_digest TEXT NOT NULL CHECK(length(request_digest)=71),
 receipt_digest TEXT NOT NULL CHECK(length(receipt_digest)=71),
 request_json TEXT NOT NULL CHECK(json_valid(request_json) AND length(CAST(request_json AS BLOB))<=8388608),
 preview_json TEXT NOT NULL CHECK(json_valid(preview_json) AND length(CAST(preview_json AS BLOB))<=8388608),
 policy_json TEXT NOT NULL CHECK(json_valid(policy_json) AND length(CAST(policy_json AS BLOB))<=8192),
 receipt_json TEXT NOT NULL CHECK(json_valid(receipt_json) AND length(CAST(receipt_json AS BLOB))<=8192),
 PRIMARY KEY(project_id,root_id,capture_id,revision),
 FOREIGN KEY(project_id,root_id,capture_id) REFERENCES intake_capture_heads(project_id,root_id,capture_id)
 DEFERRABLE INITIALLY DEFERRED,
 FOREIGN KEY(source_key) REFERENCES intake_capture_sources(source_key)
 DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE INDEX IF NOT EXISTS main.intake_capture_scope_lookup
 ON intake_capture_revisions(project_id,root_id,principal_id,authority_ref,authority_generation,capture_id,revision);
CREATE TRIGGER IF NOT EXISTS main.intake_capture_revision_update BEFORE UPDATE ON intake_capture_revisions
 BEGIN SELECT RAISE(ABORT,'captured request revision is immutable'); END;
CREATE TRIGGER IF NOT EXISTS main.intake_capture_revision_delete BEFORE DELETE ON intake_capture_revisions
 BEGIN SELECT RAISE(ABORT,'captured request revision is immutable'); END;
CREATE TRIGGER IF NOT EXISTS main.intake_capture_source_update BEFORE UPDATE ON intake_capture_sources
 BEGIN SELECT RAISE(ABORT,'captured source is immutable'); END;
CREATE TRIGGER IF NOT EXISTS main.intake_capture_source_delete BEFORE DELETE ON intake_capture_sources
 BEGIN SELECT RAISE(ABORT,'captured source is immutable'); END;
CREATE TRIGGER IF NOT EXISTS main.intake_capture_head_identity BEFORE UPDATE OF project_id,root_id,capture_id ON intake_capture_heads
 BEGIN SELECT RAISE(ABORT,'capture identity is immutable'); END;
CREATE TRIGGER IF NOT EXISTS main.intake_capture_head_delete BEFORE DELETE ON intake_capture_heads
 BEGIN SELECT RAISE(ABORT,'capture identity is immutable'); END;
