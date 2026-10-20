import { useEffect, useState } from "react";
import { ChatApiError, readBoundedJson } from "./operator-chat";

export type GitHealthCount = { value: number; lower_bound: boolean };
export type GitHealthReport = {
  schema_version: 1;
  instance_id: string;
  observed_utc: string;
  integration_ref: string | null;
  state: "disabled" | "healthy" | "attention" | "unknown";
  actionable: boolean;
  observed_commits: number;
  observed_refs: number;
  observed_checkouts: number;
  missing_parent_oids: string[];
  shallow: boolean;
  truncated: boolean;
  observation_error: boolean;
  fingerprint: string;
  issues: { code: string; severity: "attention" | "unknown"; ref_name: string | null; checkout_id: string | null; detail: string }[];
  branches: { ref_name: string; relation: "equal" | "ahead" | "behind" | "diverged" | "unrelated" | "unknown"; ahead: GitHealthCount | null; behind: GitHealthCount | null }[];
};
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Unsupported Git health response");
  return value as Record<string, unknown>;
}
function text(value: unknown): string {
  if (typeof value !== "string" || value.length > 8192) throw new Error("Invalid Git health text");
  return value;
}
function count(value: unknown): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) throw new Error("Invalid Git health count");
  return value as number;
}
function nullableText(value: unknown) { return value === null ? null : text(value); }
function bound(value: unknown): GitHealthCount | null {
  if (value === null) return null;
  const item = record(value);
  if (typeof item.lower_bound !== "boolean") throw new Error("Invalid Git health lower bound");
  return { value: count(item.value), lower_bound: item.lower_bound };
}
export function parseGitHealth(value: unknown): { reports: Record<string, GitHealthReport>; error: string | null } {
  const item = record(value), reports = record(item.reports);
  if (item.schema_version !== 1 || Object.keys(reports).length > 2000) throw new Error("Unsupported Git health version or scope");
  const result: Record<string, GitHealthReport> = Object.create(null);
  for (const [id, raw] of Object.entries(reports)) {
    const report = record(raw);
    if (report.schema_version !== 1 || !["disabled", "healthy", "attention", "unknown"].includes(String(report.state)) ||
        typeof report.actionable !== "boolean" || typeof report.shallow !== "boolean" || typeof report.truncated !== "boolean" || typeof report.observation_error !== "boolean" ||
        !Array.isArray(report.issues) || report.issues.length > 20000 || !Array.isArray(report.branches) || report.branches.length > 20000 ||
        !Array.isArray(report.missing_parent_oids) || report.missing_parent_oids.length > 20000)
      throw new Error("Invalid Git health report");
    const observed = text(report.observed_utc);
    if (!Number.isFinite(Date.parse(observed))) throw new Error("Invalid Git health observation time");
    const issues = report.issues.map((raw): GitHealthReport["issues"][number] => {
      const issue = record(raw);
      if (issue.severity !== "attention" && issue.severity !== "unknown") throw new Error("Invalid Git health severity");
      return { code: text(issue.code), severity: issue.severity, ref_name: nullableText(issue.ref_name), checkout_id: nullableText(issue.checkout_id), detail: text(issue.detail) };
    });
    const branches = report.branches.map((raw): GitHealthReport["branches"][number] => {
      const branch = record(raw);
      if (!["equal", "ahead", "behind", "diverged", "unrelated", "unknown"].includes(String(branch.relation))) throw new Error("Invalid Git health relation");
      return { ref_name: text(branch.ref_name), relation: branch.relation as GitHealthReport["branches"][number]["relation"], ahead: bound(branch.ahead), behind: bound(branch.behind) };
    });
    result[id] = {
      schema_version: 1, instance_id: text(report.instance_id), observed_utc: observed, integration_ref: nullableText(report.integration_ref),
      state: report.state as GitHealthReport["state"], actionable: report.actionable,
      observed_commits: count(report.observed_commits), observed_refs: count(report.observed_refs), observed_checkouts: count(report.observed_checkouts),
      missing_parent_oids: report.missing_parent_oids.map(text), shallow: report.shallow, truncated: report.truncated, observation_error: report.observation_error,
      fingerprint: text(report.fingerprint), issues, branches,
    };
  }
  return { reports: result, error: nullableText(item.error) };
}
export function useGitHealth(token: string | undefined, scopeEpoch: number, enabled: boolean): { reports: Record<string, GitHealthReport>; error: string } {
  const [state, setState] = useState<{ token?: string; scopeEpoch: number; reports: Record<string, GitHealthReport>; error: string } | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let stopped = false, timer: ReturnType<typeof setTimeout> | undefined, request: AbortController | undefined;
    async function poll() {
      request = new AbortController();
      const deadline = setTimeout(() => request?.abort(), 12_000);
      try {
        const response = await fetch("/v2/git/health", { signal: request.signal, cache: "no-store", headers: token ? { Authorization: `Bearer ${token}` } : {} });
        if (!response.ok) throw new ChatApiError(response.status);
        const next = parseGitHealth(await readBoundedJson(response, 4 * 1024 * 1024));
        if (!stopped) setState({ token, scopeEpoch, reports: next.reports, error: next.error ?? "" });
      } catch (failure) {
        if (stopped) return;
        const error = failure instanceof ChatApiError ? `Git health request failed (${failure.status})` : failure instanceof Error ? failure.message : "Git health unavailable";
        setState({ token, scopeEpoch, reports: {}, error });
        if (failure instanceof ChatApiError && [401, 403].includes(failure.status)) return;
      } finally {
        clearTimeout(deadline);
      }
      if (!stopped) timer = setTimeout(poll, 5_000);
    }
    void poll();
    return () => { stopped = true; request?.abort(); clearTimeout(timer); };
  }, [token, scopeEpoch, enabled]);
  return enabled && state && state.token === token && state.scopeEpoch === scopeEpoch ? state : { reports: {}, error: enabled ? "Checking Git health…" : "Git health requires current private observations." };
}
