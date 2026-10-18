/**
 * Generated from schemas/interface-contract.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: acefdf1e45ea2829052025c273c727b4d90deca83f0c3463415d390a81e2c177
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `InterfaceContract`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;
/**
 * This interface was referenced by `InterfaceContract`'s JSON-Schema
 * via the `definition` "InterfaceId".
 */
export type InterfaceId = string;
/**
 * This interface was referenced by `InterfaceContract`'s JSON-Schema
 * via the `definition` "DecimalU64".
 */
export type DecimalU64 = string;
/**
 * This interface was referenced by `InterfaceContract`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `InterfaceContract`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * This interface was referenced by `InterfaceContract`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;

export interface InterfaceContract {
  artifact_digest: string;
  /**
   * @maxItems 1024
   */
  consumers: PrincipalId[];
  interface_id: InterfaceId;
  producer: PrincipalId;
  revision: DecimalU64;
  schema_digest: string;
  scope: InterfaceScope;
}
/**
 * This interface was referenced by `InterfaceContract`'s JSON-Schema
 * via the `definition` "InterfaceScope".
 */
export interface InterfaceScope {
  authority_ref: string;
  project_id: ProjectId;
  root_id: RootId;
  task_id: TaskId;
}
