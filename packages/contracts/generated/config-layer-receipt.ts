/**
 * Generated from schemas/config-layer-receipt.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: c472ab2eba4fb0b04436d0babf96bfb0e1fc7eb89ae3cb492f5b3901a56ee225
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `ConfigLayerReceipt`'s JSON-Schema
 * via the `definition` "ConfigLayerName".
 */
export type ConfigLayerName = 'defaults' | 'user' | 'project' | 'runtime';

export interface ConfigLayerReceipt {
  /**
   * Binds the complete typed public layer, including values later overridden.
   */
  digest: string;
  layer: ConfigLayerName;
  source_ref: string;
  [k: string]: unknown;
}
