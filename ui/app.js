"use strict";

// ---------------------------------------------------------------------------
// Bridge to the Rust side (with a mock so the UI can be previewed in a browser)
// ---------------------------------------------------------------------------
const TAURI = window.__TAURI__;
const invoke = TAURI ? TAURI.core.invoke : mockInvoke;

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

const S = {
  settings: null,
  catalog: null,
  bindings: {},
  poll: null,
  eventsSeq: 0,
  logSeq: 0,
  view: "status",
  wizStep: 0,
  tasks: new Set(),
  jogAngle: { left: 0, right: 0 },
  lastChecksKey: "",
};

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------
async function boot() {
  S.settings = await invoke("get_settings");
  await loadCatalog();
  if (S.settings.setup_done) showDash(); else showWizard(0);
  bindDash();
  pollLoop();
}

async function loadCatalog() {
  const c = await invoke("get_catalog");
  S.catalog = c.catalog;
  S.bindings = c.bindings;
}

async function pollLoop() {
  try {
    const p = await invoke("poll", { eventsSince: S.eventsSeq, logSince: S.logSeq });
    S.poll = p;
    for (const e of p.events) { S.eventsSeq = Math.max(S.eventsSeq, e.seq); onControl(e.id, e.value, e.msg); }
    if (p.log.length) { S.logSeq = p.log[p.log.length - 1].seq; appendLog(p.log); }
    render();
  } catch (err) {
    console.error(err);
  }
  setTimeout(pollLoop, 150);
}

// Saves run one at a time, and the saved copy is merged into the existing
// settings objects so every screen keeps editing the same live object
// (replacing it made later changes on other screens silently get lost).
let saveChain = Promise.resolve();
function saveSettings() {
  saveChain = saveChain.then(async () => {
    try {
      const saved = await invoke("save_settings", { settings: S.settings });
      const engine = S.settings.engine;
      Object.assign(engine, saved.engine);
      saved.engine = engine;
      Object.assign(S.settings, saved);
      const c = await invoke("get_catalog");
      S.bindings = c.bindings;
    } catch (e) {
      toast("Couldn't save settings: " + e);
    }
  });
  return saveChain;
}

function toast(msg) {
  const t = $("#toast");
  t.textContent = msg;
  t.classList.remove("hidden");
  clearTimeout(t._h);
  t._h = setTimeout(() => t.classList.add("hidden"), 2600);
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------
function checkHTML(c) {
  let fix = "";
  if (c.fix) fix = `<button class="btn small" data-fix='${esc(JSON.stringify(c.fix))}'>${esc(c.fix.label)}</button>`;
  return `<div class="check ${c.state}"><span class="dot ${c.state}"></span>
    <div class="body"><div class="title">${esc(c.title)}</div>${c.detail ? `<div class="detail">${esc(c.detail)}</div>` : ""}</div>${fix}</div>`;
}

function renderChecks(el, ids) {
  if (!S.poll) return;
  const list = ids ? S.poll.checks.filter((c) => ids.includes(c.id)) : S.poll.checks;
  const html = list.map(checkHTML).join("") || `<div class="check"><span class="dot pending"></span><div class="body"><div class="title">Checking…</div></div></div>`;
  if (el._html !== html) { el.innerHTML = html; el._html = html; }
}

async function runFix(fix) {
  try {
    if (fix.type === "open_url") await invoke("open_url", { url: fix.url });
    else if (fix.type === "launch_rekordbox") { await invoke("launch_rekordbox"); toast("Opening rekordbox…"); }
    else if (fix.type === "rescan") { await invoke("rescan"); toast("Checking again…"); }
  } catch (e) { toast(String(e)); }
}

document.addEventListener("click", (ev) => {
  const b = ev.target.closest("[data-fix]");
  if (b) runFix(JSON.parse(b.dataset.fix));
});

function check(id) { return S.poll?.checks.find((c) => c.id === id); }
function isGood(id) { const c = check(id); return c && (c.state === "ok" || c.state === "warn" || c.state === "info"); }

// ---------------------------------------------------------------------------
// Controller visual
// ---------------------------------------------------------------------------
function deckHTML(side) {
  const s = side;
  return `<div class="cdeck ${s}">
    <div class="top">
      <div class="jog" data-id="${s}.jog"><div class="mark"></div></div>
      <div class="tempo"><div class="track"><div class="cap" data-id="${s}.tempo"></div></div><small>TEMPO</small></div>
    </div>
    <div class="padrow"><div class="b mode" data-id="${s}.mode">MODE</div>
      <div class="pads">${[1, 2, 3, 4].map((i) => `<div class="pad" data-id="${s}.pad${i}"></div>`).join("")}</div>
      <span class="modename" data-mode="${s}"></span></div>
    <div class="btnrow">
      <div class="b" data-id="${s}.acapel">ACAPEL</div><div class="b" data-id="${s}.instru">INSTRU</div>
    </div>
    <div class="btnrow">
      <div class="b round" data-id="${s}.cue">CUE</div><div class="b round" data-id="${s}.sync">SYNC</div><div class="b round" data-id="${s}.play">▶︎❚❚</div>
    </div>
  </div>`;
}

function controllerHTML() {
  return `<div class="ctl">${deckHTML("left")}
    <div class="mixer">
      <div class="mrow"><div class="knob" data-id="main_level"><i></i></div><div class="knob big" data-id="browse"><i></i></div><div class="knob" data-id="cue_level"><i></i></div></div>
      <div class="mrow labels"><small>MAIN</small><small>BROWSE</small><small>CUE</small></div>
      <div class="mrow"><div class="b" data-id="load_left">LOAD</div><div class="b" data-id="fadefx">FADE FX</div><div class="b" data-id="load_right">LOAD</div></div>
      <div class="mrow"><div class="knob" data-id="left.level"><i></i></div><small>LEVEL</small><div class="knob" data-id="right.level"><i></i></div></div>
      <div class="mrow"><div class="knob" data-id="left.filter"><i></i></div><small>FILTER</small><div class="knob" data-id="right.filter"><i></i></div></div>
      <div class="mrow"><div class="b" data-id="left.pfl">CUE 🎧</div><span></span><div class="b" data-id="right.pfl">CUE 🎧</div></div>
      <div class="xfader"><div class="xf-cap" data-id="crossfader"></div></div>
    </div>${deckHTML("right")}</div>`;
}

function elementId(id) {
  // shifted controls light up their base button
  return id.replace(/_shift$/, "").replace(/^browse_(press)$/, "browse");
}

function onControl(id, value, msg) {
  if (id === "unmapped") { if (S.learning && msg) learnCaptured(msg, value); return; }
  // setup tasks
  const side = id.startsWith("left.") ? "left" : id.startsWith("right.") ? "right" : null;
  if (id === "left.touch" && value > 0) S.tasks.add("touch");
  if (id === "right.jog") S.tasks.add("jog");
  if (id.endsWith(".tempo")) S.tasks.add("tempo");
  if (/\.pad\d/.test(id)) S.tasks.add("pad");
  if (/\.play$/.test(id)) S.tasks.add("play");
  if (id === "crossfader") S.tasks.add("xfader");

  // mapping rows flash so people can find a control
  const row = $(`.maprow[data-control="${CSS.escape(id)}"]`);
  if (row) { row.classList.add("flash"); clearTimeout(row._t); row._t = setTimeout(() => row.classList.remove("flash"), 400); }

  for (const host of $$(".ctl")) {
    if (id.endsWith(".touch")) {
      host.querySelector(`[data-id="${side}.jog"]`)?.classList.toggle("touched", value > 0);
      continue;
    }
    if (id.endsWith(".jog")) {
      const delta = value < 64 ? value : value - 128;
      S.jogAngle[side] += delta * (360 / 242);
      const mark = host.querySelector(`[data-id="${side}.jog"] .mark`);
      if (mark) mark.style.transform = `rotate(${S.jogAngle[side]}deg)`;
      continue;
    }
    const el = host.querySelector(`[data-id="${CSS.escape(elementId(id))}"]`);
    if (!el) continue;
    if (el.classList.contains("knob")) {
      if (id.startsWith("browse")) {
        if (id === "browse_press") flash(el);
        else { el._a = (el._a || 0) + (value < 64 ? 15 : -15); el.querySelector("i").style.transform = `rotate(${el._a}deg)`; flash(el); }
      } else {
        el.querySelector("i").style.transform = `rotate(${(value / 127) * 270 - 135}deg)`;
        flash(el);
      }
    } else if (el.classList.contains("cap")) {
      el.style.top = `${(value / 127) * 100}%`;
      flash(el);
    } else if (el.classList.contains("xf-cap")) {
      el.style.left = `${(value / 127) * 100}%`;
      flash(el);
    } else {
      if (value > 0) el.classList.add("hit"); else el.classList.remove("hit");
      if (value > 0 && el.classList.contains("mode")) setTimeout(() => el.classList.remove("hit"), 200);
    }
  }
}

function flash(el) {
  el.classList.add("hit");
  clearTimeout(el._t);
  el._t = setTimeout(() => el.classList.remove("hit"), 160);
}

function updateCtlState() {
  if (!S.poll) return;
  const snap = S.poll.snapshot;
  for (const host of $$(".ctl")) {
    ["left", "right"].forEach((s, i) => {
      const m = host.querySelector(`[data-mode="${s}"]`);
      if (m) m.textContent = snap.decks[i].pad_mode;
    });
  }
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------
function showDash() {
  $("#wizard").classList.add("hidden");
  $("#dash").classList.remove("hidden");
  $("#ctl-live").innerHTML = controllerHTML();
  renderMapping();
  renderSettings();
  setView(S.view);
}

function setView(v) {
  S.view = v;
  $$(".nav").forEach((n) => n.classList.toggle("active", n.dataset.view === v));
  $$(".view").forEach((n) => n.classList.toggle("hidden", n.id !== "view-" + v));
  if (v === "controller") renderCheats();
}

function bindDash() {
  $$(".nav").forEach((n) => n.addEventListener("click", () => setView(n.dataset.view)));
  $("#map-export").addEventListener("click", async () => {
    const e = S.settings.engine;
    const text = JSON.stringify({ rekordjog_mapping: 1, bindings: e.bindings, pad_modes: e.pad_modes, custom: e.custom }, null, 2);
    try { await navigator.clipboard.writeText(text); toast("Mapping copied — paste it anywhere to share or back it up"); }
    catch { $("#import-box").classList.remove("hidden"); $("#import-text").value = text; toast("Copy the text below"); }
  });
  $("#map-import").addEventListener("click", () => { $("#import-box").classList.remove("hidden"); $("#import-text").value = ""; $("#import-text").focus(); });
  $("#import-cancel").addEventListener("click", () => $("#import-box").classList.add("hidden"));
  $("#import-apply").addEventListener("click", async () => {
    try {
      const m = JSON.parse($("#import-text").value);
      if (typeof m !== "object" || !m) throw new Error();
      const e = S.settings.engine;
      if (m.bindings) e.bindings = m.bindings;
      if (Array.isArray(m.pad_modes)) e.pad_modes = m.pad_modes;
      if (Array.isArray(m.custom)) e.custom = m.custom;
      await saveSettings();
      renderMapping();
      $("#import-box").classList.add("hidden");
      toast("Mapping imported");
    } catch { toast("That doesn't look like a RekordJog mapping"); }
  });
  $("#map-reset").addEventListener("click", async () => {
    S.settings.engine.pad_modes = ["hot_cue", "loops", "pad_fx1", "sampler", "pad_fx2"];
    S.settings.engine.custom = [];
    S.settings.engine.bindings = {};
    await saveSettings();
    renderMapping();
    toast("Mapping reset to defaults");
  });
  window.showUpdates = () => { setView("settings"); $("#upd-check").scrollIntoView({ block: "center" }); };
  $("#side-update").addEventListener("click", () => { invoke("check_updates"); window.showUpdates(); toast("Checking for updates…"); });
  $("#refresh-lights").addEventListener("click", () => { invoke("refresh_lights"); toast("Stopping the light show and repainting the controller's lights…"); });
  $("#rerun-setup").addEventListener("click", () => showWizard(0));
  $("#quit-app").addEventListener("click", () => invoke("quit"));
  $("#mon-on").addEventListener("change", (e) => {
    invoke("set_monitor", { on: e.target.checked });
    if (!e.target.checked) { $("#monitor").innerHTML = `<div class="empty">Turn on “Record” and move something.</div>`; }
  });
  $("#copy-report").addEventListener("click", async () => {
    const text = await invoke("report");
    try { await navigator.clipboard.writeText(text); toast($("#mon-on").checked ? "Diagnostics + MIDI log copied — paste them into the chat" : "Copied — tip: turn on Record first to include the MIDI log"); }
    catch { toast("Couldn't access the clipboard"); }
  });
}

function render() {
  updateCtlState();
  if (!$("#wizard").classList.contains("hidden")) { renderWizardLive(); return; }
  const p = S.poll;
  $("#version").textContent = `v${p.version} · ${p.platform}`;
  renderUpdate(p.update);

  // sidebar
  const line = (label, state) => `<div class="dotline"><span class="dot ${state}"></span>${label}</div>`;
  const st = (id) => check(id)?.state ?? "pending";
  const side = line("Controller", st("controller")) + line("Virtual DDJ-SX", st("vport")) + line("rekordbox", check("link") ? st("link") : st("rekordbox"));
  if ($("#side-status")._h !== side) { $("#side-status").innerHTML = side; $("#side-status")._h = side; }

  if (S.view === "status") {
    const issues = p.checks.filter((c) => c.state === "error" || c.state === "warn");
    let cls = p.overall, icon = "✓", title = "All set — go play!", sub = "Your Mixtrack Go is driving rekordbox as a DDJ-SX.";
    if (p.overall === "error") { icon = "!"; title = issues.length === 1 ? issues[0].title : `${issues.length} things need attention`; sub = "See the checklist below — each item tells you how to fix it."; }
    else if (p.overall === "warn") { icon = "!"; title = issues[0]?.title ?? "Almost there"; sub = issues[0]?.detail ?? ""; }
    else if (p.overall === "pending") { icon = "…"; const first = p.checks.find((c) => c.state === "pending"); title = first?.title ?? "Getting ready"; sub = first?.detail ?? ""; }
    const banner = `<div class="big-dot">${icon}</div><div><h1>${esc(title)}</h1><p>${esc(sub)}</p></div>`;
    const b = $("#banner");
    if (b._h !== banner + cls) { b.className = "banner " + cls; b.innerHTML = banner; b._h = banner + cls; }
    renderChecks($("#checks"));
    renderDecks();
  }
}

function updateText(u) {
  if (!u) return "";
  switch (u.state) {
    case "checking": return "Checking for updates…";
    case "up_to_date": return `You're on the latest version (${u.current}).`;
    case "downloading": return `Downloading ${u.latest}… ${Math.round(u.progress * 100)}%`;
    case "ready": return `Version ${u.latest} is ready${u.waiting_for ? ` — ${u.waiting_for}` : " to install"}.`;
    case "installing": return `Installing ${u.latest}…`;
    case "unsupported": return "Automatic updates aren't available on this system.";
    case "error": return `Update problem: ${u.message}`;
    default: return `Version ${u.current}.`;
  }
}

function renderUpdate(u) {
  if (!u) return;
  const t = updateText(u);
  if ($("#upd-status").textContent !== t) $("#upd-status").textContent = t;
  $("#upd-install").classList.toggle("hidden", u.state !== "ready");
  const strip = $("#updstrip");
  const show = u.state === "ready" || u.state === "downloading" || u.state === "installing";
  strip.classList.toggle("hidden", !show);
  if (show) {
    const html = `<span>⬆</span><span class="grow">${esc(t)}</span>${u.state === "ready" ? '<button class="btn small" data-upd="1">Restart &amp; update now</button>' : ""}`;
    if (strip._h !== html) { strip.innerHTML = html; strip._h = html; const b = strip.querySelector("[data-upd]"); if (b) b.onclick = () => { invoke("install_update"); toast("Installing — RekordJog will restart"); }; }
  }
}

function renderDecks() {
  const snap = S.poll.snapshot;
  const html = ["Left", "Right"].map((name, i) => {
    const d = snap.decks[i];
    const tempo = d.tempo == null ? "" : `<span class="chip">Tempo ${(d.tempo * (S.settings.engine.invert_tempo ? 1 : -1) * 100).toFixed(0)}%</span>`;
    return `<div class="deckcard"><div class="minijog ${d.touched ? "touched" : ""} ${d.playing ? "playing" : ""}"></div>
      <div><div class="name">${name} deck ${d.playing ? '<span class="chip green">Playing</span>' : ""}</div>
      <div class="chips"><span class="chip on">Pads: ${esc(d.pad_mode)}</span>
      <span class="chip ${snap.vinyl ? "on" : ""}">Vinyl ${snap.vinyl ? "on" : "off"}</span>
      <span class="chip">Knob: ${snap.knob_mode === "filter" ? "Filter" : "Low EQ"}</span>
      ${snap.fadefx ? '<span class="chip on">Fade FX</span>' : ""}${tempo}</div></div></div>`;
  }).join("");
  const el = $("#decks");
  if (el._h !== html) { el.innerHTML = html; el._h = html; }
}

// ---------------------------------------------------------------------------
// Mapping editor
// ---------------------------------------------------------------------------
function actionsFor(control) {
  return S.catalog.actions.filter((a) => a.kind === control.kind && (!a.per_deck || control.side));
}

function actionLabel(id, kind) {
  return S.catalog.actions.find((a) => a.id === id && a.kind === kind)?.label ?? id;
}

function optionsHTML(control, cur, def) {
  const groups = {};
  for (const a of actionsFor(control)) (groups[a.group] ??= []).push(a);
  return Object.entries(groups).map(([g, as]) =>
    `<optgroup label="${esc(g)}">${as.map((a) => `<option value="${esc(a.id)}" ${a.id === cur ? "selected" : ""}>${esc(a.label)}${a.id === def ? " (default)" : ""}</option>`).join("")}</optgroup>`).join("");
}

const hex = (n) => n.toString(16).toUpperCase().padStart(2, "0");

function renderPadSlots() {
  const e = S.settings.engine;
  const LIGHTS = [[0], [1], [1, 2], [2], [3]];
  const nameOf = (id) => (S.catalog.pad_modes.find(([m]) => m === id) || [id, id])[1];
  const used = e.pad_modes.filter((m) => m !== "off");
  const dupes = [...new Set(used.filter((m, i) => used.indexOf(m) !== i))];
  const order = used.map(nameOf).join(" → ") + " → " + nameOf(used[0]);
  const notes = [];
  if (dupes.length) notes.push(`<b>${dupes.map(nameOf).join(", ")}</b> is on more than one light, so pressing MODE will seem to do nothing between them.`);
  if (used.includes("loops")) notes.push("rekordbox has no Loops pad mode for a DDJ-SX, so in Loops its pad panel keeps showing the previous mode. The pads still make loops, and the controller's LOOPS light (and RekordJog's Status page) shows you're in Loops.");
  const html = `<div class="mapsec"><h2>Pad modes (MODE button)</h2>
    <p class="muted">Tapping MODE steps through these, lighting the controller's mode lights as shown. Put any rekordbox pad mode on any light, or switch a light off to skip it.</p>
    <p><b>MODE goes:</b> ${esc(order)}</p>
    ${notes.map((n) => `<p class="callout" style="margin:0 0 10px">${n}</p>`).join("")}
    <div class="slotgrid">${S.catalog.pad_slots.map((name, i) => `<div class="slot">
      <div class="lights">${[0, 1, 2, 3].map((l) => `<i class="${LIGHTS[i].includes(l) ? "on" : ""}" title="${["HOT CUE", "LOOPS", "SAMPLER", "STEMS"][l]}"></i>`).join("")}</div>
      <b>${esc(name)}</b>
      <select data-slot="${i}"><option value="off" ${e.pad_modes[i] === "off" ? "selected" : ""}>Off (skip)</option>
      ${S.catalog.pad_modes.map(([id, n]) => `<option value="${esc(id)}" ${e.pad_modes[i] === id ? "selected" : ""}>${esc(n)}</option>`).join("")}</select></div>`).join("")}</div></div>`;
  $("#padslots").innerHTML = html;
  $$("#padslots select").forEach((sel) => sel.addEventListener("change", async () => {
    e.pad_modes[+sel.dataset.slot] = sel.value;
    await saveSettings();
    renderPadSlots();
  }));
}

function renderLearn() {
  const list = S.settings.engine.custom;
  const rows = list.map((c, i) => {
    const ctl = { kind: c.kind, side: c.side };
    return `<div class="learnrow">
      <input type="text" data-c="${i}" data-f="label" value="${esc(c.label)}">
      <code>${hex(c.status)} ${hex(c.data1)}</code>
      <select data-c="${i}" data-f="kind">${["button", "knob", "encoder"].map((k) => `<option ${c.kind === k ? "selected" : ""}>${k}</option>`).join("")}</select>
      <select data-c="${i}" data-f="side"><option value="" ${!c.side ? "selected" : ""}>No deck</option><option value="left" ${c.side === "left" ? "selected" : ""}>Left deck</option><option value="right" ${c.side === "right" ? "selected" : ""}>Right deck</option></select>
      <select data-c="${i}" data-f="action">${optionsHTML(ctl, c.action, "")}</select>
      <button class="btn ghost small danger" data-del="${i}">Remove</button></div>`;
  }).join("");
  $("#learn").innerHTML = `<div class="mapsec"><h2>MIDI Learn</h2>
    <div class="learnbar ${S.learning ? "listening" : ""}"><div class="grow">${S.learning
      ? "<b>Listening…</b> press or move the control on your controller now."
      : "Map anything RekordJog doesn't know yet: press <b>Learn</b>, then touch the control. Controls listed below can be changed directly in their row."}</div>
      <button class="btn ${S.learning ? "ghost" : "primary"} small" id="learn-btn">${S.learning ? "Cancel" : "Learn"}</button></div>
    ${list.length ? `<div class="maptable" style="margin-top:10px">${rows}</div>` : ""}</div>`;
  $("#learn-btn").onclick = () => { S.learning = !S.learning; renderLearn(); };
  $$("#learn [data-c]").forEach((el) => el.addEventListener("change", async () => {
    const c = list[+el.dataset.c], f = el.dataset.f;
    c[f] = f === "side" ? (el.value || null) : el.value;
    if (f === "kind" || f === "side") c.action = "none";
    await saveSettings();
    renderLearn();
  }));
  $$("#learn [data-del]").forEach((b) => b.addEventListener("click", async () => {
    list.splice(+b.dataset.del, 1);
    await saveSettings();
    renderLearn();
  }));
}

async function learnCaptured(msg, value) {
  S.learning = false;
  let [status, d1] = msg;
  if ((status & 0xf0) === 0x80) status = 0x90 | (status & 0x0f);
  const list = S.settings.engine.custom;
  if (!list.some((c) => c.status === status && c.data1 === d1)) {
    const cc = (status & 0xf0) === 0xb0;
    const ch = status & 0x0f;
    list.push({
      label: `Control ${hex(status)} ${hex(d1)}`,
      status, data1: d1,
      kind: cc ? (value === 1 || value === 127 ? "encoder" : "knob") : "button",
      side: ch === 0 || ch === 4 ? "left" : ch === 1 || ch === 5 ? "right" : null,
      action: "none",
    });
    await saveSettings();
    toast("Got it — now choose what it should do");
  } else toast("That control is already in the list");
  renderLearn();
}

function renderMapping() {
  renderPadSlots();
  renderLearn();
  const sections = {};
  for (const c of S.catalog.controls) (sections[c.section] ??= []).push(c);
  const order = ["Left deck", "Mixer & browser", "Right deck"];
  let html = "";
  for (const sec of order) {
    const list = sections[sec];
    if (!list) continue;
    html += `<div class="mapsec"><h2>${esc(sec)}</h2><div class="maptable">`;
    for (const c of list) {
      if (c.kind === "fixed") {
        html += `<div class="maprow" data-control="${esc(c.id)}"><div class="lbl">${esc(c.label)}</div><div class="fixed">${esc(c.default_action)}</div></div>`;
        continue;
      }
      const cur = S.bindings[c.id] ?? c.default_action;
      const opts = optionsHTML(c, cur, c.default_action);
      html += `<div class="maprow ${cur !== c.default_action ? "changed" : ""}" data-control="${esc(c.id)}">
        <div class="lbl">${esc(c.label)}</div><select data-bind="${esc(c.id)}">${opts}</select></div>`;
    }
    html += `</div></div>`;
  }
  $("#mapping").innerHTML = html;
  $$("#mapping select[data-bind]").forEach((sel) => sel.addEventListener("change", async () => {
    const id = sel.dataset.bind;
    const control = S.catalog.controls.find((c) => c.id === id);
    if (sel.value === control.default_action) delete S.settings.engine.bindings[id];
    else S.settings.engine.bindings[id] = sel.value;
    sel.closest(".maprow").classList.toggle("changed", sel.value !== control.default_action);
    await saveSettings();
  }));
}

function renderCheats() {
  const b = (id) => {
    const c = S.catalog.controls.find((x) => x.id === id);
    return c ? actionLabel(S.bindings[id] ?? c.default_action, c.kind) : "";
  };
  const items = [
    ["Shift", "Hold <kbd>MODE</kbd> for a second — buttons then do their second function."],
    ["Pad modes", "Tap <kbd>MODE</kbd>: " + S.settings.engine.pad_modes.filter((m) => m !== "off").map((m) => (S.catalog.pad_modes.find(([id]) => id === m) || [m, m])[1]).join(" → ") + ". Change the order under Mapping."],
    ["Loops mode", "Pads make 1, 2, 4 and 8-beat loops. Press the lit pad again (or Shift + any pad) to exit the loop."],
    ["Jog wheels", "Touch the top and move to scratch (vinyl mode), turn the edge to nudge. Shift + jog = fast search."],
    ["Acapel / Shift", `${b("left.acapel")} · ${b("left.acapel_shift")}`],
    ["Instru / Shift", `${b("left.instru")} · ${b("left.instru_shift")}`],
    ["Shift + Cue", b("left.cue_shift")],
    ["Shift + Sync", b("left.sync_shift")],
    ["Shift + 🎧", b("left.pfl_shift")],
    ["Shift + Load left", b("load_left_shift")],
    ["Shift + Load right", b("load_right_shift")],
    ["Fade FX", b("fadefx") + " — the crossfader also washes out the deck you fade away from."],
    ["Browse knob", `Turn: ${b("browse")} · Push: ${b("browse_press")} · Shift + Fade FX: ${b("fadefx_shift")}`],
  ];
  $("#cheats").innerHTML = items.map(([k, v]) => `<div class="cheat"><b>${esc(k)}</b><span>${v}</span></div>`).join("");
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------
function renderSettings() {
  const e = S.settings.engine;
  const pct = (v) => `${Math.round(v * 100)}%`;
  const bindRange = (id, key, label) => {
    const el = $(id); el.value = e[key]; $(label).textContent = pct(e[key]);
    el.oninput = () => { $(label).textContent = pct(+el.value); };
    el.onchange = () => { e[key] = +el.value; saveSettings(); };
  };
  bindRange("#s-scratch", "scratch_sensitivity", "#v-scratch");
  bindRange("#s-bend", "bend_sensitivity", "#v-bend");
  bindRange("#s-fade", "fadefx_depth", "#v-fade");
  const bindToggle = (id, get, set) => { const el = $(id); el.checked = get(); el.onchange = () => { set(el.checked); saveSettings(); }; };
  bindToggle("#s-vinyl", () => e.vinyl_default, (v) => (e.vinyl_default = v));
  bindToggle("#s-invert-tempo", () => e.invert_tempo, (v) => (e.invert_tempo = v));
  bindToggle("#s-invert-browse", () => e.invert_browse, (v) => (e.invert_browse = v));
  bindToggle("#s-knob-filter", () => e.knob_default_filter, (v) => (e.knob_default_filter = v));
  bindToggle("#s-tray", () => S.settings.keep_running_in_tray, (v) => (S.settings.keep_running_in_tray = v));
  bindToggle("#s-login", () => S.settings.launch_at_login, (v) => (S.settings.launch_at_login = v));
  bindToggle("#s-autoupdate", () => S.settings.auto_update, (v) => (S.settings.auto_update = v));
  $("#upd-check").onclick = () => { invoke("check_updates"); toast("Checking for updates…"); };
  $("#upd-install").onclick = () => { invoke("install_update"); toast("Installing — RekordJog will restart"); };
  renderPortSelect();
}

function renderPortSelect() {
  const sel = $("#s-port");
  const inputs = S.poll?.status.midi_inputs ?? [];
  const cur = S.settings.controller_port ?? "";
  const names = [...new Set([...inputs, ...(cur ? [cur] : [])])];
  const html = `<option value="">Automatic (find “Mixtrack Go”)</option>` + names.map((n) => `<option value="${esc(n)}" ${n === cur ? "selected" : ""}>${esc(n)}</option>`).join("");
  if (sel._h !== html) { sel.innerHTML = html; sel._h = html; }
  sel.onchange = () => { S.settings.controller_port = sel.value || null; saveSettings(); };
}
setInterval(() => { if (S.view === "settings" && S.poll) renderPortSelect(); }, 2000);

// ---------------------------------------------------------------------------
// MIDI monitor
// ---------------------------------------------------------------------------
const DIRS = { "ctl>": ["ctl", "MT GO →"], ">rb": ["rbo", "→ RB"], "rb>": ["rbi", "RB →"], ">ctl": ["led", "→ LED"] };
function appendLog(lines) {
  const m = $("#monitor");
  if (!$("#mon-on").checked) return;
  m.querySelector(".empty")?.remove();
  const atBottom = m.scrollTop + m.clientHeight >= m.scrollHeight - 30;
  const frag = document.createDocumentFragment();
  for (const l of lines) {
    const [cls, label] = DIRS[l.dir] ?? ["", l.dir];
    const div = document.createElement("div");
    div.className = "l";
    div.innerHTML = `<span class="t">${(l.t / 1000).toFixed(2)}</span><span class="d-${cls}">${label}</span><span>${l.hex}</span>`;
    frag.appendChild(div);
  }
  m.appendChild(frag);
  while (m.children.length > 600) m.firstChild.remove();
  if (atBottom) m.scrollTop = m.scrollHeight;
}

// ---------------------------------------------------------------------------
// Setup wizard
// ---------------------------------------------------------------------------
const STEPS = ["Welcome", "Connect", "Test", "rekordbox", "Tempo", "Done"];
const TASKS = [
  ["touch", "Touch the left jog wheel"],
  ["jog", "Spin the right jog wheel"],
  ["tempo", "Move a tempo fader"],
  ["pad", "Hit a performance pad"],
  ["play", "Press Play"],
  ["xfader", "Move the crossfader"],
];

function showWizard(step) {
  $("#dash").classList.add("hidden");
  $("#wizard").classList.remove("hidden");
  S.wizStep = step;
  renderWizard();
}

$("#wiz-back").addEventListener("click", () => { if (S.wizStep > 0) { S.wizStep--; renderWizard(); } });
$("#wiz-next").addEventListener("click", () => {
  if (S.wizStep < STEPS.length - 1) { S.wizStep++; renderWizard(); } else finishWizard();
});
$("#wiz-skip").addEventListener("click", finishWizard);

async function finishWizard() {
  S.settings.setup_done = true;
  await saveSettings();
  showDash();
}

function renderWizard() {
  $("#wiz-steps").innerHTML = STEPS.map((s, i) => `<li class="${i === S.wizStep ? "active" : i < S.wizStep ? "done" : ""}">${s}</li>`).join("");
  $("#wiz-back").style.visibility = S.wizStep === 0 ? "hidden" : "visible";
  const body = $("#wiz-body");
  const platform = S.poll?.platform ?? (navigator.userAgent.includes("Windows") ? "windows" : "macos");
  let html = "";
  switch (S.wizStep) {
    case 0:
      html = `<div class="hero"><h1>Mixtrack Go <span>×</span> rekordbox</h1>
        <p>RekordJog makes rekordbox think your Numark Mixtrack Go is a Pioneer DDJ-SX — so jog wheels, pads, loops, effects and LEDs all just work. No mapping files, no extra apps to babysit.</p></div>
        <div class="flow"><div class="node"><b>Mixtrack Go</b><small>USB-C</small></div><div class="arrow">→</div>
        <div class="node mid"><b>RekordJog</b><small>translates everything</small></div><div class="arrow">→</div>
        <div class="node"><b>rekordbox</b><small>sees a “DDJ-SX”</small></div></div>
        <div class="features">
          <div class="feature"><b>Real jog wheels</b><span>Scratch, nudge and search with platter-accurate resolution.</span></div>
          <div class="feature"><b>Everything mapped</b><span>Play, cue, sync, pads, loops, FX, mixer, browser — plus LED feedback.</span></div>
          <div class="feature"><b>Self-checking</b><span>Finds rekordbox and tells you exactly what to fix if anything's off.</span></div>
        </div>
        <p class="callout">This takes about two minutes. Plug your Mixtrack Go in now and keep rekordbox closed for the moment.</p>`;
      break;
    case 1:
      html = `<h1>Connect your controller</h1>
        <p class="muted">Plug the Mixtrack Go into this computer with a USB-C cable. RekordJog finds it automatically${platform === "windows" ? " and creates the virtual DDJ-SX for rekordbox" : ""}.</p>
        <div class="checks" id="wiz-checks"></div>
        ${platform === "windows" ? `<p class="callout">Windows only, one time: if asked, install the free <b>loopMIDI</b> — it contains the virtual MIDI driver. You never have to open it; RekordJog uses the driver directly and notices it as soon as it's installed.</p>` : ""}`;
      break;
    case 2:
      html = `<h1>Give it a spin</h1><p class="muted">Try each of these — they light up on screen as RekordJog receives them.</p>
        <div class="ctl-host" id="wiz-ctl">${controllerHTML()}</div><div class="tasks" id="wiz-tasks"></div>`;
      break;
    case 3:
      html = `<h1>Open rekordbox</h1><p class="muted">rekordbox detects the virtual DDJ-SX by itself — nothing to import.</p>
        <div class="checks" id="wiz-checks"></div>
        <ol class="steps-list">
          <li><b>Open rekordbox and switch to PERFORMANCE mode</b><span>Use the mode menu in the top-left corner.</span></li>
          <li><b>Pick the Mixtrack Go as your sound card</b><span>Preferences → Audio → Audio: choose “Mixtrack Go”. Main output = channels 1/2, headphones = 3/4 (Preferences → Audio → Output → Mixer mode “Internal”).</span></li>
          <li><b>That's it</b><span>rekordbox shows “DDJ-SX” as connected and your pads light up.</span></li>
        </ol>
        <p class="callout">rekordbox only enables controllers like the DDJ-SX with a plan that includes DJ hardware (Core or higher).</p>`;
      break;
    case 4:
      html = `<h1>Check the tempo fader</h1>
        <p class="muted">Faders are labelled differently on every brand, so let's make sure yours goes the right way.</p>
        <ol class="steps-list"><li><b>Load a track on the left deck and press Play</b><span>Use the browse knob and the left LOAD button.</span></li>
        <li><b>Push the left tempo fader towards you (the “+” end)</b><span>Watch the BPM / tempo % on the left deck in rekordbox.</span></li></ol>
        <div class="card"><b>Did the tempo go up?</b>
          <div class="choice"><button class="btn primary" id="tempo-yes">Yes, it went up</button><button class="btn" id="tempo-flip">No — flip it</button></div>
          <p class="muted" id="tempo-state" style="margin:14px 0 0"></p></div>`;
      break;
    case 5:
      html = `<div class="hero"><h1>You're <span>ready</span></h1><p>RekordJog runs in the ${platform === "macos" ? "menu bar" : "system tray"} and reconnects on its own whenever you plug the controller in.</p></div>
        <h2>Handy combos</h2><div class="cheats" id="cheats-wiz"></div>`;
      break;
  }
  body.innerHTML = `<div class="wiz-inner">${html}</div>`;
  if (S.wizStep === 4) {
    const upd = () => { $("#tempo-state").textContent = `Current setting: ${S.settings.engine.invert_tempo ? "standard" : "reversed"} direction.`; };
    upd();
    $("#tempo-yes").onclick = () => { S.wizStep++; renderWizard(); };
    $("#tempo-flip").onclick = async () => { S.settings.engine.invert_tempo = !S.settings.engine.invert_tempo; await saveSettings(); upd(); toast("Flipped — try the fader again"); };
  }
  if (S.wizStep === 5) { renderCheats(); $("#cheats-wiz").innerHTML = $("#cheats").innerHTML; }
  $("#wiz-next").textContent = S.wizStep === STEPS.length - 1 ? "Finish" : S.wizStep === 0 ? "Start setup" : "Next";
  renderWizardLive();
}

function renderWizardLive() {
  const hint = $("#wiz-hint");
  const next = $("#wiz-next");
  if (S.wizStep === 1) {
    renderChecks($("#wiz-checks"), ["controller", "vport"]);
    const ready = isGood("controller") && isGood("vport");
    hint.textContent = ready ? "Looking good!" : "Waiting… you can also continue and fix this later.";
    next.textContent = ready ? "Next" : "Continue anyway";
  } else if (S.wizStep === 2) {
    const t = $("#wiz-tasks");
    if (t) {
      const html = TASKS.map(([k, l]) => `<div class="task ${S.tasks.has(k) ? "done" : ""}"><span class="tick">${S.tasks.has(k) ? "✓" : ""}</span>${l}</div>`).join("");
      if (t._h !== html) { t.innerHTML = html; t._h = html; }
    }
    const n = TASKS.filter(([k]) => S.tasks.has(k)).length;
    hint.textContent = n === TASKS.length ? "Everything works!" : `${n} of ${TASKS.length} done`;
    next.textContent = n === TASKS.length ? "Next" : "Skip";
  } else if (S.wizStep === 3) {
    renderChecks($("#wiz-checks"), ["rekordbox", "mapping", "custom_mapping", "link"]);
    const linked = check("link")?.state === "ok";
    hint.textContent = linked ? "rekordbox is connected!" : "Waiting for rekordbox…";
    next.textContent = linked ? "Next" : "Continue anyway";
  } else {
    hint.textContent = "";
  }
}

// ---------------------------------------------------------------------------
// Browser preview mock (only used outside the app)
// ---------------------------------------------------------------------------
function mockInvoke(cmd, args) {
  const M = (window.__mock ??= {
    settings: { engine: { scratch_sensitivity: 1, bend_sensitivity: 1, invert_tempo: true, invert_browse: false, vinyl_default: true, knob_default_filter: true, fadefx_depth: 0.8, bindings: {}, pad_modes: ["hot_cue", "loops", "pad_fx1", "sampler", "pad_fx2"], custom: [{ label: "Bluetooth button", status: 0x9f, data1: 0x30, kind: "button", side: null, action: "ddj:SortBPM" }] }, auto_update: true, setup_done: !location.hash.includes("wizard"), controller_port: null, keep_running_in_tray: true, launch_at_login: false },
    seq: 1, t: 0,
  });
  const cats = { controls: [], actions: [] };
  if (cmd === "get_settings") return Promise.resolve(structuredClone(M.settings));
  if (cmd === "save_settings") { M.settings = structuredClone(args.settings); return Promise.resolve(M.settings); }
  if (cmd === "get_catalog") return fetch("mock-catalog.json").then((r) => r.json()).catch(() => ({ catalog: cats, bindings: {} }));
  if (cmd === "report") return Promise.resolve("mock report");
  if (cmd === "poll") {
    M.t += 150;
    const ev = [];
    const ids = ["left.jog", "left.touch", "right.play", "left.pad2", "crossfader", "left.tempo", "right.level"];
    if (Math.random() < 0.5) ev.push({ seq: M.seq++, id: ids[Math.floor(Math.random() * ids.length)], value: Math.floor(Math.random() * 128) });
    const state = location.hash.includes("issues");
    return Promise.resolve({
      status: { midi_inputs: ["Mixtrack Go", "IAC Driver Bus 1"] },
      snapshot: { decks: [{ pad_mode: "Hot Cue", touched: Math.random() < 0.3, playing: true, tempo: 0.12 }, { pad_mode: "Sampler", touched: false, playing: false, tempo: null }], vinyl: true, knob_mode: "filter", fadefx: false, controller_messages: 100, rekordbox_messages: 50 },
      overall: state ? "error" : "ok",
      checks: state ? [
        { id: "controller", state: "ok", title: "Mixtrack Go connected", detail: "Using MIDI port “Mixtrack Go”." },
        { id: "vport", state: "error", title: "One-time install: virtual MIDI driver", detail: "Windows can't create virtual MIDI devices on its own. Install the free loopMIDI…", fix: { type: "open_url", label: "Download loopMIDI", url: "https://example.com" } },
        { id: "rekordbox", state: "ok", title: "rekordbox 7.2.4 installed", detail: "C:\\Program Files\\Pioneer\\rekordbox 7.2.4\\rekordbox.exe" },
        { id: "mapping", state: "error", title: "rekordbox's DDJ-SX file was replaced by the old RekordJog", detail: "Reinstall rekordbox to fix it." },
        { id: "link", state: "pending", title: "Open rekordbox", detail: "Start rekordbox once your controller shows as connected.", fix: { type: "launch_rekordbox", label: "Open rekordbox" } },
      ] : [
        { id: "controller", state: "ok", title: "Mixtrack Go connected", detail: "Using MIDI port “Mixtrack Go”." },
        { id: "vport", state: "ok", title: "Virtual DDJ-SX ready", detail: "rekordbox will see a “PIONEER DDJ-SX” (built-in CoreMIDI virtual device)." },
        { id: "rekordbox", state: "ok", title: "rekordbox 7.2.4 installed", detail: "/Applications/rekordbox 7/rekordbox.app" },
        { id: "mapping", state: "ok", title: "rekordbox's DDJ-SX mapping is intact", detail: "Nothing to import — RekordJog uses rekordbox's built-in DDJ-SX support." },
        { id: "link", state: "ok", title: "rekordbox is connected", detail: "rekordbox is talking to the virtual DDJ-SX." },
      ],
      events: ev,
      log: [],
      platform: location.hash.includes("win") ? "windows" : "macos",
      version: "2.0.2",
      update: { state: "ready", current: "2.0.2", latest: "2.0.3", waiting_for: "rekordbox is open — it installs once you close rekordbox" },
    });
  }
  return Promise.resolve(null);
}

boot();
