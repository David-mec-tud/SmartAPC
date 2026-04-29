use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    deployment::{DeploymentCommands, windows_service_commands},
    project::ProjectService,
    project_config::{LogicParamValue, PlotConfig, ProjectConfig},
    runtime::{ExecutionSnapshot, RuntimeManager, RuntimeSnapshot},
    simulation::SimulationSnapshot,
    storage::{SeriesCatalogEntry, SignalSample},
    types::SignalValue,
};

#[derive(Clone)]
pub struct AppState {
    pub project_service: ProjectService,
    pub runtime: Arc<RuntimeManager>,
    pub executable_path: PathBuf,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/plots", get(plots_page))
        .route("/assets/plotly-2.35.2.min.js", get(plotly_asset))
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/{name}/config",
            get(get_project_config).put(update_project_config),
        )
        .route(
            "/api/projects/{name}/plots/config",
            get(get_project_plots_config).put(update_project_plots_config),
        )
        .route("/api/projects/{name}/sidebar", get(get_sidebar_tasks))
        .route("/api/runtime/start", post(start_runtime))
        .route("/api/runtime/stop", post(stop_runtime))
        .route("/api/runtime/state", get(get_runtime_state))
        .route(
            "/api/runtime/params",
            get(get_runtime_params).put(set_runtime_params),
        )
        .route("/api/runtime/plots/window", get(get_runtime_plots_window))
        .route("/api/runtime/plots/catalog", get(get_runtime_plots_catalog))
        .route("/api/simulation/start", post(start_simulation))
        .route("/api/simulation/stop", post(stop_simulation))
        .route("/api/simulation/state", get(get_simulation_state))
        .route("/api/execution/state", get(get_execution_state))
        .route("/api/execution/params", get(get_execution_params))
        .route(
            "/api/execution/plots/window",
            get(get_execution_plots_window),
        )
        .route(
            "/api/execution/plots/catalog",
            get(get_execution_plots_catalog),
        )
        .route("/api/plots", get(get_plots))
        .route("/api/simulate/step", post(simulate_step))
        .route("/api/deployment/commands", get(get_deployment_commands))
        .with_state(state)
}

pub async fn serve(state: AppState, bind: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed binding web server at {bind}"))?;

    axum::serve(listener, build_router(state))
        .await
        .context("web server failed")
}

async fn index_page() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>SmartAPC V1</title>
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <style>
    :root {
      --bg: #f3f7fb;
      --panel: #ffffff;
      --text: #1a2b3c;
      --muted: #5a6f85;
      --accent: #0f7b6c;
      --border: #d9e3ee;
    }
    body {
      margin: 0;
      font-family: "Source Sans 3", "Segoe UI", sans-serif;
      background:
        radial-gradient(circle at 15% 5%, #d2ebe8 0%, transparent 30%),
        radial-gradient(circle at 85% 0%, #e7e5f9 0%, transparent 25%),
        var(--bg);
      color: var(--text);
    }
    .container { max-width: 960px; margin: 48px auto; padding: 0 20px; }
    .panel { background: var(--panel); border: 1px solid var(--border); border-radius: 16px; padding: 24px; box-shadow: 0 14px 30px rgba(15, 27, 45, 0.07); }
    h1 { margin: 0 0 10px; font-size: 30px; }
    p { margin: 0 0 14px; color: var(--muted); }
    code { background: #ecf3fa; padding: 2px 6px; border-radius: 6px; }
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 16px; margin-top: 18px; }
    .card { border: 1px solid var(--border); border-radius: 12px; padding: 16px; }
    .card h3 { margin: 0 0 8px; }
  </style>
</head>
<body>
  <div class="container">
    <div class="panel">
      <h1>SmartAPC V1</h1>
      <p>Modulare Regelungssoftware mit OPC-UA, Watchdog, Runtime-State-Machine und SQLite-Plots.</p>
      <p>API Startpunkte: <code>/api/projects</code>, <code>/api/runtime/start</code>, <code>/api/simulation/start</code>, <code>/api/execution/state</code>.</p>
      <p><a href="/plots">Interaktive Plot-Ansicht öffnen</a></p>
      <div class="grid">
        <div class="card"><h3>Projektkonfiguration</h3><p>TOML-basierte Projekte, Sidebar-Tasks und Parametrierung.</p></div>
        <div class="card"><h3>Runtime</h3><p>State: Running -> Hold -> Alarm -> Off mit konfigurierbaren Timeouts.</p></div>
        <div class="card"><h3>Plots</h3><p>Ein-/Ausgänge und Zyklus-Metriken werden lokal in SQLite gespeichert.</p></div>
      </div>
    </div>
  </div>
</body>
</html>"#,
    )
}

async fn plots_page() -> Html<&'static str> {
    Html(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>SmartAPC Plots</title>
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <script src="/assets/plotly-2.35.2.min.js"></script>
  <style>
    :root {
      --bg: #f7f8f4;
      --surface: #ffffff;
      --ink: #1f2c2c;
      --muted: #6e7b7b;
      --border: #d5ddd8;
      --accent: #1f7a68;
      --accent-soft: #d8efe9;
      --danger: #ae3f3f;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      color: var(--ink);
      font-family: "Avenir Next", "Segoe UI", sans-serif;
      background:
        radial-gradient(circle at 12% 10%, #dbece0 0%, transparent 34%),
        radial-gradient(circle at 88% 0%, #f3e8d7 0%, transparent 30%),
        var(--bg);
    }
    .wrap { max-width: 1440px; margin: 24px auto 36px; padding: 0 16px; }
    .hero {
      border: 1px solid var(--border);
      border-radius: 14px;
      background: var(--surface);
      box-shadow: 0 18px 35px rgba(16, 30, 24, 0.08);
      padding: 16px;
      margin-bottom: 16px;
    }
    h1 { margin: 0 0 8px; font-size: 28px; }
    .muted { color: var(--muted); }
    .toolbar {
      margin-top: 12px;
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      gap: 10px;
      align-items: end;
    }
    .field { display: flex; flex-direction: column; gap: 4px; }
    label { font-size: 13px; color: var(--muted); }
    select, input, button {
      border: 1px solid var(--border);
      border-radius: 8px;
      background: #fff;
      color: var(--ink);
      padding: 8px 10px;
      font: inherit;
    }
    button {
      cursor: pointer;
      background: var(--accent-soft);
      border-color: #b5d9cf;
    }
    button:hover { background: #cae6df; }
    .danger {
      color: #fff;
      border-color: #7e2a2a;
      background: var(--danger);
    }
    .status { margin-top: 8px; font-size: 13px; color: var(--muted); min-height: 18px; }
    .grid3 {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
      gap: 12px;
    }
    .section {
      border: 1px solid var(--border);
      border-radius: 12px;
      background: var(--surface);
      overflow: hidden;
    }
    .section-head {
      padding: 10px 12px;
      border-bottom: 1px solid var(--border);
      background: #f7faf8;
      font-weight: 600;
      display: flex;
      justify-content: space-between;
      align-items: center;
    }
    .cards { padding: 10px; display: grid; gap: 10px; }
    .plot-card {
      border: 1px solid var(--border);
      border-radius: 10px;
      background: #fff;
      padding: 8px;
    }
    .plot-card[draggable="true"] { cursor: grab; }
    .plot-title {
      display: flex;
      justify-content: space-between;
      align-items: center;
      margin-bottom: 6px;
      font-size: 14px;
      font-weight: 600;
    }
    .chart { width: 100%; height: 240px; }
    .custom-builder {
      border-top: 1px solid var(--border);
      padding: 10px;
      display: grid;
      gap: 8px;
      background: #fbfcfb;
    }
    .trace-list {
      display: flex;
      flex-wrap: wrap;
      gap: 6px;
    }
    .trace-chip {
      display: inline-flex;
      gap: 6px;
      align-items: center;
      border: 1px solid var(--border);
      border-radius: 999px;
      padding: 4px 8px;
      font-size: 12px;
      background: #fff;
    }
    .trace-chip button {
      padding: 0 6px;
      border-radius: 999px;
      line-height: 1.2;
    }
  </style>
</head>
<body>
  <div class="wrap">
    <div class="hero">
      <h1>Interactive Execution Plots</h1>
      <div class="muted">Live sliding-window plots for runtime or offline simulation, plus configurable custom charts.</div>
      <div class="toolbar">
        <div class="field">
          <label for="projectSelect">Project</label>
          <select id="projectSelect"></select>
        </div>
        <div class="field">
          <label for="windowSec">Window (seconds)</label>
          <input id="windowSec" type="number" min="10" max="3600" value="180" />
        </div>
        <div class="field">
          <label>Execution</label>
          <div id="executionState" class="muted">loading...</div>
        </div>
      </div>
      <div id="status" class="status"></div>
    </div>

    <div class="grid3">
      <section class="section">
        <div class="section-head"><span>MVs</span><span class="muted">drag to reorder</span></div>
        <div id="mvCards" class="cards"></div>
      </section>
      <section class="section">
        <div class="section-head"><span>CVs</span><span class="muted">drag to reorder</span></div>
        <div id="cvCards" class="cards"></div>
      </section>
      <section class="section">
        <div class="section-head"><span>DVs</span><span class="muted">drag to reorder</span></div>
        <div id="dvCards" class="cards"></div>
      </section>
    </div>

    <section class="section" style="margin-top:12px;">
      <div class="section-head"><span>Custom Plots</span><span class="muted">choose from online params and logged series</span></div>
      <div id="customCards" class="cards"></div>
      <div class="custom-builder">
        <div class="grid3">
          <div class="field">
            <label for="customTitle">Title</label>
            <input id="customTitle" placeholder="e.g. Feedforward + Disturbance" />
          </div>
          <div class="field">
            <label for="traceDirection">Direction</label>
            <select id="traceDirection"></select>
          </div>
          <div class="field">
            <label for="traceSignal">Signal</label>
            <select id="traceSignal"></select>
          </div>
        </div>
        <div>
          <button id="addTraceBtn" type="button">Add Trace</button>
          <button id="createCustomBtn" type="button">Create Custom Plot</button>
          <button id="clearDraftBtn" type="button">Clear Draft</button>
        </div>
        <div id="draftTraces" class="trace-list"></div>
      </div>
    </section>
  </div>

<script>
(() => {
  const MIN_WINDOW = 10;
  const MAX_WINDOW = 3600;
  const POLL_MS = 1000;

  const state = {
    projectName: null,
    projects: [],
    variables: { mvs: [], cvs: [], dvs: [] },
    plotConfig: { window_sec: 180, mv_order: [], cv_order: [], dv_order: [], custom_plots: [] },
    catalog: [],
    catalogSig: "",
    draftTraces: [],
    latestSeries: new Map(),
    plotUi: new Map(),
    saveTimer: null,
    dragKind: null,
    dragName: null,
  };

  const el = {
    projectSelect: document.getElementById("projectSelect"),
    windowSec: document.getElementById("windowSec"),
    executionState: document.getElementById("executionState"),
    status: document.getElementById("status"),
    mvCards: document.getElementById("mvCards"),
    cvCards: document.getElementById("cvCards"),
    dvCards: document.getElementById("dvCards"),
    customCards: document.getElementById("customCards"),
    customTitle: document.getElementById("customTitle"),
    traceDirection: document.getElementById("traceDirection"),
    traceSignal: document.getElementById("traceSignal"),
    addTraceBtn: document.getElementById("addTraceBtn"),
    createCustomBtn: document.getElementById("createCustomBtn"),
    clearDraftBtn: document.getElementById("clearDraftBtn"),
    draftTraces: document.getElementById("draftTraces"),
  };

  function escapeHtml(value) {
    return String(value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;");
  }

  function setStatus(msg, isError = false) {
    el.status.textContent = msg || "";
    el.status.style.color = isError ? "var(--danger)" : "var(--muted)";
  }

  function clampWindow(sec) {
    return Math.max(MIN_WINDOW, Math.min(MAX_WINDOW, sec));
  }

  async function api(path, options) {
    const resp = await fetch(path, options);
    if (!resp.ok) {
      let message = `HTTP ${resp.status}`;
      try {
        const body = await resp.json();
        if (body && body.error) message = body.error;
      } catch (_err) {}
      throw new Error(message);
    }
    return resp.json();
  }

  function normalizeOrder(order, defaults) {
    const allowed = new Set(defaults);
    const seen = new Set();
    const out = [];
    for (const raw of order || []) {
      const name = String(raw || "").trim();
      if (!name || !allowed.has(name) || seen.has(name)) continue;
      seen.add(name);
      out.push(name);
    }
    for (const name of defaults) {
      if (!seen.has(name)) {
        seen.add(name);
        out.push(name);
      }
    }
    return out;
  }

  function normalizePlotConfig() {
    state.plotConfig.window_sec = clampWindow(Number(state.plotConfig.window_sec || 180));
    state.plotConfig.mv_order = normalizeOrder(state.plotConfig.mv_order, state.variables.mvs);
    state.plotConfig.cv_order = normalizeOrder(state.plotConfig.cv_order, state.variables.cvs);
    state.plotConfig.dv_order = normalizeOrder(state.plotConfig.dv_order, state.variables.dvs);
    state.plotConfig.custom_plots = (state.plotConfig.custom_plots || [])
      .map((plot, idx) => {
        const title = String(plot.title || "").trim() || `Custom Plot ${idx + 1}`;
        const traces = [];
        const seen = new Set();
        for (const trace of plot.traces || []) {
          const direction = String(trace.direction || "").trim();
          const signal = String(trace.signal || "").trim();
          if (!direction || !signal) continue;
          const key = `${direction}::${signal}`;
          if (seen.has(key)) continue;
          seen.add(key);
          traces.push({ direction, signal });
        }
        return { title, traces };
      })
      .filter((plot) => plot.traces.length > 0);
  }

  function scheduleSave() {
    clearTimeout(state.saveTimer);
    state.saveTimer = setTimeout(saveConfig, 500);
  }

  async function saveConfig() {
    if (!state.projectName) return;
    normalizePlotConfig();
    try {
      const saved = await api(`/api/projects/${encodeURIComponent(state.projectName)}/plots/config`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(state.plotConfig),
      });
      state.plotConfig = saved;
      if (document.activeElement !== el.windowSec) {
        el.windowSec.value = String(saved.window_sec);
      }
      setStatus(`Plot config saved for ${state.projectName}.`);
    } catch (err) {
      setStatus(`Saving plot config failed: ${err.message}`, true);
    }
  }

  function updateProjectSelect() {
    const options = state.projects
      .map((name) => `<option value="${escapeHtml(name)}">${escapeHtml(name)}</option>`)
      .join("");
    el.projectSelect.innerHTML = options;
    if (state.projectName) {
      el.projectSelect.value = state.projectName;
    }
  }

  function preferredProjectFromQuery() {
    const value = new URLSearchParams(window.location.search).get("project");
    return value ? value.trim() : "";
  }

  async function loadProjectsAndSelectInitial() {
    const [projects, executionState] = await Promise.all([
      api("/api/projects"),
      api("/api/execution/state"),
    ]);
    state.projects = projects;
    updateProjectSelect();
    const preferred = preferredProjectFromQuery();
    if (preferred && projects.includes(preferred)) {
      state.projectName = preferred;
    } else if (
      executionState &&
      executionState.snapshot &&
      executionState.snapshot.project_name &&
      projects.includes(executionState.snapshot.project_name)
    ) {
      state.projectName = executionState.snapshot.project_name;
    } else {
      state.projectName = projects.length > 0 ? projects[0] : null;
    }
    updateProjectSelect();
  }

  async function loadProject(projectName) {
    if (!projectName) return;
    state.projectName = projectName;
    const [cfg, plotCfg] = await Promise.all([
      api(`/api/projects/${encodeURIComponent(projectName)}/config`),
      api(`/api/projects/${encodeURIComponent(projectName)}/plots/config`),
    ]);
    state.variables = cfg.variables || { mvs: [], cvs: [], dvs: [] };
    state.plotConfig = plotCfg;
    normalizePlotConfig();
    el.windowSec.value = String(state.plotConfig.window_sec);
    renderAllCards();
    setStatus(`Loaded plot configuration for ${projectName}.`);
  }

  function seriesKey(direction, signal) {
    return `${direction}::${signal}`;
  }

  function extractSampleValue(sample) {
    if (!sample || !sample.value || typeof sample.value !== "object") return null;
    if (sample.value.kind === "f64") return { y: Number(sample.value.value || 0), isBool: false };
    if (sample.value.kind === "bool") return { y: sample.value.value ? 1 : 0, isBool: true };
    return null;
  }

  function updateSeriesFromSamples(samples) {
    state.latestSeries.clear();
    for (const sample of samples) {
      const parsed = extractSampleValue(sample);
      if (!parsed) continue;
      const key = seriesKey(sample.direction, sample.signal);
      if (!state.latestSeries.has(key)) {
        state.latestSeries.set(key, { direction: sample.direction, signal: sample.signal, points: [], isBool: parsed.isBool });
      }
      const row = state.latestSeries.get(key);
      row.points.push({ t: sample.timestamp_ms, y: parsed.y });
      if (parsed.isBool) row.isBool = true;
    }
  }

  function mergeCatalogFromSamples(samples) {
    const known = new Set(state.catalog.map((s) => seriesKey(s.direction, s.signal)));
    let changed = false;
    for (const sample of samples) {
      const parsed = extractSampleValue(sample);
      if (!parsed) continue;
      const key = seriesKey(sample.direction, sample.signal);
      if (known.has(key)) continue;
      known.add(key);
      changed = true;
      state.catalog.push({
        direction: sample.direction,
        signal: sample.signal,
        value_kind: parsed.isBool ? "bool" : "f64",
      });
    }
    state.catalog.sort((a, b) => {
      const ka = seriesKey(a.direction, a.signal);
      const kb = seriesKey(b.direction, b.signal);
      return ka.localeCompare(kb);
    });
    if (changed) {
      state.catalogSig = catalogSignature(state.catalog);
    }
    return changed;
  }

  function catalogSignature(rows) {
    return rows
      .map((row) => `${row.direction}::${row.signal}::${row.value_kind}`)
      .sort()
      .join("|");
  }

  function setCatalog(rows) {
    const normalized = (rows || [])
      .map((row) => ({
        direction: String(row.direction || "").trim(),
        signal: String(row.signal || "").trim(),
        value_kind: row.value_kind === "bool" ? "bool" : "f64",
      }))
      .filter((row) => row.direction && row.signal);
    const sig = catalogSignature(normalized);
    if (sig === state.catalogSig) {
      return false;
    }
    state.catalog = normalized;
    state.catalogSig = sig;
    return true;
  }

  async function refreshCatalog() {
    try {
      const rows = await api("/api/execution/plots/catalog");
      const changed = setCatalog(rows);
      if (changed) {
        renderTraceSelectors();
      }
    } catch (_err) {
      // Runtime may be stopped; keep previous/fallback catalog.
    }
  }

  function makeTrace(direction, signal, label) {
    return { direction, signal, label: label || signal };
  }

  function mvTraces(name) {
    return [
      makeTrace("output", `mv.${name}.write`, "write"),
      makeTrace("input", `mv.${name}.read`, "read"),
      makeTrace("input", `mv.${name}.hl`, "HL"),
      makeTrace("input", `mv.${name}.ll`, "LL"),
      makeTrace("input", `mv.${name}.enable`, "enable"),
      makeTrace("online_param", `io.mv.${name}.hl`, "HL (online)"),
      makeTrace("online_param", `io.mv.${name}.ll`, "LL (online)"),
    ];
  }

  function cvTraces(name) {
    return [
      makeTrace("input", `cv.${name}.soll`, "soll"),
      makeTrace("input", `cv.${name}.ist`, "ist"),
      makeTrace("input", `cv.${name}.sollband`, "sollband"),
      makeTrace("input", `cv.${name}.enable`, "enable"),
      makeTrace("online_param", `io.cv.${name}.sollband`, "sollband (online)"),
    ];
  }

  function dvTraces(name) {
    return [
      makeTrace("input", `dv.${name}.read`, "read"),
      makeTrace("input", `dv.${name}.enable`, "enable"),
    ];
  }

  function ensureCardsOrder(kind) {
    if (kind === "mv") state.plotConfig.mv_order = normalizeOrder(state.plotConfig.mv_order, state.variables.mvs);
    if (kind === "cv") state.plotConfig.cv_order = normalizeOrder(state.plotConfig.cv_order, state.variables.cvs);
    if (kind === "dv") state.plotConfig.dv_order = normalizeOrder(state.plotConfig.dv_order, state.variables.dvs);
  }

  function onCardDragStart(kind, name, ev) {
    state.dragKind = kind;
    state.dragName = name;
    ev.dataTransfer.effectAllowed = "move";
    ev.dataTransfer.setData("text/plain", `${kind}:${name}`);
  }

  function reorderKind(kind, targetName) {
    if (state.dragKind !== kind || !state.dragName || state.dragName === targetName) return;
    const key = kind === "mv" ? "mv_order" : kind === "cv" ? "cv_order" : "dv_order";
    const list = state.plotConfig[key].slice();
    const from = list.indexOf(state.dragName);
    const to = list.indexOf(targetName);
    if (from < 0 || to < 0) return;
    list.splice(to, 0, list.splice(from, 1)[0]);
    state.plotConfig[key] = list;
    renderAllCards();
    scheduleSave();
  }

  function cardMarkup(kind, name, title) {
    return `
      <div class="plot-card" draggable="true" data-kind="${kind}" data-name="${escapeHtml(name)}">
        <div class="plot-title"><span>${escapeHtml(title)}</span><span class="muted">${escapeHtml(name)}</span></div>
        <div id="chart-${escapeHtml(kind)}-${escapeHtml(name)}" class="chart"></div>
      </div>
    `;
  }

  function renderBuiltInCards(kind, target, order, titlePrefix) {
    ensureCardsOrder(kind);
    target.innerHTML = order.map((name) => cardMarkup(kind, name, `${titlePrefix} ${name}`)).join("");
    for (const card of target.querySelectorAll(".plot-card")) {
      const name = card.dataset.name;
      card.addEventListener("dragstart", (ev) => onCardDragStart(kind, name, ev));
      card.addEventListener("dragover", (ev) => ev.preventDefault());
      card.addEventListener("drop", (ev) => {
        ev.preventDefault();
        reorderKind(kind, name);
      });
    }
  }

  function renderCustomCards() {
    const html = state.plotConfig.custom_plots.map((plot, idx) => `
      <div class="plot-card">
        <div class="plot-title">
          <span>${escapeHtml(plot.title)}</span>
          <button class="danger" data-custom-remove="${idx}" type="button">Remove</button>
        </div>
        <div id="chart-custom-${idx}" class="chart"></div>
      </div>
    `).join("");
    el.customCards.innerHTML = html;
    for (const btn of el.customCards.querySelectorAll("[data-custom-remove]")) {
      btn.addEventListener("click", () => {
        const idx = Number(btn.dataset.customRemove);
        state.plotConfig.custom_plots.splice(idx, 1);
        renderAllCards();
        scheduleSave();
      });
    }
  }

  function renderAllCards() {
    renderBuiltInCards("mv", el.mvCards, state.plotConfig.mv_order, "MV");
    renderBuiltInCards("cv", el.cvCards, state.plotConfig.cv_order, "CV");
    renderBuiltInCards("dv", el.dvCards, state.plotConfig.dv_order, "DV");
    renderCustomCards();
  }

  function renderTraceSelectors(force = false) {
    const active = document.activeElement;
    if (!force && (active === el.traceDirection || active === el.traceSignal)) {
      return;
    }
    const previousDirection = el.traceDirection.value;
    const previousSignal = el.traceSignal.value;
    const directions = [...new Set(state.catalog.map((row) => row.direction))].sort();
    el.traceDirection.innerHTML = directions.map((d) => `<option value="${escapeHtml(d)}">${escapeHtml(d)}</option>`).join("");
    const selectedDirection = directions.includes(previousDirection) ? previousDirection : (directions[0] || "");
    el.traceDirection.value = selectedDirection;
    const signals = state.catalog
      .filter((row) => row.direction === selectedDirection)
      .map((row) => row.signal)
      .sort();
    el.traceSignal.innerHTML = signals.map((s) => `<option value="${escapeHtml(s)}">${escapeHtml(s)}</option>`).join("");
    const selectedSignal = signals.includes(previousSignal) ? previousSignal : (signals[0] || "");
    el.traceSignal.value = selectedSignal;
  }

  function renderDraftTraces() {
    el.draftTraces.innerHTML = state.draftTraces.map((trace, idx) => `
      <span class="trace-chip">
        ${escapeHtml(trace.direction)} :: ${escapeHtml(trace.signal)}
        <button type="button" data-draft-remove="${idx}">x</button>
      </span>
    `).join("");
    for (const btn of el.draftTraces.querySelectorAll("[data-draft-remove]")) {
      btn.addEventListener("click", () => {
        const idx = Number(btn.dataset.draftRemove);
        state.draftTraces.splice(idx, 1);
        renderDraftTraces();
      });
    }
  }

  function plotTracesToSeries(traceSpecs) {
    const out = [];
    for (const trace of traceSpecs) {
      const key = seriesKey(trace.direction, trace.signal);
      const row = state.latestSeries.get(key);
      if (!row || row.points.length === 0) continue;
      out.push({
        x: row.points.map((p) => new Date(p.t)),
        y: row.points.map((p) => p.y),
        mode: "lines",
        name: trace.label ? `${trace.label} (${trace.direction})` : `${trace.signal} (${trace.direction})`,
        line: { width: row.isBool ? 1.6 : 2.2, shape: row.isBool ? "hv" : "linear" },
      });
    }
    return out;
  }

  function getPlotUiState(key) {
    if (!state.plotUi.has(key)) {
      state.plotUi.set(key, { xRange: null });
    }
    return state.plotUi.get(key);
  }

  function bindPlotInteractions(div, key) {
    if (!div || div.dataset.plotUiBound === "1") {
      return;
    }
    div.dataset.plotUiBound = "1";
    div.on("plotly_relayout", (eventData) => {
      const ui = getPlotUiState(key);
      if (eventData && eventData["xaxis.autorange"] === true) {
        ui.xRange = null;
        return;
      }
      if (
        eventData &&
        Object.prototype.hasOwnProperty.call(eventData, "xaxis.range[0]") &&
        Object.prototype.hasOwnProperty.call(eventData, "xaxis.range[1]")
      ) {
        ui.xRange = [eventData["xaxis.range[0]"], eventData["xaxis.range[1]"]];
      }
    });
  }

  function renderPlot(divId, title, traces) {
    const div = document.getElementById(divId);
    if (!div) return;
    const windowMs = state.plotConfig.window_sec * 1000;
    const now = Date.now();
    const uiState = getPlotUiState(divId);
    const data = plotTracesToSeries(traces);
    const layout = {
      title: { text: title, font: { size: 13 } },
      margin: { l: 44, r: 16, t: 34, b: 34 },
      paper_bgcolor: "#fff",
      plot_bgcolor: "#fff",
      xaxis: uiState.xRange
        ? { type: "date", range: uiState.xRange }
        : { type: "date", range: [new Date(now - windowMs), new Date(now)] },
      yaxis: { automargin: true },
      legend: { orientation: "h", y: -0.2 },
      uirevision: divId,
    };
    Plotly.react(div, data, layout, { displaylogo: false, responsive: true }).then(() => {
      bindPlotInteractions(div, divId);
    });
  }

  function renderPlots() {
    for (const name of state.plotConfig.mv_order) {
      renderPlot(`chart-mv-${name}`, `MV ${name}`, mvTraces(name));
    }
    for (const name of state.plotConfig.cv_order) {
      renderPlot(`chart-cv-${name}`, `CV ${name}`, cvTraces(name));
    }
    for (const name of state.plotConfig.dv_order) {
      renderPlot(`chart-dv-${name}`, `DV ${name}`, dvTraces(name));
    }
    state.plotConfig.custom_plots.forEach((plot, idx) => {
      const traces = plot.traces.map((trace) => makeTrace(trace.direction, trace.signal, trace.signal));
      renderPlot(`chart-custom-${idx}`, plot.title, traces);
    });
  }

  async function pollExecution() {
    try {
      const execution = await api("/api/execution/state");
      const snapshot = execution ? execution.snapshot : null;
      if (!snapshot) {
        el.executionState.textContent = "not running";
        return;
      }
      el.executionState.textContent = `${execution.mode}: ${snapshot.state} (${snapshot.project_name})`;
      const windowMs = clampWindow(Number(state.plotConfig.window_sec || 180)) * 1000;
      const samples = await api(`/api/execution/plots/window?window_ms=${windowMs}`);
      updateSeriesFromSamples(samples);
      const catalogChanged = mergeCatalogFromSamples(samples);
      if (catalogChanged) {
        renderTraceSelectors();
      }
      renderPlots();
    } catch (err) {
      el.executionState.textContent = "not running";
      setStatus(`Live update paused: ${err.message}`, true);
    }
  }

  function wireEvents() {
    el.projectSelect.addEventListener("change", async () => {
      try {
        await loadProject(el.projectSelect.value);
      } catch (err) {
        setStatus(`Project load failed: ${err.message}`, true);
      }
    });
    const commitWindowSetting = () => {
      const parsed = Number(el.windowSec.value || state.plotConfig.window_sec);
      if (!Number.isFinite(parsed)) {
        el.windowSec.value = String(state.plotConfig.window_sec);
        return;
      }
      const next = clampWindow(parsed);
      el.windowSec.value = String(next);
      if (next !== state.plotConfig.window_sec) {
        state.plotConfig.window_sec = next;
        scheduleSave();
      }
    };
    el.windowSec.addEventListener("change", commitWindowSetting);
    el.windowSec.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        commitWindowSetting();
      }
    });
    el.traceDirection.addEventListener("change", () => renderTraceSelectors(true));
    el.addTraceBtn.addEventListener("click", () => {
      const direction = String(el.traceDirection.value || "").trim();
      const signal = String(el.traceSignal.value || "").trim();
      if (!direction || !signal) return;
      if (!state.draftTraces.some((t) => t.direction === direction && t.signal === signal)) {
        state.draftTraces.push({ direction, signal });
        renderDraftTraces();
      }
    });
    el.clearDraftBtn.addEventListener("click", () => {
      state.draftTraces = [];
      renderDraftTraces();
    });
    el.createCustomBtn.addEventListener("click", () => {
      if (state.draftTraces.length === 0) return;
      const title = String(el.customTitle.value || "").trim() || `Custom Plot ${state.plotConfig.custom_plots.length + 1}`;
      state.plotConfig.custom_plots.push({
        title,
        traces: state.draftTraces.map((t) => ({ direction: t.direction, signal: t.signal })),
      });
      state.draftTraces = [];
      el.customTitle.value = "";
      renderDraftTraces();
      renderAllCards();
      scheduleSave();
    });
  }

  async function boot() {
    wireEvents();
    renderDraftTraces();
    try {
      await loadProjectsAndSelectInitial();
      if (!state.projectName) {
        setStatus("No projects found. Create one via /api/projects first.", true);
        return;
      }
      await loadProject(state.projectName);
      await refreshCatalog();
      renderTraceSelectors(true);
      await pollExecution();
      setStatus("Interactive plotting ready.");
    } catch (err) {
      setStatus(`Initialization failed: ${err.message}`, true);
    }
    setInterval(pollExecution, POLL_MS);
    setInterval(refreshCatalog, 5000);
  }

  boot();
})();
</script>
</body>
</html>"##,
    )
}

async fn plotly_asset() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_bytes!("../assets/plotly-2.35.2.min.js").as_slice(),
    )
}

#[derive(Debug, Deserialize)]
struct CreateProjectRequest {
    name: String,
}

async fn create_project(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<ProjectConfig>, AppError> {
    let cfg = state.project_service.create_project(&req.name)?;
    Ok(Json(cfg))
}

async fn list_projects(State(state): State<AppState>) -> Result<Json<Vec<String>>, AppError> {
    Ok(Json(state.project_service.list_projects()?))
}

async fn get_project_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ProjectConfig>, AppError> {
    let cfg = state.project_service.load_config(&name)?;
    Ok(Json(cfg))
}

async fn update_project_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(cfg): Json<ProjectConfig>,
) -> Result<Json<ProjectConfig>, AppError> {
    state.project_service.save_config(&name, &cfg)?;
    Ok(Json(cfg))
}

async fn get_project_plots_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<PlotConfig>, AppError> {
    let cfg = state.project_service.get_plot_config(&name)?;
    Ok(Json(cfg))
}

async fn update_project_plots_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(cfg): Json<PlotConfig>,
) -> Result<Json<PlotConfig>, AppError> {
    let saved = state.project_service.update_plot_config(&name, cfg)?;
    Ok(Json(saved))
}

async fn get_sidebar_tasks(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let tasks = state.project_service.sidebar_tasks(&name)?;
    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

#[derive(Debug, Deserialize)]
struct StartRuntimeRequest {
    project_name: String,
}

async fn start_runtime(
    State(state): State<AppState>,
    Json(req): Json<StartRuntimeRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .runtime
        .start(&state.project_service, &req.project_name)
        .await?;
    Ok(Json(
        serde_json::json!({"status": "started", "project": req.project_name}),
    ))
}

async fn stop_runtime(State(state): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    state.runtime.stop().await?;
    Ok(Json(serde_json::json!({"status": "stopped"})))
}

async fn start_simulation(
    State(state): State<AppState>,
    Json(req): Json<StartRuntimeRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .runtime
        .start_simulation(&state.project_service, &req.project_name)
        .await?;
    Ok(Json(
        serde_json::json!({"status": "started", "project": req.project_name}),
    ))
}

async fn stop_simulation(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.runtime.stop_simulation().await?;
    Ok(Json(serde_json::json!({"status": "stopped"})))
}

async fn get_runtime_state(
    State(state): State<AppState>,
) -> Result<Json<Option<RuntimeSnapshot>>, AppError> {
    Ok(Json(state.runtime.state().await?))
}

async fn get_simulation_state(
    State(state): State<AppState>,
) -> Result<Json<Option<SimulationSnapshot>>, AppError> {
    Ok(Json(state.runtime.simulation_state().await?))
}

async fn get_execution_state(
    State(state): State<AppState>,
) -> Result<Json<Option<ExecutionSnapshot>>, AppError> {
    Ok(Json(state.runtime.active_state().await?))
}

async fn get_runtime_params(
    State(state): State<AppState>,
) -> Result<Json<HashMap<String, LogicParamValue>>, AppError> {
    let params = state.runtime.get_params().await?;
    Ok(Json(params))
}

async fn get_execution_params(
    State(state): State<AppState>,
) -> Result<Json<HashMap<String, LogicParamValue>>, AppError> {
    let params = state.runtime.active_get_params().await?;
    Ok(Json(params))
}

#[derive(Debug, Deserialize)]
struct SetParamsRequest {
    params: HashMap<String, LogicParamValue>,
}

async fn set_runtime_params(
    State(state): State<AppState>,
    Json(req): Json<SetParamsRequest>,
) -> Result<Json<HashMap<String, LogicParamValue>>, AppError> {
    let params = state.runtime.set_params(req.params.clone()).await?;

    if let Some(project_name) = state.runtime.active_project().await {
        state
            .project_service
            .update_logic_params(&project_name, req.params)?;
    }

    Ok(Json(params))
}

#[derive(Debug, Deserialize)]
struct PlotQuery {
    limit: Option<usize>,
}

async fn get_plots(
    State(state): State<AppState>,
    Query(query): Query<PlotQuery>,
) -> Result<Json<Vec<SignalSample>>, AppError> {
    let limit = query.limit.unwrap_or(250).clamp(1, 10_000);
    let rows = state.runtime.active_read_plots(limit).await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct RuntimePlotWindowQuery {
    window_ms: Option<u64>,
}

async fn get_runtime_plots_window(
    State(state): State<AppState>,
    Query(query): Query<RuntimePlotWindowQuery>,
) -> Result<Json<Vec<SignalSample>>, AppError> {
    let window_ms = query.window_ms.unwrap_or(180_000).clamp(10_000, 3_600_000);
    let rows = state.runtime.read_plots_window(window_ms).await?;
    Ok(Json(rows))
}

async fn get_execution_plots_window(
    State(state): State<AppState>,
    Query(query): Query<RuntimePlotWindowQuery>,
) -> Result<Json<Vec<SignalSample>>, AppError> {
    let window_ms = query.window_ms.unwrap_or(180_000).clamp(10_000, 3_600_000);
    let rows = state.runtime.active_read_plots_window(window_ms).await?;
    Ok(Json(rows))
}

async fn get_runtime_plots_catalog(
    State(state): State<AppState>,
) -> Result<Json<Vec<SeriesCatalogEntry>>, AppError> {
    let rows = state.runtime.read_plot_catalog().await?;
    Ok(Json(rows))
}

async fn get_execution_plots_catalog(
    State(state): State<AppState>,
) -> Result<Json<Vec<SeriesCatalogEntry>>, AppError> {
    let rows = state.runtime.active_read_plot_catalog().await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct SimulateRequest {
    inputs: HashMap<String, SignalValue>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SimulateResponse {
    outputs: HashMap<String, SignalValue>,
}

async fn simulate_step(
    State(state): State<AppState>,
    Json(req): Json<SimulateRequest>,
) -> Result<Json<SimulateResponse>, AppError> {
    let outputs = state.runtime.simulate_step(req.inputs).await?;
    Ok(Json(SimulateResponse { outputs }))
}

#[derive(Debug, Deserialize)]
struct DeploymentQuery {
    project: String,
}

async fn get_deployment_commands(
    State(state): State<AppState>,
    Query(query): Query<DeploymentQuery>,
) -> Result<Json<DeploymentCommands>, AppError> {
    let cfg = state.project_service.load_config(&query.project)?;
    let commands = windows_service_commands(
        &cfg.deployment,
        &state.executable_path,
        &query.project,
        state.project_service.root(),
    );
    Ok(Json(commands))
}

#[derive(Debug)]
struct AppError(anyhow::Error);

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(value: E) -> Self {
        Self(value.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let msg = self.0.to_string();
        let status = if msg.contains("not running") || msg.contains("not found") {
            StatusCode::NOT_FOUND
        } else if msg.contains("already") || msg.contains("invalid") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };

        (
            status,
            Json(serde_json::json!({
                "error": msg,
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum_test::TestServer;
    use tempfile::tempdir;

    use crate::{
        project::ProjectService,
        project_config::{
            CustomPlotConfig, CustomPlotTraceConfig, CvIoDefinition, LinearStateSpaceModelConfig,
            LogicKind, LogicParamValue, ModelConfig, MvIoDefinition, PControllerLogicConfig,
            PControllerPairConfig, PlotConfig, SignalPoint, WatchdogConfig,
        },
        runtime::RuntimeManager,
        storage::{SeriesCatalogEntry, SqliteStorage},
        types::{SignalType, SignalValue},
    };

    use super::*;

    fn configure_simulation_project(projects: &ProjectService, name: &str) {
        let mut cfg = projects.load_config(name).expect("config");
        cfg.variables.mvs = vec!["u1".to_string()];
        cfg.variables.cvs = vec!["y1".to_string()];
        cfg.io_definition.mvs.push(MvIoDefinition {
            name: "u1".to_string(),
            mv_write: "ns=2;s=U1_WRITE".to_string(),
            mv_read: "ns=2;s=U1_READ".to_string(),
            hl_opc: None,
            ll_opc: None,
            hl: Some(10.0),
            ll: Some(-10.0),
            enable: None,
        });
        cfg.io_definition.cvs.push(CvIoDefinition {
            name: "y1".to_string(),
            soll: "ns=2;s=Y1_SOLL".to_string(),
            ist: "ns=2;s=Y1_IST".to_string(),
            sollband_opc: None,
            sollband_value: Some(0.0),
            enable: None,
        });
        cfg.logic.kind = LogicKind::PController;
        cfg.logic.p_controller = PControllerLogicConfig {
            pairs: vec![PControllerPairConfig {
                cv: "y1".to_string(),
                mv: "u1".to_string(),
                kp: 1.0,
                bias: 0.0,
            }],
        };
        cfg.model = Some(ModelConfig::LinearStateSpace {
            model: LinearStateSpaceModelConfig {
                state_count: 1,
                u_order: vec!["u1".to_string()],
                y_order: vec!["y1".to_string()],
                a: vec![vec![-1.0]],
                b: vec![vec![1.0]],
                c: vec![vec![1.0]],
                d: vec![vec![0.0]],
            },
        });
        cfg.rebuild_runtime_io_from_definition()
            .expect("rebuild runtime io");
        projects.save_config(name, &cfg).expect("save cfg");
    }

    #[tokio::test]
    async fn project_api_roundtrip_and_param_persist() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        let runtime = Arc::new(RuntimeManager::new());
        let app = build_router(AppState {
            project_service: projects.clone(),
            runtime,
            executable_path: std::env::current_exe().expect("exe"),
        });

        let server = TestServer::new(app).expect("server");

        let create_resp = server
            .post("/api/projects")
            .json(&serde_json::json!({"name": "demo"}))
            .await;
        create_resp.assert_status_ok();

        let mut cfg = projects.load_config("demo").expect("config");
        cfg.communication.endpoint_url = "mock://web-project-api".to_string();
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.soll".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=PV_SOL".to_string(),
            opc_tag: None,
        });
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.ist".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=PV_IST".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "mv.u1.write".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=MV".to_string(),
            opc_tag: None,
        });
        cfg.watchdog = WatchdogConfig::None;
        cfg.logic.kind = LogicKind::PController;
        cfg.logic.p_controller = PControllerLogicConfig {
            pairs: vec![PControllerPairConfig {
                cv: "y1".to_string(),
                mv: "u1".to_string(),
                kp: 1.0,
                bias: 0.0,
            }],
        };
        projects.save_config("demo", &cfg).expect("save cfg");

        let start_resp = server
            .post("/api/runtime/start")
            .json(&serde_json::json!({"project_name": "demo"}))
            .await;
        start_resp.assert_status_ok();

        let update_resp = server
            .put("/api/runtime/params")
            .json(&serde_json::json!({
                "params": { "p.y1.u1.kp": 2.5 }
            }))
            .await;
        update_resp.assert_status_ok();

        let cfg2 = projects.load_config("demo").expect("reload cfg");
        assert_eq!(
            cfg2.logic.params.get("p.y1.u1.kp"),
            Some(&LogicParamValue::F64(2.5))
        );

        let sim_resp = server
            .post("/api/simulate/step")
            .json(&serde_json::json!({
                "inputs": {
                    "cv.y1.soll": { "kind": "f64", "value": 3.0 },
                    "cv.y1.ist": { "kind": "f64", "value": 0.0 }
                }
            }))
            .await;
        sim_resp.assert_status_ok();

        let body: SimulateResponse = sim_resp.json();
        assert_eq!(
            body.outputs.get("mv.u1.write"),
            Some(&SignalValue::F64(7.5))
        );

        let _ = server.post("/api/runtime/stop").await;
    }

    #[tokio::test]
    async fn sidebar_endpoint_exposes_tasks() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");

        let app = build_router(AppState {
            project_service: projects,
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        let resp = server.get("/api/projects/demo/sidebar").await;
        resp.assert_status_ok();

        let json: serde_json::Value = resp.json();
        let tasks = json
            .get("tasks")
            .and_then(|v| v.as_array())
            .expect("tasks array");

        assert!(!tasks.is_empty());
    }

    #[tokio::test]
    async fn plots_endpoint_reads_samples() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");

        let mut cfg = projects.load_config("demo").expect("config");
        cfg.communication.endpoint_url = "mock://web-plots".to_string();
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.soll".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=In1Soll".to_string(),
            opc_tag: None,
        });
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.ist".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=In1Ist".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "mv.u1.write".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Out1".to_string(),
            opc_tag: None,
        });
        cfg.logic.kind = LogicKind::PController;
        cfg.logic.p_controller = PControllerLogicConfig {
            pairs: vec![PControllerPairConfig {
                cv: "y1".to_string(),
                mv: "u1".to_string(),
                kp: 1.0,
                bias: 0.0,
            }],
        };
        projects.save_config("demo", &cfg).expect("save cfg");

        let app = build_router(AppState {
            project_service: projects.clone(),
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        server
            .post("/api/runtime/start")
            .json(&serde_json::json!({"project_name": "demo"}))
            .await
            .assert_status_ok();

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

        let plots_resp = server.get("/api/plots?limit=10").await;
        plots_resp.assert_status_ok();

        let samples: Vec<SignalSample> = plots_resp.json();
        assert!(!samples.is_empty());

        let _ = server.post("/api/runtime/stop").await;
    }

    #[tokio::test]
    async fn deployment_commands_endpoint_returns_windows_commands() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");

        let app = build_router(AppState {
            project_service: projects,
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        let resp = server.get("/api/deployment/commands?project=demo").await;
        resp.assert_status_ok();

        let body: DeploymentCommands = resp.json();
        assert!(body.install.contains("sc.exe create"));
        assert!(body.start.contains("sc.exe start"));
    }

    #[tokio::test]
    async fn project_plot_config_endpoint_roundtrip_and_normalization() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");

        let mut cfg = projects.load_config("demo").expect("config");
        cfg.variables.mvs = vec!["u1".to_string(), "u2".to_string()];
        cfg.variables.cvs = vec!["y1".to_string()];
        cfg.variables.dvs = vec!["d1".to_string(), "d2".to_string()];
        projects.save_config("demo", &cfg).expect("save cfg");

        let app = build_router(AppState {
            project_service: projects.clone(),
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        let resp = server
            .put("/api/projects/demo/plots/config")
            .json(&PlotConfig {
                window_sec: 8,
                mv_order: vec!["u2".to_string(), "unknown".to_string(), "u1".to_string()],
                cv_order: vec![],
                dv_order: vec!["d2".to_string()],
                custom_plots: vec![
                    CustomPlotConfig {
                        title: "".to_string(),
                        traces: vec![CustomPlotTraceConfig {
                            direction: "input".to_string(),
                            signal: "cv.y1.ist".to_string(),
                        }],
                    },
                    CustomPlotConfig {
                        title: "empty".to_string(),
                        traces: vec![CustomPlotTraceConfig {
                            direction: "".to_string(),
                            signal: "".to_string(),
                        }],
                    },
                ],
            })
            .await;
        resp.assert_status_ok();

        let saved: PlotConfig = resp.json();
        assert_eq!(saved.window_sec, 10);
        assert_eq!(saved.mv_order, vec!["u2", "u1"]);
        assert_eq!(saved.cv_order, vec!["y1"]);
        assert_eq!(saved.dv_order, vec!["d2", "d1"]);
        assert_eq!(saved.custom_plots.len(), 1);

        let get_resp = server.get("/api/projects/demo/plots/config").await;
        get_resp.assert_status_ok();
        let loaded: PlotConfig = get_resp.json();
        assert_eq!(loaded.window_sec, 10);
        assert_eq!(loaded.custom_plots.len(), 1);
    }

    #[tokio::test]
    async fn runtime_plot_window_and_catalog_endpoints_include_expected_series() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");

        let mut cfg = projects.load_config("demo").expect("config");
        cfg.communication.endpoint_url = "mock://web-runtime-plots".to_string();
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.soll".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=InSoll".to_string(),
            opc_tag: None,
        });
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.ist".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=InIst".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "mv.u1.write".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=OutMv".to_string(),
            opc_tag: None,
        });
        cfg.logic.kind = LogicKind::PController;
        cfg.logic.p_controller = PControllerLogicConfig {
            pairs: vec![PControllerPairConfig {
                cv: "y1".to_string(),
                mv: "u1".to_string(),
                kp: 1.0,
                bias: 0.0,
            }],
        };
        projects.save_config("demo", &cfg).expect("save cfg");

        let app = build_router(AppState {
            project_service: projects.clone(),
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        server
            .post("/api/runtime/start")
            .json(&serde_json::json!({"project_name": "demo"}))
            .await
            .assert_status_ok();

        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

        let db = SqliteStorage::new(
            projects
                .project_dir("demo")
                .join("data")
                .join("runtime.sqlite"),
        )
        .expect("open runtime db");
        db.store_signal("user_log", "ff.signal", &SignalValue::F64(7.0))
            .expect("insert user log sample");

        let window_resp = server
            .get("/api/runtime/plots/window?window_ms=180000")
            .await;
        window_resp.assert_status_ok();
        let window_rows: Vec<SignalSample> = window_resp.json();
        assert!(!window_rows.is_empty());

        let catalog_resp = server.get("/api/runtime/plots/catalog").await;
        catalog_resp.assert_status_ok();
        let catalog: Vec<SeriesCatalogEntry> = catalog_resp.json();

        assert!(catalog.iter().any(|row| row.direction == "input"));
        assert!(catalog.iter().any(|row| row.direction == "output"));
        assert!(catalog.iter().any(|row| row.direction == "online_param"));
        assert!(
            catalog
                .iter()
                .any(|row| row.direction == "user_log" && row.signal == "ff.signal")
        );

        let _ = server.post("/api/runtime/stop").await;
    }

    #[tokio::test]
    async fn simulation_and_active_execution_endpoints_follow_simulation_mode() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");
        configure_simulation_project(&projects, "demo");

        let app = build_router(AppState {
            project_service: projects.clone(),
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        server
            .post("/api/simulation/start")
            .json(&serde_json::json!({"project_name": "demo"}))
            .await
            .assert_status_ok();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let simulation_state_resp = server.get("/api/simulation/state").await;
        simulation_state_resp.assert_status_ok();
        let simulation_state: serde_json::Value = simulation_state_resp.json();
        assert_eq!(simulation_state["project_name"], "demo");

        let execution_state_resp = server.get("/api/execution/state").await;
        execution_state_resp.assert_status_ok();
        let execution_state: serde_json::Value = execution_state_resp.json();
        assert_eq!(execution_state["mode"], "simulation");
        assert_eq!(execution_state["snapshot"]["project_name"], "demo");

        let runtime_start_resp = server
            .post("/api/runtime/start")
            .json(&serde_json::json!({"project_name": "demo"}))
            .await;
        runtime_start_resp.assert_status_bad_request();

        let plots_resp = server
            .get("/api/execution/plots/window?window_ms=180000")
            .await;
        plots_resp.assert_status_ok();
        let samples: Vec<SignalSample> = plots_resp.json();
        assert!(!samples.is_empty());

        let catalog_resp = server.get("/api/execution/plots/catalog").await;
        catalog_resp.assert_status_ok();
        let catalog: Vec<SeriesCatalogEntry> = catalog_resp.json();
        assert!(catalog.iter().any(|row| row.direction == "input"));
        assert!(catalog.iter().any(|row| row.direction == "output"));

        let _ = server.post("/api/simulation/stop").await;
    }
}
