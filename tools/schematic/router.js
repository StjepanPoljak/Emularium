// Part geometry and the wire router for tools/schematic.py's editor.
//
// Everything is in grid cells (10 px). A wire runs between cell centres,
// horizontally or vertically, like a PCB track:
// - two nets never share a track (same cell, same axis), so they can cross
//   but never overlap or touch at a corner;
// - a net is a tree: each pin joins the nearest part of the wires already
//   laid, with a dot where it branches;
// - parts and their pins are obstacles; a wire leaves a pin straight out.
// Each connection is an A* search over (cell, axis) with a cost for length,
// bends and crossings; nets that fail are routed first on the next pass.
"use strict";

const SCH = (() => {
  const STUB = 2;                   // pin length, cells
  const BEND = 5, CROSS = 6, HUG = 1;

  // ---- geometry ---------------------------------------------------------

  // Rows above the first pin: room for the reference, model and parameters.
  function headRows(part) {
    const lines = 2 + Math.min(shownParams(part).length, 3);
    const px = 10 + lines * 13;
    return Math.max(4, 2 * Math.ceil(px / 20));
  }

  function shownParams(part) {
    return Object.entries(part.params || {})
      .filter(([k]) => !["keys", "scale", "vnc", "rotate"].includes(k))
      .map(([k, v]) => `${k} = ${String(v).split("/").pop()}`);
  }

  // Wide enough for the title and for the pin names on both sides.
  function bodyWidth(part, symbol) {
    const nameW = num => {
      if (num === null) return 0;
      const [name, alt] = symbol.pins[num];
      return name.length * 5.75 + (alt ? (alt.length + 1) * 5.15 : 0);
    };
    const side = list => Math.max(0, ...list.map(nameW));
    const pinsW = side(symbol.left) + side(symbol.right) + 10 + 16;
    const titleW = Math.max((part.model || "").length * 6.6,
                            ...shownParams(part).map(t => t.length * 5.45)) + 16;
    // Turned sideways, the names point inwards and the title sits between them.
    const sideways = ((part.rotate || 0) / 90) % 2 === 1;
    const lines = 2 + Math.min(shownParams(part).length, 3);
    const need = sideways ? pinsW + lines * 13 + 6 : Math.max(pinsW, titleW);
    const cells = Math.ceil(need / 10);
    return Math.max(symbol.width, cells + cells % 2);
  }

  // A symbol for a model the library doesn't know: one pin per name used.
  function fallbackSymbol(model, pinNames) {
    const pins = {}, left = [];
    pinNames.forEach((n, i) => { pins[String(i + 1)] = [n, "", "io"]; left.push(String(i + 1)); });
    return { kind: "other", package: "", width: 10, pins, left, right: [] };
  }

  // Maps local symbol coordinates (rotation 0, no mirror) to the board.
  function transformer(part, w, h) {
    const rot = ((part.rotate || 0) % 360 + 360) % 360, mirror = !!part.mirror;
    const W = rot % 180 ? h : w, H = rot % 180 ? w : h;
    const sideMap = { L: "L", R: "R", T: "T", B: "B" };
    if (mirror) { sideMap.L = "R"; sideMap.R = "L"; }
    const turn = { L: "T", T: "R", R: "B", B: "L" };
    for (let k = 0; k < rot / 90; k++) for (const s in sideMap) sideMap[s] = turn[sideMap[s]];
    function pt(x, y) {
      if (mirror) x = w - x;
      let cw = w, ch = h;
      for (let k = 0; k < rot / 90; k++) { [x, y] = [ch - y, x]; [cw, ch] = [ch, cw]; }
      return { x: part.x + x, y: part.y + y };
    }
    return { pt, side: s => sideMap[s], W, H, rot };
  }

  // Body rectangle, pins (with base on the body edge and tip at the wire
  // end) and where the title goes, for a placed part.
  function geometry(ref, part, symbol) {
    const n = Math.max(symbol.left.length, symbol.right.length, 1);
    const head = headRows(part);
    const w = bodyWidth(part, symbol), h = head + 2 * n;
    const T = transformer(part, w, h);
    const pins = [];
    for (const [sideName, list] of [["L", symbol.left], ["R", symbol.right]]) {
      list.forEach((num, i) => {
        if (num === null) return;
        const [name, alt, type] = symbol.pins[num];
        const y = head + 2 * i;
        const bx = sideName === "L" ? 0 : w, tx = sideName === "L" ? -STUB : w + STUB;
        pins.push({ ref, num, name, alt, type, side: T.side(sideName),
                    base: T.pt(bx, y), tip: T.pt(tx, y) });
      });
    }
    const title = T.rot % 180 ? T.pt(w / 2, h / 2) : T.pt(w / 2, 0);
    return { ref, part, symbol, rect: { x: part.x, y: part.y, w: T.W, h: T.H }, pins, title, rot: T.rot, head };
  }

  const OUT = { L: [-1, 0], R: [1, 0], T: [0, -1], B: [0, 1] };

  // Cells a power symbol takes, beyond the pin tip.
  function powerCells(pin, power) {
    const [dx, dy] = OUT[pin.side], cells = [];
    const len = power === "vcc" ? 5 : 3;
    for (let k = 1; k <= len; k++) {
      const x = pin.tip.x + dx * k, y = pin.tip.y + dy * k;
      cells.push([x, y]);
      if (k >= 2) cells.push([x + dy, y + dx], [x - dy, y - dx]);
    }
    return cells;
  }

  // ---- routing ----------------------------------------------------------

  // Binary heap of state ids keyed by f.
  class Heap {
    constructor() { this.ids = []; this.keys = []; }
    get size() { return this.ids.length; }
    push(id, key) {
      const a = this.ids, k = this.keys;
      let i = a.length; a.push(id); k.push(key);
      while (i > 0) {
        const p = (i - 1) >> 1;
        if (k[p] <= key) break;
        a[i] = a[p]; k[i] = k[p]; i = p;
      }
      a[i] = id; k[i] = key;
    }
    pop() {
      const a = this.ids, k = this.keys, top = a[0];
      const lastId = a.pop(), lastKey = k.pop();
      if (a.length) {
        let i = 0;
        for (;;) {
          let c = 2 * i + 1;
          if (c >= a.length) break;
          if (c + 1 < a.length && k[c + 1] < k[c]) c++;
          if (k[c] >= lastKey) break;
          a[i] = a[c]; k[i] = k[c]; i = c;
        }
        a[i] = lastId; k[i] = lastKey;
      }
      return top;
    }
  }

  // geoms: [geometry]; nets: [{name, power, pins: [[ref, pin]]}].
  // Returns {wires: [{net, segments: [[x1,y1,x2,y2]], dots: [[x,y]], ok}], unrouted}.
  function route(geoms, nets, opts = {}) {
    const margin = opts.margin ?? 14;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const g of geoms) {
      x0 = Math.min(x0, g.rect.x); y0 = Math.min(y0, g.rect.y);
      x1 = Math.max(x1, g.rect.x + g.rect.w); y1 = Math.max(y1, g.rect.y + g.rect.h);
    }
    x0 -= margin; y0 -= margin; x1 += margin; y1 += margin;
    const W = x1 - x0 + 1, H = y1 - y0 + 1, N = W * H;
    const idx = (x, y) => (y - y0) * W + (x - x0);
    const inside = (x, y) => x >= x0 && x <= x1 && y >= y0 && y <= y1;

    const block = new Uint8Array(N);      // 1: no horizontal moves, 2: no vertical
    const term = new Int32Array(N).fill(-1);   // pin tip: its net, -2 if none
    const soft = new Uint8Array(N);       // extra cost near parts

    // Which net each pin is on.
    const pinNet = new Map();
    nets.forEach((net, i) => net.pins.forEach(([r, p]) => pinNet.set(r + "." + p, i)));

    const setBlock = (x, y, b) => { if (inside(x, y)) block[idx(x, y)] |= b; };
    for (const g of geoms) {
      const { x, y, w, h } = g.rect;
      for (let yy = y - 1; yy <= y + h + 1; yy++)
        for (let xx = x - 1; xx <= x + w + 1; xx++) setBlock(xx, yy, 3);
      for (let yy = y - 2; yy <= y + h + 2; yy++) { setBlock(x - 2, yy, 2); setBlock(x + w + 2, yy, 2); }
      for (let xx = x - 2; xx <= x + w + 2; xx++) { setBlock(xx, y - 2, 1); setBlock(xx, y + h + 2, 1); }
      for (let yy = y - 3; yy <= y + h + 3; yy++)
        for (let xx = x - 3; xx <= x + w + 3; xx++) if (inside(xx, yy)) soft[idx(xx, yy)] = HUG;
      for (const p of g.pins) {
        if (!inside(p.tip.x, p.tip.y)) continue;
        const i = idx(p.tip.x, p.tip.y);
        const n = pinNet.get(g.ref + "." + p.name);
        const net = n === undefined ? null : nets[n];
        if (net && net.power) {
          term[i] = -2;
          for (const [cx, cy] of powerCells(p, net.power)) setBlock(cx, cy, 3);
        } else {
          // Keep other nets' wires off the front of a pin, further for a
          // pin with no wire, so that nothing seems to touch it.
          const own = n === undefined ? -2 : n;
          term[i] = own;
          const [dx, dy] = OUT[p.side];
          for (let k = 1; k <= (own === -2 ? 3 : 1); k++) {
            const x = p.tip.x + dx * k, y = p.tip.y + dy * k;
            if (inside(x, y) && term[idx(x, y)] === -1) term[idx(x, y)] = own;
          }
        }
      }
    }

    // Wires laid so far. A cell's horizontal and vertical tracks each
    // belong to at most one net; deg counts the edges on each track.
    const hNet = new Int32Array(N).fill(-1), vNet = new Int32Array(N).fill(-1);
    const degH = new Uint8Array(N), degV = new Uint8Array(N);
    const tipOn = new Int32Array(N).fill(-1);      // a pin tip joined to its net
    const netDeg = (c, n) => (hNet[c] === n ? degH[c] : 0) + (vNet[c] === n ? degV[c] : 0) + (tipOn[c] === n ? 1 : 0);

    const gCost = new Float32Array(2 * N), from = new Int32Array(2 * N);
    const stamp = new Int32Array(2 * N), closed = new Int32Array(2 * N);
    let epoch = 0, expanded = 0;
    const weight = opts.weight ?? 1;

    const tipOf = new Map();
    for (const g of geoms) for (const p of g.pins) tipOf.set(g.ref + "." + p.name, p);

    // A* from pin tip s to any cell of net's tree (tb: the tree's bounding
    // box, for the heuristic). Returns {path, cost} or null.
    function search(net, s, sAxis, tb) {
      epoch++;
      const heap = new Heap();
      const hcost = c => {
        const x = c % W + x0, y = (c / W | 0) + y0;
        const dx = x < tb[0] ? tb[0] - x : x > tb[2] ? x - tb[2] : 0;
        const dy = y < tb[1] ? tb[1] - y : y > tb[3] ? y - tb[3] : 0;
        return weight * (dx + dy + (dx && dy ? BEND : 0));
      };
      const st = 2 * s + sAxis;
      stamp[st] = epoch; gCost[st] = 0; from[st] = -1;
      heap.push(st, hcost(s));
      let budget = 3 * N;
      while (heap.size && budget-- > 0) {
        const cur = heap.pop();
        if (closed[cur] === epoch) continue;
        closed[cur] = epoch;
        expanded++;
        const c = cur >> 1, axis = cur & 1;
        if (c !== s) {
          const d = netDeg(c, net);
          if (d >= 1 && d < 3) {
            const path = [];
            for (let k = cur; k !== -1; k = from[k]) path.push(k >> 1);
            return { path: path.reverse(), cost: gCost[cur] };
          }
        }
        const cx = c % W, cy = c / W | 0;
        for (let d = 0; d < 4; d++) {
          const nAxis = d < 2 ? 0 : 1;
          const nx = cx + (d === 0 ? 1 : d === 1 ? -1 : 0), ny = cy + (d === 2 ? 1 : d === 3 ? -1 : 0);
          if (nx < 0 || ny < 0 || nx >= W || ny >= H) continue;
          const nc = ny * W + nx;
          if (c === s && nAxis !== sAxis) continue;            // leave the pin straight out
          const bit = nAxis ? 2 : 1;
          if (block[nc] & bit || block[c] & bit) continue;
          if (term[nc] !== -1 && term[nc] !== net) continue;
          const occ = nAxis ? vNet : hNet;
          if (occ[c] !== -1 && occ[c] !== net) continue;
          if (occ[nc] !== -1 && occ[nc] !== net) continue;
          let step = 1 + soft[nc];
          if (nAxis !== axis && c !== s) step += BEND;
          const other = nAxis ? hNet : vNet;
          if (other[nc] !== -1 && other[nc] !== net) step += CROSS;
          const ns = 2 * nc + nAxis, gNew = gCost[cur] + step;
          if (stamp[ns] === epoch && gCost[ns] <= gNew) continue;
          stamp[ns] = epoch; gCost[ns] = gNew; from[ns] = cur;
          heap.push(ns, gNew * 0.999 + hcost(nc));     // ties: the deeper first
        }
      }
      return null;
    }

    // Per net: its edges [a, b] (a < b), the tips it has joined, its cost.
    const netEdges = nets.map(() => []), netTips = nets.map(() => []);
    const netCost = new Float64Array(nets.length), netOk = nets.map(() => true);
    const edgeKeys = nets.map(() => new Set());

    function addEdge(n, a, b) {
      if (a > b) [a, b] = [b, a];
      const key = a * N + b;
      if (edgeKeys[n].has(key)) return;
      edgeKeys[n].add(key);
      if (b - a === 1) { hNet[a] = hNet[b] = n; degH[a]++; degH[b]++; }
      else { vNet[a] = vNet[b] = n; degV[a]++; degV[b]++; }
      netEdges[n].push([a, b]);
    }
    function addTip(n, c) { tipOn[c] = n; netTips[n].push(c); }

    function ripUp(n) {
      for (const [a, b] of netEdges[n]) {
        if (b - a === 1) {
          for (const c of [a, b]) if (--degH[c] === 0) hNet[c] = -1;
        } else {
          for (const c of [a, b]) if (--degV[c] === 0) vNet[c] = -1;
        }
      }
      for (const c of netTips[n]) tipOn[c] = -1;
      const old = { edges: netEdges[n], tips: netTips[n], cost: netCost[n], ok: netOk[n] };
      netEdges[n] = []; netTips[n] = []; edgeKeys[n] = new Set(); netCost[n] = 0;
      return old;
    }
    function restore(n, old) {
      for (const c of old.tips) addTip(n, c);
      for (const [a, b] of old.edges) addEdge(n, a, b);
      netCost[n] = old.cost; netOk[n] = old.ok;
    }

    // Joins a net's pins one at a time, nearest first, each to the wires
    // already laid. Returns false if a pin couldn't be reached.
    function routeNet(n) {
      const tips = nets[n].pins.map(([r, p]) => tipOf.get(r + "." + p)).filter(p => p && inside(p.tip.x, p.tip.y));
      netOk[n] = true;
      if (tips.length < 2) return true;
      // Start from a microcontroller pin when there is one.
      const first = Math.max(0, tips.findIndex(p => opts.kindOf && opts.kindOf(p.ref) === "mcu"));
      const done = [tips[first]], todo = tips.filter((_, i) => i !== first);
      addTip(n, idx(done[0].tip.x, done[0].tip.y));
      const bb = [done[0].tip.x, done[0].tip.y, done[0].tip.x, done[0].tip.y];
      while (todo.length) {
        let best = 0, bestD = Infinity;
        todo.forEach((p, i) => {
          for (const q of done) {
            const d = Math.abs(p.tip.x - q.tip.x) + Math.abs(p.tip.y - q.tip.y);
            if (d < bestD) { bestD = d; best = i; }
          }
        });
        const p = todo.splice(best, 1)[0];
        const s = idx(p.tip.x, p.tip.y);
        const found = search(n, s, p.side === "L" || p.side === "R" ? 0 : 1, bb);
        if (!found) { netOk[n] = false; continue; }
        addTip(n, s);
        for (let k = 0; k + 1 < found.path.length; k++) addEdge(n, found.path[k], found.path[k + 1]);
        netCost[n] += found.cost;
        for (const c of found.path) {
          const x = c % W + x0, y = (c / W | 0) + y0;
          bb[0] = Math.min(bb[0], x); bb[1] = Math.min(bb[1], y);
          bb[2] = Math.max(bb[2], x); bb[3] = Math.max(bb[3], y);
        }
        done.push(p);
      }
      return netOk[n];
    }

    const signal = nets.map((n, i) => i).filter(i => !nets[i].power && nets[i].pins.length > 1);
    const span = i => {
      const pts = nets[i].pins.map(([r, p]) => tipOf.get(r + "." + p)).filter(Boolean);
      const xs = pts.map(p => p.tip.x), ys = pts.map(p => p.tip.y);
      return Math.max(...xs) - Math.min(...xs) + Math.max(...ys) - Math.min(...ys) + pts.length * 4;
    };
    const base = signal.slice().sort((a, b) => span(a) - span(b) || a - b);

    // First passes: route everything; if some nets fail, start over with
    // those first.
    const passes = opts.passes ?? 3;
    function routeAll(start) {
      let order = start;
      for (let pass = 0; pass < passes; pass++) {
        for (const n of signal) ripUp(n);
        const failed = order.filter(n => !routeNet(n));
        if (!failed.length) break;
        order = failed.concat(order.filter(n => !failed.includes(n)));
      }
      let total = 0;
      for (const n of signal) total += netOk[n] ? netCost[n] : 1000;
      return { order, total };
    }
    // Neighbouring wires of one kind (a bus such as LCD_DB0..7, or KEY_D0..3
    // with KEY_DA) nest without crossing or cross each other depending on
    // which goes first, so also try each family together, in the order of
    // their pins and the reverse.
    const family = n => (nets[n].bus || nets[n].name).split("_")[0];
    const pinPos = n => {
      const tips = nets[n].pins.map(([r, p]) => tipOf.get(r + "." + p)).filter(Boolean);
      const t = tips.find(p => opts.kindOf && opts.kindOf(p.ref) === "mcu") || tips[0];
      return t ? t.tip.y * 1e4 + t.tip.x : 0;
    };
    const families = new Map();
    for (const n of base) {
      const f = family(n);
      if (!families.has(f)) families.set(f, []);
      families.get(f).push(n);
    }
    // One family at a time: gather it where its first net was, in pin
    // order or reversed; keep the change if all wires cost less.
    const totals = [];
    let best = { ...routeAll(base), start: base }, last = best.start;
    totals.push(best.total.toFixed(0));
    if ((opts.improve ?? 1) > 0) {
      for (const [f, members] of families) {
        if (members.length < 2) continue;
        for (const reverse of [false, true]) {
          const sorted = members.slice().sort((x, y) => pinPos(x) - pinPos(y));
          if (reverse) sorted.reverse();
          const o = [];
          for (const n of best.start) {
            if (family(n) !== f) o.push(n);
            else if (n === best.start.find(m => family(m) === f)) o.push(...sorted);
          }
          const r = routeAll(o);
          last = o;
          totals.push(f + (reverse ? "<" : ">") + r.total.toFixed(0));
          if (r.total < best.total - 1e-6) best = { ...r, start: o };
        }
      }
    }
    if (last !== best.start) routeAll(best.start);
    const order = best.order;

    // Then rip up each net and route it again around all the others; keep
    // the new route when it's cheaper.
    const improve = opts.improve ?? 1;
    for (let it = 0; it < improve; it++) {
      let better = false;
      for (const n of order) {
        const old = ripUp(n);
        if (routeNet(n) && (!old.ok || netCost[n] < old.cost - 1e-6)) { better = true; continue; }
        ripUp(n);
        restore(n, old);
      }
      if (!better) break;
    }

    // Straight runs and branch dots, per net.
    const wires = [];
    const failed = signal.filter(n => !netOk[n]);
    for (const n of signal) {
      const segs = [], dots = [];
      const hor = netEdges[n].filter(([a, b]) => b - a === 1), ver = netEdges[n].filter(([a, b]) => b - a !== 1);
      for (const [list, stepC] of [[hor, 1], [ver, W]]) {
        const starts = new Set(list.map(e => e[0])), ends = new Set(list.map(e => e[1]));
        for (const [a] of list) {
          if (ends.has(a)) continue;                     // not the start of a run
          let b = a;
          while (starts.has(b)) b += stepC;
          segs.push([a % W + x0, (a / W | 0) + y0, b % W + x0, (b / W | 0) + y0]);
        }
      }
      const cells = new Set(netEdges[n].flat());
      for (const c of cells) if (netDeg(c, n) >= 3) dots.push([c % W + x0, (c / W | 0) + y0]);
      wires.push({ net: n, segs, dots, ok: netOk[n] });
    }
    // Pins that couldn't be reached: straight lines between the net's pins.
    const unrouted = failed.map(n => ({
      net: n, pts: nets[n].pins.map(([r, p]) => tipOf.get(r + "." + p)).filter(Boolean).map(p => p.tip) }));
    let cost = 0;
    for (const n of signal) cost += netCost[n];
    cost += 1000 * failed.length;
    const netCosts = {};
    for (const n of signal) netCosts[nets[n].name] = +netCost[n].toFixed(1);
    return { wires, unrouted, cost, expanded, netCosts, order: best.order.map(n => nets[n].name), totals };
  }


  // Checks a routing result: every routed net connects all its pins, no two
  // nets share a track, nets only cross straight through each other, and
  // no wire touches another net's pin. Returns a list of problems.
  function check(geoms, nets, result) {
    const bad = [];
    const tip = new Map(), tipNet = new Map();
    const pinNet = new Map();
    nets.forEach((n, i) => n.pins.forEach(([r, p]) => pinNet.set(r + "." + p, i)));
    for (const g of geoms) for (const p of g.pins) {
      tip.set(g.ref + "." + p.name, p.tip);
      const n = pinNet.get(g.ref + "." + p.name);
      tipNet.set(p.tip.x + "," + p.tip.y, n === undefined ? -1 : n);
    }
    const at = new Map();          // "x,y" -> Map(net -> Set of directions)
    const mark = (x, y, n, d) => {
      const k = x + "," + y;
      if (!at.has(k)) at.set(k, new Map());
      const m = at.get(k);
      if (!m.has(n)) m.set(n, new Set());
      m.get(n).add(d);
    };
    for (const w of result.wires) {
      const parent = new Map();
      const find = k => { while (parent.get(k) !== k) { parent.set(k, parent.get(parent.get(k))); k = parent.get(k); } return k; };
      const add = k => { if (!parent.has(k)) parent.set(k, k); };
      for (const [x1, y1, x2, y2] of w.segs) {
        if (x1 !== x2 && y1 !== y2) bad.push(`${nets[w.net].name}: diagonal segment`);
        const dx = Math.sign(x2 - x1), dy = Math.sign(y2 - y1);
        for (let x = x1, y = y1; x !== x2 || y !== y2; x += dx, y += dy) {
          const a = x + "," + y, b = (x + dx) + "," + (y + dy);
          add(a); add(b); parent.set(find(a), find(b));
          mark(x, y, w.net, dx > 0 ? "E" : dx < 0 ? "W" : dy > 0 ? "S" : "N");
          mark(x + dx, y + dy, w.net, dx > 0 ? "W" : dx < 0 ? "E" : dy > 0 ? "N" : "S");
        }
      }
      if (!w.ok) continue;
      const pins = nets[w.net].pins.map(([r, p]) => tip.get(r + "." + p)).filter(Boolean).map(t => t.x + "," + t.y);
      const roots = new Set(pins.map(k => parent.has(k) ? find(k) : "missing:" + k));
      if (roots.size !== 1) bad.push(`${nets[w.net].name}: pins not all connected`);
    }
    for (const [k, m] of at) {
      const t = tipNet.get(k);
      for (const n of m.keys()) if (t !== undefined && t !== n) bad.push(`${nets[n].name} touches a pin at ${k}`);
      if (m.size < 2) continue;
      const kinds = [...m.values()].map(d => [...d].sort().join(""));
      if (m.size > 2 || !(kinds.includes("EW") && kinds.includes("NS")))
        bad.push(`${[...m.keys()].map(n => nets[n].name).join(" & ")} meet at ${k} (${kinds.join(" ")})`);
    }
    return bad;
  }

  return { STUB, check, geometry, bodyWidth, fallbackSymbol, shownParams, powerCells, route, OUT };
})();

if (typeof module !== "undefined") module.exports = SCH;
