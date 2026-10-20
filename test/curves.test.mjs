// Node test for curve geometry: node test/curves.test.mjs
import { Curve, arcThroughPoints } from '../src/js/core/curves.js';

let fails = 0;
const ok = (cond, msg) => { if (!cond) { fails++; console.log('FAIL', msg); } else console.log('ok  ', msg); };
const near = (a, b, e = 1e-6) => Math.abs(a - b) <= e;

// ---------------- three point circular arc (points on a circle of radius 3 in the XZ plane)
const R = 3;
const A = [R, 0, 0], B = [0, 0, R], C = [R * Math.SQRT1_2, 0, R * Math.SQRT1_2]; // 0, 90 and 45 degrees
{
  const pts = arcThroughPoints(A, B, C);
  ok(!!pts && pts.length > 2, 'quarter arc: returns sampled points');
  ok(pts[0][0] === A[0] && pts[0][1] === A[1] && pts[0][2] === A[2], 'quarter arc: starts at the start point');
  const last = pts[pts.length - 1];
  ok(last[0] === B[0] && last[1] === B[1] && last[2] === B[2], 'quarter arc: ends at the end point');
  ok(pts.every(p => near(Math.hypot(p[0], p[2]), R, 1e-9)), 'quarter arc: every point lies on the circumcircle');
  const ang = (p) => Math.atan2(-p[2], p[0]); // 0 at A, -90 degrees at B
  ok(pts.every(p => ang(p) <= 1e-9 && ang(p) >= -Math.PI / 2 - 1e-9), 'quarter arc: follows the minor arc through the third point');
  ok(pts.some(p => near(p[0], C[0], 1e-9) && near(p[2], C[2], 1e-9)), 'quarter arc: passes through the third point');
  const c = new Curve(pts, 3, 'open', 'ep');
  ok(near(c.length(), R * Math.PI / 2, R * Math.PI / 200), 'quarter arc: curve length matches the arc length');
}
// third point on the major arc: the arc must still pass through it
{
  const pts = arcThroughPoints(A, B, [-R, 0, 0]); // 180 degrees
  ok(!!pts && pts.every(p => near(Math.hypot(p[0], p[2]), R, 1e-9)), 'major arc: every point lies on the circumcircle');
  ok(pts.some(p => p[0] < 0 && near(p[2], 0, 1e-9)), 'major arc: passes through the third point');
  ok(near(new Curve(pts, 3, 'open', 'ep').length(), R * 3 * Math.PI / 2, 0.1), 'major arc: curve length matches the arc length');
}
// a circle in an arbitrary plane: the points stay on the plane through the three clicked points
{
  const r = 2, s = Math.SQRT1_2, u = [s, s, 0], w = [0, 0, 1];
  const pt = (a) => { const ca = Math.cos(a), sa = Math.sin(a); return [r * (ca * u[0] + sa * w[0]), r * (ca * u[1] + sa * w[1]), r * (ca * u[2] + sa * w[2])]; };
  const a = pt(0), b = pt(Math.PI / 2), c = pt(Math.PI / 4);
  const pts = arcThroughPoints(a, b, c);
  ok(!!pts && pts.every(p => near(Math.hypot(p[0], p[1], p[2]), r, 1e-9)), '3D arc: every point lies on the sphere');
  ok(pts.every(p => near(p[0] - p[1], 0, 1e-9)), '3D arc: every point stays in the plane of the three points');
  ok(pts[0][0] === a[0] && pts[0][1] === a[1] && pts[0][2] === a[2], '3D arc: starts at the start point');
  ok(pts[pts.length - 1][2] === b[2], '3D arc: ends at the end point');
}
// degenerate cases
ok(arcThroughPoints([0, 0, 0], [2, 0, 0], [1, 0, 0]) === null, 'collinear points return null');
ok(arcThroughPoints([0, 0, 0], [0, 0, 0], [1, 0, 0]) === null, 'coincident start and end return null');

console.log(fails ? `\n${fails} FAILED` : '\nall passed');
process.exit(fails ? 1 : 0);
