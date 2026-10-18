/**
 * Generated from schemas/config-layer.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 0d91b07bf58f1a5c47021ae89ef49bd039c0786a0e9609f37b465c5191da349c
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `ConfigLayer`'s JSON-Schema
 * via the `definition` "ConfigLayerName".
 */
export type ConfigLayerName = 'defaults' | 'user' | 'project' | 'runtime';

export interface ConfigLayer {
  layer: ConfigLayerName;
  source_ref: string;
  values: ConfigPatch;
}
/**
 * This interface was referenced by `ConfigLayer`'s JSON-Schema
 * via the `definition` "ConfigPatch".
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
 * This interface was referenced by `ConfigLayer`'s JSON-Schema
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
 * This interface was referenced by `ConfigLayer`'s JSON-Schema
 * via the `definition` "NodeConfig".
 */
export interface NodeConfig {
  address?: string | null;
  id: string;
  label: string;
}
