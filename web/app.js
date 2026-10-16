// otto's web page. Every action here is one `/api` call to the same core the CLI uses, so the
// page and `otto ls` never disagree. Everything a wake wrote — gate questions, handoffs, logs,
// pane contents — is untrusted text and only ever reaches the DOM as textContent.
"use strict";

const view = document.getElementById("view");

// ---------------------------------------------------------------------------
// plumbing
// ---------------------------------------------------------------------------

/** h("div.card", {onclick}, child, "text", ...) — a tiny element builder. */
function h(spec, attrs, ...children) {
  const [tag, ...classes] = spec.split(".");
  const el = document.createElement(tag || "div");
  if (classes.length) el.className = classes.join(" ");
  if (attrs && (typeof attrs !== "object" || attrs instanceof Node || Array.isArray(attrs))) {
    children.unshift(attrs);
    attrs = null;
  }
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value == null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "dataset") Object.assign(el.dataset, value);
    else if (key in el && key !== "list" && key !== "form") el[key] = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

async function api(path, body) {
  const init = body === undefined
    ? {}
    : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) };
  const response = await fetch("/api" + path, init);
  let data = null;
  try { data = await response.json(); } catch { /* an empty or non-JSON body */ }
  if (!response.ok) {
    const error = new Error((data && data.error) || `${response.status} ${response.statusText}`);
    error.status = response.status;
    throw error;
  }
  return data;
}

let toastTimer;
function toast(message, bad = false) {
  const el = document.getElementById("toast");
  el.textContent = message;
  el.className = bad ? "bad" : "";
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, bad ? 9000 : 4000);
}

/** Disable `button` while `work` runs; report its error, if any, as a toast. */
async function busy(button, work) {
  const label = button.textContent;
  button.disabled = true;
  try {
    return await work();
  } catch (err) {
    toast(err.message, true);
    return undefined;
  } finally {
    button.disabled = false;
    button.textContent = label;
  }
}

// Mirrors paths::label: a run is `#7 legenda`; one from before numbering is its old id.
function runLabel(id, name) {
  if (/^\d+$/.test(id)) return name ? `#${id} ${name}` : `#${id}`;
  return name && !id.endsWith(name) ? `${id} ${name}` : id;
}

function relative(iso) {
  if (!iso) return "";
  const seconds = Math.round((new Date(iso) - Date.now()) / 1000);
  const abs = Math.abs(seconds);
  const [n, unit] = abs < 60 ? [abs, "s"] : abs < 3600 ? [Math.round(abs / 60), "m"]
    : abs < 86400 * 2 ? [Math.round(abs / 3600), "h"] : [Math.round(abs / 86400), "d"];
  return seconds >= 0 ? `in ${n}${unit}` : `${n}${unit} ago`;
}

/** When something poke acts on is due — mirrors clock::due: once passed, it waits for the next poke. */
function due(iso) {
  if (!iso) return "";
  return new Date(iso) <= Date.now() ? "next poke" : relative(iso);
}

function localTime(iso) {
  return iso ? new Date(iso).toLocaleString() : "";
}

// Mirrors clock::local_clock: just the time today, the day too otherwise.
function clockTime(iso) {
  const d = new Date(iso);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === new Date().toDateString()) return time;
  return `${d.toLocaleDateString([], { month: "short", day: "numeric" })} ${time}`;
}

function statusClass(status) {
  return (status || "").split(" ")[0];
}

function pill(status, live = false) {
  return h("span.pill." + statusClass(status) + (live ? ".live" : ""), status.replace("_", " "));
}

// One line of a run's activity (`core::ActivityLine`): when, the name the wake gave it, and what
// it said. Untrusted text, like everything a wake wrote — it only ever reaches the DOM as text.
function activityLine(a, spec = "div.activity-item") {
  return h(spec + (a.event === "error" ? ".bad" : ""), { title: `${localTime(a.at)} · ${a.event}${a.text ? ": " + a.text : ""}` },
    h("span.when", relative(a.at)),
    h("span.event", a.event),
    h("span.text", a.text));
}

// A todo's counts (`todo::Counts`) as a bar in the marks' proportions — what's finished leads,
// what's left trails — over "accounted for / total". `leftOpen` is a done run that wasn't.
function progressBar(t, leftOpen) {
  const left = t.open + t.doing;
  const segments = [["done", t.done], ["failed", t.failed], ["skipped", t.skipped], ["doing", t.doing], ["open", t.open]];
  const said = [[t.done, "done"], [t.doing, "doing"], [t.open, "to do"], [t.skipped, "skipped"], [t.failed, "failed"]]
    .filter(([n]) => n).map(([n, what]) => `${n} ${what}`).join(" · ");
  return h("div.progress", { title: said },
    h("div.bar", segments.filter(([, n]) => n).map(([mark, n]) => h(`span.${mark}`, { style: `flex-grow:${n}` }))),
    h("small", `${t.total - left}/${t.total}`,
      t.failed ? h("span.failed", ` · ${t.failed} failed`) : null,
      leftOpen ? h("span.left-open", ` · ${left} left open`) : null));
}

// Timers and streams belong to one view; leaving it closes them.
let cleanups = [];
function onLeave(fn) { cleanups.push(fn); }
function every(ms, fn) {
  const id = setInterval(fn, ms);
  onLeave(() => clearInterval(id));
}
function stream(url, handlers) {
  const source = new EventSource(url);
  for (const [name, fn] of Object.entries(handlers)) {
    source.addEventListener(name, (e) => fn(JSON.parse(e.data)));
  }
  onLeave(() => source.close());
  return source;
}

// ---------------------------------------------------------------------------
// ANSI → spans (for the live pane)
// ---------------------------------------------------------------------------

const ANSI16 = [
  "#1d1f24", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4",
  "#5c6370", "#f28b95", "#b5e08f", "#f0d197", "#8cc4ff", "#dba2f0", "#7fd3dd", "#ffffff",
];

function color256(n) {
  if (n < 16) return ANSI16[n];
  if (n >= 232) { const v = 8 + (n - 232) * 10; return `rgb(${v},${v},${v})`; }
  n -= 16;
  const c = (x) => (x === 0 ? 0 : 55 + x * 40);
  return `rgb(${c(Math.floor(n / 36))},${c(Math.floor(n / 6) % 6)},${c(n % 6)})`;
}

function ansiToNodes(text) {
  const out = document.createDocumentFragment();
  let style = {};
  let last = 0;
  const pattern = /\x1b\[([0-9;?]*)([A-Za-z])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][A-Za-z0-9]|\x1b[=>]/g;
  const flush = (chunk) => {
    if (!chunk) return;
    const css = [];
    if (style.fg) css.push(`color:${style.fg}`);
    if (style.bg) css.push(`background:${style.bg}`);
    if (style.bold) css.push("font-weight:700");
    if (style.dim) css.push("opacity:.65");
    if (style.italic) css.push("font-style:italic");
    if (style.underline) css.push("text-decoration:underline");
    if (style.inverse) css.push("filter:invert(1)");
    if (!css.length) { out.append(chunk); return; }
    const span = document.createElement("span");
    span.style.cssText = css.join(";");
    span.textContent = chunk;
    out.append(span);
  };
  for (const match of text.matchAll(pattern)) {
    flush(text.slice(last, match.index));
    last = match.index + match[0].length;
    if (match[2] !== "m") continue;
    const codes = (match[1] || "0").split(";").map((c) => parseInt(c || "0", 10));
    for (let i = 0; i < codes.length; i++) {
      const c = codes[i];
      if (c === 0) style = {};
      else if (c === 1) style.bold = true;
      else if (c === 2) style.dim = true;
      else if (c === 3) style.italic = true;
      else if (c === 4) style.underline = true;
      else if (c === 7) style.inverse = true;
      else if (c === 22) style.bold = style.dim = false;
      else if (c === 23) style.italic = false;
      else if (c === 24) style.underline = false;
      else if (c === 27) style.inverse = false;
      else if (c >= 30 && c <= 37) style.fg = ANSI16[c - 30];
      else if (c >= 90 && c <= 97) style.fg = ANSI16[c - 90 + 8];
      else if (c === 39) style.fg = null;
      else if (c >= 40 && c <= 47) style.bg = ANSI16[c - 40];
      else if (c >= 100 && c <= 107) style.bg = ANSI16[c - 100 + 8];
      else if (c === 49) style.bg = null;
      else if (c === 38 || c === 48) {
        let value = null;
        if (codes[i + 1] === 5) { value = color256(codes[i + 2]); i += 2; }
        else if (codes[i + 1] === 2) { value = `rgb(${codes[i + 2]},${codes[i + 3]},${codes[i + 4]})`; i += 4; }
        if (c === 38) style.fg = value; else style.bg = value;
      }
    }
  }
  flush(text.slice(last));
  return out;
}

// ---------------------------------------------------------------------------
// answering a gate — shared by the runs list and the run page
// ---------------------------------------------------------------------------

async function sendAnswer(id, body, button) {
  const result = await busy(button, () => api(`/runs/${encodeURIComponent(id)}/answer`, body));
  if (!result) return false;
  const how = result.wake ? (result.wake.note || "the run is continuing") : result.note;
  toast(`Recorded “${result.answer.slice(0, 60)}” against gate ${result.gateId}${how ? " — " + how : ""}`);
  return true;
}

function optionButtons(id, options, defaultOption, getNoWake, after) {
  return h("div.options", options.map((option) => h(
    "button" + (option === defaultOption ? ".primary" : ""),
    {
      onclick: async (e) => {
        if (await sendAnswer(id, { choice: option, noWake: getNoWake() }, e.currentTarget)) after();
      },
    },
    option,
    option === defaultOption ? h("span.tag", "default") : null,
  )));
}

// ---------------------------------------------------------------------------
// #/ — runs
// ---------------------------------------------------------------------------

let showAll = false;
try { showAll = localStorage.getItem("otto.all") === "1"; } catch { /* storage unavailable */ }

function runsView() {
  const needs = h("section");
  const banner = h("div");
  const table = h("div");
  const allToggle = h("input", {
    type: "checkbox",
    checked: showAll,
    onchange: (e) => {
      showAll = e.target.checked;
      try { localStorage.setItem("otto.all", showAll ? "1" : "0"); } catch { /* ignore */ }
      refresh();
    },
  });
  view.replaceChildren(
    h("div.row.spread",
      h("h1", "Runs"),
      h("div.row", h("label.check", allToggle, "Include finished"), h("a.btn", { href: "#/new" }, "New run"))),
    banner, needs,
    h("h2", "All runs"),
    table,
  );

  async function refresh() {
    let data;
    try {
      data = await api("/runs" + (showAll ? "?all=true" : ""));
    } catch (err) {
      banner.replaceChildren(h("div.banner.bad", err.message));
      return;
    }
    banner.replaceChildren(data.strandedSleepers
      ? h("div.banner.warn",
        `${data.strandedSleepers} run(s) are sleeping on a timer, but the reviver is not loaded — they will never wake on their own. `,
        h("a", { href: "#/agent" }, "Start the reviver"))
      : "");
    renderNeeds(data.needsYou);
    renderTable(data.rows);
  }

  function renderNeeds(list) {
    if (!list.length) { needs.replaceChildren(); return; }
    needs.replaceChildren(
      h("h2", `Needs you · ${list.length}`),
      ...list.map((n) => h("div.card.needs",
        h("div.row.spread",
          h("div", h("h3", h("a", { href: `#/run/${encodeURIComponent(n.id)}` }, runLabel(n.id, n.name))), h("div.faint.mono", n.label)),
          h("a.btn", { href: `#/run/${encodeURIComponent(n.id)}` }, "Read the question")),
        n.options.length
          ? optionButtons(n.id, n.options, n.options[0], () => false, refresh)
          : h("p.muted", "Free-form question — open it to answer."))),
    );
  }

  function renderTable(rows) {
    if (!rows.length) {
      table.replaceChildren(h("div.card.empty",
        h("p", showAll ? "No runs yet." : "No live runs."),
        h("a.btn", { href: "#/new" }, "Start one")));
      return;
    }
    const columns = ["Run", "Status", "Phase", "Waiting on", "Next wake", "Period", "Wakes"];
    table.replaceChildren(h("table.table",
      h("thead", h("tr", columns.map((t) => h("th", t)))),
      // A tbody per run, so the run's row and its where-this-is line under it are one thing to
      // hover and click.
      rows.map((r) => h("tbody", { onclick: () => { location.hash = `#/run/${encodeURIComponent(r.id)}`; } },
        h(r.whereThisIs || r.lastActivity ? "tr.has-where" : "tr",
          // The name leads — it's what a person recognises; the number underneath is what to type.
          h("td.run",
            h("a", { href: `#/run/${encodeURIComponent(r.id)}`, onclick: (e) => e.stopPropagation() }, r.name || r.id),
            h("small", /^\d+$/.test(r.id) ? `#${r.id}` : r.id)),
          h("td", { dataset: { label: "" } }, pill(r.status, r.running)),
          h("td", { dataset: { label: "phase" } }, r.phase, r.todo ? progressBar(r.todo, r.todoLeftOpen) : null),
          h("td", { dataset: { label: "waiting on" } }, r.blocking),
          h("td", { dataset: { label: "next wake" }, title: r.nextWakeAt ? localTime(r.nextWakeAt) : "" },
            r.nextWakeAt && !r.running && !r.terminal ? `${due(r.nextWakeAt)} · ${clockTime(r.nextWakeAt)}` : "—"),
          h("td.mono", { dataset: { label: "period" } }, r.period),
          h("td.mono", { dataset: { label: "wakes" } }, r.wakes)),
        // The last wake's own account of where the run stands, clamped to two lines, and the newest
        // line of its activity on one; the rest of each is on hover and on the run page.
        r.whereThisIs || r.lastActivity
          ? h("tr.where", h("td", { colSpan: columns.length },
            r.whereThisIs ? h("div", { title: r.whereThisIs }, r.whereThisIs) : null,
            r.lastActivity ? activityLine(r.lastActivity, "div.last") : null))
          : null)),
    ));
  }

  refresh();
  every(5000, refresh);
}

// ---------------------------------------------------------------------------
// #/run/:id
// ---------------------------------------------------------------------------

function runView(id) {
  const path = `/runs/${encodeURIComponent(id)}`;
  // What a person wants first: where the run stands, what it needs, what it has done. Everything
  // else — the handoff, the journal, a live wake, the settings — is a click away below.
  const head = h("div");
  const summary = h("div");
  const gateBox = h("div");
  const todoBox = h("div");
  const activityBox = h("div");
  const notesBox = h("div");
  let shownGate = null;

  // ----- the drill-down. Each section is built once, so a refresh never closes it, and whether
  // it was left open is remembered per viewer.
  function drill(key, title, body, onToggle) {
    const note = h("span.faint");
    const el = h("details.drill", h("summary", h("span.drill-title", title), note), body);
    try { el.open = localStorage.getItem(`otto.drill.${key}`) === "1"; } catch { /* storage unavailable */ }
    el.addEventListener("toggle", () => {
      try { localStorage.setItem(`otto.drill.${key}`, el.open ? "1" : "0"); } catch { /* ignore */ }
      if (onToggle) onToggle(el.open);
    });
    return { el, note, body };
  }
  const handoff = drill("handoff", "Handoff", h("div"));
  const log = drill("log", "Log", h("div"), (open) => setStream("log", open));
  const live = drill("live", "Live wake", h("div"), (open) => setStream("live", open));
  const settings = drill("settings", "Run settings", h("div"));
  log.note.textContent = "every event otto and the wakes wrote — filter it, follow it";
  const actions = h("div");
  const factsEl = h("div");
  const checkEl = h("div");
  settings.body.append(actions, factsEl, checkEl);

  view.replaceChildren(
    h("p", h("a.faint", { href: "#/" }, "← Runs")),
    head, summary, gateBox, todoBox, activityBox, notesBox,
    h("h2", "Details"),
    h("div.drills", handoff.el, log.el, live.el, settings.el),
  );

  async function refresh() {
    let d;
    try {
      d = await api(path);
    } catch (err) {
      summary.replaceChildren(h("div.banner.bad", err.message));
      return;
    }
    renderHead(d);
    renderSummary(d);
    renderGate(d);
    renderTodo(d);
    renderActivity(d);
    renderNotes(d);
    renderHandoff(d);
    renderSettings(d);
    live.note.textContent = d.running ? `wake ${d.state.wake ? d.state.wake.n : ""} is running — what it is saying and doing` : "no wake is running now";
  }

  function renderHead(d) {
    if (head.dataset.editing) return; // a rename is being typed; a refresh must not eat it
    head.replaceChildren(h("div.row.spread",
      h("div",
        h("div.row.title",
          h("h1", runLabel(d.state.id, d.name)),
          h("button.ghost.small", { title: "Rename this run", onclick: () => renameInPlace(d) }, "Rename")),
        d.state.formerId ? h("div.faint.mono", `was ${d.state.formerId}`) : ""),
      pill(d.statusLabel, d.running)),
      h("p.goal", d.state.goal));
  }

  // The name is edited where it is shown. Any run can be renamed, finished or not: nothing on
  // disk is keyed by the name, and the page's URL is the run's number, which never changes.
  function renameInPlace(d) {
    head.dataset.editing = "1";
    const input = h("input.rename", { value: d.name, "aria-label": "Name", spellcheck: false });
    const close = () => { delete head.dataset.editing; refresh(); };
    let saveButton;
    const save = (button) => busy(button, async () => {
      if (input.value.trim() === d.name) { close(); return; }
      const r = await api(path + "/rename", { name: input.value });
      toast(r.from === r.to ? "Name unchanged" : `Renamed to ${runLabel(r.id, r.to)}`);
      close();
    });
    const number = /^\d+$/.test(d.state.id) ? `#${d.state.id}` : d.state.id;
    head.replaceChildren(h("div.row.spread",
      h("div",
        h("div.row.title",
          h("h1", number),
          input,
          (saveButton = h("button.primary.small", { onclick: (e) => save(e.currentTarget) }, "Save")),
          h("button.ghost.small", { onclick: close }, "Cancel")),
        h("div.faint", "Letters, digits and hyphens — not just a number, and not another run's name.")),
      pill(d.statusLabel, d.running)));
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") saveButton.click();
      if (e.key === "Escape") close();
    });
    input.focus();
    input.select();
  }

  // Mirrors clock::format_minutes: the largest unit that divides evenly.
  function formatMinutes(m) {
    if (m % 1440 === 0) return `${m / 1440}d`;
    if (m % 60 === 0) return `${m / 60}h`;
    return `${m}m`;
  }

  // What the run is doing right now, as a person would say it: a state, then its particulars.
  function now(d) {
    const s = d.state;
    const next = s.nextWakeAt ? `${due(s.nextWakeAt)} · ${clockTime(s.nextWakeAt)}` : null;
    if (d.running) return ["Working now", `wake ${s.wake ? s.wake.n : ""}, started ${s.wake ? relative(s.wake.startedAt) : ""}`];
    if (s.gate) return ["Waiting on you", `gate ${s.gate.id}, ${s.gate.slug} — below`];
    // `limitedUntil` matching the wake time is `RunState::usage_limited`.
    if (s.limitedUntil && s.limitedUntil === s.nextWakeAt) return ["Waiting out the usage limit", `resumes ${next}`];
    switch (s.status) {
      case "sleeping": return ["Asleep", next ? `next wake ${next}` : "nothing scheduled"];
      case "blocked": return ["Blocked", d.blocking];
      case "done": return ["Done", s.updatedAt ? relative(s.updatedAt) : ""];
      case "stopped": return ["Stopped", "resume it to bring it back"];
      case "failed": return ["Failed", "resume it to try again"];
      default: return ["Running", d.blocking];
    }
  }

  function renderSummary(d) {
    const [state, detail] = now(d);
    const status = d.state.status;
    summary.replaceChildren(
      h("div.card.summary",
        d.headline ? h("p.headline", d.headline) : h("p.headline.muted", "Nothing from a wake yet."),
        d.whereMore ? h("p.more", d.whereMore) : null,
        h("p.now", h("strong", state), detail ? ` · ${detail}` : "",
          d.running ? h("button.ghost.small", { style: "margin-left:10px", onclick: () => { live.el.open = true; live.el.scrollIntoView({ behavior: "smooth" }); } }, "Watch") : null),
        d.todo ? progressBar(d.todo.counts, d.todoLeftOpen) : null),
      // Not a gate: the run carries on without an answer. It is what it would like from you.
      d.needs ? h("div.banner.info.needs-callout", h("strong", "It could use"), h("div.needs-text", d.needs)) : "",
      d.blockedExplanation ? h("div.banner.bad", { style: "margin-top:12px;white-space:pre-wrap" }, d.blockedExplanation) : "",
      status === "stopped" || status === "failed"
        ? h("div.row", { style: "margin-top:12px" }, h("button.primary", {
          disabled: d.running,
          title: d.running ? "A wake is still running" : "Bring this run back and wake it",
          onclick: (e) => busy(e.currentTarget, async () => {
            const r = await api(path + "/resume", {});
            toast(`${runLabel(r.id)} is ${r.status}${r.note ? " — " + r.note : ""}`);
            refresh();
          }),
        }, "Resume run"))
        : "",
    );
  }

  function renderGate(d) {
    const key = d.gate ? d.gate.id : "none";
    if (key === shownGate) return; // don't wipe a half-written answer on refresh
    shownGate = key;
    if (!d.gate) { gateBox.replaceChildren(); return; }
    const g = d.gate;
    const noWake = h("input", { type: "checkbox" });
    const text = h("textarea", { placeholder: "Or answer in your own words…" });
    const after = () => { shownGate = null; refresh(); };
    gateBox.replaceChildren(
      h("h2", `Waiting on you — gate ${g.id}, ${g.slug}`),
      h("div.card.needs",
        // The question alone: the file's header and its unanswered `## Answer` are the run's
        // bookkeeping, and the whole file is in the log a click away.
        g.question ? h("div.prose", g.question.trim()) : g.text != null ? h("div.prose", g.text) : h("p.muted", `(gate file ${g.file} is missing)`),
        g.options.length ? optionButtons(d.state.id, g.options, g.default, () => noWake.checked, after) : "",
        text,
        h("div.row.spread", { style: "margin-top:10px" },
          h("label.check", noWake, "Record only — don't wake the run yet"),
          h("button", {
            onclick: async (e) => {
              if (!text.value.trim()) { toast("Write an answer first", true); return; }
              if (await sendAnswer(d.state.id, { text: text.value, noWake: noWake.checked }, e.currentTarget)) after();
            },
          }, "Send answer"))),
    );
  }

  // The run's checklist, as the wake last wrote it (`todo::Todo`). Read-only: a person steers it
  // with a note, since the wake is its only writer. Its bar is in the summary above.
  const marks = { open: ["○", "to do"], doing: ["◐", "doing"], done: ["✓", "done"], skipped: ["–", "skipped"], failed: ["✕", "failed"] };
  let shownTodo = null;

  function renderTodo(d) {
    const key = JSON.stringify([d.todo, d.todoLeftOpen]);
    if (key === shownTodo) return;
    shownTodo = key;
    const t = d.todo;
    if (!t) { todoBox.replaceChildren(); return; }
    const left = t.counts.open + t.counts.doing;
    todoBox.replaceChildren(
      h("h2", `Todo · ${t.counts.total - left}/${t.counts.total}`),
      d.todoLeftOpen
        ? h("div.banner.warn", `Marked done with ${left} item(s) still open, and nothing says whether they happened. They're highlighted below.`)
        : "",
      h(d.todoLeftOpen ? "div.card.left-open" : "div.card", { style: "margin-top:10px" },
        h("div.todo", { style: "margin-top:0" }, t.lines.map((line) => {
          if (line.kind === "heading") return h(line.level === 1 ? "div.todo-title" : "div.todo-group", line.text);
          if (line.kind === "text") return h("p.muted.todo-text", line.text);
          const [glyph, name] = marks[line.mark];
          return h(`div.todo-item.${line.mark}`,
            h("span.mark", { title: name, "aria-label": name }, glyph),
            h("div", line.text, line.detail.length ? h("div.detail", line.detail.join("\n")) : null));
        }))),
    );
  }

  // What the run's wakes have said they did, newest first — for a run that watches or repeats,
  // this is its progress, where a run with an end has its todo. The newest few; the rest on asking.
  // Redrawn every refresh, so the times stay current; there is nothing in it to lose.
  const ACTIVITY_FIRST = 5;
  let activityAll = false;
  function renderActivity(d) {
    if (!d.activity.length) { activityBox.replaceChildren(); return; }
    const shown = activityAll ? d.activity : d.activity.slice(0, ACTIVITY_FIRST);
    const rest = d.activity.length - shown.length;
    activityBox.replaceChildren(
      h("h2", "Activity"),
      h("div.card", { style: "margin-top:10px" },
        h("div.activity", shown.map((a) => activityLine(a))),
        rest > 0 || activityAll
          ? h("button.ghost.small", { style: "margin-top:10px", onclick: () => { activityAll = !activityAll; renderActivity(d); } },
            activityAll ? "Show fewer" : `Show ${rest} more`)
          : null),
    );
  }

  // The form is built once and kept, so a refresh never wipes a half-written note; only the
  // list above it is redrawn, and only when it changed.
  const noteList = h("div");
  const noteText = h("textarea", { placeholder: "Tell the run something — how to do the work, not an answer to a gate. Its next wake reads it verbatim." });
  const noteStanding = h("input", { type: "checkbox" });
  const noteNow = h("input", { type: "checkbox" });
  const noteForm = h("div.card", { style: "margin-top:10px" },
    noteText,
    h("div.row.spread", { style: "margin-top:10px" },
      h("div.row",
        h("label.check", { title: "Given to every wake until you drop it" }, noteStanding, "Standing"),
        h("label.check", { title: "Wake the run now to read it" }, noteNow, "Wake now")),
      h("button", {
        onclick: (e) => busy(e.currentTarget, async () => {
          if (!noteText.value.trim()) { toast("Write the note first", true); return; }
          const r = await api(path + "/notes", { text: noteText.value, standing: noteStanding.checked, now: noteNow.checked });
          const woke = r.wake ? " — waking it now" : r.notWoken ? ` — not waking it: ${r.notWoken}` : "";
          toast(`Note ${r.note.id} recorded — ${r.delivery}${woke}`, false);
          noteText.value = "";
          noteStanding.checked = false;
          noteNow.checked = false;
          shownNotes = null;
          refresh();
        }),
      }, "Leave note")));
  let shownNotes = null;

  function renderNotes(d) {
    const over = ["done", "failed", "stopped"].includes(d.state.status);
    const key = JSON.stringify([over, d.notes.map((n) => [n.id, n.givenToWake])]);
    if (key === shownNotes) return;
    shownNotes = key;
    if (over && !d.notes.length) { notesBox.replaceChildren(); return; }
    noteList.replaceChildren(...d.notes.map((n) => {
      const state = n.standing ? "standing — every wake"
        : n.givenToWake ? `given to wake ${n.givenToWake}, not yet delivered` : "not yet read";
      return h("div.card", { style: "margin-top:10px" },
        h("div.row.spread",
          h("span.faint", `Note ${n.id} · ${state} · added ${relative(n.addedAt)}`),
          h("button.ghost", {
            title: n.standing ? "Retire it: no wake is given it again" : "Withdraw it before a wake reads it",
            onclick: (e) => busy(e.currentTarget, async () => {
              await api(`${path}/notes/${encodeURIComponent(n.id)}/drop`, {});
              toast(`Dropped note ${n.id}`);
              shownNotes = null;
              refresh();
            }),
          }, "Drop")),
        h("div.prose", n.text));
    }));
    notesBox.replaceChildren(
      h("h2", "Tell the run something"),
      noteList,
      over ? h("p.muted", "The run is over — no wake will read a new note.") : noteForm);
  }

  // ----- details: the handoff, whole
  let shownHandoff = null;
  function renderHandoff(d) {
    handoff.note.textContent = d.handoff ? "what the last wake left the next one, in full" : "none yet";
    if (d.handoff === shownHandoff) return;
    shownHandoff = d.handoff;
    handoff.body.replaceChildren(d.handoff ? h("div.prose", { style: "margin-top:0" }, d.handoff) : h("p.muted", "No handoff yet."));
  }

  // ----- details: the settings — what the run is, what it costs, and the controls
  function renderSettings(d) {
    const s = d.state;
    const over = ["done", "failed", "stopped"].includes(s.status);
    settings.note.textContent = [
      `phase ${s.phase}`,
      over ? null : `every ${formatMinutes(s.policy.periodMinutes || 60)}`,
      `${s.budget.spentWakes} wake(s)`,
      d.usage.wakes > 0 ? `${humaniseTokens(tokenTotal(d.usage))} tokens` : null,
      over ? null : d.check ? "check script" : "no check script",
    ].filter(Boolean).join(" · ");
    const rows = [
      ["Goal", s.goal],
      ["Done when", s.doneCondition || (s.policy.perpetual ? "never — perpetual; stop it to retire it" : "not yet decided — the next wake proposes one and asks")],
      ["Wraps", `${d.wrapsKind}${s.wraps.ref ? " " + s.wraps.ref : ""}`],
      ["Phase", s.phase],
    ];
    if (s.wake) {
      rows.push(["Wake", `${s.wake.n} — ${d.running ? "running now" : "finished"}${s.wake.outcome ? ", " + s.wake.outcome : ""} · started ${relative(s.wake.startedAt)}`]);
    }
    let used = `${s.budget.spentWakes} wake(s)`;
    if (s.budget.wakes > 0) used += ` of ${s.budget.wakes}`;
    if (s.budget.hours > 0) used += `, ${s.budget.hours}h budget`;
    rows.push(["Used", used]);
    if (s.incompleteWakes > 0) rows.push(["Failed", `${s.incompleteWakes} wake(s) in a row did not finish`]);
    if (!over) {
      const limited = s.limitedUntil && s.limitedUntil === s.nextWakeAt ? " — when the usage limit resets" : "";
      rows.push(["Next wake", s.nextWakeAt && !d.running ? `${due(s.nextWakeAt)} (${localTime(s.nextWakeAt)})${limited}`
        : d.running ? "set when this wake finishes" : s.gate ? "after the gate is answered" : "nothing scheduled"]);
      rows.push(["Period", `every ${formatMinutes(s.policy.periodMinutes || 60)}`]);
    }
    rows.push(["Launcher", `${s.launcher.kind || ""} · wakes in ${s.launcher.detach}`]);
    if (d.usage.wakes > 0) rows.push(["Tokens", tokenSummary(d.usage)]);
    if (d.workdir && d.workdir.kind === "own") rows.push(["Workdir", `${d.workdir.path} (not the default)`]);
    if (d.workdir && d.workdir.kind === "unset") rows.push(["Workdir", "none — set a default with otto config workdir <dir>"]);
    factsEl.replaceChildren(h("div.card", { style: "margin-top:12px" }, h("dl.facts", rows.map(([k, v]) => [h("dt", k), h("dd", v)]))));
    checkEl.replaceChildren(over ? "" : renderCheck(d));
    renderActions(d);
  }

  // The check script is the cheapest wake there is, so the page says plainly whether this run has
  // one — and, when it doesn't, what each full wake is costing instead.
  function renderCheck(d) {
    const c = d.check;
    const cost = d.wakeCost
      ? `~${d.wakeCost.avgTurns} turns and ${humaniseTokens(d.wakeCost.avgCacheCreation)} cache-creation tokens a wake (last ${d.wakeCost.wakes})`
      : null;
    if (!c) {
      return h("div", h("h3.sub", "Check script"),
        h("div.banner.warn", { style: "white-space:pre-wrap" },
          "None — every wake is a full model session" + (cost ? `, ${cost}` : "") + ".\n" +
          "A script poke runs before each wake, with no model, spends one only when something changed:\n",
          h("span.mono", `otto check ${d.short} --script <file>`)));
    }
    const period = formatMinutes(d.state.policy.periodMinutes || 60);
    const retry = c.retrySeconds ? formatMinutes(Math.max(1, Math.round(c.retrySeconds / 60))) : period;
    const last = c.lastResult
      ? `${c.lastResult.replace("-", " ")}${c.lastAt ? " " + relative(c.lastAt) : ""}${c.lastNote ? " — " + c.lastNote : ""}`
      : "not run yet";
    const rows = [
      ["Runs", `${c.script} before each wake, every ${retry} · ${!c.pinned ? "set by a wake, for this sleep only" : c.setByWake ? "set by the run itself" : "set by you"}`],
      ["Last", last],
      ["Saved", `${c.noChangeTotal} check(s) found nothing, each a wake not spent${cost ? " — " + cost : ""}`],
      ["Safety net", c.wakeAfter ? `wakes anyway after ${c.wakeAfter} in a row (${c.consecutiveNoChange} so far)` : "off"],
    ];
    return h("div", h("h3.sub", "Check script"),
      h("div.card", { style: "margin-top:10px" },
        h("dl.facts", rows.map(([k, v]) => [h("dt", k), h("dd", v)])),
        c.text != null ? h("div.prose", c.text)
          : h("p.muted", `(${c.script} is missing — poke will treat it as an error and wake the run)`)));
  }

  // Resume is the summary's, for a stopped or failed run; these are for a live one.
  function renderActions(d) {
    const status = d.state.status;
    if (["done", "stopped", "failed"].includes(status)) { actions.replaceChildren(); return; }
    if (actions.dataset.open) return; // a form is being filled in
    actions.replaceChildren(h("div.row", { style: "margin-top:4px" },
      h("button", {
        disabled: d.running,
        title: d.running ? "A wake is already running" : "Start one wake now",
        onclick: (e) => busy(e.currentTarget, async () => {
          const r = await api(path + "/wake", {});
          toast(r.note || "Wake started");
          refresh();
        }),
      }, "Wake now"),
      h("button", { onclick: () => periodForm(d) }, "Change period…"),
      h("button.danger", { onclick: () => stopForm() }, "Stop run…")));
  }

  function periodForm(d) {
    actions.dataset.open = "period";
    const current = formatMinutes(d.state.policy.periodMinutes || 60);
    const input = h("input", { value: current, placeholder: "30m, 4h, 1d" });
    const close = () => { delete actions.dataset.open; refresh(); };
    let saveButton;
    const save = (button) => busy(button, async () => {
      const r = await api(path + "/period", { period: input.value });
      toast(`Wakes every ${formatMinutes(r.periodMinutes)}` + (r.rescheduled ? ` — next wake ${due(r.nextWakeAt)}`
        : r.nextWakeAt ? ` — the timer a wake armed (${due(r.nextWakeAt)}) is kept` : ""));
      close();
    });
    actions.replaceChildren(h("div.card", { style: "margin-top:4px" },
      h("h3", "Change the period"),
      h("p.muted", `How often it wakes when a wake doesn't ask for something else, now every ${current}. ` +
        "A sleep already scheduled on the old period moves to the new one; a timer a wake armed itself is kept."),
      h("div.field", h("span", "Wakes every"), input),
      h("div.row", { style: "margin-top:10px" },
        h("button.ghost", { onclick: close }, "Cancel"),
        (saveButton = h("button.primary", { onclick: (e) => save(e.currentTarget) }, "Save")))));
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") saveButton.click(); });
    input.focus();
    input.select();
  }

  function stopForm() {
    actions.dataset.open = "stop";
    const reason = h("input", { placeholder: "Why (optional)" });
    const failed = h("input", { type: "checkbox" });
    const close = () => { delete actions.dataset.open; refresh(); };
    actions.replaceChildren(h("div.card", { style: "margin-top:4px" },
      h("h3", "Retire this run?"),
      h("p.muted", "It is marked stopped (or failed), its repo locks are released, and any wake still running is killed. You can resume it later."),
      h("div.field", h("span", "Reason"), reason),
      h("div.row.spread", { style: "margin-top:10px" },
        h("label.check", failed, "It failed — it could not do its job"),
        h("div.row",
          h("button.ghost", { onclick: close }, "Cancel"),
          h("button.danger", {
            onclick: (e) => busy(e.currentTarget, async () => {
              const r = await api(path + "/stop", { reason: reason.value, failed: failed.checked });
              toast(`${runLabel(r.id)} is ${r.status}${r.killedWake ? " — killed the live wake" : ""}`);
              close();
            }),
          }, "Stop run")))));
    reason.focus();
  }

  // ----- details: the log and the live wake stream only while their section is open
  const streams = { log: null, live: null };
  function setStream(which, open) {
    if (open && !streams[which]) streams[which] = which === "log" ? logsPanel(log.body) : livePanel(live.body);
    if (!open && streams[which]) { streams[which](); streams[which] = null; }
  }

  function logsPanel(panel) {
    const decisions = h("input", { type: "checkbox" });
    const follow = h("input", { type: "checkbox", checked: true });
    const since = h("input", { placeholder: "since: 2h, 3d, 2026-09-15" });
    const events = h("input", { placeholder: "events: gate-opened,gate-closed" });
    const out = h("pre.term.wrap");
    let source = null;
    const append = (lines) => {
      const atBottom = out.scrollTop + out.clientHeight >= out.scrollHeight - 30;
      for (const line of lines) {
        for (const part of line.text.split("\n")) {
          if (part.startsWith("──")) { out.append(h("span.rule", part), "\n"); continue; }
          const m = part.match(/^(\S+\s+)(\S+)(.*)$/);
          if (!m) { out.append(part, "\n"); continue; }
          const ev = m[2];
          const cls = ev.startsWith("gate-") ? ".gate" : /incomplete|killed|abandoned|exhausted|lost/.test(ev) ? ".bad" : "";
          out.append(m[1], h("span.ev" + cls, ev), m[3], "\n");
        }
      }
      if (atBottom) out.scrollTop = out.scrollHeight;
    };
    const load = async () => {
      if (source) { source.close(); source = null; }
      const q = new URLSearchParams();
      if (decisions.checked) q.set("decisions", "true");
      if (since.value.trim()) q.set("since", since.value.trim());
      if (events.value.trim()) q.set("event", events.value.trim());
      out.replaceChildren();
      if (follow.checked) {
        source = new EventSource(`/api${path}/logs/stream?${q}`);
        source.addEventListener("header", (e) => out.append(h("span.header", JSON.parse(e.data)), "\n"));
        source.addEventListener("lines", (e) => append(JSON.parse(e.data)));
        source.onerror = () => { /* the browser reconnects on its own */ };
      } else {
        try {
          const page = await api(`${path}/logs?${q}`);
          out.append(h("span.header", page.header), "\n");
          append(page.lines);
          out.scrollTop = out.scrollHeight;
        } catch (err) { out.append(err.message); }
      }
    };
    for (const input of [decisions, follow]) input.addEventListener("change", load);
    for (const input of [since, events]) input.addEventListener("keydown", (e) => { if (e.key === "Enter") load(); });
    panel.replaceChildren(
      h("div.filters", since, events, h("label.check", decisions, "Decisions only"), h("label.check", follow, "Follow")),
      out);
    load();
    return () => { if (source) source.close(); };
  }

  function livePanel(panel) {
    const status = h("p.muted", "Connecting…");
    const out = h("pre.term");
    const source = new EventSource(`/api${path}/live`);
    const show = (text, label) => {
      const atBottom = out.scrollTop + out.clientHeight >= out.scrollHeight - 30;
      out.replaceChildren(ansiToNodes(text));
      status.replaceChildren(pill("running", true), " ", label);
      if (atBottom) out.scrollTop = out.scrollHeight;
    };
    // What the wake has said and done, from its transcript: the normal case.
    const marks = { said: "●", tool: "▸", result: "  ", error: "✗" };
    source.addEventListener("activity", (e) => {
      const atBottom = out.scrollTop + out.clientHeight >= out.scrollHeight - 30;
      const entries = JSON.parse(e.data);
      out.classList.add("wrap");
      out.replaceChildren(...entries.map((a) => h(`div.act.${a.kind}`,
        h("span.at", a.at ? new Date(a.at).toLocaleTimeString() : ""), " ",
        h("span.mark", marks[a.kind] || " "), " ",
        h("span.text", a.text))));
      if (!entries.length) out.append("The wake has started; nothing to show yet.");
      status.replaceChildren(pill("running", true), " ", "what the wake is saying and doing — `otto attach` follows the same feed");
      if (atBottom) out.scrollTop = out.scrollHeight;
    });
    source.addEventListener("pane", (e) => show(JSON.parse(e.data), "tmux pane (this wake's transcript can't be read)"));
    source.addEventListener("log", (e) => show(JSON.parse(e.data), "wake.log (this wake is not in tmux)"));
    source.addEventListener("ended", (e) => {
      source.close();
      status.replaceChildren(JSON.parse(e.data));
      if (!out.childNodes.length) out.append("No wake is running.");
    });
    source.onerror = () => { if (source.readyState === EventSource.CLOSED) status.textContent = "Disconnected."; };
    panel.replaceChildren(status, out);
    return () => source.close();
  }

  onLeave(() => { setStream("log", false); setStream("live", false); });
  refresh();
  setStream("log", log.el.open);
  setStream("live", live.el.open);
  every(5000, refresh);
}

// ---------------------------------------------------------------------------
// tokens — shared by the run page and #/usage
// ---------------------------------------------------------------------------

// Mirrors core::humanise.
function humaniseTokens(n) {
  return n < 10000 ? String(n) : n < 1e6 ? `${Math.round(n / 1000)}k` : `${(n / 1e6).toFixed(1)}M`;
}

function tokenTotal(t) {
  return t.inputTokens + t.outputTokens + t.cacheCreation + t.cacheRead;
}

// Mirrors TokenTotals::summary.
function tokenSummary(t) {
  const h = humaniseTokens;
  return `${h(tokenTotal(t))} over ${t.wakes} wake(s) — ${h(t.inputTokens)} in, ${h(t.outputTokens)} out, ` +
    `${h(t.cacheCreation)} cache write, ${h(t.cacheRead)} cache read` +
    (t.unmeasured ? ` (${t.unmeasured} not measured)` : "");
}

// ---------------------------------------------------------------------------
// #/usage
// ---------------------------------------------------------------------------

function usageView() {
  const windows = [["24h", "Last 24 hours"], ["7d", "Last 7 days"], ["30d", "Last 30 days"], ["", "All time"]];
  let since = "7d";
  let by = "run";
  const out = h("div");
  const picker = (name, options, current, pick) => h("div.segmented", options.map(([value, label]) =>
    h("label", h("input", { type: "radio", name, value, checked: value === current,
      onchange: () => { pick(value); load(); } }), label)));
  view.replaceChildren(
    h("h1", "Usage"),
    h("p.muted", "What wakes have used, in tokens, across every run — finished ones included. Read from each wake's session transcript."),
    h("div.row", { style: "margin:14px 0;flex-wrap:wrap;gap:10px" },
      picker("since", windows, since, (v) => { since = v; }),
      picker("by", [["run", "By run"], ["day", "By day"]], by, (v) => { by = v; })),
    out);

  async function load() {
    let r;
    try { r = await api(`/usage?since=${encodeURIComponent(since)}&by=${by}`); } catch (err) { out.replaceChildren(h("p", err.message)); return; }
    if (!r.rows.length) { out.replaceChildren(h("p.muted", "No wakes in this window.")); return; }
    const hT = humaniseTokens;
    const cells = (label, t, link) => [
      h("td.id", link ? h("a", { href: link }, label) : label),
      h("td", { dataset: { label: "wakes" } }, `${t.wakes}${t.unmeasured ? "*" : ""}`),
      h("td", { dataset: { label: "total" } }, h("strong", hT(tokenTotal(t)))),
      h("td", { dataset: { label: "input" } }, hT(t.inputTokens)),
      h("td", { dataset: { label: "output" } }, hT(t.outputTokens)),
      h("td", { dataset: { label: "cache write" } }, hT(t.cacheCreation)),
      h("td", { dataset: { label: "cache read" } }, hT(t.cacheRead))];
    out.replaceChildren(
      h("table.table",
        h("thead", h("tr", [by === "run" ? "Run" : "Day", "Wakes", "Total", "Input", "Output", "Cache write", "Cache read"].map((c) => h("th", c)))),
        h("tbody",
          r.rows.map((row) => h("tr", cells(row.label, row, by === "run" ? `#/run/${encodeURIComponent(row.key)}` : null))),
          h("tr", cells("Total", r.totals, null)))),
      r.totals.unmeasured
        ? h("p.muted", { style: "margin-top:10px" }, `* includes ${r.totals.unmeasured} wake(s) whose usage couldn't be read — usually a sandbox that hides ~/.claude. Counted, tokens unknown.`)
        : "");
  }
  load();
}

// ---------------------------------------------------------------------------
// #/new
// ---------------------------------------------------------------------------

function newRunView() {
  const f = {};
  const field = (name, label, input, help) => {
    f[name] = input;
    return h("label.field", h("span", label), input, help ? h("small", help) : null);
  };
  const lines = (el) => el.value.split("\n").map((s) => s.trim()).filter(Boolean);
  const select = (options, value) => h("select", options.map(([v, l]) => h("option", { value: v, selected: v === value }, l)));

  const wrap = h("div.segmented", ["goal", "skill", "instructions"].map((kind) =>
    h("label", h("input", { type: "radio", name: "wrap", value: kind, checked: kind === "goal", onchange: syncWrap }),
      { goal: "Just the goal", skill: "A skill", instructions: "An instructions file" }[kind])));
  const skillField = field("skill", "Skill name", h("input", { placeholder: "manage-pr" }));
  const instrField = field("instructions", "Instructions file", h("input", { placeholder: "/path/to/runbook.md" }), "A path on this machine.");
  function syncWrap() {
    const kind = wrap.querySelector("input:checked").value;
    skillField.hidden = kind !== "skill";
    instrField.hidden = kind !== "instructions";
  }

  const result = h("div");
  let saveDefault;
  const form = h("form.form", { onsubmit: (e) => e.preventDefault() },
    h("fieldset", h("legend", "What to run"),
      field("goal", "Goal", h("textarea", { required: true, placeholder: "What this run is trying to achieve" }),
        "The only thing that survives every wake unaltered."),
      field("name", "Name", h("input", { placeholder: "legenda-picks" }),
        "Short and unique — what you'll call it. Empty: made up from the target, the wrapped skill or the goal."),
      h("div.field", h("span", "Wraps"), wrap),
      skillField, instrField,
      h("div.grid2",
        field("target", "Target", h("input", { placeholder: "PROJ-123, a PR URL…" }), "Recorded as facts.target."),
        field("repos", "Repositories", h("textarea", { rows: 2, placeholder: "One directory per line" })))),
    h("fieldset", h("legend", "When it is done"),
      field("until", "Done when", h("input", { placeholder: "Otherwise the first wake proposes one and asks" })),
      h("label.check", { style: "margin-top:10px" }, (f.perpetual = h("input", { type: "checkbox" })), "Perpetual — never done; retired by stopping it"),
      field("period", "Wakes every", h("input", { placeholder: "1h" }), "30m, 1h, 1d. How often it wakes when a wake doesn't ask for something else."),
      h("div.grid2",
        field("budgetWakes", "Wake budget", h("input", { type: "number", min: 0, value: 0 }), "Block with a gate after this many wakes. 0: unlimited."),
        field("budgetHours", "Hours budget", h("input", { type: "number", min: 0, value: 0 }), "0: unlimited."))),
    h("fieldset", h("legend", "Where it runs"),
      h("div.grid2",
        field("launcher", "Launcher", select([["claude", "claude"]], "claude"),
          "A sandbox is the right choice for anything unattended. Add launchers in config.json under otto's home."),
        field("detach", "Wakes run in", select([["tmux", "tmux session"], ["none", "detached process"]], "tmux"),
          "tmux lets you watch a wake live.")),
      field("workdir", "Working directory", h("input", { placeholder: "~/work" }),
        "Where every wake starts. Empty: the default."),
      (saveDefault = h("label.check", { hidden: true },
        (f.saveWorkdir = h("input", { type: "checkbox", checked: true })), "Make this the default for new runs"))),
    h("fieldset", h("legend", "What it may do"),
      field("permissionMode", "Permission mode", select([
        ["bypassPermissions", "bypassPermissions (default)"], ["acceptEdits", "acceptEdits"], ["auto", "auto"],
        ["dontAsk", "dontAsk"], ["manual", "manual"], ["plan", "plan"]], "bypassPermissions"),
      "A wake is unattended; the real guardrail is the launcher."),
      h("div.grid2",
        field("allowedTools", "Allowed tools", h("textarea", { rows: 2, placeholder: "Read\nBash(git *)" })),
        field("disallowedTools", "Denied tools", h("textarea", { rows: 2, placeholder: "WebFetch" })))),
    h("details", h("summary", "Advanced"),
      h("fieldset", { style: "margin-top:10px" },
        field("id", "Run id", h("input", { placeholder: "the next number" })),
        field("phase", "Starting phase", h("input", { value: "start" })),
        h("div.grid2",
          field("fact", "Facts", h("textarea", { rows: 2, placeholder: "branch=feat/x" }), "K=V, one per line."),
          field("policy", "Policy", h("textarea", { rows: 2, placeholder: "maxWakeMinutes=45" }), "K=V, one per line.")))),
    h("div.row",
      h("button.primary", { type: "button", onclick: (e) => submit(e.currentTarget, false) }, "Start run"),
      h("button", { type: "button", onclick: (e) => submit(e.currentTarget, true) }, "Dry run")),
    result,
  );
  syncWrap();
  // The launchers and the default working directory are the machine's, from config.json, so
  // they come from the server. With no default, the field is required and offers to save one.
  let defaultWorkdir = null;
  api("/meta").then((m) => {
    const current = f.launcher.value;
    f.launcher.replaceChildren(...m.launchers.map((name) => h("option", { value: name, selected: name === current }, name)));
    defaultWorkdir = m.workdir;
    if (m.workdir) {
      f.workdir.placeholder = m.workdir;
    } else {
      f.workdir.required = true;
      f.workdir.nextElementSibling.textContent = "Where every wake starts. No default is set yet — give one.";
      saveDefault.hidden = false;
    }
  }).catch(() => {});

  function body() {
    const kind = wrap.querySelector("input:checked").value;
    const text = (name) => f[name].value.trim() || null;
    const b = {
      goal: f.goal.value.trim(),
      skill: kind === "skill" ? text("skill") : null,
      instructions: kind === "instructions" ? text("instructions") : null,
      target: text("target"),
      repos: lines(f.repos),
      until: text("until"),
      perpetual: f.perpetual.checked,
      period: text("period"),
      budgetWakes: Number(f.budgetWakes.value) || 0,
      budgetHours: Number(f.budgetHours.value) || 0,
      launcher: f.launcher.value,
      workdir: text("workdir"),
      detach: f.detach.value,
      permissionMode: f.permissionMode.value,
      allowedTools: lines(f.allowedTools),
      disallowedTools: lines(f.disallowedTools),
      id: text("id"),
      name: text("name"),
      phase: text("phase") || "start",
      fact: lines(f.fact),
      policy: lines(f.policy),
    };
    for (const k of Object.keys(b)) if (b[k] === null) delete b[k];
    return b;
  }

  async function submit(button, dry) {
    const b = body();
    if (!b.goal) { toast("A run needs a goal", true); f.goal.focus(); return; }
    if (b.until && b.perpetual) { toast("A run is either perpetual or has a done-condition, not both", true); return; }
    if (b.skill === undefined && wrap.querySelector("input:checked").value === "skill") { toast("Name the skill", true); return; }
    if (!b.workdir && !defaultWorkdir) { toast("Give a working directory — there is no default yet", true); f.workdir.focus(); return; }
    if (b.workdir && !defaultWorkdir && f.saveWorkdir.checked && !dry) {
      const saved = await busy(button, () => api("/config/workdir", { workdir: b.workdir }));
      if (!saved) return;
      defaultWorkdir = b.workdir;
    }
    const r = await busy(button, () => api("/runs" + (dry ? "?dryRun=true" : ""), b));
    if (!r) return;
    if (dry) {
      result.replaceChildren(h("div.card",
        h("h3", "Dry run — nothing was created"),
        h("dl.facts", { style: "margin-top:10px" }, h("dt", "Run"), h("dd.mono", runLabel(r.planned.id, r.planned.name))),
        h("div.prose", r.planned.firstWake)));
      return;
    }
    if (r.wakeError) toast(`Created ${runLabel(r.id)}, but its first wake did not start: ${r.wakeError}`, true);
    else toast(`Started ${runLabel(r.id)}`);
    location.hash = `#/run/${encodeURIComponent(r.id)}`;
  }

  view.replaceChildren(h("h1", "New run"), h("p.muted", "The same run `otto run` would start — every field is one of its flags."), form);
}

// ---------------------------------------------------------------------------
// #/agent
// ---------------------------------------------------------------------------

function agentView() {
  const card = h("div.card");
  const output = h("div");
  view.replaceChildren(
    h("h1", "Reviver"),
    h("p.muted", "The launchd agent that runs `otto poke` every five minutes. Without it, a sleeping run never wakes on its own."),
    card, output);

  async function refresh() {
    let s;
    try { s = await api("/agent"); } catch (err) { card.replaceChildren(h("p", err.message)); return; }
    const badge = s.loaded === true ? h("span.pill.done", "loaded")
      : s.loaded === false ? h("span.pill.failed", "not loaded") : h("span.pill", "unknown");
    card.replaceChildren(
      h("div.row.spread", h("h3", "Status"), badge),
      h("dl.facts", { style: "margin-top:12px" },
        h("dt", "Last poke"), h("dd", s.lastPoke ? `${s.lastPokeRelative} (${localTime(s.lastPoke)})` : "never (no log yet)"),
        h("dt", "Plist"), h("dd.mono", s.plist),
        h("dt", "Log"), h("dd.mono", s.log)),
      h("div.row", { style: "margin-top:14px" },
        h("button.primary", {
          onclick: (e) => busy(e.currentTarget, async () => {
            const r = await api("/agent/start", {});
            toast(`The reviver is running (${r.target})`);
            refresh();
          }),
        }, s.loaded ? "Restart" : "Start"),
        h("button", {
          disabled: s.loaded === false,
          onclick: (e) => busy(e.currentTarget, async () => {
            const r = await api("/agent/stop", {});
            toast(r.wasRunning ? "Stopped the reviver" : "The reviver was not running");
            refresh();
          }),
        }, "Stop"),
        h("button", {
          title: "Start the wakes that are due now, as launchd would",
          onclick: (e) => busy(e.currentTarget, async () => {
            const r = await api("/poke", {});
            output.replaceChildren(h("h2", "Poke"), h("pre.term.wrap", [...r.lines, ...r.errors].join("\n")));
            refresh();
          }),
        }, "Poke now")),
    );
  }
  refresh();
  every(15000, refresh);
}

// ---------------------------------------------------------------------------
// router
// ---------------------------------------------------------------------------

function route() {
  for (const fn of cleanups) fn();
  cleanups = [];
  const hash = location.hash || "#/";
  const run = hash.match(/^#\/run\/(.+)$/);
  const section = run ? "runs" : hash === "#/new" ? "new" : hash === "#/agent" ? "agent" : hash === "#/usage" ? "usage" : "runs";
  for (const a of document.querySelectorAll("nav a")) a.classList.toggle("active", a.dataset.nav === section);
  if (run) runView(decodeURIComponent(run[1]));
  else if (hash === "#/new") newRunView();
  else if (hash === "#/agent") agentView();
  else if (hash === "#/usage") usageView();
  else runsView();
  window.scrollTo(0, 0);
}

window.addEventListener("hashchange", route);
route();
api("/meta").then((m) => {
  document.getElementById("home").textContent = `otto ${m.version} · ${m.home}`;
}).catch(() => {});
