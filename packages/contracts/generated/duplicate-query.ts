/**
 * Generated from schemas/duplicate-query.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 05f2d879c43053199ce430a5e737a9b3267c782c0bbdbf9788e6fd51026fc6b7
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `DuplicateQuery`'s JSON-Schema
 * via the `definition` "CommandId".
 */
export type CommandId = string;
/**
 * This interface was referenced by `DuplicateQuery`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;

export interface DuplicateQuery {
  canonical_digest: string;
  command_id?: CommandId | null;
  cursor?: string | null;
  limit: number;
  origin?: OriginReference | null;
  project_id: ProjectId;
  raw_digest: string;
  schema_version: number;
}
/**
 * Opaque lookup data only: no fetching, path traversal or authorization.
 *
 * This interface was referenced by `DuplicateQuery`'s JSON-Schema
 * via the `definition` "OriginReference".
 */
export interface OriginReference {
  record_id: string;
  revision: string;
  system: string;
}
