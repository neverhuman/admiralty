import { describe, expect, it } from "vitest";
import { parseGitHealth } from "./gitHealth";

function fixture() {
  return {
    schema_version: 1, error: null,
    reports: { dope: {
      schema_version: 1, instance_id: "fixture", observed_utc: "2026-10-20T12:00:00Z", integration_ref: "refs/heads/main",
      integration_oid: "a".repeat(40), state: "unknown", actionable: false, observed_commits: 3, observed_refs: 2, observed_checkouts: 1,
      missing_parent_oids: ["0".repeat(40)], shallow: false, truncated: true, observation_error: false, fingerprint: "fixture-fingerprint",
      issues: [{ code: "missing_parents", severity: "unknown", ref_name: null, checkout_id: null, detail: "Incomplete ancestry." }],
      branches: [{ ref_name: "refs/heads/worker", relation: "unknown", ahead: { value: 2, lower_bound: true }, behind: null }],
    } },
  };
}
describe("bounded Git health projection", () => {
  it("preserves unknown ancestry, lower bounds and explicit integration policy", () => {
    const report = parseGitHealth(fixture()).reports.dope;
    expect(report.state).toBe("unknown");
    expect(report.branches[0]).toMatchObject({ relation: "unknown", ahead: { value: 2, lower_bound: true }, behind: null });
    expect(report.integration_ref).toBe("refs/heads/main");
    expect(report.integration_oid).toBe("a".repeat(40));
    expect(report.truncated).toBe(true);
  });
  it("preserves disabled analysis without inventing an integration target", () => {
    const input = fixture(); Object.assign(input.reports.dope, {state:"disabled",integration_ref:null,integration_oid:null,branches:[]});
    expect(parseGitHealth(input).reports.dope).toMatchObject({state:"disabled",integration_ref:null,integration_oid:null,branches:[]});
  });
  it("rejects malformed state, time, counts and issue severity", () => {
    const report = fixture().reports.dope;
    for (const patch of [
      { schema_version: 2 }, { integration_oid: 123 }, { state: "green" }, { observed_utc: "not a time" }, { observed_commits: -1 },
      { issues: [{ ...report.issues[0], severity: "healthy" }] },
      { branches: [{ ...report.branches[0], ahead: { value: 2, lower_bound: "yes" } }] },
    ]) expect(() => parseGitHealth({ schema_version: 1, error: null, reports: { dope: { ...report, ...patch } } })).toThrow();
  });
  it("keeps source absence explicit and rejects giant project inventories", () => {
    expect(parseGitHealth({ schema_version: 1, reports: {}, error: "Producer source unavailable" })).toEqual({ reports: {}, error: "Producer source unavailable" });
    expect(() => parseGitHealth({ schema_version: 1, reports: Object.fromEntries(Array.from({ length: 2001 }, (_, i) => [String(i), fixture().reports.dope])), error: null })).toThrow("scope");
  });
});
