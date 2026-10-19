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


import {
  validateIntakeBatch, validateSourceProvenance, validateIntakePreview, validateWorkOrderRevision,
  validateRecordIntakeRequest, validateRecordIntakeReceipt, validateDeliveryRequirement,
  validateDuplicateQuery, validateDuplicatePage,
} from '@admiralty/contracts';
import type {
  IntakeBatch, SourceProvenance, IntakePreview, WorkOrderRevision, RecordIntakeRequest,
  RecordIntakeReceipt, DeliveryRequirement, DuplicateQuery, DuplicatePage,
  SourcePayload, ScopeSnapshot, RequesterAttribution, WorkOrderTaskBinding, SelectedTask,
} from '@admiralty/contracts';
import type { IntakeBatch as BatchSubpath } from '@admiralty/contracts/intake-batch';
import type { SourceProvenance as SourceSubpath } from '@admiralty/contracts/source-provenance';
import type { IntakePreview as PreviewSubpath } from '@admiralty/contracts/intake-preview';
import type { WorkOrderRevision as OrderSubpath } from '@admiralty/contracts/work-order-revision';
import type { RecordIntakeRequest as RecordSubpath } from '@admiralty/contracts/record-intake-request';
import type { RecordIntakeReceipt as IntakeReceiptSubpath } from '@admiralty/contracts/record-intake-receipt';
import type { DeliveryRequirement as DeliverySubpath } from '@admiralty/contracts/delivery-requirement';
import type { DuplicateQuery as QuerySubpath } from '@admiralty/contracts/duplicate-query';
import type { DuplicatePage as PageSubpath } from '@admiralty/contracts/duplicate-page';
if (validateIntakeBatch(input)) { const batch: IntakeBatch & BatchSubpath = input; void batch.items; }
if (validateSourceProvenance(input)) { const source: SourceProvenance & SourceSubpath = input; void source.requester; }
if (validateIntakePreview(input)) { const preview: IntakePreview & PreviewSubpath = input; const generation: string = preview.authorized_scope.authority_generation; void generation; }
if (validateWorkOrderRevision(input)) { const order: WorkOrderRevision & OrderSubpath = input; const generation: string = order.scope.authority_generation; void [generation, order.format]; }
if (validateRecordIntakeRequest(input)) { const request: RecordIntakeRequest & RecordSubpath = input; const generation: string = request.authority_generation; void [generation, request.selected_tasks]; }
if (validateRecordIntakeReceipt(input)) { const receipt: RecordIntakeReceipt & IntakeReceiptSubpath = input; void receipt.duplicate; }
if (validateDeliveryRequirement(input)) { const requirement: DeliveryRequirement & DeliverySubpath = input; void requirement.targets; }
if (validateDuplicateQuery(input)) { const query: DuplicateQuery & QuerySubpath = input; void query.cursor; }
if (validateDuplicatePage(input)) { const page: DuplicatePage & PageSubpath = input; void page.coverage; }
const scopeSnapshot: ScopeSnapshot = { principal: 'claimed', project_id: null, root_id: null,
  authority_ref: 'scope:claim', authority_generation: '18446744073709551615' };
// @ts-expect-error Public authority generation cannot be a rounded JavaScript number.
const numericGeneration: ScopeSnapshot['authority_generation'] = 9007199254740992;
// @ts-expect-error A public snapshot is data, not the independently supplied trusted scope.
import type { AuthorizedIntakeScope } from '@admiralty/contracts';
// @ts-expect-error Current trusted policy is not a caller-deserializable contract.
import type { IntakePolicy } from '@admiralty/contracts';
void [scopeSnapshot, numericGeneration];
export type NestedIntakeTypes = [SourcePayload, ScopeSnapshot, RequesterAttribution, WorkOrderTaskBinding, SelectedTask];

import { validateCoordinationEnvelopeV2, validateViewsSnapshot } from '@admiralty/contracts';
import type {
  CoordinationEnvelopeV2, ViewsSnapshot, ViewAgent, ViewProject, ViewPresence,
  ViewRole, GitRepository, GitCheckout, GitWorkingState, GitCommit, GitRef, ViewSource,
} from '@admiralty/contracts';
import type { CoordinationEnvelopeV2 as EnvelopeSubpath } from '@admiralty/contracts/coordination-envelope-v2';
import type {
  ViewsSnapshot as ViewsSnapshotSubpath,
  ViewAgent as ViewAgentSubpath,
  ViewProject as ViewProjectSubpath,
  ViewPresence as ViewPresenceSubpath,
  ViewRole as ViewRoleSubpath,
} from '@admiralty/contracts/views-snapshot';
if (validateCoordinationEnvelopeV2(input)) {
  const envelope: CoordinationEnvelopeV2 & EnvelopeSubpath = input;
  const version: 2 = envelope.protocol_version;
  const eventSequence: string = envelope.source_cursor.event_seq;
  void [version, eventSequence, envelope.authority.policy_digest, envelope.payload_ref.sha256];
}
if (validateViewsSnapshot(input)) {
  const snapshot: ViewsSnapshot & ViewsSnapshotSubpath = input;
  const agents: ViewAgent[] & ViewAgentSubpath[] = snapshot.agents;
  const projects: ViewProject[] & ViewProjectSubpath[] = snapshot.projects;
  const presence: (ViewPresence & ViewPresenceSubpath) | undefined = agents[0]?.presence;
  const role: (ViewRole & ViewRoleSubpath) | undefined = agents[0]?.role;
  const simulated: boolean = snapshot.simulated;
  void [agents, projects, presence, role, simulated, snapshot.revision, snapshot.sources];
}
export type NestedViewsSnapshotTypes = [
  ViewAgent, ViewProject, ViewPresence, ViewRole,
  GitRepository, GitCheckout, GitWorkingState, GitCommit, GitRef, ViewSource,
];
// @ts-expect-error Comparison bindings are trusted in-process inputs, not public wire grants.
import type { ExpectedBindings } from '@admiralty/contracts';

