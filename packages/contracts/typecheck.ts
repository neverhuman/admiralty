// Compile-only consumer fixture: public roots, subpaths and runtime narrowing.
import {
  validateWorkplan, validateVerificationReceipt, validateVerificationPolicy,
  validateInterfaceContract, validateConfigLayer, validateConfigPatch,
  validateConfigLayerReceipt, validateProvenanceContribution,
} from '@admiralty/contracts';
import type {
  Workplan, VerificationReceipt, VerificationPolicy, InterfaceContract,
  ConfigLayer, ConfigPatch, ConfigLayerReceipt, ProvenanceContribution,
  DecimalU64, CheckResult, VerificationInputs, RequiredNegativeCheck,
  InterfaceScope, NodeConfig, ModelConfig,
} from '@admiralty/contracts';
import type { Workplan as WorkplanSubpath } from '@admiralty/contracts/workplan';
import type { VerificationReceipt as ReceiptSubpath } from '@admiralty/contracts/verification-receipt';
import type { VerificationPolicy as PolicySubpath } from '@admiralty/contracts/verification-policy';
import type { InterfaceContract as InterfaceSubpath } from '@admiralty/contracts/interface-contract';
import type { ConfigLayer as LayerSubpath } from '@admiralty/contracts/config-layer';
import type { ConfigPatch as PatchSubpath } from '@admiralty/contracts/config-patch';
import type { ConfigLayerReceipt as LayerReceiptSubpath } from '@admiralty/contracts/config-layer-receipt';
import type { ProvenanceContribution as ProvenanceSubpath } from '@admiralty/contracts/provenance-contribution';

declare const input: unknown;
if (validateWorkplan(input)) { const plan: WorkplanSubpath & Workplan = input; void plan.tasks; }
if (validateVerificationReceipt(input)) { const receipt: ReceiptSubpath & VerificationReceipt = input; void receipt.inputs; }
if (validateVerificationPolicy(input)) { const policy: PolicySubpath & VerificationPolicy = input; void policy.required_negative_checks; }
if (validateInterfaceContract(input)) { const contract: InterfaceSubpath & InterfaceContract = input; const revision: string = contract.revision; void revision; }
if (validateConfigLayer(input)) { const layer: LayerSubpath & ConfigLayer = input; void layer.values; }
if (validateConfigPatch(input)) { const patch: PatchSubpath & ConfigPatch = input; void patch.models; }
if (validateConfigLayerReceipt(input)) { const receipt: LayerReceiptSubpath & ConfigLayerReceipt = input; void receipt.digest; }
if (validateProvenanceContribution(input)) { const provenance: ProvenanceSubpath & ProvenanceContribution = input; void provenance.origin; }

const revision: DecimalU64 = '18446744073709551615';
// @ts-expect-error Large counters must retain their precision-safe wire encoding.
const roundedRevision: DecimalU64 = 9007199254740992;
const omitted: ConfigPatch = {};
const nulls: ConfigPatch = { listen: null, max_agents: null, models: null };
const extensibleReceipt: ConfigLayerReceipt = { layer: 'project', source_ref: 'file', digest: 'sha256:claim', extra_metadata: 'preserved' };
const extensibleProvenance: ProvenanceContribution = { origin: 'private', source_ref: null, extra_metadata: 'preserved' };
// @ts-expect-error Private policy is not a public configuration patch.
const override: ConfigPatch = { private_policy: { max_agents: 65535 } };
// @ts-expect-error An opaque validated receipt is not an exported input type.
import type { ValidatedReceipt } from '@admiralty/contracts';
void [revision, roundedRevision, omitted, nulls, extensibleReceipt, extensibleProvenance, override];
export type NestedPublicTypes = [CheckResult, VerificationInputs, RequiredNegativeCheck, InterfaceScope, NodeConfig, ModelConfig];
