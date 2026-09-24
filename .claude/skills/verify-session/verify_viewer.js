/**
 * Runs the viewer's aggregation headless against a real recording.
 *
 * Checks the figures a grouped row reports against the sample arrays read
 * independently, that the table renders and sorts, and that a selection band is
 * drawn identically on every chart.
 *
 *   node .claude/skills/verify-session/verify_viewer.js <recording.json>
 */
const fs = require('fs');

const html = fs.readFileSync('crates/system_info_collector/src/serving/session_viewer.html', 'utf8');
const script = html.slice(html.indexOf('<script>\n', html.indexOf('</head>')) + '<script>\n'.length, html.lastIndexOf('</script>'));

// ── DOM stub ──────────────────────────────────────────────────────────────────

const controls = { groupByExe: false, hideIdle: false, showKernel: false, filter: '' };
const calls = { fillRect: [], canvases: [] };

function makeElement(id) {
    const element = {
        id, style: {}, dataset: {}, children: [],
        _html: '', textContent: '', scrollLeft: 0, scrollTop: 0, width: 600, height: 18,
        classList: { toggle() {}, add() {}, remove() {}, contains: () => false },
        get checked() { return !!controls[id]; },
        set checked(value) { controls[id] = value; },
        get value() { return controls[id] ?? ''; },
        set value(next) { controls[id] = next; },
        get innerHTML() { return element._html; },
        set innerHTML(next) { element._html = next; element.children = parseCanvases(next); },
        getAttribute: name => element.dataset[name] ?? null,
        querySelector: selector => element.children.find(child => child._selector === selector) ?? null,
        querySelectorAll: selector => element.children.filter(child => child._selector === selector),
        addEventListener() {}, appendChild(child) { element.children.push(child); },
        getContext: () => context,
        getBoundingClientRect: () => ({ left: 0, top: 0, width: 600, height: 200 }),
    };
    return element;
}

/// The table is built as an HTML string, so the sparkline canvases and the scroll
/// container have to be recovered from it for the stub to hand them back.
function parseCanvases(html) {
    const found = [];
    if (html.includes('table-scroll')) {
        const scroller = makeElement('table-scroll');
        scroller._selector = '.table-scroll';
        found.push(scroller);
    }
    for (const match of html.matchAll(/<canvas[^>]*class="spark"[^>]*>/g)) {
        const canvas = makeElement('spark');
        canvas._selector = 'canvas.spark';
        canvas.dataset['data-key'] = (match[0].match(/data-key="([^"]*)"/) || [])[1];
        canvas.dataset['data-field'] = (match[0].match(/data-field="([^"]*)"/) || [])[1];
        found.push(canvas);
        calls.canvases.push(canvas);
    }
    return found;
}

const context = {
    save() {}, restore() {}, beginPath() {}, moveTo() {}, lineTo() {}, stroke() {}, clearRect() {},
    fillText() {}, measureText: text => ({ width: text.length * 6 }),
    fillRect(x, y, w, h) { calls.fillRect.push({ x, y, w, h }); },
};

const elements = new Map();
const byId = id => {
    if (!elements.has(id)) elements.set(id, makeElement(id));
    return elements.get(id);
};
const allCanvases = () => [...elements.values()].flatMap(element => element.querySelectorAll('canvas.spark'));

globalThis.window = { SESSION_DATA: undefined, location: { search: '' }, addEventListener() {} };
globalThis.document = {
    getElementById: byId,
    querySelector: selector => (selector === '.table-scroll' ? byId('viewHost').querySelector(selector) : makeElement('x')),
    querySelectorAll: selector => (selector === 'canvas.spark' ? allCanvases() : []),
    addEventListener() {}, createElement: () => makeElement('x'), title: '',
};
globalThis.Chart = class {
    constructor(ctx, config) {
        this.ctx = context;
        this.config = config;
        this.chartArea = { left: 0, right: 600, top: 0, bottom: 200 };
        // 1 s = 10 px would make every selection look narrow; this is a realistic scale.
        this.scales = { x: { getPixelForValue: seconds => seconds * 40 } };
    }
    draw() { for (const plugin of this.config.plugins || []) plugin.afterDatasetsDraw?.(this); }
    destroy() {}
    update() {}
};
globalThis.fetch = () => Promise.reject(new Error('no server in the harness'));

(0, eval)(script + `
;globalThis.__api = {
    state, buildRows, normalizeSamples, chartTitle, render, setView, sortBy, drawBrush,
    COLUMNS, SYSTEM_CHARTS, buildSystemCharts, levelSeries, flowSeries,
};`);
const api = globalThis.__api;

// ── recording ─────────────────────────────────────────────────────────────────

const path = process.argv[2];
if (!path) {
    console.error('usage: node .claude/skills/verify-session/verify_viewer.js <recording.json>   (from the repository root)');
    process.exit(2);
}
const data = JSON.parse(fs.readFileSync(path, 'utf8'));
api.normalizeSamples(data);
api.state.data = data;
api.state.pidIndex = new Map();
api.state.childIndex = new Map();
for (const proc of data.procs) {
    api.state.pidIndex.set(proc.pid, proc.id);
    if (proc.ppid === null || proc.ppid === undefined) continue;
    if (!api.state.childIndex.has(proc.ppid)) api.state.childIndex.set(proc.ppid, []);
    api.state.childIndex.get(proc.ppid).push(proc);
}
const ticks = data.system.t_ms.length;
api.state.sel = { a: 0, b: ticks - 1 };
api.state.baseline = null;

let failures = 0;
const check = (ok, message) => {
    if (!ok) failures++;
    console.log(`${ok ? 'ok  ' : 'FAIL'}  ${message}`);
};
const note = message => console.log(`      ${message}`);

// ── ground truth, computed straight from the sample arrays ────────────────────

const procById = new Map(data.procs.map(proc => [proc.id, proc]));
const groupKey = proc => proc.exe || proc.name || ('pid ' + proc.pid);
const members = new Map();
for (const proc of data.procs) {
    if (!members.has(groupKey(proc))) members.set(groupKey(proc), []);
    members.get(groupKey(proc)).push(proc);
}

/// Combined CPU per tick, and the combined RSS level per tick with each member
/// counted only between its first and last tick.
function truthFor(key) {
    const cpu = new Float64Array(ticks);
    const rss = new Float64Array(ticks);
    for (const proc of members.get(key)) {
        const s = data.samples[proc.id];
        if (!s) continue;
        s.t.forEach((tick, i) => { cpu[tick] += s.cpu[i]; });
        let held = 0, next = 0;
        for (let tick = proc.first_tick; tick <= proc.last_tick; tick++) {
            while (next < s.t.length && s.t[next] === tick) held = s.rss_mb[next++];
            rss[tick] += held;
        }
    }
    return { cpu: Math.max(0, ...cpu), rssMax: Math.max(0, ...rss), rssDelta: rss[ticks - 1] - rss[0] };
}

// ── checks ────────────────────────────────────────────────────────────────────

for (const grouped of [false, true]) {
    controls.groupByExe = grouped;
    const rows = api.buildRows();
    const label = grouped ? 'grouped by executable' : 'one row per process';
    check(rows.length > 0, `${label}: ${rows.length} rows`);

    const inverted = rows.filter(row => row.rates.cpuMax + 1e-6 < row.rates.cpuAvg);
    check(inverted.length === 0, `${label}: no row has CPU max below CPU avg`
        + (inverted.length ? ` (worst: ${inverted[0].label} avg=${inverted[0].rates.cpuAvg.toFixed(2)} max=${inverted[0].rates.cpuMax.toFixed(2)})` : ''));

    if (!grouped) continue;

    const multi = rows.filter(row => row.members.length > 1);
    note(`${multi.length} rows hold more than one instance`);
    const wrongCpu = rows.filter(row => Math.abs(row.rates.cpuMax - truthFor(row.key).cpu) > 0.01);
    check(wrongCpu.length === 0, `${label}: CPU max equals the peak of the members' combined load`
        + (wrongCpu.length ? ` (${wrongCpu[0].label}: ${wrongCpu[0].rates.cpuMax.toFixed(2)} vs ${truthFor(wrongCpu[0].key).cpu.toFixed(2)})` : ''));

    const wrongRss = rows.filter(row => Math.abs(row.agg.rssMax - truthFor(row.key).rssMax) > 0.01);
    check(wrongRss.length === 0, `${label}: RSS max equals the peak of the members' combined level`
        + (wrongRss.length ? ` (${wrongRss[0].label}: ${wrongRss[0].agg.rssMax.toFixed(2)} vs ${truthFor(wrongRss[0].key).rssMax.toFixed(2)})` : ''));

    const wrongDelta = rows.filter(row => Math.abs(row.agg.rssDelta - truthFor(row.key).rssDelta) > 0.01);
    check(wrongDelta.length === 0, `${label}: RSS change is the change of the combined level`
        + (wrongDelta.length ? ` (${wrongDelta[0].label}: ${wrongDelta[0].agg.rssDelta.toFixed(2)} vs ${truthFor(wrongDelta[0].key).rssDelta.toFixed(2)})` : ''));

    const respawned = multi.filter(row => row.members.some(proc => !proc.alive_at_end));
    if (respawned.length > 0) {
        const worst = respawned.sort((x, y) => y.members.length - x.members.length)[0];
        const summed = worst.members.reduce((total, proc) => {
            const s = data.samples[proc.id];
            return total + (s ? Math.max(0, ...s.rss_mb) : 0);
        }, 0);
        note(`${worst.label}: ${worst.members.length} instances, RSS max ${worst.agg.rssMax.toFixed(1)} MB`
            + ` (their peaks added up would be ${summed.toFixed(1)} MB)`);
        check(worst.agg.rssMax <= summed + 0.01, 'a row of instances that took turns reports less than their peaks added up');
    }

    const busiest = [...rows].sort((x, y) => y.rates.cpuAvg - x.rates.cpuAvg)[0];
    note(`busiest group: ${busiest.label} (${busiest.members.length} instances)`
        + ` cpu avg=${busiest.rates.cpuAvg.toFixed(2)}% max=${busiest.rates.cpuMax.toFixed(2)}%`);
}

// The shape cells must be drawn from the same series as the numbers beside them.
controls.groupByExe = true;
{
    const rows = api.buildRows();
    const row = [...rows].sort((x, y) => y.members.length - x.members.length)[0];
    const rss = api.levelSeries(row.members, 'rss_mb', api.state.sel.a, api.state.sel.b);
    check(Math.abs(Math.max(0, ...rss) - row.agg.rssMax) < 0.01,
        'the RSS shape cell and the RSS max column come from the same series');
    const cpu = api.flowSeries(row.members, 'cpu', api.state.sel.a, api.state.sel.b);
    check(Math.abs(Math.max(0, ...cpu) - row.agg.cpuMax) < 0.01,
        'the CPU shape cell and the CPU max column come from the same series');
}

// Rendering, sorting and the kernel-thread toggle.
controls.groupByExe = false;
api.state.view = 'table';
api.render();
const rowCountWithout = api.buildRows().length;
controls.showKernel = true;
api.render();
const rowCountWith = api.buildRows().length;
controls.showKernel = false;
check(rowCountWith >= rowCountWithout, `showKernel adds rows rather than removing them (${rowCountWithout} -> ${rowCountWith})`);
if (data.procs.some(proc => proc.kernel)) {
    check(rowCountWith > rowCountWithout, 'kernel threads appear when asked for');
} else {
    note('no kernel threads in this recording, so the toggle cannot change the row count');
}

let sortFailures = [];
for (const column of api.COLUMNS) {
    for (const pass of [1, 2]) {
        try { api.sortBy(column.key); } catch (error) { sortFailures.push(`${column.key} (pass ${pass}): ${error.message}`); }
    }
}
check(sortFailures.length === 0, `sorting by each of the ${api.COLUMNS.length} columns, both directions, does not throw`
    + (sortFailures.length ? ` (${sortFailures[0]})` : ''));

const scroller = byId('viewHost').querySelector('.table-scroll');
if (scroller) {
    scroller.scrollLeft = 123;
    scroller.scrollTop = 45;
    api.sortBy('rss');
    const after = byId('viewHost').querySelector('.table-scroll');
    check(after.scrollLeft === 123 && after.scrollTop === 45,
        `sorting keeps the table scrolled where it was (${after.scrollLeft}, ${after.scrollTop})`);
} else {
    check(false, 'the table rendered a .table-scroll container');
}

// Tree and lifetime views must render too.
for (const view of ['tree', 'gantt', 'table']) {
    try { api.setView(view); check(true, `the ${view} view renders`); }
    catch (error) { check(false, `the ${view} view renders (${error.message})`); }
}

// The selection band, drawn identically on all three charts.
api.buildSystemCharts();
calls.fillRect.length = 0;
api.drawBrush(1, 3);
const bands = calls.fillRect.filter((_, index) => index % 3 === 0);
check(bands.length === api.SYSTEM_CHARTS.length, `a band is drawn on each of the ${api.SYSTEM_CHARTS.length} charts (${bands.length})`);
check(bands.every(band => Math.abs(band.x - bands[0].x) < 0.01 && Math.abs(band.w - bands[0].w) < 0.01),
    'the band is at the same place and width on every chart');
const fullWidth = (() => { calls.fillRect.length = 0; api.drawBrush(0, (data.system.t_ms[ticks - 1] ?? 0) / 1000); return calls.fillRect[0]?.w ?? 0; })();
check(bands[0].w < fullWidth, `a narrow selection draws a narrower band (${bands[0].w.toFixed(0)} px vs ${fullWidth.toFixed(0)} px)`);

// Selection-scoped columns must shrink with the selection; lifetime ones must not.
controls.groupByExe = false;
api.state.sel = { a: 0, b: ticks - 1 };
const whole = new Map(api.buildRows().map(row => [row.key, row]));
api.state.sel = { a: 0, b: Math.max(0, Math.floor(ticks / 3)) };
const part = new Map(api.buildRows().map(row => [row.key, row]));
const writers = [...whole.values()].filter(row => row.agg.wchar > 0 && part.has(row.key));
if (writers.length > 0) {
    const worst = writers.sort((x, y) => y.agg.wchar - x.agg.wchar)[0];
    const narrowed = part.get(worst.key);
    check(narrowed.agg.wchar <= worst.agg.wchar,
        `"Write syscall" shrinks with the selection (${(worst.agg.wchar / 2 ** 20).toFixed(2)} MB -> ${(narrowed.agg.wchar / 2 ** 20).toFixed(2)} MB)`);
    check(narrowed.sessWchar === worst.sessWchar,
        `"Write session" is independent of the selection (${(worst.sessWchar / 2 ** 20).toFixed(2)} MB)`);
} else {
    note('no process wrote anything in this recording, so the selection-scope checks are skipped');
}
api.state.sel = { a: 0, b: ticks - 1 };

// The system CPU series.
const cpuSpec = api.SYSTEM_CHARTS.find(spec => spec.key === 'cpu');
const flat = data.system.cpu_pct.every(value => value === 0);
check(!flat, `system CPU series is not flat zero (max ${Math.max(...data.system.cpu_pct).toFixed(2)}%)`);
check(api.chartTitle(cpuSpec) === cpuSpec.title, 'the CPU chart keeps its plain title when the series was recorded');
const saved = data.system.cpu_pct;
data.system.cpu_pct = saved.map(() => 0);
check(api.chartTitle(cpuSpec).includes('not recorded'), 'the CPU chart says so when the series is missing');
data.system.cpu_pct = saved;

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} checks failed`);
process.exit(failures === 0 ? 0 : 1);
