/**
 * Generated from schemas/config-patch.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: c18decfc5bcf880cbfa4b9ef94adf087046d8eab58bf9b54e441e027f2f020aa
 * Do not edit. Rust domain validation still applies at runtime.
 */

export interface ConfigPatch {
  /**
   * @maxItems 256
   */
  denied_paths?: string[] | null;
  listen?: string | null;
  max_agents?: number | null;
  max_context_tokens?: number | null;
  /**
   * @maxItems 256
   */
  models?: ModelConfig[] | null;
  /**
   * @maxItems 256
   */
  nodes?: NodeConfig[] | null;
}
/**
 * This interface was referenced by `ConfigPatch`'s JSON-Schema
 * via the `definition` "ModelConfig".
 */
export interface ModelConfig {
  id: string;
  max_concurrency: number;
  model_ref: string;
  node_id?: string | null;
  provider: string;
}
/**
 * This interface was referenced by `ConfigPatch`'s JSON-Schema
 * via the `definition` "NodeConfig".
 */
export interface NodeConfig {
  address?: string | null;
  id: string;
  label: string;
}
