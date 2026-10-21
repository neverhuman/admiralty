import type { GraphBounds, GraphEdge } from "./model";

export type ParentCurve = {
  from: { x: number; y: number };
  control1: { x: number; y: number };
  control2: { x: number; y: number };
  to: { x: number; y: number };
};

/** Route one actual loaded parent edge in world coordinates. Explicit secondary
 * parent returns enter the selected backbone from above; its first parent stays
 * on the ordinary horizontal spine. No missing ancestry or range-alias loop is
 * turned into a connector.
 */
export function parentCurve(edge: GraphEdge): ParentCurve | null {
  const parent = edge.parent, child = edge.child;
  if (!parent || parent === child) return null;
  const bend = Math.max(20, (child.x - parent.x) / 2);
  return {
    from: { x: parent.x, y: parent.y },
    control1: { x: parent.x + bend, y: parent.y },
    control2: edge.backboneReturn === true && edge.parentIndex > 0
      ? { x: child.x, y: child.y - 28 }
      : { x: child.x - bend, y: child.y },
    to: { x: child.x, y: child.y },
  };
}

/** The cubic remains inside the conservative hull of its four control points. */
export function curveBounds(curve: ParentCurve): GraphBounds {
  return {
    minX: Math.min(curve.from.x, curve.control1.x, curve.control2.x, curve.to.x),
    minY: Math.min(curve.from.y, curve.control1.y, curve.control2.y, curve.to.y),
    maxX: Math.max(curve.from.x, curve.control1.x, curve.control2.x, curve.to.x),
    maxY: Math.max(curve.from.y, curve.control1.y, curve.control2.y, curve.to.y),
  };
}
