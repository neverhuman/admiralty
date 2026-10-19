CREATE TABLE IF NOT EXISTS usage_observation_origins (
 scope_key TEXT NOT NULL CHECK(length(CAST(scope_key AS BLOB))=71),
 observation_id TEXT NOT NULL CHECK(length(CAST(observation_id AS BLOB)) BETWEEN 1 AND 128),
 scope_json TEXT NOT NULL CHECK(length(CAST(scope_json AS BLOB))<=4096),
 contract_json TEXT NOT NULL CHECK(length(CAST(contract_json AS BLOB))<=65536),
 identity_digest TEXT NOT NULL CHECK(length(CAST(identity_digest AS BLOB))=71),
 PRIMARY KEY(scope_key,observation_id)
) STRICT;
CREATE TABLE IF NOT EXISTS usage_observation_revisions (
 scope_key TEXT NOT NULL CHECK(length(CAST(scope_key AS BLOB))=71),
 observation_id TEXT NOT NULL CHECK(length(CAST(observation_id AS BLOB)) BETWEEN 1 AND 128),
 revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 32),
 scope_sequence INTEGER NOT NULL CHECK(scope_sequence BETWEEN 1 AND 8192),
 command_id TEXT NOT NULL CHECK(length(CAST(command_id AS BLOB)) BETWEEN 1 AND 128),
 producer_id TEXT NOT NULL CHECK(length(CAST(producer_id AS BLOB)) BETWEEN 1 AND 128),
 incarnation TEXT NOT NULL CHECK(length(CAST(incarnation AS BLOB)) BETWEEN 1 AND 128),
 source_sequence TEXT NOT NULL CHECK(length(CAST(source_sequence AS BLOB)) BETWEEN 1 AND 20),
 command_json TEXT NOT NULL CHECK(length(CAST(command_json AS BLOB))<=65536),
 command_digest TEXT NOT NULL CHECK(length(CAST(command_digest AS BLOB))=71),
 receipt_json TEXT NOT NULL CHECK(length(CAST(receipt_json AS BLOB))<=8192),
 receipt_digest TEXT NOT NULL CHECK(length(CAST(receipt_digest AS BLOB))=71),
 PRIMARY KEY(scope_key,observation_id,revision),
 UNIQUE(scope_key,scope_sequence), UNIQUE(scope_key,command_id), UNIQUE(producer_id,incarnation,source_sequence),
 FOREIGN KEY(scope_key,observation_id) REFERENCES usage_observation_origins(scope_key,observation_id)
) STRICT;
CREATE TRIGGER IF NOT EXISTS usage_observation_origins_no_update BEFORE UPDATE ON usage_observation_origins BEGIN SELECT RAISE(ABORT,'immutable usage history'); END;
CREATE TRIGGER IF NOT EXISTS usage_observation_origins_no_delete BEFORE DELETE ON usage_observation_origins BEGIN SELECT RAISE(ABORT,'immutable usage history'); END;
CREATE TRIGGER IF NOT EXISTS usage_observation_revisions_no_update BEFORE UPDATE ON usage_observation_revisions BEGIN SELECT RAISE(ABORT,'immutable usage history'); END;
CREATE TRIGGER IF NOT EXISTS usage_observation_revisions_no_delete BEFORE DELETE ON usage_observation_revisions BEGIN SELECT RAISE(ABORT,'immutable usage history'); END;
