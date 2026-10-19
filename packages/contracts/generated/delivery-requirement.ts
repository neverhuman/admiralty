/**
 * Generated from schemas/delivery-requirement.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 455b23d3281d24ff73efc53d01aada5f68606f0544eca9f6d3e1b846a386640c
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `DeliveryRequirement`'s JSON-Schema
 * via the `definition` "TargetRequirement".
 */
export type TargetRequirement =
  | {
      kind: 'unknown';
    }
  | {
      kind: 'required';
      stage: DeliveryStage;
    }
  | {
      kind: 'optional';
      stage: DeliveryStage;
    }
  | {
      kind: 'not_applicable';
      reason: string;
    };
/**
 * This interface was referenced by `DeliveryRequirement`'s JSON-Schema
 * via the `definition` "DeliveryStage".
 */
export type DeliveryStage =
  | 'candidate'
  | 'verified'
  | 'merged'
  | 'packaged'
  | 'staged'
  | 'installed'
  | 'running'
  | 'released';
/**
 * This interface was referenced by `DeliveryRequirement`'s JSON-Schema
 * via the `definition` "DeliveryTargetKind".
 */
export type DeliveryTargetKind = 'repository' | 'deployment';
/**
 * This interface was referenced by `DeliveryRequirement`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;

/**
 * Outside TaskContractv1. A generation is a new immutable requirement, never
 * a rewrite of prior evidence. Unknown is never optional/not-applicable.
 */
export interface DeliveryRequirement {
  contract_digest: string;
  contract_revision: number;
  generation: number;
  schema_version: number;
  targets: DeliveryTarget[];
  task_id: TaskId;
}
/**
 * This interface was referenced by `DeliveryRequirement`'s JSON-Schema
 * via the `definition` "DeliveryTarget".
 */
export interface DeliveryTarget {
  id: string;
  requirement: TargetRequirement;
  target_kind: DeliveryTargetKind;
}
