/**
 * Generated from schemas/task-interface-contract-v2.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: f06d9b3c41eacbdd28efaa14d5654371b05bd8f2ba7c126d306381534900810b
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `TaskInterfaceContractV2`'s JSON-Schema
 * via the `definition` "CoordinationSha256Digest".
 */
export type CoordinationSha256Digest = string;
/**
 * This interface was referenced by `TaskInterfaceContractV2`'s JSON-Schema
 * via the `definition` "ConsumerCompatibilityPolicy".
 */
export type ConsumerCompatibilityPolicy =
  | {
      policy: 'exact_producer_revision';
      required_producer_revision: number;
    }
  | {
      max_producer_revision: number;
      min_producer_revision: number;
      policy: 'backward_compatible_range';
      required_final_producer_revision: number;
    }
  | {
      min_provisional_producer_revision: number;
      policy: 'provisional_development_allowed';
      required_final_producer_revision: number;
    };
/**
 * This interface was referenced by `TaskInterfaceContractV2`'s JSON-Schema
 * via the `definition` "InterfaceId".
 */
export type InterfaceId = string;
/**
 * This interface was referenced by `TaskInterfaceContractV2`'s JSON-Schema
 * via the `definition` "DecimalU64".
 */
export type DecimalU64 = string;
/**
 * This interface was referenced by `TaskInterfaceContractV2`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `TaskInterfaceContractV2`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * This interface was referenced by `TaskInterfaceContractV2`'s JSON-Schema
 * via the `definition` "InterfaceBindingStatus".
 */
export type InterfaceBindingStatus =
  | {
      state: 'provisional';
    }
  | {
      state: 'verified';
    }
  | {
      reason: string;
      state: 'revoked';
    }
  | {
      state: 'superseded';
      superseded_by_interface_revision: DecimalU64;
    };

export interface TaskInterfaceContractV2 {
  acceptance_check_ids: string[];
  artifact_digest: CoordinationSha256Digest;
  authority_ref: string;
  compatibility_policy: ConsumerCompatibilityPolicy;
  consumer_tasks: TaskRevisionRef[];
  interface_id: InterfaceId;
  interface_revision: DecimalU64;
  producer_task: TaskRevisionRef;
  project_id: ProjectId;
  root_id: RootId;
  schema_digest: CoordinationSha256Digest;
  schema_version: 1;
  status: InterfaceBindingStatus;
}
/**
 * This interface was referenced by `TaskInterfaceContractV2`'s JSON-Schema
 * via the `definition` "TaskRevisionRef".
 */
export interface TaskRevisionRef {
  contract_digest: CoordinationSha256Digest;
  revision: number;
  task_id: string;
}
