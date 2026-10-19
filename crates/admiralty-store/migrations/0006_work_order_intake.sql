-- WO2 storage only; no task events, outbox effects or global user_version change.
CREATE TABLE IF NOT EXISTS work_order_intake_orders (
 project_id TEXT NOT NULL, root_id TEXT NOT NULL, work_order_id TEXT NOT NULL,
 head_revision INTEGER NOT NULL CHECK(head_revision BETWEEN 1 AND 64),
 PRIMARY KEY(project_id,root_id,work_order_id),
 FOREIGN KEY(project_id,root_id,work_order_id,head_revision)
 REFERENCES work_order_intake_revisions(project_id,root_id,work_order_id,revision)
 DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE IF NOT EXISTS work_order_intake_sources (
 source_key TEXT PRIMARY KEY CHECK(length(source_key)=71),
 origin_key TEXT UNIQUE CHECK(origin_key IS NULL OR length(origin_key)=71),
 project_id TEXT NOT NULL,root_id TEXT NOT NULL,work_order_id TEXT NOT NULL,
 request_index INTEGER NOT NULL CHECK(request_index BETWEEN 0 AND 255),
 raw_digest TEXT NOT NULL CHECK(length(raw_digest)=71),canonical_digest TEXT NOT NULL CHECK(length(canonical_digest)=71),
 FOREIGN KEY(project_id,root_id,work_order_id)
 REFERENCES work_order_intake_orders(project_id,root_id,work_order_id) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE IF NOT EXISTS work_order_intake_revisions (
 project_id TEXT NOT NULL, root_id TEXT NOT NULL, work_order_id TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 64),
 command_id TEXT NOT NULL UNIQUE CHECK(length(CAST(command_id AS BLOB)) BETWEEN 1 AND 128),
 principal_id TEXT NOT NULL CHECK(length(CAST(principal_id AS BLOB)) BETWEEN 1 AND 128),
 authority_ref TEXT NOT NULL CHECK(length(CAST(authority_ref AS BLOB)) BETWEEN 1 AND 1024),
 authority_generation TEXT NOT NULL CHECK(length(authority_generation) BETWEEN 1 AND 20),
 request_digest TEXT NOT NULL CHECK(length(request_digest)=71),
 preview_digest TEXT NOT NULL CHECK(length(preview_digest)=71),
 raw_digest TEXT NOT NULL CHECK(length(raw_digest)=71),
 canonical_digest TEXT NOT NULL CHECK(length(canonical_digest)=71),
 source_key TEXT NOT NULL CHECK(length(source_key)=71),
 request_index INTEGER NOT NULL CHECK(request_index BETWEEN 0 AND 255),
 plan_id TEXT NOT NULL, plan_revision INTEGER NOT NULL CHECK(plan_revision BETWEEN 1 AND 64),
 request_json TEXT NOT NULL CHECK(json_valid(request_json) AND length(CAST(request_json AS BLOB))<=1048576),
 policy_json TEXT NOT NULL CHECK(json_valid(policy_json) AND length(CAST(policy_json AS BLOB))<=8192),
 receipt_json TEXT NOT NULL CHECK(json_valid(receipt_json) AND length(CAST(receipt_json AS BLOB))<=1048576),
 PRIMARY KEY(project_id,root_id,work_order_id,revision),
 FOREIGN KEY(project_id,root_id,work_order_id)
 REFERENCES work_order_intake_orders(project_id,root_id,work_order_id) DEFERRABLE INITIALLY DEFERRED,
 FOREIGN KEY(source_key) REFERENCES work_order_intake_sources(source_key),
 FOREIGN KEY(project_id,plan_id,plan_revision)
 REFERENCES workplan_registry_revisions(project_id,plan_id,revision)
) STRICT;
CREATE INDEX IF NOT EXISTS work_order_intake_content_lookup
 ON work_order_intake_revisions(project_id,root_id,raw_digest,canonical_digest);
CREATE TABLE IF NOT EXISTS work_order_intake_bindings (
 project_id TEXT NOT NULL, root_id TEXT NOT NULL, work_order_id TEXT NOT NULL,
 revision INTEGER NOT NULL, outcome_id TEXT NOT NULL, task_id TEXT NOT NULL,
 contract_digest TEXT NOT NULL CHECK(length(contract_digest)=71),
 binding_json TEXT NOT NULL CHECK(json_valid(binding_json) AND length(CAST(binding_json AS BLOB))<=32768),
 outcome_json TEXT NOT NULL CHECK(json_valid(outcome_json) AND length(CAST(outcome_json AS BLOB))<=262144),
 delivery_json TEXT NOT NULL CHECK(json_valid(delivery_json) AND length(CAST(delivery_json AS BLOB))<=262144),
 PRIMARY KEY(project_id,root_id,work_order_id,revision,outcome_id),
 UNIQUE(project_id,root_id,work_order_id,revision,task_id),
 FOREIGN KEY(project_id,root_id,work_order_id,revision)
 REFERENCES work_order_intake_revisions(project_id,root_id,work_order_id,revision),
 FOREIGN KEY(task_id,contract_digest) REFERENCES workplan_registry_tasks(task_id,contract_digest)
) STRICT;
CREATE INDEX IF NOT EXISTS work_order_intake_task_lookup ON work_order_intake_bindings(task_id,project_id,root_id);
CREATE TRIGGER IF NOT EXISTS work_order_intake_revision_update BEFORE UPDATE ON work_order_intake_revisions
 BEGIN SELECT RAISE(ABORT,'work order revision is immutable'); END;
CREATE TRIGGER IF NOT EXISTS work_order_intake_revision_delete BEFORE DELETE ON work_order_intake_revisions
 BEGIN SELECT RAISE(ABORT,'work order revision is immutable'); END;
CREATE TRIGGER IF NOT EXISTS work_order_intake_binding_update BEFORE UPDATE ON work_order_intake_bindings
 BEGIN SELECT RAISE(ABORT,'work order binding is immutable'); END;
CREATE TRIGGER IF NOT EXISTS work_order_intake_binding_delete BEFORE DELETE ON work_order_intake_bindings
 BEGIN SELECT RAISE(ABORT,'work order binding is immutable'); END;
CREATE TRIGGER IF NOT EXISTS work_order_intake_order_identity BEFORE UPDATE OF project_id,root_id,work_order_id ON work_order_intake_orders
 BEGIN SELECT RAISE(ABORT,'work order identity is immutable'); END;
CREATE TRIGGER IF NOT EXISTS work_order_intake_order_delete BEFORE DELETE ON work_order_intake_orders
 BEGIN SELECT RAISE(ABORT,'work order identity is immutable'); END;
CREATE TRIGGER IF NOT EXISTS work_order_intake_source_update BEFORE UPDATE ON work_order_intake_sources
 BEGIN SELECT RAISE(ABORT,'work order source identity is immutable'); END;
CREATE TRIGGER IF NOT EXISTS work_order_intake_source_delete BEFORE DELETE ON work_order_intake_sources
 BEGIN SELECT RAISE(ABORT,'work order source identity is immutable'); END;
CREATE INDEX IF NOT EXISTS work_order_intake_scope_lookup ON work_order_intake_revisions(project_id,root_id,principal_id,authority_ref,authority_generation,work_order_id,revision);
