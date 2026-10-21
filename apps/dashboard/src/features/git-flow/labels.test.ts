import { describe, expect, it } from "vitest";
import { placeGitLabels, type GitLabelCandidate, type GitPlacedLabel } from "./labels";

const measure = (text: string) => Array.from(text).reduce((width, character) => width + (character === "W" ? 13 : character === "i" ? 3 : 7), 0);
const candidate = (id: string, patch: Partial<GitLabelCandidate> = {}): GitLabelCandidate => ({ id, x: 180, y: 100, names: [`refs/heads/${id}`], color: "#70b8ff", priority: 0, ...patch });
function assertPlacement(labels: GitPlacedLabel[], viewport = { width: 500, height: 300 }, padding = 24) {
  for (const label of labels) {
    expect(label.height).toBe(20);
    expect(label.width).toBeLessThanOrEqual(270);
    expect(label.x).toBeGreaterThanOrEqual(padding); expect(label.y).toBeGreaterThanOrEqual(padding);
    expect(label.x + label.width).toBeLessThanOrEqual(viewport.width - padding);
    expect(label.y + label.height).toBeLessThanOrEqual(viewport.height - padding);
    expect(measure(label.text) + 16).toBeLessThanOrEqual(label.width);
    expect([label.x, label.y, label.width, label.height].every(Number.isFinite)).toBe(true);
  }
  for (let first = 0; first < labels.length; first++) for (let second = first + 1; second < labels.length; second++) {
    const a = labels[first], b = labels[second];
    expect(a.x + a.width <= b.x || b.x + b.width <= a.x || a.y + a.height <= b.y || b.y + b.height <= a.y).toBe(true);
  }
}

describe("Git screen label policy", () => {
  it("middle truncates long measured branch names while keeping their useful ends", () => {
    const name = "codex/very-wide-WWW-branch-with-an-important-unique-final-suffix";
    const result = placeGitLabels([candidate("long", { names: [name] })], { width: 500, height: 300 }, 24, measure);
    const label = result.labels[0];
    expect(label.text).toContain("…"); expect(label.text.startsWith("codex/")).toBe(true); expect(label.text.endsWith("suffix")).toBe(true);
    expect(label.fullText).toBe(name); expect(result.hiddenRefCount).toBe(0);
    assertPlacement(result.labels);
  });

  it("groups co-located refs in one candidate and keeps every source name", () => {
    const names = ["main", "origin/main", "release/v1", "tag/v1"];
    const result = placeGitLabels([candidate("head", { names })], { width: 500, height: 300 }, 24, measure);
    expect(result.labels).toHaveLength(1);
    expect(result.labels[0]).toMatchObject({ text: "main +3", fullText: names.join(" · "), refCount: 4 });
    expect(result.hiddenRefCount).toBe(0);
  });

  it("uses supplied resolved-backbone priority rather than guessing a branch name", () => {
    const result = placeGitLabels([candidate("named-main", { names: ["main"], priority: 0 }), candidate("actual-trunk", { names: ["integration/custom"], priority: 100 })], { width: 500, height: 300 }, 24, measure, 1);
    expect(result.labels[0].id).toBe("actual-trunk");
    expect(result.hiddenRefCount).toBe(1);
  });

  it("uses above, below and side alternatives without overlaps in a dense cluster", () => {
    const candidates = Array.from({ length: 20 }, (_, index) => candidate(`branch-${index}`, { names: [`topic-${index}`], priority: index % 3 }));
    const result = placeGitLabels(candidates, { width: 500, height: 300 }, 24, measure);
    expect(result.labels.length).toBeGreaterThan(1); expect(result.labels.length).toBeLessThan(20);
    expect(result.hiddenRefCount + result.labels.reduce((count, label) => count + label.refCount, 0)).toBe(20);
    expect(new Set(result.labels.map((label) => label.y)).size).toBeGreaterThan(1);
    assertPlacement(result.labels);
  });

  it("stays deterministic when source candidates arrive in another order", () => {
    const candidates = Array.from({ length: 20 }, (_, index) => candidate(`branch-${index}`, { x: 60 + index % 4 * 15, y: 80 + index % 3 * 8, priority: index % 5 }));
    const viewport = { width: 500, height: 300 };
    const first = placeGitLabels(candidates, viewport, 24, measure), reversed = placeGitLabels([...candidates].reverse(), viewport, 24, measure);
    expect(reversed).toEqual(first);
  });

  it("truncates whole Unicode graphemes and preserves grouped-ref suffixes at small widths", () => {
    const name = "👩🏽‍💻/équipe/a\u0301-project-🐳-unique-end";
    const result = placeGitLabels([candidate("unicode", { names: [name, "other"] })], { width: 170, height: 80 }, 8, measure);
    const text = result.labels[0].text;
    expect(text.startsWith("👩🏽‍💻")).toBe(true); expect(text.endsWith("end +1")).toBe(true); expect(text).toContain("…");
    expect(text).not.toMatch(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u);
    expect(result.labels[0].fullText).toBe(`${name} · other`);
    assertPlacement(result.labels, { width: 170, height: 80 }, 8);
  });

  it("accounts hidden references in invalid positions and impossibly small panes", () => {
    const candidates = [candidate("one", { names: ["a", "b"] }), candidate("invalid", { x: NaN, names: ["c", "d", "e"] }), candidate("bad-priority", { priority: Infinity, names: ["f"] }), candidate("empty", { names: [] })];
    const result = placeGitLabels(candidates, { width: 500, height: 300 }, 24, measure);
    expect(result.hiddenRefCount).toBe(4);
    expect(result.labels[0].refCount).toBe(2);
    for (const viewport of [{ width: 1, height: 1 }, { width: 20, height: 100 }, { width: 500, height: 0 }, { width: Infinity, height: 100 }])
      expect(placeGitLabels(candidates, viewport, 24, measure)).toEqual({ labels: [], hiddenRefCount: 6 });
    expect(placeGitLabels([], { width: 1, height: 1 }, 0, measure)).toEqual({ labels: [], hiddenRefCount: 0 });
  });

  it("keeps all boxes inside feasible insets near every viewport corner", () => {
    const viewport = { width: 280, height: 120 }, candidates = [candidate("a", { x: -40, y: -40 }), candidate("b", { x: 300, y: -40 }), candidate("c", { x: -40, y: 160 }), candidate("d", { x: 300, y: 160 })];
    const result = placeGitLabels(candidates, viewport, 12, measure);
    assertPlacement(result.labels, viewport, 12);
    expect(result.labels.length + result.hiddenRefCount).toBe(4);
  });

  it("caps placed labels at200 and avoids measuring additional candidates after the cap", () => {
    const candidates = Array.from({ length: 500 }, (_, index) => candidate(`${index}`, { x: 30 + index % 20 * 300, y: 50 + Math.floor(index / 20) * 80 }));
    let calls = 0;
    const result = placeGitLabels(candidates, { width: 6500, height: 2400 }, 0, (text) => { calls++; return measure(text); }, 1000);
    expect(result.labels).toHaveLength(200); expect(result.hiddenRefCount).toBe(300);
    expect(calls).toBe(200);
    expect(placeGitLabels(candidates, { width: 6500, height: 2400 }, 0, measure, 0)).toEqual({ labels: [], hiddenRefCount: 500 });
  });

  it("handles invalid font metrics without emitting nonfinite or overflowing boxes", () => {
    const candidates = [candidate("font", { names: ["branch"] })];
    expect(placeGitLabels(candidates, { width: 100, height: 100 }, 8, () => NaN)).toEqual({ labels: [], hiddenRefCount: 1 });
    expect(placeGitLabels(candidates, { width: 100, height: 100 }, 8, () => { throw new Error("Font unavailable"); })).toEqual({ labels: [], hiddenRefCount: 1 });
    const result = placeGitLabels(candidates, { width: 40, height: 40 }, 4, measure);
    assertPlacement(result.labels, { width: 40, height: 40 }, 4);
    expect(result.labels.length + result.hiddenRefCount).toBe(1);
  });

  it("moves a ref below an occupied collapsed-range button", () => {
    const result = placeGitLabels([candidate("head", { names: ["main"] })], { width: 500, height: 300 }, 24, measure, 200,
      [{ x: 172, y: 70, width: 60, height: 20 }]);
    expect(result.labels[0].y).toBe(114);
    expect(result.hiddenRefCount).toBe(0);
  });

  it("uses a side alternative when controls occupy above and below", () => {
    const result = placeGitLabels([candidate("head", { names: ["main"] })], { width: 500, height: 300 }, 24, measure, 200,
      [{ x: 172, y: 70, width: 60, height: 20 }, { x: 172, y: 114, width: 60, height: 20 }]);
    expect(result.labels[0].y).toBe(90);
    expect(result.labels[0].x + result.labels[0].width).toBeLessThanOrEqual(168);
  });

  it("counts every grouped ref as hidden when controls occupy all alternatives", () => {
    expect(placeGitLabels([candidate("head", { names: ["main", "origin/main", "tag/v1"] })], { width: 500, height: 300 }, 24, measure, 200,
      [{ x: 0, y: 0, width: 500, height: 300 }])).toEqual({ labels: [], hiddenRefCount: 3 });
  });

  it("clips partly offscreen obstacles and ignores invalid or empty rectangles", () => {
    const result = placeGitLabels([candidate("head", { names: ["main"] })], { width: 500, height: 300 }, 24, measure, 200,
      [{ x: -100, y: 60, width: 330, height: 40 }, { x: NaN, y: 0, width: 500, height: 300 },
        { x: 0, y: 0, width: Infinity, height: 300 }, { x: 0, y: 0, width: 500, height: 0 }, { x: 0, y: 0, width: -1, height: 300 }]);
    expect(result.labels[0].y).toBe(114);
    assertPlacement(result.labels);
  });

  it("stays deterministic when obstacle and candidate order change", () => {
    const candidates = Array.from({ length: 12 }, (_, index) => candidate(`branch-${index}`, { x: 100 + index % 3 * 40, y: 90 + index % 2 * 30 }));
    const obstacles = [{ x: 120, y: 60, width: 160, height: 26 }, { x: 50, y: 170, width: 300, height: 38 }];
    const first = placeGitLabels(candidates, { width: 500, height: 300 }, 24, measure, 200, obstacles);
    expect(placeGitLabels([...candidates].reverse(), { width: 500, height: 300 }, 24, measure, 200, [...obstacles].reverse())).toEqual(first);
    for (const label of first.labels) for (const obstacle of obstacles)
      expect(label.x + label.width + 4 <= obstacle.x || obstacle.x + obstacle.width + 4 <= label.x || label.y + label.height + 4 <= obstacle.y || obstacle.y + obstacle.height + 4 <= label.y).toBe(true);
  });

  it("keeps labels clear of every actual range-control height", () => {
    for (const height of [14, 20, 26, 38, 52]) {
      const obstacle = { x: 0, y: 70, width: 500, height };
      const result = placeGitLabels([candidate("head", { names: ["main"] })], { width: 500, height: 300 }, 24, measure, 200, [obstacle]);
      expect(result.labels).toHaveLength(1);
      const label = result.labels[0];
      expect(label.y + label.height + 4 <= obstacle.y || obstacle.y + obstacle.height + 4 <= label.y).toBe(true);
      expect(result.hiddenRefCount).toBe(0);
    }
  });
});
