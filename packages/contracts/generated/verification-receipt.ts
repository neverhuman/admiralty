/**
 * Generated from schemas/verification-receipt.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 3d8c36677287b1cb78ee14bc33dd45fad4f33045b29d8d3e962dc6575cc1c487
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "AttemptId".
 */
export type AttemptId = string;
/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "ReceiptProducer".
 */
export type ReceiptProducer = 'independent_verifier' | 'provider_claim';
/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;

export interface VerificationReceipt {
  /**
   * @maxItems 256
   */
  checks: CheckResult[];
  inputs: VerificationInputs;
  /**
   * @maxItems 64
   */
  negative_checks: CheckResult[];
  producer: ReceiptProducer;
  receipt_id: string;
  schema_version: number;
  verifier: PrincipalId;
}
/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "CheckResult".
 */
export interface CheckResult {
  check: CheckSpec;
  exit_code?: number | null;
  stderr_digest: string;
  stdout_digest: string;
}
/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "CheckSpec".
 */
export interface CheckSpec {
  args: string[];
  cwd: string;
  id: string;
  program: string;
  timeout_secs: number;
}
/**
 * This interface was referenced by `VerificationReceipt`'s JSON-Schema
 * via the `definition` "VerificationInputs".
 */
export interface VerificationInputs {
  attempt_id: AttemptId;
  candidate_ref: string;
  contract_digest: string;
  contract_revision: number;
  project_id: ProjectId;
  protected_oracle_digest: string;
  root_id: RootId;
  source_artifact_digest: string;
  source_commit_sha: string;
  source_tree_sha: string;
  task_id: TaskId;
  verification_policy_digest: string;
}
