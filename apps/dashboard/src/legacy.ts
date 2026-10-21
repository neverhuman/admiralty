import { ApiError, readBoundedJson } from './api';
import { assertBoundedJson } from './jsonBounds';

export type Data = Record<string, unknown>;
export type LegacySnapshot = Data & { generated_utc: string; projects: Record<string, Data>; order?: string[] };
export type DetailSection = { title: string; value: unknown; open?: boolean };
export type LegacyAlert = { message: string; sourceMessage: string; count: number; taskIds: string[]; kind: 'evidence' | 'warning' | 'error' };
export type ProjectDetailModel = { id: string; name: string; source: string; observed: string; total: number | null;
  stages: Record<string, number>; labels: Record<string, string>; alerts: LegacyAlert[]; recent: unknown[]; captain: Data;
  workers: unknown[]; pr: Data; sections: DetailSection[]; raw: Data };
export const data = (value: unknown): Data => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Data : {};
export const list = (value: unknown): unknown[] => Array.isArray(value) ? value : [];
export const text = (value: unknown): string => typeof value === 'string' ? value : typeof value === 'number' || typeof value === 'boolean' ? String(value) : '';
export const label = (value: string): string => value.replaceAll('_', ' ').replace(/\bsuper captain\b/gi, 'Admiral').replace(/\bcaptains\b/gi, 'Commodores').replace(/\bcaptain\b/gi, 'Commodore');
export const number = (value: unknown): number | null => typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
export function freshness(value: string, now = Date.now()): string {
  const age = (now - Date.parse(value)) / 1000;
  return !Number.isFinite(age) ? 'age unknown' : age < -30 ? 'future source time' : age > 180 ? `stale · ${Math.round(age / 60)}m old` : `${Math.max(0, Math.round(age))}s old`;
}
export function safeHref(value: unknown): string | undefined {
  try { const url = new URL(text(value)); return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password ? url.href : undefined; } catch { return undefined; }
}
export function parseLegacy(value: unknown): LegacySnapshot {
  assertBoundedJson(value);
  const snapshot = data(value);
  if (typeof snapshot.generated_utc !== 'string' || !snapshot.projects || Array.isArray(snapshot.projects)
    || typeof snapshot.projects !== 'object' || (snapshot.order !== undefined && (!Array.isArray(snapshot.order) || !snapshot.order.every(item => typeof item === 'string')))) {
    throw new Error('Unsupported detailed dashboard source');
  }
  for (const project of Object.values(snapshot.projects)) if (!project || typeof project !== 'object' || Array.isArray(project)) throw new Error('Invalid detailed project source');
  return snapshot as LegacySnapshot;
}
type LegacyView = 'full' | 'detail' | 'initial';
const legacyPaths: Record<LegacyView, string> = { full: '/v1/legacy', detail: '/v1/legacy/detail', initial: '/v1/legacy/detail/initial' };
export async function getLegacy(token?: string, signal?: AbortSignal, fetcher: typeof fetch = fetch, view: LegacyView = 'full'): Promise<LegacySnapshot | null> {
  const response = await fetcher(legacyPaths[view], { headers: token ? { Authorization: `Bearer ${token}` } : undefined, signal, cache: 'no-store' });
  if (response.status === 404) return null;
  if (!response.ok) throw new ApiError(`Detailed snapshot request failed (${response.status})`, response.status);
  return parseLegacy(await readBoundedJson(response));
}
export function watchLegacy(options: { token?: string; onSnapshot: (value: LegacySnapshot | null) => void; onError: (value: string) => void; fetcher?: typeof fetch; view?: 'full' | 'detail'; progressive?: boolean }): () => void {
  let stopped = false, timer: ReturnType<typeof setTimeout> | undefined;
  let phase: LegacyView = options.progressive ? 'initial' : options.view ?? 'full';
  let request: AbortController | undefined;
  async function poll() {
    const controller = new AbortController();
    request = controller;
    let timedOut = false;
    const timeout = setTimeout(() => { timedOut = true; controller.abort(); }, 12_000);
    let repeat = true;
    let nextDelay = 15_000;
    try {
      const snapshot = await getLegacy(options.token, controller.signal, options.fetcher, phase);
      if (snapshot === null && phase !== 'full') {
        // Compatibility with older and synthetic servers. Keep any rendered
        // initial view while falling back to the original full source.
        phase = 'full'; nextDelay = 0;
        return;
      }
      if (!stopped) { options.onSnapshot(snapshot); options.onError(''); }
      if (snapshot === null) repeat = false;
      else if (phase === 'initial') { phase = options.view ?? 'full'; nextDelay = 0; }
    } catch (error) {
      if (!stopped) options.onError(timedOut
        ? 'The live snapshot did not arrive within 12 seconds. Retrying automatically; check the dashboard connection.'
        : error instanceof Error ? error.message : 'Detailed source unavailable');
      if (error instanceof ApiError && [401, 403].includes(error.status)) repeat = false;
    } finally {
      clearTimeout(timeout);
      if (!stopped && repeat) timer = setTimeout(() => void poll(), nextDelay);
    }
  }
  void poll();
  return () => { stopped = true; request?.abort(); clearTimeout(timer); };
}

const sections: Array<[string, string]> = [
  ['validator', 'Validator audit'], ['lease', 'Lease'], ['by_kind', 'Ledger progress'], ['leases', 'Task leases'],
  ['live_status', 'Live status'], ['waves', 'Evolution waves'], ['declared_counter', 'Declared source counter'],
  ['active_claims', 'Active claims'], ['in_progress_breakdown', 'In-progress accounting'],
  ['assigned_from_captain_state', 'Commodore assignments'], ['queued_assignments_no_live_screen', 'Queued assignments without live screen'],
  ['coord_log', 'Coordination log'], ['stage_by_task', 'Per-task stages'], ['stage_meta', 'Stage provenance'],
  ['raw_status_counts', 'Raw ledger status counts'], ['current_focus', 'Current focus'],
];
function counts(value: unknown): Record<string, number> {
  return Object.fromEntries(Object.entries(data(value)).filter((entry): entry is [string, number] => number(entry[1]) !== null));
}
function warnings(project: Data): LegacyAlert[] {
  return [...new Set([...list(project.flags), ...list(project.blockers), ...list(project.needs_jepson),
    ...Object.entries(data(project.blocked_detail)).map(([id, value]) => `${id}: ${text(value)}`)].map(text).filter(Boolean))].map(message => ({ message, sourceMessage: message, count: 1, taskIds: [], kind: 'error' }));
}
/** Recorded component checks are evidence, never accepted completion. */
export function stageLabel(stage: string, labels: Record<string, string> = {}): string {
  return stage === 'verified' ? 'checks recorded' : stage === 'integrated' ? 'integration recorded' : labels[stage] || label(stage);
}
export function stageClass(stage: string): string {
  return stage === 'verified' ? 'stage-evidence-recorded' : `stage-${stage}`;
}
function rolloutAlerts(rollout: Data): LegacyAlert[] {
  const groups = new Map<string, LegacyAlert>();
  const add = (value: unknown, taskId = '', kind: LegacyAlert['kind'] = 'error') => {
    const sourceMessage = text(value).trim(); if (!sourceMessage) return;
    const provenance = /^Integration provenance missing; displayed as verification recorded\.?$/i.test(sourceMessage);
    if (provenance) kind = 'evidence';
    const key = `${kind}:${sourceMessage}`, existing = groups.get(key);
    if (existing) { existing.count++; if (taskId && !existing.taskIds.includes(taskId)) existing.taskIds.push(taskId); }
    else groups.set(key, { sourceMessage, message: provenance ? 'Integration evidence is missing. Recorded checks do not establish integration or accepted completion.' : sourceMessage,
      count: 1, taskIds: taskId ? [taskId] : [], kind });
  };
  add(rollout.error);
  if (data(rollout.freshness).state === 'stale') add('Rollout evidence is stale; showing the producer’s last recorded state.', '', 'warning');
  for (const packet of list(rollout.packets).map(data)) for (const warning of list(packet.warnings)) add(warning, text(packet.id));
  return [...groups.values()];
}
function observedManagerId(observed: Data, identity: Data): string {
  if (observed.identity_state !== 'observed') return '';
  const id = text(observed.id), match = /^([1-9]\d*)\.(.+)$/.exec(id);
  if (!match || text(observed.name) !== match[2] || number(observed.pid) !== Number(match[1])) return '';
  const configured = text(identity.captain_screen);
  if (configured && configured !== id && configured !== match[2]) return '';
  for (const pin of [identity.captain_screen_pid, identity.reported_captain_screen_pid]) {
    if (pin !== undefined && pin !== null && pin !== Number(match[1])) return '';
  }
  return id;
}
export function projectDetails(snapshot: LegacySnapshot): ProjectDetailModel[] {
  const order = [...new Set([...(snapshot.order ?? []), ...Object.keys(snapshot.projects)])];
  const projects: ProjectDetailModel[] = order.filter(id => Object.hasOwn(snapshot.projects, id)).map(id => {
    const project = snapshot.projects[id];
    return { id, name: text(project.name) || id, source: text(project.ledger_source) || 'Legacy collector', observed: text(project.ledger_updated_utc) || text(project.observed_utc) || snapshot.generated_utc,
      total: number(project.total), stages: counts(project.stages ?? project.counts), labels: Object.fromEntries(Object.entries(data(project.stage_labels)).map(([key, value]) => [key, text(value)])),
      alerts: warnings(project), recent: list(project.recent), captain: data(project.captain), workers: list(project.workers), pr: data(project.pr),
      sections: sections.filter(([key]) => project[key] !== undefined).map(([key, title]) => ({ title, value: project[key], open: ['coord_log', 'validator', 'live_status', 'waves', 'by_kind'].includes(key) })), raw: project };
  });
  const rollout = data(snapshot.rollout), identity = data(rollout.project);
  if (text(identity.id)) {
    const id = text(identity.id), existing = projects.find(project => project.id === id);
    const allAgents = list(snapshot.agents).map(data);
    const agents = allAgents.filter(agent => agent.project === id);
    const observedCaptain = data(data(rollout.live_observation).captain);
    const configuredCaptain = text(identity.captain_screen);
    // A configured manager is selected by its observed full identity. A screen
    // naming convention must not silently promote an implementation worker.
    const observedCaptainId = observedManagerId(observedCaptain, identity);
    const captainAgent = observedCaptainId ? allAgents.find(agent => agent.id === observedCaptainId)
      : configuredCaptain ? undefined : agents.find(agent => agent.screen === `captain_${id}`);
    const captain: Data = observedCaptainId ? { ...observedCaptain, ...captainAgent,
      screen: observedCaptain.name,
      model: text(captainAgent?.model) && !/unknown/i.test(text(captainAgent?.model)) ? captainAgent?.model : observedCaptain.model,
    } : configuredCaptain ? { name: configuredCaptain, screen: configuredCaptain, state: 'unknown', model: 'unknown' } : captainAgent ?? {};
    // Rollout lanes own the project-to-screen mapping. The legacy process
    // classifier can still call a newly named screen "unassigned".
    const lanes = list(rollout.lanes).map(data);
    const workers: Data[] = lanes.map(lane => {
      const agent = allAgents.find(value => text(lane.screen) && value.screen === lane.screen) ?? {};
      return { ...agent, id: lane.id, screen: lane.screen ?? agent.screen ?? lane.id,
        node: lane.node ?? agent.host, model: agent.model ?? lane.model, effort: agent.effort ?? lane.effort,
        tier: agent.tier ?? lane.tier, task: lane.packet_id ?? agent.task,
        state: text(agent.state) && agent.state !== 'unknown' ? agent.state : lane.status,
        lane_report_status: lane.status, screen_changed_utc: agent.last_token_utc ?? lane.source_updated_utc,
      };
    });
    for (const agent of agents.filter(agent => agent !== captainAgent)) {
      if (!workers.some(worker => worker.screen === agent.screen)) workers.push(agent);
    }
    const extra: DetailSection[] = [{ title: 'Work packets', value: rollout.packets, open: true }, { title: 'Execution lanes', value: rollout.lanes, open: false },
      { title: 'Manifest provenance', value: { updated_utc: rollout.updated_utc, source: rollout.source, producer: rollout.producer, freshness: rollout.freshness, assurance: rollout.assurance }, open: false }];
    if (existing) { existing.sections.push(...extra); existing.alerts.push(...rolloutAlerts(rollout)); }
    else projects.push({ id, name: text(identity.name) || id, source: text(rollout.producer) || 'Rollout manifest', observed: text(rollout.updated_utc) || snapshot.generated_utc,
      total: number(rollout.total) ?? list(rollout.packets).length, stages: counts(rollout.counts), labels: { verified: 'checks recorded', integrated: 'integration recorded' },
      alerts: rolloutAlerts(rollout), recent: [], captain,
      workers, pr: data(rollout.pr), sections: extra, raw: rollout });
  }
  return projects;
}

export function hierarchyCaptains(snapshot: LegacySnapshot): Data[] {
  const captains = list(data(snapshot.hierarchy).captains).map(data);
  const known = new Set(captains.map(captain => text(captain.key ?? captain.project ?? captain.id)));
  for (const project of projectDetails(snapshot)) {
    if (!known.has(project.id) && Object.keys(project.captain).length) {
      captains.push({ ...project.captain, key: project.id, name: project.name, workers: project.workers });
      known.add(project.id);
    }
  }
  return captains;
}
