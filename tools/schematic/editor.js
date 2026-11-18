// The schematic editor: draws the board, lets parts be dragged, rotated and
// mirrored, re-routes the wires as they move, and saves the layout and SVG.
"use strict";

(() => {
  const G = 10;                                   // px per grid cell
  const BOARD = window.BOARD;
  const NS = "http://www.w3.org/2000/svg";
  const params = new URLSearchParams(location.search);
  const EXPORT = params.has("export");

  const C = {
    paper: "#fbfaf6", ink: "#1f2328", body: "#5b2a2a", sub: "#6b645c", faint: "#a39d94",
    stub: "#3a3632", unused: "#c4beb3", vcc: "#c0392b", gnd: "#3b3b45",
    fill: { mcu: "#e9f0fa", display: "#e8f4ea", memory: "#faf2e1", input: "#f0ebf9", other: "#f0efec" },
    nets: ["#2563eb", "#d97706", "#059669", "#db2777", "#7c3aed", "#0891b2",
           "#65a30d", "#ea580c", "#4f46e5", "#0d9488", "#a21caf", "#b45309"],
    // Colours (indices into nets) for common kinds of signals.
    known: { SPI: 0, LCD: 4, KEY: 1, SER: 3, UART: 3, EE: 7, RAM: 2, I2C: 5, SDA: 5, SCL: 5, CS: 7 },
  };
  const SANS = "Inter, 'Segoe UI', system-ui, -apple-system, 'Helvetica Neue', Arial, sans-serif";
  const MONO = "'JetBrains Mono', 'SF Mono', 'Cascadia Mono', 'DejaVu Sans Mono', Menlo, Consolas, monospace";

  // ---- model ------------------------------------------------------------

  const parts = BOARD.parts;
  let nets = BOARD.nets;
  const refs = Object.keys(parts);
  const symbols = {};
  for (const ref of refs) {
    const model = parts[ref].model;
    if (BOARD.symbols[model]) { symbols[ref] = BOARD.symbols[model]; continue; }
    const used = [];
    for (const n of nets) for (const [r, p] of n.pins) if (r === ref && !used.includes(p)) used.push(p);
    symbols[ref] = SCH.fallbackSymbol(model, used);
  }
  const kindOf = ref => symbols[ref].kind;

  // As tools/schematic.py and the emulator tell power nets.
  const powerOf = name => /^(GND.*|VSS|0V)$/.test(name) ? "gnd" : /^(\+.*|VCC|VDD)$/.test(name) ? "vcc" : null;

  // Colours: one per family of nets (SPI_*, LCD_*, KEY_*, ...), picked by
  // the family's name so that it's the same on every board; the next free
  // one if two families of a board would get the same. A family keeps its
  // colour when nets are edited.
  const family = n => (n.bus || n.name).split("_")[0].replace(/\d+(\.\.\d+)?$/, "") || n.name;
  const hash = t => [...t].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7);
  const famColor = new Map(), taken = new Set();
  function colorOf(n) {
    if (n.power === "vcc") return C.vcc;
    if (n.power === "gnd") return C.gnd;
    const f = family(n);
    if (!famColor.has(f)) {
      let i = C.known[f] ?? hash(f) % C.nets.length;
      for (let k = 0; k < C.nets.length && taken.has(i); k++) i = (i + 1) % C.nets.length;
      taken.add(i);
      famColor.set(f, C.nets[i]);
    }
    return famColor.get(f);
  }

  // What follows from the nets; again after every edit of them.
  let netColor, pinNet, problems;
  function netsChanged() {
    netColor = nets.map(colorOf);
    pinNet = new Map();
    nets.forEach((n, i) => n.pins.forEach(([r, p]) => pinNet.set(r + "." + p, i)));
    // Pins named in nets that the symbol doesn't have: say so instead of
    // quietly dropping them.
    problems = [];
    for (const n of nets) for (const [r, p] of n.pins) {
      if (!parts[r]) problems.push(`${n.name}: no part ${r}`);
      else if (!Object.values(symbols[r].pins).some(v => v[0] === p)) problems.push(`${n.name}: ${r} has no pin ${p}`);
    }
  }
  netsChanged();
  const netsText = () => JSON.stringify(nets.map(n => [n.name, n.pins]));
  let savedNets = netsText();

  let layout = {};
  for (const ref of refs) {
    const l = BOARD.layout[ref];
    if (l) layout[ref] = { x: l.x | 0, y: l.y | 0, rotate: l.rotate | 0, mirror: !!l.mirror };
  }

  function geoms() {
    return refs.map(ref => SCH.geometry(ref, { ...parts[ref], ...layout[ref] }, symbols[ref]));
  }

  // ---- automatic placement ----------------------------------------------

  // Tries a few arrangements and keeps the one whose wires cost least
  // (length, bends, crossings) for the area it takes.
  function autoPlaceBest(which) {
    const keep = JSON.stringify(layout);
    let best = null;
    for (const limit of [1, 1.4, 2, 3, 99])
      for (const gap of [0, 8])
        for (const flip of [false, true]) {
          layout = JSON.parse(keep);
          if (!autoPlace(which, { limit, gap, flip }) && flip) continue;
          const gs = geoms();
          const r = SCH.route(gs, nets, { kindOf, improve: 0, passes: 2 });
          const b = bbox(gs, r);
          const score = r.cost + 0.004 * (b.w / G) * (b.h / G);
          if (!best || score < best.score) best = { score, layout: JSON.stringify(layout) };
        }
    layout = JSON.parse(best.layout);
  }

  // Microcontrollers in the middle; every other part on the side of the
  // microcontroller its signals leave from, facing it, in the order of the
  // pins it uses. With flip, parts wired to both sides go to the other one.
  // Returns whether flip changed anything.
  function autoPlace(which, opt = { limit: 1.4, gap: 0, flip: false }) {
    let flipped = false;
    const mcus = refs.filter(r => kindOf(r) === "mcu");
    const centre = mcus[0] || refs[0];
    if (which.includes(centre)) layout[centre] = { x: 0, y: 0, rotate: 0, mirror: false };
    const cg = SCH.geometry(centre, { ...parts[centre], ...layout[centre] }, symbols[centre]);
    const cPin = new Map(cg.pins.map(p => [p.name, p]));
    const sides = { L: [], R: [] };
    for (const ref of refs) {
      if (ref === centre || !which.includes(ref)) continue;
      let l = 0, r = 0, ys = [], own = { L: 0, R: 0 };
      const g0 = SCH.geometry(ref, { ...parts[ref], x: 0, y: 0 }, symbols[ref]);
      for (const n of nets) {
        if (n.power) continue;
        const mine = n.pins.filter(([pr]) => pr === ref).map(([, p]) => g0.pins.find(q => q.name === p)).filter(Boolean);
        const theirs = n.pins.filter(([pr]) => pr === centre).map(([, p]) => cPin.get(p)).filter(Boolean);
        if (!mine.length || !theirs.length) continue;
        for (const t of theirs) { if (t.side === "L") l++; else r++; ys.push(t.tip.y); }
        for (const m of mine) own[m.side] = (own[m.side] || 0) + 1;
      }
      let side = l > r ? "L" : "R";
      if (opt.flip && l && r) { side = side === "L" ? "R" : "L"; flipped = true; }
      // Face the microcontroller: pins towards it.
      const mirror = side === "R" ? own.R > own.L : own.L > own.R;
      const y = ys.length ? ys.reduce((a, b) => a + b, 0) / ys.length : 1e9;
      sides[side].push({ ref, y, mirror, nets: l + r });
    }
    // Columns no taller than the microcontroller (or the tallest part),
    // each part level with the pins it uses where there's room.
    for (const s of ["L", "R"]) {
      const list = sides[s].sort((a, b) => a.y - b.y).map(p => {
        const g = SCH.geometry(p.ref, { ...parts[p.ref], x: 0, y: 0, mirror: p.mirror }, symbols[p.ref]);
        const ys = g.pins.filter(q => pinNet.has(p.ref + "." + q.name) && !nets[pinNet.get(p.ref + "." + q.name)].power)
          .map(q => q.tip.y);
        const mid = ys.length ? (Math.min(...ys) + Math.max(...ys)) / 2 : g.rect.h / 2;
        return { ...p, w: g.rect.w, h: g.rect.h, mid };
      });
      const limit = Math.max(cg.rect.h * opt.limit, ...list.map(p => p.h));
      const columns = [];
      let col = null;
      for (const p of list) {
        if (!col || col.h + 8 + p.h > limit) columns.push(col = { parts: [], h: -8, w: 0, nets: 0 });
        col.parts.push(p); col.h += 8 + p.h; col.w = Math.max(col.w, p.w); col.nets += p.nets;
      }
      // Room for the wires that pass between the columns.
      let left = s === "R" ? cg.rect.x + cg.rect.w : cg.rect.x;
      let passing = list.reduce((a, p) => a + p.nets, 0);
      for (const c of columns) {
        const gap = 12 + opt.gap + 2 * Math.min(14, Math.ceil(passing / 2));
        passing -= c.nets;
        const x = s === "R" ? left + gap : left - gap - c.w;
        let next = -Infinity;
        for (const p of c.parts) {
          let y = Math.round(p.y === 1e9 ? cg.rect.h / 2 : p.y - p.mid);
          y = Math.max(y, next);
          // Facing the microcontroller, flush with the column's near edge.
          const px0 = s === "R" ? x : x + c.w - p.w;
          layout[p.ref] = { x: px0, y, rotate: 0, mirror: p.mirror };
          next = y + p.h + 8;
        }
        left = s === "R" ? x + c.w : x;
      }
    }
    return flipped;
  }

  // ---- drawing ----------------------------------------------------------

  function el(name, attrs, parent, text) {
    const e = document.createElementNS(NS, name);
    for (const k in attrs) if (attrs[k] !== undefined && attrs[k] !== null) e.setAttribute(k, attrs[k]);
    if (text !== undefined) e.textContent = text;
    if (parent) parent.appendChild(e);
    return e;
  }

  const px = v => +(v * G).toFixed(2);

  function drawPart(g, root, interactive) {
    const grp = el("g", { class: "part", "data-ref": g.ref }, root);
    const r = g.rect;
    el("rect", { x: px(r.x), y: px(r.y), width: px(r.w), height: px(r.h), rx: 3,
                 fill: C.fill[g.symbol.kind] || C.fill.other, stroke: C.body, "stroke-width": 1.6,
                 class: "body" }, grp);
    if (interactive) el("rect", { x: px(r.x) - 4, y: px(r.y) - 4, width: px(r.w) + 8, height: px(r.h) + 8,
                                  rx: 6, class: "sel", fill: "none" }, grp);
    // Title: reference, model, parameters.
    const lines = [[g.ref, 12.5, 700, C.ink, SANS], [parts[g.ref].model, 11, 500, C.sub, SANS]];
    for (const p of SCH.shownParams(parts[g.ref]).slice(0, 3)) lines.push([p, 9, 400, C.faint, MONO]);
    const lh = 13, total = lines.length * lh;
    let ty;
    if (g.rot % 180) ty = px(r.y + r.h / 2) - total / 2 + 10;
    else if (g.rot === 180) ty = px(r.y + r.h - g.head) + 15;
    else ty = px(r.y) + 15;
    lines.forEach(([t, size, weight, fill, font], i) => {
      el("text", { x: px(r.x + r.w / 2), y: ty + i * lh, "text-anchor": "middle", "font-size": size,
                   "font-weight": weight, fill, "font-family": font }, grp, t);
    });
    for (const p of g.pins) drawPin(g, p, grp, interactive);
    return grp;
  }

  function drawPin(g, p, grp, interactive) {
    const key = g.ref + "." + p.name;
    const n = pinNet.get(key);
    const used = n !== undefined;
    const color = used ? netColor[n] : C.unused;
    const wireable = p.type === "io";
    const pg = el("g", { class: (used ? `pin net n${n}` : "pin") + (wireable ? " io" : ""),
                         "data-net": used ? n : null, "data-pin": key }, grp);
    el("title", {}, pg, `${key} (pin ${p.num})` + (used ? ` — ${nets[n].name}` : "") +
       (interactive && wireable ? "\nDrag from its end to connect; click, then Delete, to disconnect" : "") +
       (interactive && p.type === "hw" ? "\nNot modelled by the emulator" : ""));
    if (interactive) {
      // Something to grab: the stub and a little beyond its end.
      const x0 = Math.min(p.base.x, p.tip.x), x1 = Math.max(p.base.x, p.tip.x);
      const y0 = Math.min(p.base.y, p.tip.y), y1 = Math.max(p.base.y, p.tip.y);
      el("rect", { x: px(x0) - 5, y: px(y0) - 5, width: px(x1 - x0) + 10, height: px(y1 - y0) + 10,
                   fill: "transparent", class: "grab" }, pg);
      if (key === pickedPin) el("circle", { cx: px(p.tip.x), cy: px(p.tip.y), r: 5.5, class: "ring" }, pg);
    }
    el("line", { x1: px(p.base.x), y1: px(p.base.y), x2: px(p.tip.x), y2: px(p.tip.y),
                 stroke: used ? color : C.unused, "stroke-width": used ? 2.2 : 1.4, "stroke-linecap": "round" }, pg);
    if (used) el("circle", { cx: px(p.tip.x), cy: px(p.tip.y), r: 1.6, fill: color }, pg);
    const [ox, oy] = SCH.OUT[p.side];
    const vertical = p.side === "T" || p.side === "B";
    const nameFill = used ? C.ink : p.type === "nc" ? C.unused : C.faint;
    // Name inside the body.
    const nx = px(p.base.x) - ox * 5, ny = px(p.base.y) - oy * 5;
    const anchor = p.side === "L" || p.side === "T" ? (vertical ? "end" : "start") : (vertical ? "start" : "end");
    const t = el("text", { x: nx, y: ny, "font-size": 9.5, "font-family": MONO, fill: nameFill,
                           "text-anchor": anchor, "dominant-baseline": "central",
                           transform: vertical ? `rotate(-90 ${nx} ${ny})` : null }, pg);
    el("tspan", {}, t, p.name);
    if (p.alt) el("tspan", { fill: C.unused, "font-size": 8.5 }, t, "/" + p.alt);
    // Number on the stub.
    const mx = px((p.base.x + p.tip.x) / 2) + (vertical ? -3.5 : 0), my = px((p.base.y + p.tip.y) / 2) + (vertical ? 0 : -3.5);
    el("text", { x: mx, y: my, "font-size": 7.5, "font-family": SANS, fill: C.faint, "text-anchor": "middle",
                 transform: vertical ? `rotate(-90 ${mx} ${my})` : null }, pg, p.num);
    if (used && nets[n].power) drawPower(p, nets[n], n, pg);
  }

  function drawPower(p, net, n, pg) {
    const [ox, oy] = SCH.OUT[p.side];
    const color = netColor[n];
    const at = k => [px(p.tip.x + ox * k), px(p.tip.y + oy * k)];
    const [ax, ay] = at(0), [bx, by] = at(2);
    el("line", { x1: ax, y1: ay, x2: bx, y2: by, stroke: color, "stroke-width": 2.2, "stroke-linecap": "round" }, pg);
    const bar = (k, half, w) => {
      const [cx, cy] = at(k);
      el("line", { x1: cx - oy * half, y1: cy - ox * half, x2: cx + oy * half, y2: cy + ox * half,
                   stroke: color, "stroke-width": w, "stroke-linecap": "round" }, pg);
    };
    if (net.power === "gnd") {
      bar(2, 8, 2); bar(2.45, 5, 1.8); bar(2.9, 2, 1.6);
    } else {
      bar(2, 7, 2.2);
      const [lx, ly] = at(2.6);
      const anchor = ox < 0 ? "end" : ox > 0 ? "start" : "middle";
      el("text", { x: lx, y: ly + (oy ? oy * 4 : 0), "font-size": 9.5, "font-weight": 600, "font-family": SANS,
                   fill: color, "text-anchor": anchor, "dominant-baseline": "central" }, pg, net.name);
    }
  }

  function drawWires(result, root, showLabels, gs) {
    const grp = el("g", { class: "wires" }, root);
    for (const w of result.wires) {
      const color = netColor[w.net];
      const ng = el("g", { class: `net n${w.net}`, "data-net": w.net }, grp);
      el("title", {}, ng, nets[w.net].name);
      const d = w.segs.map(([a, b, c, e]) => `M${px(a)} ${px(b)}H${px(c)}V${px(e)}`).join("");
      el("path", { d, stroke: "transparent", "stroke-width": 9, fill: "none", class: "hit" }, ng);
      el("path", { d, stroke: color, "stroke-width": 2.2, fill: "none", "stroke-linecap": "round",
                   "stroke-linejoin": "round" }, ng);
      for (const [x, y] of w.dots) el("circle", { cx: px(x), cy: px(y), r: 3.4, fill: color }, ng);
    }
    for (const u of result.unrouted) {
      const ng = el("g", { class: `net n${u.net} unrouted`, "data-net": u.net }, grp);
      el("title", {}, ng, nets[u.net].name + " (not routed)");
      for (let i = 1; i < u.pts.length; i++)
        el("line", { x1: px(u.pts[0].x), y1: px(u.pts[0].y), x2: px(u.pts[i].x), y2: px(u.pts[i].y),
                     stroke: "#dc2626", "stroke-width": 1.5, "stroke-dasharray": "5 4" }, ng);
    }
    if (showLabels) drawLabels(result, grp, gs);
    return grp;
  }

  // Net names, each above its net's longest horizontal run where the text
  // touches no other wire, part or name; nets with no such run go without.
  function drawLabels(result, grp, gs) {
    const CH = 4.85, PAD = 6;                      // px per character at 8 px
    const boxes = [];                              // [x0, y0, x1, y1] taken
    for (const g of gs) {
      const r = g.rect;
      boxes.push([px(r.x - 2) - 2, px(r.y - 2) - 2, px(r.x + r.w + 2) + 2, px(r.y + r.h + 2) + 2]);
    }
    const segs = [];
    for (const w of result.wires) {
      for (const s of w.segs) segs.push([w.net, ...s.map(px)]);
      for (const [x, y] of w.dots) boxes.push([px(x) - 4, px(y) - 4, px(x) + 4, px(y) + 4]);
    }
    const hits = (b, net) =>
      boxes.some(o => b[0] < o[2] && o[0] < b[2] && b[1] < o[3] && o[1] < b[3]) ||
      segs.some(([n, x1, y1, x2, y2]) => {
        const sx0 = Math.min(x1, x2), sx1 = Math.max(x1, x2), sy0 = Math.min(y1, y2), sy1 = Math.max(y1, y2);
        if (n === net && sy0 === sy1 && sy0 === b[3] + 2) return false;   // its own run, under it
        return sx0 <= b[2] && b[0] <= sx1 && sy0 <= b[3] && b[1] <= sy1;
      });
    // Longest nets first, so short ones fill the gaps.
    const order = result.wires.slice().sort((a, b) => {
      const len = w => w.segs.reduce((t, [x1, y1, x2, y2]) => t + Math.abs(x2 - x1) + Math.abs(y2 - y1), 0);
      return len(b) - len(a);
    });
    for (const w of order) {
      const name = nets[w.net].name, tw = name.length * CH;
      const runs = w.segs.filter(([, y1, , y2]) => y1 === y2)
        .map(([x1, y, x2]) => [px(Math.min(x1, x2)), px(y), px(Math.max(x1, x2))])
        .filter(([a, , b]) => b - a >= tw + 2 * PAD)
        .sort((a, b) => (b[2] - b[0]) - (a[2] - a[0]));
      for (const [a, y, b] of runs) {
        // Near the start of the run, else further along.
        let placed = false;
        for (let x = a + PAD; x + tw + PAD <= b && !placed; x += 10) {
          const box = [x - 1, y - 11, x + tw + 1, y - 2];
          if (hits(box, w.net)) continue;
          boxes.push(box);
          el("text", { x, y: y - 3.5, "font-size": 8, "font-family": MONO, "font-weight": 600,
                       fill: netColor[w.net], class: `label net n${w.net}` }, grp, name);
          placed = true;
        }
        if (placed) break;
      }
    }
  }

  // ---- the page ---------------------------------------------------------

  const svg = document.getElementById("canvas");
  const world = document.getElementById("world");
  const status = document.getElementById("status");
  let view = null, selected = null, pickedPin = null, dirty = false, showLabels = true, result = null, gsCache = null;
  const undo = [];

  // fast: one quick pass while a part is being dragged.
  function draw(fast) {
    const t0 = performance.now();
    const gs = geoms();
    result = SCH.route(gs, nets, fast ? { kindOf, improve: 0, passes: 1, weight: 1.6 } : { kindOf });
    const ms = performance.now() - t0;
    gsCache = gs;
    world.replaceChildren();
    const partsG = el("g", { class: "parts" }, world);
    for (const g of gs) {
      const pg = drawPart(g, partsG, true);
      if (g.ref === selected) pg.classList.add("selected");
    }
    drawWires(result, world, showLabels, gs);
    const routed = result.wires.filter(w => w.ok).length, bad = result.unrouted.length;
    if (status) {
      status.textContent = `${refs.length} parts · ${nets.length} nets · ` +
        (bad ? `${bad} not routed` : "all routed") + ` · ${ms.toFixed(0)} ms, cost ${result.cost.toFixed(0)}, ${result.expanded} steps` +
        (problems.length ? ` · ${problems.length} unknown pins` : "");
      status.classList.toggle("warn", bad > 0 || problems.length > 0);
      status.title = problems.join("\n");
      if (params.has("debug")) status.textContent += ` · ` +
        result.order.map(n => `${n}=${result.netCosts[n]}`).join(" ") + " | " + result.totals.join(" | ");
      if (pickedPin) {
        const n = pinNet.get(pickedPin);
        status.textContent = `${pickedPin}: ` + (n === undefined ? "not connected · drag from its end to a pin or a wire"
          : `on ${nets[n].name} · Delete disconnects it, or drag from its end to connect it elsewhere`);
        status.classList.remove("warn");
      }
    }
    void routed;
    renderNetList();
    // The new drawing lit as the old one was.
    const lit = hl;
    hl = undefined;
    highlight(lit);
  }

  function bbox(gs, res) {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    const add = (x, y) => { x0 = Math.min(x0, x); y0 = Math.min(y0, y); x1 = Math.max(x1, x); y1 = Math.max(y1, y); };
    for (const g of gs) { add(g.rect.x - 6, g.rect.y - 2); add(g.rect.x + g.rect.w + 6, g.rect.y + g.rect.h + 2); }
    for (const w of res.wires) for (const [a, b, c, d] of w.segs) { add(a, b); add(c, d); }
    return { x: px(x0), y: px(y0), w: px(x1 - x0), h: px(y1 - y0) };
  }

  function fit() {
    const b = bbox(gsCache, result);
    const r = svg.getBoundingClientRect();
    const s = Math.max(b.w / r.width, b.h / r.height) * 1.08;
    view = { x: b.x + b.w / 2 - r.width * s / 2, y: b.y + b.h / 2 - r.height * s / 2, s };
    applyView();
  }

  function applyView() {
    const r = svg.getBoundingClientRect();
    svg.setAttribute("viewBox", `${view.x} ${view.y} ${r.width * view.s} ${r.height * view.s}`);
  }

  function toWorld(e) {
    const r = svg.getBoundingClientRect();
    return { x: view.x + (e.clientX - r.left) * view.s, y: view.y + (e.clientY - r.top) * view.s };
  }

  // A standalone SVG of the drawing, styles inline.
  function exportSVG() {
    const gs = geoms();
    const res = SCH.route(gs, nets, { kindOf });
    const b = bbox(gs, res);
    const pad = 24, head = 34;
    const out = document.createElementNS(NS, "svg");
    out.setAttribute("xmlns", NS);
    out.setAttribute("viewBox", `${b.x - pad} ${b.y - pad - head} ${b.w + 2 * pad} ${b.h + 2 * pad + head}`);
    out.setAttribute("width", b.w + 2 * pad);
    out.setAttribute("height", b.h + 2 * pad + head);
    el("rect", { x: b.x - pad, y: b.y - pad - head, width: b.w + 2 * pad, height: b.h + 2 * pad + head, fill: C.paper }, out);
    el("text", { x: b.x - pad + 18, y: b.y - pad - head + 30, "font-size": 17, "font-weight": 700,
                 "font-family": SANS, fill: C.ink }, out, BOARD.name);
    el("text", { x: b.x + b.w + pad - 18, y: b.y - pad - head + 30, "font-size": 10, "text-anchor": "end",
                 "font-family": SANS, fill: C.faint }, out, BOARD.file);
    const pg = el("g", {}, out);
    for (const g of gs) drawPart(g, pg, false);
    drawWires(res, out, showLabels, gs);
    for (const t of out.querySelectorAll("title")) t.remove();
    for (const h of out.querySelectorAll(".hit")) h.remove();
    for (const e of out.querySelectorAll("[class]")) e.removeAttribute("class");
    for (const e of out.querySelectorAll("[data-ref],[data-net]")) { e.removeAttribute("data-ref"); e.removeAttribute("data-net"); }
    return '<?xml version="1.0" encoding="UTF-8"?>\n' + new XMLSerializer().serializeToString(out) + "\n";
  }

  const missing = refs.filter(r => !layout[r]);
  if (missing.length) autoPlaceBest(missing);

  if (EXPORT) {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "/api/save", false);
    xhr.setRequestHeader("Content-Type", "application/json");
    // Without a layout file yet, keep the automatic one the drawing used.
    const fresh = Object.keys(BOARD.layout).length === 0;
    xhr.send(JSON.stringify({ svg: exportSVG(), export: true, layout: fresh ? layout : null }));
    return;
  }

  // ---- net list -----------------------------------------------------------

  const netList = document.getElementById("nets");
  let listed = null;
  function renderNetList() {
    if (!netList || listed === netsText()) return;
    listed = netsText();
    netList.replaceChildren();
    nets.forEach((n, i) => {
      const li = document.createElement("li");
      li.dataset.net = i;
      li.title = "Double-click to rename";
      const sw = document.createElement("span");
      sw.className = "sw" + (n.power ? " pw" : "");
      sw.style.background = netColor[i];
      const name = document.createElement("span");
      name.className = "nm";
      name.textContent = n.name;
      const pins = document.createElement("span");
      pins.className = "pp";
      pins.textContent = n.pins.map(p => p.join(".")).join(" ");
      const del = document.createElement("button");
      del.className = "del";
      del.title = "Delete this net";
      del.textContent = "×";
      li.append(sw, name, pins, del);
      netList.append(li);
    });
  }

  // ---- interaction --------------------------------------------------------

  let hl = null, pinned = null;
  function highlight(n) {
    if (hl === n) return;
    hl = n;
    svg.classList.toggle("hl", n !== null);
    for (const e of svg.querySelectorAll(".on")) e.classList.remove("on");
    for (const e of document.querySelectorAll("#nets li.on")) e.classList.remove("on");
    if (n !== null) {
      for (const e of svg.querySelectorAll(`.n${n}`)) e.classList.add("on");
      const li = netList && netList.querySelector(`li[data-net="${n}"]`);
      if (li) li.classList.add("on");
    }
  }
  const netAt = e => { const t = e.target.closest && e.target.closest("[data-net]"); return t ? +t.dataset.net : null; };

  function snapshot() { undo.push(JSON.stringify({ layout, nets })); if (undo.length > 200) undo.shift(); }
  function setDirty(d) { dirty = d; document.title = (d ? "• " : "") + BOARD.name + " — schematic"; }

  // ---- editing connections ------------------------------------------------

  const tipOf = key => gsCache.flatMap(g => g.pins.map(p => [g.ref + "." + p.name, p])).find(([k]) => k === key)?.[1];
  const split = key => [key.slice(0, key.indexOf(".")), key.slice(key.indexOf(".") + 1)];
  const NAME = /^[A-Za-z0-9_+-]+$/;

  // After an edit of nets: lone signal nets go (they connect nothing), the
  // drawing and the list follow.
  function editedNets(message) {
    nets = nets.filter(n => n.pins.length > (n.power ? 0 : 1));
    pinned = null;
    hl = null;
    netsChanged();
    setDirty(true);
    draw();
    if (message) toast(message + " · Ctrl+Z undoes");
  }

  // Joins pin key to net n.
  function joinNet(key, n) {
    snapshot();
    nets[n] = { ...nets[n], pins: [...nets[n].pins, split(key)] };
    editedNets(`${key} → ${nets[n].name}`);
  }

  // One net out of nets a and b, keeping the power net's name, else the
  // bigger one's (a's on a tie).
  function mergeNets(a, b) {
    const A = nets[a], B = nets[b];
    if (A.power && B.power && A.name !== B.name) {
      toast(`That would short ${A.name} to ${B.name}`, true);
      return;
    }
    const [keep, gone] = B.power && !A.power || (!A.power && B.pins.length > A.pins.length) ? [b, a] : [a, b];
    snapshot();
    nets[keep] = { ...nets[keep], pins: [...nets[keep].pins, ...nets[gone].pins] };
    const goneName = nets[gone].name;
    nets.splice(gone, 1);
    editedNets(`Joined ${goneName} into ${nets[keep > gone ? keep - 1 : keep].name}`);
  }

  // Connects pin key to pin other (both "REF.PIN").
  async function connectPins(key, other, at) {
    const a = pinNet.get(key), b = pinNet.get(other);
    if (a !== undefined && a === b) { toast(`Already both on ${nets[a].name}`); return; }
    if (a !== undefined && b !== undefined) return mergeNets(b, a);
    if (a !== undefined) return joinNet(other, a);
    if (b !== undefined) return joinNet(key, b);
    // A new net; the microcontroller's pin first, where the router starts.
    const pins = [split(key), split(other)].sort((p, q) => (kindOf(q[0]) === "mcu") - (kindOf(p[0]) === "mcu"));
    const name = await ask(at, { title: `Name the net from ${key} to ${other}`, value: suggestName(pins) });
    if (!name) return;
    const n = nets.findIndex(x => x.name === name);
    snapshot();
    if (n >= 0) nets[n] = { ...nets[n], pins: [...nets[n].pins, ...pins] };
    else nets.push({ name, bus: null, power: powerOf(name), pins });
    editedNets(n >= 0 ? `${key} and ${other} → ${name}` : `New net ${name}`);
  }

  // Connects pin key to a net named by hand: an existing one, or a power
  // rail (a signal net needs a second pin).
  async function connectByName(key, at) {
    const name = await ask(at, { title: `Connect ${key} to`, value: "", placeholder: "GND, +5V or a net's name" });
    if (!name) return;
    const n = nets.findIndex(x => x.name === name);
    const a = pinNet.get(key);
    if (n >= 0 && n === a) return;
    if (a !== undefined) { toast(`${key} is on ${nets[a].name}; disconnect it first (click it, Delete)`, true); return; }
    if (n >= 0) return joinNet(key, n);
    if (!powerOf(name)) { toast(`No net ${name}; to make one, drag to the pin it should go to`, true); return; }
    snapshot();
    nets.push({ name, bus: null, power: powerOf(name), pins: [split(key)] });
    editedNets(`${key} → ${name}`);
  }

  // KiCad-like: the other part's pin, e.g. U5_HOLD.
  function suggestName(pins) {
    const [r, p] = pins[pins.length - 1];
    let name = `${r}_${p}`.replace(/[^A-Za-z0-9_+-]/g, "_"), k = 2;
    while (nets.some(n => n.name === name)) name = `${r}_${p}_${k++}`;
    return name;
  }

  function disconnectPin(key) {
    const n = pinNet.get(key);
    if (n === undefined) { toast(`${key} isn't connected`); return; }
    snapshot();
    const net = nets[n];
    nets[n] = { ...net, pins: net.pins.filter(([r, p]) => r + "." + p !== key) };
    const left = nets[n].pins.length;
    editedNets(left > (net.power ? 0 : 1) ? `${key} off ${net.name}` : `${key} off ${net.name}; nothing else on it, so it's gone`);
  }

  function deleteNet(n) {
    snapshot();
    const name = nets[n].name;
    nets.splice(n, 1);
    editedNets(`Deleted ${name}`);
  }

  async function renameNet(n, at) {
    const old = nets[n].name;
    const name = await ask(at, { title: `Rename ${old}`, value: old });
    if (!name || name === old) return;
    const m = nets.findIndex(x => x.name === name);
    if (m >= 0) return mergeNets(m, n);
    snapshot();
    nets[n] = { ...nets[n], name, bus: null, power: powerOf(name) };
    editedNets(`${old} is now ${name}`);
  }

  // A small box at client point at that asks for a net's name; resolves
  // to the name, or null when cancelled.
  function ask(at, { title, value, placeholder }) {
    const box = document.getElementById("ask");
    const input = box.querySelector("input");
    box.querySelector("label").textContent = title;
    const list = document.getElementById("netnames");
    list.replaceChildren(...[...new Set([...nets.map(n => n.name), "GND", "+5V"])].map(v => {
      const o = document.createElement("option"); o.value = v; return o;
    }));
    input.value = value;
    input.placeholder = placeholder || "";
    box.hidden = false;
    const r = box.getBoundingClientRect();
    box.style.left = Math.max(8, Math.min(innerWidth - r.width - 8, at.x + 12)) + "px";
    box.style.top = Math.max(8, Math.min(innerHeight - r.height - 8, at.y + 12)) + "px";
    input.focus();
    input.select();
    return new Promise(resolve => {
      const done = v => {
        box.hidden = true;
        box.onsubmit = input.onkeydown = input.onblur = null;
        resolve(v);
      };
      box.onsubmit = e => {
        e.preventDefault();
        const v = input.value.trim();
        if (!NAME.test(v) || v.includes("..")) { toast("A net's name is letters, digits, _, + and -", true); return; }
        done(v);
      };
      input.onkeydown = e => { if (e.key === "Escape") { e.stopPropagation(); done(null); } };
      input.onblur = () => setTimeout(() => { if (!box.hidden) done(null); }, 150);
    });
  }

  // The rubber band while a connection is dragged out of a pin: out of the
  // pin, then across and down to the pointer.
  const overlay = document.getElementById("overlay");
  function rubber(from, to, target) {
    overlay.replaceChildren();
    const t = tipOf(from);
    if (!t) return;
    const [ox, oy] = SCH.OUT[t.side];
    const x0 = px(t.tip.x), y0 = px(t.tip.y), x1 = x0 + ox * 10, y1 = y0 + oy * 10;
    const d = ox ? `M${x0} ${y0}H${x1}V${to.y}H${to.x}` : `M${x0} ${y0}V${y1}H${to.x}V${to.y}`;
    el("path", { d, class: "rubber" + (target ? " hot" : "") }, overlay);
    el("circle", { cx: to.x, cy: to.y, r: target ? 5 : 3, class: "rubber-end" + (target ? " hot" : "") }, overlay);
  }

  // What's under the pointer for a connection: a wireable pin, a net's
  // wire, or nothing.
  function dropTarget(e) {
    for (const t of document.elementsFromPoint(e.clientX, e.clientY)) {
      if (!svg.contains(t) || overlay.contains(t)) continue;
      const pin = t.closest(".pin");
      if (pin) return pin.classList.contains("io") ? { pin: pin.dataset.pin } : { bad: pin.dataset.pin };
      const net = t.closest(".wires [data-net]");
      if (net) return { net: +net.dataset.net };
      if (t.closest(".part")) return null;
    }
    return null;
  }

  function overlaps(ref, l) {
    const g = SCH.geometry(ref, { ...parts[ref], ...l }, symbols[ref]);
    const m = 4;
    for (const o of gsCache) {
      if (o.ref === ref) continue;
      const a = g.rect, b = o.rect;
      if (a.x - m < b.x + b.w + m && b.x - m < a.x + a.w + m && a.y - m < b.y + b.h + m && b.y - m < a.y + a.h + m) return true;
    }
    return false;
  }

  let drag = null, pending = false;
  function schedule() {
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => { pending = false; draw(!!drag); });
  }

  svg.addEventListener("pointerdown", e => {
    const p = toWorld(e);
    const partEl = e.target.closest(".part");
    const pinEl = e.target.closest(".pin");
    if (pinEl && e.button === 0) {
      // A pin: click picks it, dragging out of it makes a connection.
      if (!pinEl.classList.contains("io")) {
        toast(`${pinEl.dataset.pin} isn't modelled by the emulator, so it can't be wired`);
        return;
      }
      selected = null;
      drag = { wire: pinEl.dataset.pin, x: e.clientX, y: e.clientY, moved: false };
      svg.setPointerCapture(e.pointerId);
    } else if (partEl && e.button === 0) {
      pickedPin = null;
      const ref = partEl.dataset.ref;
      selected = ref;
      drag = { ref, dx: p.x / G - layout[ref].x, dy: p.y / G - layout[ref].y, moved: false };
      snapshot();
      svg.setPointerCapture(e.pointerId);
      draw();
    } else {
      // The net under the pointer now: a redraw replaces the element.
      const net = netAt(e);
      if (selected || pickedPin) { selected = null; pickedPin = null; draw(); }
      drag = { pan: true, net, x: e.clientX, y: e.clientY, vx: view.x, vy: view.y };
      svg.setPointerCapture(e.pointerId);
      svg.classList.add("panning");
    }
  });
  svg.addEventListener("pointermove", e => {
    if (!drag) {
      highlight(pinned ?? netAt(e));
      return;
    }
    if (drag.wire) {
      if (!drag.moved && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 4) return;
      drag.moved = true;
      const target = dropTarget(e);
      const ok = target && (target.pin && target.pin !== drag.wire || target.net !== undefined);
      highlight(target && target.net !== undefined ? target.net : target && target.pin ? pinNet.get(target.pin) ?? null : null);
      rubber(drag.wire, toWorld(e), ok);
      return;
    }
    if (drag.pan) {
      view.x = drag.vx - (e.clientX - drag.x) * view.s;
      view.y = drag.vy - (e.clientY - drag.y) * view.s;
      applyView();
      return;
    }
    const p = toWorld(e);
    const l = layout[drag.ref];
    const nx = Math.round(p.x / G - drag.dx), ny = Math.round(p.y / G - drag.dy);
    if (nx === l.x && ny === l.y) return;
    const cand = { ...l, x: nx, y: ny };
    // Slide along an obstacle instead of stopping dead.
    const tries = [cand, { ...l, x: nx }, { ...l, y: ny }];
    const ok = tries.find(c => !overlaps(drag.ref, c) && (c.x !== l.x || c.y !== l.y));
    if (!ok) return;
    layout[drag.ref] = ok;
    drag.moved = true;
    setDirty(true);
    schedule();
  });
  const endDrag = e => {
    if (drag && drag.wire) {
      const from = drag.wire, moved = drag.moved;
      drag = null;
      overlay.replaceChildren();
      if (!moved) { pickedPin = pickedPin === from ? null : from; pinned = null; draw(); return; }
      if (e.type !== "pointerup") return;
      const at = { x: e.clientX, y: e.clientY }, target = dropTarget(e);
      if (!target) connectByName(from, at);
      else if (target.bad) toast(`${target.bad} isn't modelled by the emulator, so it can't be wired`, true);
      else if (target.pin === from) return;
      else if (target.pin) connectPins(from, target.pin, at);
      else {
        const a = pinNet.get(from);
        if (a === target.net) toast(`${from} is already on ${nets[a].name}`);
        else if (a !== undefined) mergeNets(target.net, a);
        else joinNet(from, target.net);
      }
      return;
    }
    if (drag && drag.pan && e.type === "pointerup" && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 4) {
      // A click on a wire keeps its net lit (and picked for Delete); on
      // the paper, lets it go.
      pinned = drag.net;
      highlight(pinned);
    }
    if (drag && !drag.pan && !drag.moved) undo.pop();
    if (drag && !drag.pan && drag.moved) { drag = null; draw(); }
    drag = null;
    svg.classList.remove("panning");
  };
  svg.addEventListener("pointerup", endDrag);
  svg.addEventListener("pointercancel", endDrag);
  svg.addEventListener("pointerleave", () => { if (!drag) highlight(pinned); });
  svg.addEventListener("dblclick", e => {
    const t = e.target.closest(".wires [data-net]");
    if (t) renameNet(+t.dataset.net, { x: e.clientX, y: e.clientY });
  });

  svg.addEventListener("wheel", e => {
    e.preventDefault();
    const p = toWorld(e);
    const k = Math.exp(e.deltaY * (e.deltaMode ? 0.05 : 0.0015));
    const s = Math.min(4, Math.max(0.15, view.s * k));
    view.x = p.x - (p.x - view.x) * s / view.s;
    view.y = p.y - (p.y - view.y) * s / view.s;
    view.s = s;
    applyView();
  }, { passive: false });

  if (netList) {
    netList.addEventListener("mousemove", e => {
      const li = e.target.closest("li");
      if (li) highlight(+li.dataset.net);
    });
    netList.addEventListener("mouseleave", () => highlight(pinned));
    netList.addEventListener("click", e => {
      const li = e.target.closest("li");
      if (!li) return;
      if (e.target.closest(".del")) { deleteNet(+li.dataset.net); return; }
      pinned = pinned === +li.dataset.net ? null : +li.dataset.net;
      highlight(pinned);
    });
    netList.addEventListener("dblclick", e => {
      const li = e.target.closest("li");
      if (li && !e.target.closest(".del")) renameNet(+li.dataset.net, { x: e.clientX, y: e.clientY });
    });
  }

  function transform(fn) {
    if (!selected) { toast("Select a part first"); return; }
    snapshot();
    const l = layout[selected], before = { ...l };
    fn(l);
    if (overlaps(selected, l)) { layout[selected] = before; undo.pop(); toast("No room to turn it here"); return; }
    setDirty(true);
    draw();
  }
  const rotate = d => transform(l => { l.rotate = ((l.rotate + d) % 360 + 360) % 360; });
  const mirror = () => transform(l => { l.mirror = !l.mirror; });

  function autoAll() {
    snapshot();
    layout = {};
    autoPlaceBest(refs);
    setDirty(true);
    draw();
    fit();
  }

  async function save() {
    const svgText = exportSVG();
    try {
      const changed = netsText() !== savedNets;
      const r = await fetch("/api/save", { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ layout, svg: svgText, nets: changed ? nets.map(n => ({ name: n.name, pins: n.pins })) : null }) });
      const j = await r.json().catch(() => ({ error: r.statusText }));
      if (!r.ok) throw new Error(j.error || r.statusText);
      if (changed) savedNets = netsText();
      setDirty(false);
      toast("Saved " + j.saved.join(", "));
    } catch (err) {
      toast("Couldn't save: " + err.message + " (is tools/schematic.py still running?)", true);
    }
  }

  function download() {
    const blob = new Blob([exportSVG()], { type: "image/svg+xml" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = BOARD.file.replace(/\.toml$/, "") + ".svg";
    a.click();
    URL.revokeObjectURL(a.href);
  }

  let toastTimer = null;
  function toast(text, bad) {
    const t = document.getElementById("toast");
    t.textContent = text;
    t.className = "show" + (bad ? " bad" : "");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { t.className = ""; }, 2600);
  }

  const actions = {
    fit, save, download, auto: autoAll,
    rotate: () => rotate(90), mirror,
    labels: () => { showLabels = !showLabels; document.querySelector('[data-act="labels"]').classList.toggle("on", showLabels); draw(); },
    undo: () => {
      if (!undo.length) return;
      const u = JSON.parse(undo.pop());
      layout = u.layout;
      if (JSON.stringify(u.nets) !== JSON.stringify(nets)) { nets = u.nets; pinned = null; hl = null; netsChanged(); }
      setDirty(true);
      draw();
      toast("Undone");
    },
  };
  for (const b of document.querySelectorAll("[data-act]")) b.addEventListener("click", () => actions[b.dataset.act]());
  document.querySelector('[data-act="labels"]').classList.toggle("on", showLabels);

  window.addEventListener("keydown", e => {
    if (e.target.closest && e.target.closest("input, textarea")) return;
    const k = e.key.toLowerCase();
    if ((e.ctrlKey || e.metaKey) && k === "s") { e.preventDefault(); save(); }
    else if ((e.ctrlKey || e.metaKey) && k === "z") { e.preventDefault(); actions.undo(); }
    else if (e.ctrlKey || e.metaKey || e.altKey) return;
    else if (k === "r") rotate(e.shiftKey ? -90 : 90);
    else if (k === "m") mirror();
    else if (k === "f") fit();
    else if (k === "l") actions.labels();
    else if (k === "escape") { selected = null; pickedPin = null; pinned = null; highlight(null); draw(); }
    else if (k === "delete" || k === "backspace") {
      e.preventDefault();
      if (pickedPin) disconnectPin(pickedPin);
      else if (pinned !== null) deleteNet(pinned);
      else toast("Click a pin or a wire first, then Delete");
    }
    else if (selected && k.startsWith("arrow")) {
      e.preventDefault();
      const d = e.shiftKey ? 10 : 1;
      const l = layout[selected];
      const c = { ...l, x: l.x + (k === "arrowleft" ? -d : k === "arrowright" ? d : 0),
                         y: l.y + (k === "arrowup" ? -d : k === "arrowdown" ? d : 0) };
      if (!overlaps(selected, c)) { snapshot(); layout[selected] = c; setDirty(true); draw(); }
    }
  });
  window.addEventListener("beforeunload", e => { if (dirty) { e.preventDefault(); e.returnValue = ""; } });
  window.addEventListener("resize", () => view && applyView());

  // ?selftest=N: moves parts at random N times, routing and checking each
  // layout, and reports in the status bar.
  if (params.has("selftest")) {
    let seed = 12345;
    const rnd = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
    const runs = +params.get("selftest") || 200, problems = [], times = [];
    let unrouted = 0, cost = 0;
    gsCache = geoms();
    for (let i = 0; i < runs; i++) {
      const ref = refs[Math.floor(rnd() * refs.length)];
      const l = layout[ref];
      const c = { ...l, x: l.x + Math.round((rnd() - 0.5) * 30), y: l.y + Math.round((rnd() - 0.5) * 30) };
      if (rnd() < 0.1) c.rotate = (c.rotate + 90) % 360;
      if (rnd() < 0.1) c.mirror = !c.mirror;
      if (overlaps(ref, c)) continue;
      layout[ref] = c;
      const t0 = performance.now();
      const gs = geoms();
      const ro = { kindOf };
      for (const k of ["improve", "passes", "weight", "margin"]) if (params.has(k)) ro[k] = +params.get(k);
      const r = SCH.route(gs, nets, ro);
      cost += r.cost;
      times.push(performance.now() - t0);
      gsCache = gs;
      unrouted += r.unrouted.length;
      for (const p of SCH.check(gs, nets, r)) problems.push(`#${i}: ${p}`);
    }
    times.sort((a, b) => a - b);
    draw(); fit();
    status.textContent = `selftest: ${times.length} layouts, ${problems.length} problems, ${unrouted} unrouted nets, ` +
      `median ${times[times.length >> 1].toFixed(0)} ms, max ${times[times.length - 1].toFixed(0)} ms, mean cost ${(cost / times.length).toFixed(0)}` +
      (problems.length ? " — " + problems.slice(0, 4).join("; ") : "");
    return;
  }

  document.getElementById("board-name").textContent = BOARD.name;
  document.getElementById("board-file").textContent = BOARD.file;
  setDirty(Object.keys(BOARD.layout).length === 0);
  draw();
  fit();
})();
