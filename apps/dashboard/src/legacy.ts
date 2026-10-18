import { ApiError, readBoundedJson } from './api';
import { assertBoundedJson } from './validation';

export type Data = Record<string, unknown>;
export type LegacySnapshot = Data & { generated_utc: string; projects: Record<string, Data>; order?: string[] };
export type DetailSection = { title: string; value: unknown; open?: boolean };
export type ProjectDetailModel = { id: string; name: string; source: string; observed: string; total: number | null;
  stages: Record<string, number>; labels: Record<string, string>; alerts: string[]; recent: unknown[]; captain: Data;
  workers: unknown[]; pr: Data; sections: DetailSection[]; raw: Data };
export const data = (value: unknown): Data => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Data : {};
export const list = (value: unknown): unknown[] => Array.isArray(value) ? value : [];
export const text = (value: unknown): string => typeof value === 'string' ? value : typeof value === 'number' || typeof value === 'boolean' ? String(value) : '';
export const label = (value: string): string => value.replaceAll('_', ' ');
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
export async function getLegacy(token?: string, signal?: AbortSignal, fetcher: typeof fetch = fetch): Promise<LegacySnapshot | null> {
  const response = await fetcher('/v1/legacy', { headers: token ? { Authorization: `Bearer ${token}` } : undefined, signal, cache: 'no-store' });
  if (response.status === 404) return null;
  if (!response.ok) throw new ApiError(`Detailed snapshot request failed (${response.status})`, response.status);
  return parseLegacy(await readBoundedJson(response));
}
export function watchLegacy(options: { token?: string; onSnapshot: (value: LegacySnapshot | null) => void; onError: (value: string) => void; fetcher?: typeof fetch }): () => void {
  let stopped = false, timer: ReturnType<typeof setTimeout> | undefined;
  let request: AbortController | undefined;
  async function poll() {
    request = new AbortController();
    const timeout = setTimeout(() => request?.abort(), 12_000);
    let repeat = true;
    try {
      const snapshot = await getLegacy(options.token, request.signal, options.fetcher);
      if (!stopped) { options.onSnapshot(snapshot); options.onError(''); }
      if (snapshot === null) repeat = false;
    } catch (error) {
      if (!stopped) options.onError(error instanceof Error ? error.message : 'Detailed source unavailable');
      if (error instanceof ApiError && [401, 403].includes(error.status)) repeat = false;
    } finally {
      clearTimeout(timeout);
      if (!stopped && repeat) timer = setTimeout(() => void poll(), 15_000);
    }
  }
  void poll();
  return () => { stopped = true; request?.abort(); clearTimeout(timer); };
}

const sections: Array<[string, string]> = [
  ['validator', 'Validator audit'], ['lease', 'Lease'], ['by_kind', 'Ledger progress'], ['leases', 'Task leases'],
  ['live_status', 'Live status'], ['waves', 'Evolution waves'], ['declared_counter', 'Declared source counter'],
  ['active_claims', 'Active claims'], ['in_progress_breakdown', 'In-progress accounting'],
  ['assigned_from_captain_state', 'Captain assignments'], ['queued_assignments_no_live_screen', 'Queued assignments without live screen'],
  ['coord_log', 'Coordination log'], ['stage_by_task', 'Per-task stages'], ['stage_meta', 'Stage provenance'],
  ['raw_status_counts', 'Raw ledger status counts'], ['current_focus', 'Current focus'],
];
function counts(value: unknown): Record<string, number> {
  return Object.fromEntries(Object.entries(data(value)).filter((entry): entry is [string, number] => number(entry[1]) !== null));
}
function warnings(project: Data): string[] {
  return [...new Set([...list(project.flags), ...list(project.blockers), ...list(project.needs_jepson),
    ...Object.entries(data(project.blocked_detail)).map(([id, value]) => `${id}: ${text(value)}`)].map(text).filter(Boolean))];
}
export function projectDetails(snapshot: LegacySnapshot): ProjectDetailModel[] {
  const order = [...new Set([...(snapshot.order ?? []), ...Object.keys(snapshot.projects)])];
  const projects: ProjectDetailModel[] = order.filter(id => Object.hasOwn(snapshot.projects, id)).map(id => {
    const project = snapshot.projects[id];
    return { id, name: text(project.name) || id, source: text(project.ledger_source) || 'Legacy collector', observed: snapshot.generated_utc,
      total: number(project.total), stages: counts(project.stages ?? project.counts), labels: Object.fromEntries(Object.entries(data(project.stage_labels)).map(([key, value]) => [key, text(value)])),
      alerts: warnings(project), recent: list(project.recent), captain: data(project.captain), workers: list(project.workers), pr: data(project.pr),
      sections: sections.filter(([key]) => project[key] !== undefined).map(([key, title]) => ({ title, value: project[key], open: ['coord_log', 'validator', 'live_status', 'waves', 'by_kind'].includes(key) })), raw: project };
  });
  const rollout = data(snapshot.rollout), identity = data(rollout.project);
  if (text(identity.id)) {
    const id = text(identity.id), existing = projects.find(project => project.id === id);
    const allAgents = list(snapshot.agents).map(data);
    const agents = allAgents.filter(agent => agent.project === id);
    const captain = agents.find(agent => agent.screen === `captain_${id}`) ?? {};
    // Rollout lanes own the project-to-screen mapping. The legacy process
    // classifier can still call a newly named screen "unassigned".
    const lanes = list(rollout.lanes).map(data);
    const workers: Data[] = lanes.map(lane => {
      const agent = allAgents.find(value => text(lane.screen) && value.screen === lane.screen) ?? {};
      return { ...agent, id: lane.id, screen: lane.screen ?? agent.screen ?? lane.id,
        node: lane.node ?? agent.host, model: agent.model ?? lane.model, effort: agent.effort ?? lane.effort,
        tier: agent.tier ?? lane.tier, task: lane.packet_id ?? agent.task,
        state: lane.status, screen_changed_utc: agent.last_token_utc ?? lane.source_updated_utc,
      };
    });
    for (const agent of agents.filter(agent => agent !== captain)) {
      if (!workers.some(worker => worker.screen === agent.screen)) workers.push(agent);
    }
    const extra: DetailSection[] = [{ title: 'Work packets', value: rollout.packets, open: true }, { title: 'Execution lanes', value: rollout.lanes, open: true },
      { title: 'Manifest provenance', value: { source: rollout.source, producer: rollout.producer, freshness: rollout.freshness, assurance: rollout.assurance }, open: false }];
    if (existing) { existing.sections.push(...extra); }
    else projects.push({ id, name: text(identity.name) || id, source: text(rollout.producer) || 'Rollout manifest', observed: text(rollout.updated_utc) || snapshot.generated_utc,
      total: number(rollout.total) ?? list(rollout.packets).length, stages: counts(rollout.counts), labels: { verified: 'verification recorded', integrated: 'integration recorded' },
      alerts: [rollout.error, data(rollout.freshness).state === 'stale' ? 'Rollout source is stale; producer age is preserved.' : '',
        ...list(rollout.packets).flatMap(packet => list(data(packet).warnings))].map(text).filter(Boolean), recent: [], captain,
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
