/**
 * Generated from schemas/duplicate-page.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: ee2c91378916e7b8a3b75c9432db3b5ea0aa623ff6b755d9c95dce09398cef2b
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `DuplicatePage`'s JSON-Schema
 * via the `definition` "DuplicateReason".
 */
export type DuplicateReason =
  | 'exact_command_identity'
  | 'exact_source_identity'
  | 'exact_command_collision'
  | 'exact_source_collision'
  | 'advisory_raw_content'
  | 'advisory_canonical_content';
/**
 * This interface was referenced by `DuplicatePage`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `DuplicatePage`'s JSON-Schema
 * via the `definition` "DuplicateCoverage".
 */
export type DuplicateCoverage =
  | {
      kind: 'complete';
    }
  | {
      continuation: string;
      kind: 'partial';
    }
  | {
      kind: 'incomplete';
      reason: string;
    };

export interface DuplicatePage {
  candidates: DuplicateCandidate[];
  coverage: DuplicateCoverage;
  query_digest: string;
  schema_version: number;
}
/**
 * This interface was referenced by `DuplicatePage`'s JSON-Schema
 * via the `definition` "DuplicateCandidate".
 */
export interface DuplicateCandidate {
  contract_digest: string;
  contract_revision: number;
  evidence_refs: string[];
  freshness: string;
  lifecycle: string;
  reason: DuplicateReason;
  task_id: TaskId;
}
