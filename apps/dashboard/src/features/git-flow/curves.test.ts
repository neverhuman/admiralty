import { describe, expect, it } from "vitest";
import type { CommitMark, GraphEdge } from "./model";
import { curveBounds, parentCurve, type ParentCurve } from "./curves";

const mark = (id: number, x: number, y: number): CommitMark => ({
  x, y, lane: y === 0 ? 0 : 1, rank: id,
  commit: { oid: id.toString(16).padStart(40, "0"), parents: [], subject: `Commit ${id}`, committed_utc: "2026-10-22T00:00:00Z" },
});
const edge = (parent: CommitMark | null, child: CommitMark, patch: Partial<GraphEdge> = {}): GraphEdge => ({
  id: "actual-edge", parentOid: parent?.commit.oid ?? "f".repeat(40), childOid: child.commit.oid,
  parent, child, parentIndex: 0, kind: parent ? "parent" : "missing", ...patch,
});
function point(curve: ParentCurve, t: number) {
  const before = 1 - t;
  return {
    x: before ** 3 * curve.from.x + 3 * before ** 2 * t * curve.control1.x + 3 * before * t ** 2 * curve.control2.x + t ** 3 * curve.to.x,
    y: before ** 3 * curve.from.y + 3 * before ** 2 * t * curve.control1.y + 3 * before * t ** 2 * curve.control2.y + t ** 3 * curve.to.y,
  };
}

describe("actual Git parent curves", () => {
  it("keeps the actual first-parent spine straight even if a return flag is present", () => {
    const parent = mark(1, 64, 0), child = mark(2, 288, 0);
    const curve = parentCurve(edge(parent, child, { kind: "backbone", backboneReturn: true }))!;
    expect(curve).toEqual({ from: { x: 64, y: 0 }, control1: { x: 176, y: 0 }, control2: { x: 176, y: 0 }, to: { x: 288, y: 0 } });
    expect(curveBounds(curve)).toEqual({ minX: 64, minY: 0, maxX: 288, maxY: 0 });
  });

  it("routes only an explicitly flagged additional parent into main from above", () => {
    const parent = mark(1, 100, -108), child = mark(2, 500, 0);
    const ordinary = parentCurve(edge(parent, child, { parentIndex: 1, kind: "merge-parent" }))!;
    const returning = parentCurve(edge(parent, child, { parentIndex: 1, kind: "merge-parent", backboneReturn: true }))!;
    expect(ordinary.control2).toEqual({ x: 300, y: 0 });
    expect(returning.control1).toEqual({ x: 300, y: -108 });
    expect(returning.control2).toEqual({ x: 500, y: -28 });
    expect(returning.to.x - returning.control2.x).toBe(0);
    expect(returning.to.y - returning.control2.y).toBeGreaterThan(0);
    expect(point(returning, 0.999).y).toBeLessThan(child.y);
  });

  it("retains three actual ordered parents without inventing another connector", () => {
    const child = mark(4, 600, 0), parents = [mark(1, 64, 0), mark(2, 200, -54), mark(3, 300, -108)];
    child.commit.parents = parents.map((parent) => parent.commit.oid);
    const curves = parents.map((parent, parentIndex) => parentCurve(edge(parent, child, { parentIndex, kind: parentIndex ? "merge-parent" : "backbone", backboneReturn: parentIndex > 0 }))!);
    expect(curves).toHaveLength(child.commit.parents.length);
    expect(curves.map((curve) => curve.from)).toEqual(parents.map((parent) => ({ x: parent.x, y: parent.y })));
    expect(curves[0].control2.y).toBe(0);
    expect(curves.slice(1).map((curve) => curve.control2)).toEqual([{ x: 600, y: -28 }, { x: 600, y: -28 }]);
  });

  it("does not draw missing ancestry or a compressed self-alias as a parent curve", () => {
    const child = mark(2, 200, 0);
    expect(parentCurve(edge(null, child, { parentIndex: 1, backboneReturn: true }))).toBeNull();
    expect(parentCurve(edge(child, child, { parentIndex: 1, backboneReturn: true }))).toBeNull();
  });

  it("bounds include every sampled cubic point, including a same-Y redundant-parent return", () => {
    const cases = [
      edge(mark(1, 100, -108), mark(2, 500, 0), { parentIndex: 1, backboneReturn: true }),
      edge(mark(1, 100, 0), mark(2, 500, 0), { parentIndex: 2, backboneReturn: true }),
      edge(mark(1, 100, -54), mark(2, 105, -108)),
    ];
    for (const actual of cases) {
      const curve = parentCurve(actual)!, bounds = curveBounds(curve);
      for (let step = 0; step <= 100; step++) {
        const sample = point(curve, step / 100), epsilon = 1e-9;
        expect(sample.x).toBeGreaterThanOrEqual(bounds.minX - epsilon); expect(sample.x).toBeLessThanOrEqual(bounds.maxX + epsilon);
        expect(sample.y).toBeGreaterThanOrEqual(bounds.minY - epsilon); expect(sample.y).toBeLessThanOrEqual(bounds.maxY + epsilon);
      }
    }
    expect(curveBounds(parentCurve(cases[1])!).minY).toBe(-28);
    expect(curveBounds(parentCurve(cases[2])!)).toEqual({ minX: 85, minY: -108, maxX: 120, maxY: -54 });
  });

  it("has deterministic world geometry without camera/scale inputs or source mutation", () => {
    const actual = edge(mark(1, 112, -54), mark(2, 448, 0), { parentIndex: 1, backboneReturn: true });
    const before = structuredClone(actual);
    Object.freeze(actual.parent); Object.freeze(actual.child); Object.freeze(actual);
    expect(parentCurve(actual)).toEqual(parentCurve(actual));
    expect(parentCurve(actual)).toEqual({ from: { x: 112, y: -54 }, control1: { x: 280, y: -54 }, control2: { x: 448, y: -28 }, to: { x: 448, y: 0 } });
    expect(actual).toEqual(before);
  });
});
