import { data, list, number, text, type LegacySnapshot } from '../../legacy';

/** Signed finite number (disk growth can be negative); anything else is unknown. */
export const signed = (value: unknown): number | null => typeof value === 'number' && Number.isFinite(value) ? value : null;

export type Severity = 'critical' | 'warning' | 'info';
export type Finding = { severity: Severity; subject: string; message: string; action: string; href?: string };

export type Count = { value: number; lowerBound: boolean } | null;
const count = (value: unknown): Count => {
  const item = data(value), n = number(item.value);
  return n === null ? null : { value: n, lowerBound: item.lower_bound === true };
};

export type GitRow = {
  id: string; name: string; state: string; observedUtc: string; integrationRef: string; threshold: number | null;
  issues: string[];
  unmerged: number | null; merged: number | null; diverged: number | null; remoteUnmerged: number | null;
  maxBehind: Count; maxAhead: Count; oldestUnmergedAgeS: number | null;
  dirty: number | null; conflicted: number | null; detached: number | null; cleanup: string[] | null;
  tipAgeS: number | null; commits24h: Count; commits7d: Count; merges7d: Count; reverts7d: Count; fixups7d: Count;
};

export type DiskRow = { mount: string; pct: number | null; size: number | null; avail: number | null;
  growthGbPerHour: number | null; hoursToFull: number | null; level: string };
export type HostRow = {
  id: string; reachable: boolean; error: string; nproc: number | null; load1: number | null; loadPct: number | null;
  memAvailPct: number | null; memAvailGb: number | null; memTotalGb: number | null; swapUsedGb: number | null; swapTotalGb: number | null;
  disks: DiskRow[]; pressure: { cpuSome: number | null; memoryFull: number | null; ioFull: number | null } | null;
  dState: number | null; top: { pid: number; comm: string; rssGb: number | null; pcpu: number | null; user: string }[];
  gpus: { index: string; name: string; util: number | null; memUsed: number | null; memTotal: number | null; temp: number | null }[];
  warnings: { level: string; text: string }[];
};

function projectName(snapshot: LegacySnapshot, id: string): string {
  const fromProject = text(data(snapshot.projects[id]).name);
  if (fromProject) return fromProject;
  const catalog = list(snapshot.project_catalog).map(data).find(item => text(item.id) === id);
  return text(catalog?.name) || id;
}

/** Git hygiene rows in the producer's project order; projects without a report are omitted. */
export function gitRows(snapshot: LegacySnapshot): GitRow[] {
  const reports = data(snapshot.git_health);
  const order = [...(snapshot.order ?? []), ...Object.keys(reports)].filter((id, index, all) => all.indexOf(id) === index && id in reports);
  return order.map(id => {
    const report = data(reports[id]), metrics = data(report.metrics), cleanup = metrics.cleanup_candidate_checkouts;
    return {
      id, name: projectName(snapshot, id), state: text(report.state) || 'unknown', observedUtc: text(report.observed_utc),
      integrationRef: text(report.integration_ref), threshold: number(report.divergence_threshold),
      issues: [...new Set(list(report.issues).map(issue => text(data(issue).code)).filter(Boolean))],
      unmerged: number(metrics.unmerged_branches), merged: number(metrics.merged_branches), diverged: number(metrics.diverged_branches),
      remoteUnmerged: number(metrics.remote_unmerged_refs), maxBehind: count(metrics.max_unmerged_behind), maxAhead: count(metrics.max_unmerged_ahead),
      oldestUnmergedAgeS: number(metrics.oldest_unmerged_tip_age_s), dirty: number(metrics.dirty_checkouts),
      conflicted: number(metrics.conflicted_checkouts), detached: number(metrics.detached_checkouts),
      cleanup: Array.isArray(cleanup) ? cleanup.map(text).filter(Boolean) : null,
      tipAgeS: number(metrics.integration_tip_age_s), commits24h: count(metrics.integration_commits_24h), commits7d: count(metrics.integration_commits_7d),
      merges7d: count(metrics.integration_merges_7d), reverts7d: count(metrics.integration_reverts_7d), fixups7d: count(metrics.integration_fixups_7d),
    };
  });
}

/** Host resource rows; unreachable hosts keep their reason and no invented numbers. */
export function hostRows(snapshot: LegacySnapshot): HostRow[] {
  return list(data(snapshot.global_resources).hosts).map(data).map(host => {
    const mem = data(host.mem), pressure = data(host.pressure), nproc = number(host.nproc), load1 = number(list(host.load)[0]);
    const psi = (resource: string, key: string) => number(data(pressure[resource])[key]);
    return {
      id: text(host.host) || 'unknown', reachable: host.reachable === true, error: text(host.error), nproc, load1,
      loadPct: nproc && load1 !== null ? 100 * load1 / nproc : null,
      memAvailPct: number(mem.avail_pct), memAvailGb: number(mem.avail_gb), memTotalGb: number(mem.total_gb),
      swapUsedGb: number(mem.swap_used_gb), swapTotalGb: number(mem.swap_total_gb),
      disks: list(host.disks).map(data).map(disk => ({ mount: text(disk.mount) || '?', pct: number(disk.pct), size: number(disk.size), avail: number(disk.avail),
        growthGbPerHour: signed(disk.growth_gb_per_h), hoursToFull: number(disk.hours_to_full), level: text(disk.level) || 'unknown' })),
      pressure: Object.keys(pressure).length ? { cpuSome: psi('cpu', 'some_avg60'), memoryFull: psi('memory', 'full_avg60'), ioFull: psi('io', 'full_avg60') } : null,
      dState: number(host.d_state_procs),
      top: list(host.top_rss).map(data).map(item => ({ pid: number(item.pid) ?? 0, comm: text(item.comm), rssGb: number(item.rss_gb), pcpu: number(item.pcpu), user: text(item.user) })),
      gpus: list(host.gpus).map(data).map(gpu => ({ index: text(gpu.idx), name: text(gpu.name), util: number(gpu.util), memUsed: number(gpu.mem_used_mib), memTotal: number(gpu.mem_total_mib), temp: number(gpu.temp_c) })),
      warnings: list(host.warnings).map(data).map(item => ({ level: text(item.level) || 'amber', text: text(item.text) })).filter(item => item.text),
    };
  });
}

const severityRank: Record<Severity, number> = { critical: 0, warning: 1, info: 2 };

export type Readiness = { status: 'ready' | 'not_ready' | 'unreachable' | 'checking'; detail: string };

/**
 * What needs attention, most severe first. Every finding states its subject,
 * what was observed and the next action; nothing here takes the action.
 */
export function findings(snapshot: LegacySnapshot | null, readiness: Readiness, now = Date.now()): Finding[] {
  const result: Finding[] = [];
  if (readiness.status === 'not_ready' || readiness.status === 'unreachable') {
    result.push({ severity: 'critical', subject: 'admiraltyd', message: readiness.detail || 'Service is not ready.',
      action: 'Check journalctl -u admiraltyd and admiralty logs digest.' });
  }
  if (!snapshot) return result;
  const generated = Date.parse(snapshot.generated_utc), interval = number(snapshot.interval_s) ?? 15;
  const age = (now - generated) / 1000;
  if (Number.isFinite(age) && age > Math.max(180, interval * 6)) {
    result.push({ severity: 'warning', subject: 'Observer', message: `Status was produced ${Math.round(age / 60)} min ago.`,
      action: 'Confirm admiralty-observe is running; values below may be stale.' });
  }
  for (const host of hostRows(snapshot)) {
    if (!host.reachable) result.push({ severity: 'warning', subject: host.id, message: `Unreachable: ${host.error || 'reason unknown'}.`, action: 'Check SSH reachability from the observer node.' });
    for (const warning of host.warnings) {
      if (!host.reachable && warning.text === host.error) continue;
      result.push({ severity: warning.level === 'red' ? 'critical' : 'warning', subject: host.id, message: warning.text,
        action: /disk|full/i.test(warning.text) ? 'Free space or move build output before it fills.' : /pressure/i.test(warning.text) ? 'Reduce concurrent jobs on this host.' : 'Avoid placing new work on this host.' });
    }
  }
  for (const row of gitRows(snapshot)) {
    const href = `#/git?project=${encodeURIComponent(row.id)}`;
    if (row.state === 'attention') result.push({ severity: 'warning', subject: row.name, message: `Git health needs attention: ${row.issues.join(', ') || 'see report'}.`, action: 'Review in Git Flow before integrating.', href });
    if (row.state === 'unknown') result.push({ severity: 'info', subject: row.name, message: `Git health is unknown: ${row.issues.join(', ') || 'incomplete observation'}.`, action: 'Fix the observation gap before trusting cleanup.', href });
    if (row.merges7d && row.merges7d.value > 0) result.push({ severity: 'warning', subject: row.name, message: `${row.merges7d.value} merge commit${row.merges7d.value === 1 ? '' : 's'} reached ${row.integrationRef || 'integration'} this week.`, action: 'Merges here should be rebase-only; check the merge path.', href });
    const threshold = row.threshold ?? 20;
    if (row.maxBehind && row.maxBehind.value > threshold) result.push({ severity: row.maxBehind.value > threshold * 5 ? 'warning' : 'info', subject: row.name, message: `An unmerged lane trails integration by ${row.maxBehind.lowerBound ? 'at least ' : ''}${row.maxBehind.value} commits.`, action: 'Rebase the lane onto the integration head.', href });
    if (row.cleanup && row.cleanup.length) result.push({ severity: 'info', subject: row.name, message: `${row.cleanup.length} worktree${row.cleanup.length === 1 ? ' is' : 's are'} merged and clean.`, action: 'Confirm no process uses them, then git worktree remove.', href });
    if (row.merged && row.merged > 0) result.push({ severity: 'info', subject: row.name, message: `${row.merged} local branch${row.merged === 1 ? ' is' : 'es are'} fully merged.`, action: 'Delete after the owner confirms.', href });
    if (row.reverts7d && row.reverts7d.value > 0) result.push({ severity: 'info', subject: row.name, message: `${row.reverts7d.value} revert${row.reverts7d.value === 1 ? '' : 's'} landed this week.`, action: 'Check whether the reverted change needs a follow-up.', href });
    if (row.fixups7d && row.fixups7d.value > 0) result.push({ severity: 'info', subject: row.name, message: `${row.fixups7d.value} fixup or squash commit${row.fixups7d.value === 1 ? '' : 's'} reached integration.`, action: 'Autosquash lane history before it lands.', href });
  }
  return result.sort((a, b) => severityRank[a.severity] - severityRank[b.severity]);
}

export type RouteStat = { route: string; requests: number; clientErrors: number; serverErrors: number; p95LeSeconds: number | null };
export type MetricsSummary = { routes: RouteStat[]; gauges: Record<string, number> };

const sample = /^([a-zA-Z_:][\w:]*)(?:\{(.*)\})?\s+(\S+)$/;
function labels(source: string | undefined): Record<string, string> {
  const result: Record<string, string> = {};
  if (!source) return result;
  const pattern = /(\w+)="((?:\\.|[^"\\])*)"/g;
  for (const match of source.matchAll(pattern)) result[match[1]] = match[2].replace(/\\(.)/g, (_, char: string) => char === 'n' ? '\n' : char);
  return result;
}

/** Summarize Admiralty's Prometheus exposition: per-route totals, errors and p95 bucket. */
export function parsePrometheus(textValue: string): MetricsSummary {
  const routes = new Map<string, RouteStat>(), buckets = new Map<string, { le: number; count: number }[]>();
  const gauges: Record<string, number> = {};
  const route = (name: string) => {
    let stat = routes.get(name);
    if (!stat) routes.set(name, stat = { route: name, requests: 0, clientErrors: 0, serverErrors: 0, p95LeSeconds: null });
    return stat;
  };
  for (const line of textValue.split('\n')) {
    if (!line || line.startsWith('#')) continue;
    const match = sample.exec(line.trim());
    if (!match) continue;
    const [, name, labelText, raw] = match, value = Number(raw), tags = labels(labelText);
    if (!Number.isFinite(value) && raw !== '+Inf') continue;
    if (name === 'admiralty_http_requests_total' && tags.route) {
      const stat = route(tags.route);
      stat.requests += value;
      if (tags.class === '4xx') stat.clientErrors += value;
      if (tags.class === '5xx') stat.serverErrors += value;
    } else if (name === 'admiralty_http_request_duration_seconds_bucket' && tags.route) {
      const le = tags.le === '+Inf' ? Infinity : Number(tags.le);
      if (Number.isFinite(le) || le === Infinity) (buckets.get(tags.route) ?? buckets.set(tags.route, []).get(tags.route)!).push({ le, count: value });
    } else if (!labelText) gauges[name] = value;
  }
  for (const [name, rows] of buckets) {
    const stat = route(name), total = Math.max(...rows.map(row => row.count));
    if (total <= 0) continue;
    const target = Math.ceil(total * 0.95);
    const hit = rows.sort((a, b) => a.le - b.le).find(row => row.count >= target);
    stat.p95LeSeconds = hit && Number.isFinite(hit.le) ? hit.le : null;
  }
  return { routes: [...routes.values()].sort((a, b) => b.requests - a.requests || a.route.localeCompare(b.route)), gauges };
}

/** "3h", "2d", "45m" — compact, never more precise than the data. */
export function age(seconds: number | null): string {
  if (seconds === null) return 'unknown';
  if (seconds < 90) return `${Math.round(seconds)}s`;
  if (seconds < 5400) return `${Math.round(seconds / 60)}m`;
  if (seconds < 172800) return `${Math.round(seconds / 3600)}h`;
  return `${Math.round(seconds / 86400)}d`;
}

export const countText = (value: Count): string => value === null ? 'unknown' : `${value.lowerBound ? '≥' : ''}${value.value}`;
