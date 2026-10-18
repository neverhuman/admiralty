/**
 * Generated from schemas/verification-policy.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 6f507eb3147d8909fa7055785d8c7c2b654e3f441ef4c5c88766f6065c9eef2c
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `VerificationPolicy`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;

export interface VerificationPolicy {
  protected_oracle_digest: string;
  /**
   * @minItems 1
   * @maxItems 64
   */
  required_negative_checks: [RequiredNegativeCheck, ...RequiredNegativeCheck[]];
  schema_version: number;
  /**
   * Expected identity remains an unauthenticated claim until the caller
   * verifies custody and actor identity outside this pure component.
   */
  verifier: string;
}
/**
 * This interface was referenced by `VerificationPolicy`'s JSON-Schema
 * via the `definition` "RequiredNegativeCheck".
 */
export interface RequiredNegativeCheck {
  check: CheckSpec;
  /**
   * A rejecting command can expect a nonzero exit; an assertion test that
   * proves rejection normally expects zero. The receipt cannot choose it.
   */
  expected_exit_code: number;
  fixture_digest: string;
}
/**
 * This interface was referenced by `VerificationPolicy`'s JSON-Schema
 * via the `definition` "CheckSpec".
 */
export interface CheckSpec {
  args: string[];
  cwd: string;
  id: string;
  program: string;
  timeout_secs: number;
}
