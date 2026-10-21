import { data, list, number, text, type LegacySnapshot } from '../../legacy';

/** A count that may grow when unobserved history arrives. */
export type Count = { value: number; lowerBound: boolean };
export type HealthState = 'healthy' | 'attention' | 'unknown' | 'disabled' | 'not_reported';
export type Level = 'ok' | 'amber' | 'red' | 'unknown';

export type ProjectHealth = {
  id: string;
  name: string;
  state: HealthState;
  issues: string[];
  fingerprint: string | null;
  integrationRef: string | null;
  complete: boolean | null;
  branches: { compared: number; merged: number; ahead: number; diverged: number; unknown: number; older30d: number } | null;
  worktrees: { total: number; dirty: number; unknown: number; conflicts: number; detached: number } | null;
  unintegrated: Count | null;
  tipAgeSeconds: number | null;
  commits24h: Count | null;
  commits7d: Count | null;
};

export type HostHealth = {
  host: string;
  reachable: boolean;
  level: Level;
  loadPercent: number | null;
  memoryAvailablePercent: number | null;
  swapUsedPercent: number | null;
  rootUsedPercent: number | null;
  rootGrowthGbPerHour: number | null;
  rootHoursToFull: number | null;
  /** Disk closest to full by projection, else by usage. */
  worstDisk: { mount: string; usedPercent: number | null; hoursToFull: number | null; growthGbPerHour: number | null; level: Level } | null;
  gpuUtilization: number[];
};

export type CycleStage = { id: string; label: string; ms: number | null };
export type HealthModel = {
  generated: string | null;
  ageSeconds: number | null;
  stale: boolean | null;
  host: string;
  intervalSeconds: number | null;
  cycle: { durationMs: number | null; modelTurns: number | null; collect: CycleStage[]; git: CycleStage[]; serial: CycleStage[] };
  summary: { projects: number; attention: number; unknown: number; healthy: number; disabled: number; dirtyWorktrees: number | null; unmergedOlder30d: number | null; hostsAtRisk: number; hostsUnreachable: number };
  projects: ProjectHealth[];
  hosts: HostHealth[];
};

const count = (value: unknown): Count | null => {
  const raw = data(value);
  const n = number(raw.value);
  return n === null ? null : { value: n, lowerBound: raw.lower_bound === true };
};
const finite = (value: unknown): number | null => (typeof value === 'number' && Number.isFinite(value) ? value : null);
const level = (value: unknown): Level => (value === 'ok' || value === 'amber' || value === 'red' ? value : 'unknown');
const worse = (a: Level, b: Level): Level => {
  const rank: Record<Level, number> = { ok: 0, unknown: 1, amber: 2, red: 3 };
  return rank[a] >= rank[b] ? a : b;
};
/** Sum only when every part is known; one unknown keeps the total unknown. */
const sumKnown = (values: Array<number | null>): number | null => (values.length && values.every(v => v !== null) ? values.reduce<number>((a, b) => a + (b ?? 0), 0) : null);

function projectIds(snapshot: LegacySnapshot): string[] {
  const ids = [...(snapshot.order ?? [])];
  for (const map of [snapshot.projects, data(snapshot.git_health), data(snapshot.git_metrics)]) {
    for (const id of Object.keys(map)) if (!ids.includes(id)) ids.push(id);
  }
  return ids;
}

function projectHealth(snapshot: LegacySnapshot, id: string): ProjectHealth {
  const report = data(data(snapshot.git_health)[id]);
  const metrics = data(data(snapshot.git_metrics)[id]);
  const reported = Object.keys(report).length > 0;
  const state = (['healthy', 'attention', 'unknown', 'disabled'] as const).find(s => s === report.state) ?? 'not_reported';
  const issues = [...new Set(list(report.issues).map(issue => text(data(issue).code)).filter(Boolean))].sort();
  const b = data(metrics.branches), c = data(metrics.checkouts);
  const hasMetrics = Object.keys(metrics).length > 0;
  const windows = list(metrics.activity).map(data);
  const window = (hours: number) => windows.find(w => w.window_hours === hours);
  const fingerprint = text(report.fingerprint).replace(/^sha256:/, '').slice(0, 8) || null;
  return {
    id,
    name: text(data(snapshot.projects[id]).name) || id,
    state: reported ? state : 'not_reported',
    issues,
    fingerprint,
    integrationRef: text(report.integration_ref) || text(metrics.integration_ref) || null,
    complete: typeof metrics.complete === 'boolean' ? metrics.complete : null,
    branches: hasMetrics ? { compared: number(b.compared) ?? 0, merged: number(b.merged) ?? 0, ahead: number(b.ahead_only) ?? 0, diverged: number(b.diverged) ?? 0, unknown: number(b.unknown) ?? 0, older30d: number(b.unmerged_tips_older_30d) ?? 0 } : null,
    worktrees: hasMetrics ? { total: number(c.total) ?? 0, dirty: number(c.dirty) ?? 0, unknown: number(c.state_unknown) ?? 0, conflicts: number(c.with_conflicts) ?? 0, detached: number(c.detached) ?? 0 } : null,
    unintegrated: count(b.unintegrated_commits),
    tipAgeSeconds: finite(data(metrics.integration).tip_age_seconds),
    commits24h: count(window(24)?.integration_first_parent),
    commits7d: count(window(168)?.integration_first_parent),
  };
}

function hostHealth(raw: unknown): HostHealth {
  const host = data(raw);
  const disks = list(host.disks).map(data);
  const root = disks.find(d => d.mount === '/');
  const load = list(host.load).map(finite)[0] ?? null;
  const nproc = number(host.nproc);
  const mem = data(host.mem);
  const swapTotal = finite(mem.swap_total_gb), swapUsed = finite(mem.swap_used_gb);
  const ranked = [...disks].sort((a, b) => {
    const ha = finite(a.hours_to_full) ?? Infinity, hb = finite(b.hours_to_full) ?? Infinity;
    return ha !== hb ? ha - hb : (finite(b.pct) ?? -1) - (finite(a.pct) ?? -1);
  });
  const worst = ranked[0];
  const reachable = host.reachable !== false;
  let hostLevel: Level = reachable ? 'ok' : 'unknown';
  for (const d of disks) hostLevel = worse(hostLevel, level(d.level));
  if (mem.level !== undefined) hostLevel = worse(hostLevel, level(mem.level));
  if (host.load_level !== undefined && host.load_level !== null) hostLevel = worse(hostLevel, level(host.load_level));
  return {
    host: text(host.host) || 'unknown host',
    reachable,
    level: hostLevel,
    loadPercent: load !== null && nproc ? (100 * load) / nproc : null,
    memoryAvailablePercent: finite(mem.avail_pct),
    swapUsedPercent: swapTotal && swapUsed !== null ? (100 * swapUsed) / swapTotal : null,
    rootUsedPercent: finite(root?.pct),
    rootGrowthGbPerHour: finite(root?.growth_gb_per_h),
    rootHoursToFull: finite(root?.hours_to_full),
    worstDisk: worst ? { mount: text(worst.mount), usedPercent: finite(worst.pct), hoursToFull: finite(worst.hours_to_full), growthGbPerHour: finite(worst.growth_gb_per_h), level: level(worst.level) } : null,
    gpuUtilization: list(host.gpus).map(g => finite(data(g).util)).filter((u): u is number => u !== null),
  };
}

export function buildHealthModel(snapshot: LegacySnapshot, now = Date.now()): HealthModel {
  const generated = text(snapshot.generated_utc) || null;
  const parsed = generated ? Date.parse(generated) : NaN;
  const ageSeconds = Number.isFinite(parsed) ? Math.round((now - parsed) / 1000) : null;
  const intervalSeconds = number(snapshot.interval_s);
  // Same rule as the agent brief: three missed cycles (at least 90 s).
  const stale = ageSeconds === null ? null : ageSeconds > Math.max(90, 3 * (intervalSeconds ?? 30));
  const collector = data(snapshot.collector);
  const stages = data(collector.stages_ms);
  const stage = (id: string, label: string): CycleStage => ({ id, label, ms: number(stages[id]) });
  const projects = projectIds(snapshot).map(id => projectHealth(snapshot, id));
  const hosts = list(data(snapshot.global_resources).hosts).map(hostHealth).sort((a, b) => a.host.localeCompare(b.host));
  const withMetrics = projects.filter(p => p.worktrees !== null);
  return {
    generated,
    ageSeconds,
    stale,
    host: text(snapshot.host) || 'unknown host',
    intervalSeconds,
    cycle: {
      durationMs: number(collector.duration_ms),
      modelTurns: number(collector.model_turns),
      collect: [stage('fleet', 'Fleet'), stage('projects', 'Projects')],
      git: [stage('git', 'Git scan')],
      serial: [stage('git_health', 'Health'), stage('usage', 'Usage'), stage('history', 'History')],
    },
    summary: {
      projects: projects.length,
      attention: projects.filter(p => p.state === 'attention').length,
      unknown: projects.filter(p => p.state === 'unknown').length,
      healthy: projects.filter(p => p.state === 'healthy').length,
      disabled: projects.filter(p => p.state === 'disabled' || p.state === 'not_reported').length,
      dirtyWorktrees: withMetrics.length ? sumKnown(withMetrics.map(p => p.worktrees?.dirty ?? null)) : null,
      unmergedOlder30d: withMetrics.length ? sumKnown(withMetrics.map(p => p.branches?.older30d ?? null)) : null,
      hostsAtRisk: hosts.filter(h => h.level === 'amber' || h.level === 'red').length,
      hostsUnreachable: hosts.filter(h => !h.reachable).length,
    },
    projects,
    hosts,
  };
}

/** Compact human duration for ages and projections. */
export function duration(seconds: number | null): string {
  if (seconds === null) return 'unknown';
  if (seconds < 0) return 'clock skew';
  if (seconds < 90) return `${Math.round(seconds)} s`;
  if (seconds < 90 * 60) return `${Math.round(seconds / 60)} min`;
  if (seconds < 48 * 3600) return `${Math.round(seconds / 3600)} h`;
  return `${Math.round(seconds / 86_400)} d`;
}

/** At most two decimals (the dashboard-wide display rule); unknown stays unknown. */
export function figure(value: number | null, suffix = ''): string {
  if (value === null) return 'unknown';
  const rounded = Math.round(value * 100) / 100;
  return `${rounded.toLocaleString('en-US', { maximumFractionDigits: 2 })}${suffix}`;
}

export function countText(value: Count | null): string {
  return value === null ? 'unknown' : `${value.value}${value.lowerBound ? '+' : ''}`;
}
