// otto's web page. Every action here is one `/api` call to the same core the CLI uses, so the
// page and `otto ls` never disagree. Everything a wake wrote — questions, messages, handoffs, logs,
// pane contents — is untrusted text and only ever reaches the DOM as text: `md()` below builds
// elements from it, never HTML.
//
// The page says things the way a person would. A run is an "agent" here, a wake is "a run" of it,
// and a gate is a question; the engine's own words are under each agent's "Under the hood".
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

const enc = encodeURIComponent;

/** After a note or a reply: when it will be read, in the page's words — busy or waiting on a
 *  question says why it wasn't run now — or the error, when a run asked for didn't start. */
function heard(r) {
  if (r.wake) return "it's reading it now";
  if (r.notWoken && /did not start/.test(r.notWoken)) return "couldn't run it now: " + r.notWoken;
  return r.deliveryPlain || r.delivery;
}

/** "0 B", "64 KB", "1.2 MB" — as `cleanup::size` says it. */
function sizeText(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let n = bytes / 1024, unit = 0;
  while (n >= 1024 && unit < units.length - 1) { n /= 1024; unit++; }
  return `${n < 10 ? n.toFixed(1) : Math.round(n)} ${units[unit]}`;
}

/** "1 run" / "3 runs". */
const count = (n, one) => `${n} ${n === 1 ? one : one + "s"}`;

/** "in 24m" / "12m ago". */
function relative(iso) {
  if (!iso) return "";
  const seconds = Math.round((new Date(iso) - Date.now()) / 1000);
  const abs = Math.abs(seconds);
  const [n, unit] = abs < 60 ? [abs, "s"] : abs < 3600 ? [Math.round(abs / 60), "m"]
    : abs < 86400 * 2 ? [Math.round(abs / 3600), "h"] : [Math.round(abs / 86400), "d"];
  if (abs < 45) return seconds >= 0 ? "now" : "just now";
  return seconds >= 0 ? `in ${n}${unit}` : `${n}${unit} ago`;
}

/** A relative time spelled out — "in 55 minutes", "2 hours ago" — for sentences a person reads. */
function spelled(iso) {
  const seconds = Math.round((new Date(iso) - Date.now()) / 1000);
  const abs = Math.abs(seconds);
  const [n, unit] = abs < 90 ? [abs, "second"] : abs < 90 * 60 ? [Math.round(abs / 60), "minute"]
    : abs < 36 * 3600 ? [Math.round(abs / 3600), "hour"] : [Math.round(abs / 86400), "day"];
  const text = `${n} ${unit}${n === 1 ? "" : "s"}`;
  return seconds >= 0 ? `in ${text}` : `${text} ago`;
}

function localTime(iso) {
  return iso ? new Date(iso).toLocaleString() : "";
}

// Just the time today, the weekday within the week, the date otherwise.
function clockTime(iso) {
  const d = new Date(iso);
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const today = new Date();
  if (d.toDateString() === today.toDateString()) return time;
  const tomorrow = new Date(today);
  tomorrow.setDate(today.getDate() + 1);
  if (d.toDateString() === tomorrow.toDateString()) return `tomorrow ${time}`;
  if (Math.abs(d - today) < 6 * 86400 * 1000) return `${d.toLocaleDateString([], { weekday: "short" })} ${time}`;
  return `${d.toLocaleDateString([], { month: "short", day: "numeric" })} ${time}`;
}

/** When something comes next, for a person: "9:03 PM", or "any minute" once it is due. */
function nextAt(iso) {
  return new Date(iso) <= Date.now() ? "any minute" : clockTime(iso);
}

// Mirrors clock::format_minutes: the largest unit that divides evenly.
function formatMinutes(m) {
  if (m % 1440 === 0) return `${m / 1440}d`;
  if (m % 60 === 0) return `${m / 60}h`;
  return `${m}m`;
}

// How often, in words: `every hour`, `every 2 days`, `every 30 minutes`.
function everyText(m) {
  if (m % 1440 === 0) return m === 1440 ? "every day" : `every ${m / 1440} days`;
  if (m % 60 === 0) return m === 60 ? "every hour" : `every ${m / 60} hours`;
  return `every ${m} minutes`;
}

// Mirrors paths::label: a run is `#7 legenda`; one from before numbering is its old id.
function runLabel(id, name) {
  if (/^\d+$/.test(id)) return name ? `#${id} ${name}` : `#${id}`;
  return name && !id.endsWith(name) ? `${id} ${name}` : id;
}

const idText = (id) => (/^\d+$/.test(id) ? `#${id}` : id);

// Timers and streams belong to one view; leaving it closes them.
let cleanups = [];
function onLeave(fn) { cleanups.push(fn); }
function every(ms, fn) {
  const id = setInterval(fn, ms);
  onLeave(() => clearInterval(id));
}

function store(key, value) {
  try {
    if (value === undefined) return localStorage.getItem(key);
    localStorage.setItem(key, value);
  } catch { /* storage unavailable */ }
  return null;
}

// ---------------------------------------------------------------------------
// markdown → elements, for the text wakes write
// ---------------------------------------------------------------------------

/** Inline markdown — `code`, **bold**, links — as text and elements. Links only to http(s). */
function inline(text) {
  const out = [];
  const pattern = /(`[^`]+`)|(\*\*[^*]+\*\*)|\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])|(?<![\w*])\*([^*\s][^*]*?)\*(?![\w*])/g;
  let last = 0;
  for (const m of text.matchAll(pattern)) {
    if (m.index > last) out.push(text.slice(last, m.index));
    if (m[1]) out.push(h("code", m[1].slice(1, -1)));
    else if (m[2]) out.push(h("strong", m[2].slice(2, -2)));
    else if (m[3]) out.push(h("a", { href: m[4], target: "_blank", rel: "noopener noreferrer" }, m[3]));
    else if (m[5]) out.push(h("a", { href: m[5], target: "_blank", rel: "noopener noreferrer" }, m[5]));
    else if (m[6]) out.push(h("em", m[6]));
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** Block markdown — headings, lists, quotes, fences, paragraphs — as a `div.md`. */
function md(text) {
  const root = h("div.md");
  const lines = String(text || "").replace(/\r/g, "").split("\n");
  let para = [];
  let list = null;
  const flush = () => {
    if (para.length) root.append(h("p", inline(para.join(" "))));
    para = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    const line = raw.replace(/^\s*>\s?/, "");
    if (/^\s*```/.test(line)) {
      flush(); list = null;
      const body = [];
      for (i++; i < lines.length && !/^\s*```/.test(lines[i]); i++) body.push(lines[i]);
      root.append(h("pre.code", body.join("\n")));
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    const bullet = line.match(/^\s*[-*+]\s+(.*)$/);
    const numbered = line.match(/^\s*(\d+)[.)]\s+(.*)$/);
    if (heading) {
      flush(); list = null;
      root.append(h(heading[1].length <= 2 ? "h3" : "h4", inline(heading[2])));
    } else if (bullet || numbered) {
      flush();
      const tag = bullet ? "ul" : "ol";
      if (!list || list.tagName.toLowerCase() !== tag) {
        list = h(tag);
        if (numbered && numbered[1] !== "1") list.start = Number(numbered[1]);
        root.append(list);
      }
      list.append(h("li", inline(bullet ? bullet[1] : numbered[2])));
    } else if (!line.trim()) {
      flush(); list = null;
    } else if (list && /^\s{2,}\S/.test(raw)) {
      list.lastElementChild.append(" ", ...inline(line.trim()));
    } else {
      list = null;
      para.push(line.trim());
    }
  }
  flush();
  return root;
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
// the bar across the top: the inbox count and whether otto itself is well
// ---------------------------------------------------------------------------

async function refreshChrome() {
  try {
    const inbox = await api("/inbox");
    const count = document.getElementById("inbox-count");
    count.textContent = inbox.open > 99 ? "99+" : String(inbox.open);
    count.hidden = inbox.open === 0;
    document.getElementById("inbox-btn").title = inbox.open ? `${inbox.open} waiting for you` : "Nothing waiting for you";
  } catch { /* the server is down; the next refresh says so */ }
  try {
    const health = await api("/health");
    const el = document.getElementById("health");
    el.className = "health" + (health.ok ? "" : " bad");
    el.replaceChildren(h("span", health.ok ? "All good" : health.problems.length === 1 ? "1 problem" : `${health.problems.length} problems`));
    el.title = health.ok ? "otto is waking your agents on time" : health.problems.join("\n");
    lastHealth = health;
  } catch {
    const el = document.getElementById("health");
    el.className = "health bad";
    el.replaceChildren(h("span", "Can't reach otto"));
  }
}
let lastHealth = null;
refreshChrome();
setInterval(refreshChrome, 10000);

// The problems with otto itself, as a banner — only when there are any.
function healthBanner() {
  const box = h("div");
  const draw = (health) => {
    if (!health || health.ok) { box.replaceChildren(); return; }
    box.replaceChildren(h("div.banner.warn", health.problems.map((p) => h("div", p)), h("a", { href: "#/system" }, "Details")));
  };
  draw(lastHealth);
  api("/health").then(draw).catch(() => {});
  return box;
}

// ---------------------------------------------------------------------------
// pieces shared by several views
// ---------------------------------------------------------------------------

const MOOD_WORDS = {
  waiting: "waiting on you", working: "working now", watching: "watching", idle: "quiet",
  paused: "paused", trouble: "needs a look", finished: "finished",
};

function glyph(mood) {
  return h(`span.sg.${mood || "idle"}`, { title: MOOD_WORDS[mood] || "", "aria-label": MOOD_WORDS[mood] || "" });
}

const KIND_LABEL = { question: "Needs your answer", report: "Report", draft: "Draft to send", update: "Update" };

function kindChip(kind, open = true) {
  return h(`span.kind.${open ? kind : "done"}`, open ? KIND_LABEL[kind] || kind : kind === "question" ? "Answered" : "Done");
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
    h("small", `${t.total - left} of ${t.total} done`,
      t.failed ? h("span.failed", ` · ${t.failed} failed`) : null,
      leftOpen ? h("span.left-open", ` · ${left} left open`) : null));
}

/** Answering a question: its text, its options as buttons, and a box for anything else. */
// The question's own text, less its first line when that line is just `title` again — the inbox
// shows the title above it, and a question usually opens with a heading saying the same.
function withoutTitle(question, title) {
  if (!title) return question;
  const plain = (s) => s.replace(/^#+\s*/, "").replace(/[`*_]/g, "").replace(/…$/, "").trim();
  const lines = question.split("\n");
  const first = lines.findIndex((l) => l.trim());
  if (first < 0 || !plain(title) || !plain(lines[first]).startsWith(plain(title))) return question;
  return lines.slice(first + 1).join("\n");
}

function questionCard(runId, gate, after, title) {
  const text = h("textarea", { placeholder: "Or answer in your own words…", rows: 3 });
  const send = async (body, button) => {
    const r = await busy(button, () => api(`/runs/${enc(runId)}/answer`, body));
    if (!r) return;
    toast(`Answered “${r.answer.slice(0, 60)}”${r.wake ? " — it's carrying on" : r.note ? " — " + r.note : ""}`);
    after();
  };
  return h("div.ask-card",
    md(withoutTitle(gate.question || gate.text || `(the question's file ${gate.file} is missing)`, title)),
    gate.options.length
      ? h("div.options", gate.options.map((option) => h("button" + (option === gate.default ? ".primary" : ""),
        { onclick: (e) => send({ choice: option }, e.currentTarget) },
        option, option === gate.default ? h("span.tag", "suggested") : null)))
      : null,
    text,
    h("div.row.end", h("button", {
      onclick: (e) => {
        if (!text.value.trim()) { toast("Write an answer first", true); return; }
        send({ text: text.value }, e.currentTarget);
      },
    }, "Send answer")));
}

// The next 9:00 in the morning from now, `days` days ahead (1: tomorrow) — or the coming
// Monday's, for `days` = "monday".
function morning(days) {
  const d = new Date();
  if (days === "monday") {
    const add = ((8 - d.getDay()) % 7) || 7;
    d.setDate(d.getDate() + add);
  } else {
    d.setDate(d.getDate() + days);
  }
  d.setHours(9, 0, 0, 0);
  return d.toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** The Pause ▾ control and its menu. `after` redraws whatever shows the run's state. */
function pauseControl(runId, after) {
  const wrap = h("div.menu-wrap");
  const pause = async (body, label, button) => {
    const r = await busy(button, () => api(`/runs/${enc(runId)}/pause`, body));
    if (!r) return;
    close();
    toast(`Paused ${label}${r.running ? " — the run going now will finish first" : ""}. Questions stay open; you can still run it by hand.`);
    after();
  };
  const choices = [
    ["For an hour", { for: "1h" }, "for an hour"],
    ["Until tomorrow morning", () => ({ until: morning(1) }), "until tomorrow morning"],
    ["Until Monday morning", () => ({ until: morning("monday") }), "until Monday morning"],
    ["Until I resume it", {}, "until you resume it"],
  ];
  const menu = h("div.menu", { hidden: true },
    h("b", "Pause — hold back what's scheduled"),
    choices.map(([label, body, said]) => h("button", {
      onclick: (e) => pause(typeof body === "function" ? body() : body, said, e.currentTarget),
    }, label)));
  const close = () => { menu.hidden = true; document.removeEventListener("click", outside); };
  const outside = (e) => { if (!document.body.contains(wrap) || !wrap.contains(e.target)) close(); };
  wrap.append(h("button.small", {
    onclick: () => {
      menu.hidden = !menu.hidden;
      if (!menu.hidden) setTimeout(() => document.addEventListener("click", outside), 0);
    },
  }, "Pause ▾"), menu);
  return wrap;
}

// ---------------------------------------------------------------------------
// the week, as a strip
// ---------------------------------------------------------------------------

const MARK_WORDS = {
  did: "did something", ran: "looked, nothing to do", failed: "stopped part-way",
  "couldnt-start": "couldn't start", limited: "hit the usage limit",
};

/** A run's last six days, today and tomorrow (`story::Timeline`), as one strip. */
function weekStrip(t, onMade) {
  const now = new Date(t.now);
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  start.setDate(start.getDate() - 6);
  const end = new Date(start);
  end.setDate(end.getDate() + 8);
  const span = end - start;
  const at = (iso) => Math.max(0, Math.min(1, (new Date(iso) - start) / span));
  const pct = (x) => `${(x * 100).toFixed(3)}%`;
  const strip = h("div.strip", h("div.track"));
  const seg = (cls, from, to, title) => {
    const a = at(from), b = at(to);
    if (b <= a) return;
    strip.append(h(`div.seg.${cls}`, { style: `left:${pct(a)};width:${pct(b - a)}`, title }));
  };
  const present = new Set();
  for (const s of t.looking) { seg("look", s.from, s.to, "a free check was looking for news"); present.add("look"); }
  for (const s of t.paused) { seg("paused", s.from, s.to, "paused"); present.add("paused"); }
  for (const s of t.waiting) {
    seg("wait", s.from, s.to, `waited on you ${spelled(s.from).replace(" ago", "")}`);
    present.add("wait");
  }
  seg("future", t.now, end.toISOString(), "");

  // Marks close together of the same kind are one mark with a count, or five failed starts in
  // ninety minutes would be five glyphs on one pixel.
  const groups = [];
  for (const w of t.wakes) {
    const x = at(w.at);
    if (x <= 0) continue;
    const g = groups[groups.length - 1];
    if (g && g.mark === w.mark && x - g.x < 0.012) { g.n += 1; g.titles.push(w); continue; }
    groups.push({ x, mark: w.mark, n: 1, titles: [w] });
  }
  for (const g of groups) {
    present.add(g.mark);
    const title = g.titles.map((w) => `${clockTime(w.at)}: ${MARK_WORDS[w.mark]}${w.text ? " — " + w.text : ""}`).join("\n");
    const bad = g.mark === "failed" || g.mark === "couldnt-start";
    strip.append(h(`div.mark.${g.mark}`, { style: `left:${pct(g.x)}`, title },
      bad ? "✕" : g.mark === "limited" ? "◷" : null, g.n > 1 ? h("small", String(g.n)) : null));
  }
  for (const m of t.made || []) {
    const x = at(m.at);
    if (x <= 0) continue;
    present.add("made");
    strip.append(h("button.mark.made", {
      type: "button",
      style: `left:${pct(x)}`,
      title: `${clockTime(m.at)}: made ${m.title}${m.version > 1 ? ` (v${m.version})` : ""}`,
      "aria-label": `Open ${m.title}`,
      onclick: () => onMade && onMade(m.key, m.version),
    }));
  }
  strip.append(h("div.now", { style: `left:${pct(at(t.now))}`, title: "now" }));
  if (t.next && at(t.next) < 1) {
    strip.append(h("div.mark.next", { style: `left:${pct(at(t.next))}`, title: `next run ${clockTime(t.next)}` }));
    present.add("next");
  }
  const days = h("div.days");
  for (let i = 0; i < 8; i++) {
    const d = new Date(start);
    d.setDate(start.getDate() + i);
    const label = i === 6 ? "Today" : i === 7 ? "Tomorrow" : `${d.toLocaleDateString([], { weekday: "short" })} ${d.getDate()}`;
    days.append(h(i === 6 ? "span.today" : "span", label));
  }
  const legendItems = [
    ["look", h("i.look"), "looking, free"], ["made", h("i.made"), "made something"], ["did", h("i.did"), "did something"], ["ran", h("i.ran"), "looked, nothing to do"],
    ["failed", h("span.x", "✕"), "stopped part-way"], ["couldnt-start", h("span.x", "✕"), "couldn't start"],
    ["limited", h("span.x.lim", "◷"), "usage limit"], ["wait", h("i.wait"), "waiting on you"], ["paused", h("i.paused"), "paused"],
    ["next", h("i.next"), "next run"],
  ];
  const seen = new Set();
  const legend = h("div.legend", legendItems.filter(([k, , word]) => present.has(k) && !seen.has(word) && seen.add(word))
    .map(([, icon, word]) => h("span", icon, word)));
  return h("div.strip-wrap", strip, days, legend);
}

// ---------------------------------------------------------------------------
// #/ — agents, one line each
// ---------------------------------------------------------------------------

let showAll = store("otto.all") === "1";

// When, at the end of a row: what's next for a live run, when it last ran otherwise.
function rowWhen(r) {
  if (r.running) return "now";
  if (!r.terminal && r.mood !== "paused" && r.mood !== "waiting" && r.nextWakeAt) return `next ${nextAt(r.nextWakeAt)}`;
  return r.lastRunAt ? relative(r.lastRunAt) : "";
}

function agentsView() {
  const list = h("div");
  const toggle = h("input", {
    type: "checkbox",
    checked: showAll,
    onchange: (e) => { showAll = e.target.checked; store("otto.all", showAll ? "1" : "0"); refresh(); },
  });
  view.replaceChildren(
    h("div.view-title", h("h1", "Agents"),
      h("div.row", h("label.check", toggle, "Show finished"), h("a.btn", { href: "#/new" }, "New agent"))),
    healthBanner(),
    list);

  async function refresh() {
    let data;
    try {
      data = await api("/runs" + (showAll ? "?all=true" : ""));
    } catch (err) {
      list.replaceChildren(h("div.banner.bad", err.message));
      return;
    }
    if (!data.rows.length) {
      list.replaceChildren(h("div.card.empty",
        h("p", showAll ? "No agents yet." : "No agents running."),
        h("a.btn.primary", { href: "#/new" }, "Start one")));
      return;
    }
    // What needs a person first, then what's working, then the rest; finished last.
    const order = { waiting: 0, trouble: 1, working: 2, watching: 3, idle: 3, paused: 4, finished: 5 };
    const rows = [...data.rows].sort((a, b) => (order[a.mood] ?? 3) - (order[b.mood] ?? 3));
    list.replaceChildren(h("div.list", rows.map((r) => h(`a.list-row.${r.mood}`, { href: `#/run/${enc(r.id)}` },
      glyph(r.mood),
      h("span.name", r.name || r.id),
      h("span.line", { title: r.line }, r.line),
      h("span.meta",
        r.messages ? h("span.msgs", { title: `${count(r.messages, "message")} waiting in your inbox` }, `✉ ${r.messages}`) : null,
        rowWhen(r))))));
  }

  refresh();
  every(5000, refresh);
}

// ---------------------------------------------------------------------------
// #/inbox — everything waiting for you
// ---------------------------------------------------------------------------

let inboxFilter = store("otto.inbox") || "todo";

const itemKey = (i) => `${i.run}/${i.kind === "question" ? "q" : "m"}${i.id}`;

function inboxView(selected) {
  const listBox = h("div.mail-list");
  const pane = h("div.mail-pane");
  let items = [];
  let shownKey = null;
  let drawn = false; // the pane has been drawn for shownKey — false forces a redraw, even of nothing
  view.replaceChildren(
    h("div.view-title", h("h1", "Inbox"), h("span.faint", { id: "inbox-summary" })),
    h("div.mail", listBox, pane));

  const filters = h("div.mail-filters");
  const drawFilters = (open) => {
    filters.replaceChildren(...[["todo", `To do · ${open}`], ["done", "Done"], ["all", "All"]].map(([key, label]) =>
      h("button", { "aria-pressed": String(inboxFilter === key), onclick: () => { inboxFilter = key; store("otto.inbox", key); refresh(); } }, label)));
  };

  async function refresh() {
    let data;
    try {
      data = await api("/inbox" + (inboxFilter === "todo" ? "" : "?all=true"));
    } catch (err) {
      listBox.replaceChildren(h("div.banner.bad", err.message));
      return;
    }
    items = data.items.filter((i) => inboxFilter === "all" || (inboxFilter === "todo" ? i.open : !i.open));
    document.getElementById("inbox-summary").textContent = data.open ? `${data.open} waiting for you` : "Nothing waiting for you";
    drawFilters(data.open);
    listBox.replaceChildren(filters, items.length
      ? h("div", items.map((i) => h("a.mail-item" + (i.open ? "" : ".done"), {
        href: `#/inbox/${itemKey(i)}`,
        "aria-current": String(itemKey(i) === selected),
      },
        h("span.unread" + (i.open ? "" : ".read")),
        h("span.from", i.name || i.run), h("span.when", relative(i.at)),
        h("span.title", i.title),
        h("span.chip-row", kindChip(i.kind, i.open)))))
      : h("p.mail-empty", inboxFilter === "todo" ? "Nothing is waiting for you. Your agents put questions and results here." : "Nothing here yet."));
    if (!selected && items.length && window.matchMedia("(min-width: 761px)").matches) {
      selected = itemKey(items[0]);
      history.replaceState(null, "", `#/inbox/${selected}`);
      for (const a of listBox.querySelectorAll(".mail-item")) a.setAttribute("aria-current", String(a.getAttribute("href") === `#/inbox/${selected}`));
    }
    if (!drawn || selected !== shownKey) drawPane();
  }

  async function drawPane() {
    shownKey = selected;
    drawn = true;
    pane.classList.toggle("blank", !selected);
    if (!selected) { pane.replaceChildren(h("p.faint", items.length ? "Choose something on the left." : "All caught up.")); return; }
    const [run, ref] = selected.split("/");
    const item = items.find((i) => itemKey(i) === selected);
    pane.replaceChildren(h("p.faint", "Loading…"));
    try {
      if (ref.startsWith("q")) await drawQuestion(run, ref.slice(1), item);
      else await drawMessage(run, ref.slice(1), item);
    } catch (err) {
      pane.replaceChildren(h("div.banner.bad", err.message));
    }
  }

  // Done with one: on to whatever is next, as an inbox does.
  const after = () => {
    selected = null;
    drawn = false; // whatever comes next is drawn, even nothing — the done one must not linger
    history.replaceState(null, "", "#/inbox");
    refresh();
    refreshChrome();
  };

  async function drawQuestion(run, id, item) {
    const d = await api(`/runs/${enc(run)}`);
    const head = h("div.pane-head",
      h("div.row", h("a", { href: `#/run/${enc(run)}` }, d.name || run), kindChip("question", !!(d.gate && d.gate.id === id)),
        item ? h("span.faint", relative(item.at)) : null),
      h("h2", item ? item.title : d.questionTitle || "Question"));
    if (d.gate && d.gate.id === id) {
      pane.replaceChildren(head, questionCard(run, d.gate, after, item ? item.title : d.questionTitle));
    } else {
      pane.replaceChildren(head, h("div.resolved", `✓ You answered: ${item && item.answer ? item.answer : "(see its log)"}`));
    }
  }

  async function drawMessage(run, id, item) {
    const m = await api(`/runs/${enc(run)}/messages/${enc(id)}`);
    // Opened by a link while the list shows something else, the list can't name its sender.
    if (!(item && item.name) && !runNameCache[run]) {
      runNameCache[run] = await api(`/runs/${enc(run)}`).then((d) => d.name).catch(() => null);
    }
    const reply = h("textarea", { rows: 3, placeholder: m.kind === "report" ? "Reply — what to dig into next, what it got wrong…" : m.kind === "draft" ? "Reply — what the draft should say differently…" : "Reply…" });
    const now = h("input", { type: "checkbox" });
    const done = async (body, button, what) => {
      const r = await busy(button, () => api(`/runs/${enc(run)}/messages/${enc(id)}/done`, body));
      if (!r) return;
      toast(r.done.note ? `${what} — ${heard(r)}` : what);
      after();
    };
    const parts = [
      h("div.pane-head",
        h("div.row", h("a", { href: `#/run/${enc(run)}` }, (item && item.name) || runNameCache[run] || idText(run)),
          kindChip(m.kind, m.open), h("span.faint", relative(m.sentAt))),
        h("h2", m.title)),
      md(m.text),
    ];
    if (m.draft) {
      parts.push(h("div.draft-box",
        h("div.row.spread", h("span.label", "Draft to send — otto never sends it for you"),
          h("button.small", {
            onclick: async (e) => {
              try { await navigator.clipboard.writeText(m.draft); toast("Copied — paste it where it goes"); }
              catch { toast("Couldn't reach the clipboard; select the text and copy it", true); }
              e.currentTarget.blur();
            },
          }, "Copy")),
        h("div.draft-text", m.draft)));
    }
    const out = m.outputRef;
    if (out) {
      // The message is the news; the output is the work. A card opens it on the agent's page.
      parts.push(h("a.output-card", { href: `#/run/${enc(run)}/work/${enc(out.key)}${out.version < out.versions ? `/v${out.version}` : ""}` },
        h("span.oc-icon", { "aria-hidden": "true" }, h("i"), h("i"), h("i")),
        h("span.oc-text",
          h("span.oc-meta", [dayText(out.at), out.versions > 1 ? `version ${out.version}` : null, out.words ? `${out.words} words` : null, out.link ? "a link" : null].filter(Boolean).join(" · ")),
          h("span.oc-title", out.title)),
        h("span.oc-go", "Open it →")));
      if (!m.draft && out.draft) {
        const waiting = out.state === "waiting";
        parts.push(h("div.draft-box",
          h("div.row.spread", h("span.label", "Draft to send — otto never sends it for you"),
            h("button.small", { onclick: () => copyText(out.draft, "Copied — paste it where it goes") }, "Copy")),
          h("div.draft-text", out.draft),
          waiting && m.open ? h("div.row", h("button.small.primary", {
            title: "Mark the draft sent, and this message done",
            onclick: async (e) => {
              const r = await busy(e.currentTarget, async () => {
                await api(`/runs/${enc(run)}/outputs/${enc(out.key)}/state`, { state: "done" });
                return api(`/runs/${enc(run)}/messages/${enc(id)}/done`, {});
              });
              if (!r) return;
              toast("Marked sent");
              after();
            },
          }, "I sent it")) : null));
      }
    }
    for (const a of m.attached) {
      parts.push(h("details.attachment", h("summary", `Attached: ${a.path}`),
        a.text != null ? (a.path.endsWith(".md") ? md(a.text) : h("pre.code", a.text)) : h("p.faint", "Too big to show here, or not text.")));
    }
    if (!m.open) {
      parts.push(h("div.resolved", m.reply ? `✓ Done — you replied: ${m.reply}` : `✓ Done ${m.doneAt ? relative(m.doneAt) : ""}`));
    } else {
      parts.push(h("div.row",
        // A reply already written goes with it: Done must not quietly throw it away.
        h("button.primary", {
          onclick: (e) => reply.value.trim()
            ? done({ reply: reply.value, now: now.checked }, e.currentTarget, "Reply sent")
            : done({}, e.currentTarget, "Done"),
        }, "Done"),
        m.kind === "report" ? h("button", { title: "Mark it done and tell the agent it got this one right", onclick: (e) => done({ reply: "Looks right." }, e.currentTarget, "Told it so") }, "Looks right") : null));
    }
    parts.push(h("div.reply-box", reply,
      h("div.row.spread",
        h("label.check", now, "and run it now"),
        h("button", {
          onclick: (e) => {
            if (!reply.value.trim()) { toast("Write a reply first", true); return; }
            done({ reply: reply.value, now: now.checked }, e.currentTarget, "Reply sent");
          },
        }, m.open ? "Reply and mark done" : "Reply")),
      h("p.hint", "It reads your reply the next time it runs.")));
    pane.replaceChildren(...parts);
  }

  refresh();
  every(10000, refresh);
}

const runNameCache = {};
const LATELY_FIRST = 8;

// ---------------------------------------------------------------------------
// Its work — what an agent made, kept (`outputs`)
// ---------------------------------------------------------------------------
//
// There is no agent "type". What is on screen follows from the outputs themselves: a new key each
// time is a dated list; a key published again has versions, and what changed between them; stages
// make a track; outputs with a draft or a state are cards to deal with, one each.

const latestOf = (o) => o.versions[o.versions.length - 1];
const hasDraft = (o) => o.versions.some((v) => v.draft);

/** An output's state in words, or null. */
function outputStateLabel(o) {
  if (!o.state) return null;
  const draft = latestOf(o).draft;
  if (o.state === "waiting") return draft ? "Draft waiting" : "Waiting on you";
  if (o.state === "done") return draft ? "You sent it" : "Done";
  return o.stateNote || "Not needed";
}

function outputStateChip(o) {
  const label = outputStateLabel(o);
  return label ? h(`span.kind.${o.state === "waiting" ? "question" : o.state === "done" ? "done" : "update"}`, label) : null;
}

/** "Fri Oct 16", or "Today". */
function dayText(iso) {
  const d = new Date(iso);
  if (d.toDateString() === new Date().toDateString()) return "Today";
  return d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
}

/** "This week", "Last week", or the month — the heading a dated list groups under. */
function weekGroup(iso) {
  const age = (Date.now() - new Date(iso)) / 86400000;
  if (age < 7) return "This week";
  if (age < 14) return "Last week";
  return new Date(iso).toLocaleDateString([], { month: "long", year: "numeric" });
}

/** Lines of `a` and `b`, as ["same"|"del"|"add", text] — a longest-common-subsequence diff. */
function lineDiff(a, b) {
  const x = a.replace(/\s+$/, "").split("\n");
  const y = b.replace(/\s+$/, "").split("\n");
  const n = x.length;
  const m = y.length;
  if (n * m > 4000000) return null; // too big to diff in a page; say so rather than hang
  const L = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) L[i][j] = x[i] === y[j] ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1]);
  }
  const out = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (x[i] === y[j]) { out.push(["same", x[i]]); i++; j++; }
    else if (L[i + 1][j] >= L[i][j + 1]) out.push(["del", x[i++]]);
    else out.push(["add", y[j++]]);
  }
  while (i < n) out.push(["del", x[i++]]);
  while (j < m) out.push(["add", y[j++]]);
  return out;
}

/** A diff as lines a person reads: headings bold, long unchanged stretches folded. */
function diffView(lines) {
  const box = h("div.diff");
  const line = ([kind, raw]) => {
    const heading = /^#{1,6}\s/.test(raw);
    if (!raw.trim()) return h(`div.dl.blank.${kind}`); // a paragraph break, not a row
    const text = raw.replace(/^\s*>\s?/, ""); // a quote reads as its words
    return h(`div.dl.${kind}${heading ? ".h" : ""}`,
      h("span.dm", { "aria-hidden": "true" }, kind === "add" ? "+" : kind === "del" ? "−" : ""),
      h("span.dt", heading ? text.replace(/^#+\s*/, "") : text.trim() ? inline(text.replace(/^(\s*)[-*]\s+/, "$1• ")) : "\u00a0"));
  };
  for (let i = 0; i < lines.length;) {
    let j = i;
    while (j < lines.length && lines[j][0] === "same") j++;
    const run = lines.slice(i, j);
    if (run.length > 11) { // fold only what's worth a click: at least five lines hidden
      const hidden = run.slice(3, -3);
      box.append(...run.slice(0, 3).map(line));
      const fold = h("button.fold", {
        type: "button",
        onclick: () => fold.replaceWith(...hidden.map(line)),
      }, `${hidden.length} unchanged lines`);
      box.append(fold, ...run.slice(-3).map(line));
    } else {
      box.append(...run.map(line));
    }
    if (j >= lines.length) break;
    let k = j;
    while (k < lines.length && lines[k][0] !== "same") k++;
    box.append(...lines.slice(j, k).map(line));
    i = k;
  }
  return box;
}

/**
 * The "Its work" panel of an agent's page. `render(d)` redraws from a run detail when what it
 * shows has changed; `select(key, v)` opens one output (from the week strip, Lately, a link).
 */
function workPanel(runId, initial, onChanged) {
  const path = `/runs/${enc(runId)}`;
  const el = h("section.panel.work", { id: "work" });
  el.hidden = true;
  const cache = new Map(); // "key@n" → a version, with its text
  let sel = initial ? { key: initial.key, v: initial.v || null } : null;
  let showDiff = true;
  let showFull = false; // a document with a summary opens on the summary
  let stageFilter = null;
  let stateFilter = store("otto.work.state") || "all";
  let query = "";
  let opened = !!initial; // whether a card has been opened yet, by the person or a link
  let last = null;
  let shown = null;
  const typed = {}; // what's been typed in "tell it about this", per output

  const load = async (key, n) => {
    const k = `${key}@${n}`;
    if (!cache.has(k)) cache.set(k, await api(`${path}/outputs/${enc(key)}?v=${n}`));
    return cache.get(k);
  };

  function select(key, v = null, scroll = true) {
    if (!sel || sel.key !== key) showFull = false;
    sel = { key, v };
    showDiff = true;
    history.replaceState(null, "", `#/run/${enc(runId)}/work/${enc(key)}${v ? `/v${v}` : ""}`);
    shown = null;
    if (el.contains(document.activeElement)) document.activeElement.blur(); // they chose to move on
    if (last) render(last);
    if (scroll) el.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function render(d) {
    last = d;
    const { outputs, stages } = d.outputs;
    el.hidden = !outputs.length;
    if (!outputs.length) return;
    const messages = d.messages.filter((m) => m.output);
    const key = JSON.stringify([outputs.map((o) => [o.key, latestOf(o).n, o.title, o.state, o.asking, o.stage,
      o.versions.map((v) => (v.reviews || []).length)]), stages, sel, showDiff, showFull, stageFilter, stateFilter, query,
    messages.map((m) => [m.id, m.open, m.reply])]);
    if (key === shown) return;
    if (el.contains(document.activeElement) && document.activeElement.matches("textarea, input[type=search]")) return;
    shown = key;

    const items = outputs.filter((o) => o.state || hasDraft(o)).length * 2 >= outputs.length;
    const current = outputs.find((o) => sel && o.key === sel.key);
    if (!current) sel = null;

    const head = h("div.work-head",
      h("p.label", "Its work"),
      h("span.faint", outputs.length === 1 ? "1 thing" : `${outputs.length} things`, ` since ${dayText(outputs[outputs.length - 1].firstAt)}`));
    if (items) {
      const counts = { waiting: outputs.filter((o) => o.state === "waiting").length };
      counts.done = outputs.length - counts.waiting;
      head.append(h("div.seg-pills", { role: "group", "aria-label": "Show" },
        [["waiting", `Waiting · ${counts.waiting}`], ["done", `Done · ${counts.done}`], ["all", `All · ${outputs.length}`]].map(([k, label]) =>
          h("button", { type: "button", "aria-pressed": String(stateFilter === k), onclick: () => { stateFilter = k; store("otto.work.state", k); shown = null; render(last); } }, label))));
    }
    if (outputs.length > 6) {
      head.append(h("input.work-search", {
        type: "search", placeholder: "Search its work", "aria-label": "Search its work", value: query,
        oninput: (e) => { query = e.target.value; },
        onkeydown: (e) => { if (e.key === "Enter" || e.key === "Escape") { if (e.key === "Escape") query = ""; e.target.blur(); shown = null; render(last); } },
        onblur: () => { shown = null; render(last); },
      }));
    }

    let list = outputs;
    if (stageFilter) list = list.filter((o) => o.stage === stageFilter);
    if (items && stateFilter !== "all") list = list.filter((o) => (stateFilter === "waiting") === (o.state === "waiting"));
    if (query.trim()) {
      const q = query.trim().toLowerCase();
      list = list.filter((o) => o.title.toLowerCase().includes(q) || o.key.includes(q));
    }

    const parts = [head];
    if (stages.length) parts.push(stageTrack(stages, outputs));
    if (items) {
      // Open the first one waiting on the person, once; after that, what they open stays open.
      if (!opened) {
        opened = true;
        const waiting = list.find((o) => o.state === "waiting");
        if (waiting) sel = { key: waiting.key, v: null };
      }
      parts.push(list.length ? h("div.cards", list.map((o) => itemCard(o, d))) : h("p.faint", "Nothing here."));
    } else {
      if (!sel) {
        const pick = outputs.find((o) => o.asking) || list[0] || outputs[0];
        sel = { key: pick.key, v: null };
      }
      const reader = h("article.reader");
      parts.push(h("div.work-cols", workList(list, stages.length > 0), reader));
      drawReader(reader, outputs.find((o) => o.key === sel.key), d);
    }
    el.replaceChildren(...parts);
  }

  function stageTrack(stages, outputs) {
    return h("ol.stages", stages.map((s, i) => h("li",
      h("button.stage." + s.status, {
        type: "button",
        "aria-pressed": String(stageFilter === s.name),
        title: stageFilter === s.name ? "Show every stage" : `Show only ${s.name}`,
        onclick: () => {
          stageFilter = stageFilter === s.name ? null : s.name;
          const first = outputs.find((o) => o.stage === s.name);
          if (stageFilter && first) sel = { key: first.key, v: null };
          shown = null;
          render(last);
        },
      },
      h("span.sn", `${i + 1} · ${s.status === "done" ? "done" : s.status === "stopped" ? "stopped here" : s.status === "current"
        ? (outputs.some((o) => o.asking && o.stage === s.name) ? "waiting on you" : "now") : "not yet"}`),
      h("span.st", s.name)))));
  }

  function workList(list, byStage) {
    const box = h("div.work-list");
    let group = null;
    for (const o of list) {
      const g = byStage ? (o.stage || "Other") : weekGroup(o.latestAt);
      if (g !== group) { group = g; box.append(h("div.work-group", g)); }
      const n = latestOf(o).n;
      const on = sel && sel.key === o.key;
      box.append(h("button.work-row", {
        type: "button", "aria-current": String(on),
        onclick: () => select(o.key, null, false),
      },
      h("span.wr-top", h("span", dayText(o.latestAt)),
        h("span.wr-chips", n > 1 ? h("span.vchip", `v${n}`) : null, latestOf(o).link ? h("span.vchip", "link") : null)),
      h("span.wr-title", o.title),
      o.asking ? h("span.kind.question", "Asking you") : outputStateChip(o)));
    }
    if (!list.length) box.append(h("p.faint.pad", "Nothing matches."));
    return box;
  }

  async function drawReader(reader, o, d) {
    if (!o) { reader.replaceChildren(h("p.faint", "Choose something on the left.")); return; }
    const latest = latestOf(o);
    const n = sel.v && o.versions.some((v) => v.n === sel.v) ? sel.v : latest.n;
    const version = o.versions.find((v) => v.n === n);
    reader.replaceChildren(h("p.faint", "Loading…"));
    let doc;
    let prev = null;
    try {
      doc = await load(o.key, n);
      const before = o.versions.filter((v) => v.n < n).pop();
      if (showDiff && (showFull || !doc.summary) && before && before.file && version.file) prev = await load(o.key, before.n);
    } catch (err) {
      reader.replaceChildren(h("div.banner.bad", err.message));
      return;
    }
    const parts = [];
    parts.push(h("div.reader-head",
      o.stage ? h("span.kind.report", o.stage) : null,
      h("span", dayText(version.at), " · ", clockTime(version.at)),
      version.words && !doc.summary ? h("span", `· ${version.words} words`) : null, // with a summary, its toggle says
      h("span.grow"),
      readerSteps(o)));
    parts.push(h("h3.reader-title", o.title));
    if (o.versions.length > 1) {
      const before = o.versions.filter((v) => v.n < n).pop();
      parts.push(h("div.versions",
        h("div.seg-pills", { role: "group", "aria-label": "Version" }, o.versions.map((v) =>
          h("button", { type: "button", "aria-pressed": String(v.n === n), onclick: () => select(o.key, v.n === latest.n ? null : v.n, false) },
            `v${v.n} · ${dayText(v.at)}${v.n === latest.n ? " · latest" : ""}`))),
        before && before.file && version.file && (showFull || !doc.summary)
          ? h("label.check", h("input", { type: "checkbox", checked: showDiff, onchange: (e) => { showDiff = e.target.checked; shown = null; render(last); } }),
            `Show what changed since v${before.n}`)
          : null));
    }
    if (doc.summary && doc.text != null) parts.push(summaryToggle(version));
    for (const r of version.reviews || []) {
      parts.push(h("div.review", h("strong", `Your review of v${n} · ${dayText(r.at)}`), md(r.answer)));
    }
    if (o.asking && n === latest.n) {
      parts.push(h("div.review.asking", h("strong", "It's asking you about this version"),
        h("a", { href: "#", onclick: (e) => { e.preventDefault(); window.scrollTo({ top: 0, behavior: "smooth" }); } }, "Answer it at the top of the page")));
    }
    if (version.link) {
      parts.push(h("a.link-card", { href: version.link, target: "_blank", rel: "noopener noreferrer" },
        h("span.lc-title", o.title), h("span.lc-url", version.link)));
    } else if (doc.text == null) {
      parts.push(h("p.faint", `Its file ${version.file} is missing.`));
    } else if (doc.summary && !showFull) {
      parts.push(md(doc.summary));
    } else if (prev && prev.text != null) {
      const lines = lineDiff(prev.text, doc.text);
      parts.push(lines ? diffView(lines) : h("p.faint", "Too long to compare here."));
    } else {
      parts.push(/\.md$/.test(version.file) ? md(doc.text) : h("pre.code", doc.text));
    }
    if (o.about) parts.push(h("p.about", "About: ", /^https?:\/\//.test(o.about) ? h("a", { href: o.about, target: "_blank", rel: "noopener noreferrer" }, o.about) : o.about));
    if (version.draft) parts.push(draftBox(o, version));
    if (doc.text != null) {
      parts.push(h("div.row",
        h("button.small", { type: "button", onclick: () => copyText(doc.summary && !showFull ? doc.summary : doc.text, "Copied") }, "Copy"),
        h("button.small", { type: "button", onclick: () => download(doc.text, `${o.key}-v${n}${(version.file.match(/\.\w+$/) || [".md"])[0]}`) }, "Download")));
    }
    parts.push(...inboxLines(o, d));
    parts.push(tellAbout(o, n, d));
    reader.replaceChildren(...parts);
  }

  // A document over a page comes with a summary: what's shown is that, or the whole of it.
  function summaryToggle(version) {
    const words = version.words ? ` · ${version.words.toLocaleString()} words` : "";
    return h("div.seg-pills", { role: "group", "aria-label": "Show" }, [[false, "Summary"], [true, `Full${words}`]].map(([full, label]) =>
      h("button", { type: "button", "aria-pressed": String(showFull === full), onclick: () => { showFull = full; shown = null; render(last); } }, label)));
  }

  // Older and newer, for a dated list: the next thing along, not the next version.
  function readerSteps(o) {
    const list = last.outputs.outputs;
    const i = list.findIndex((x) => x.key === o.key);
    const step = (j, label) => h("button.small", { type: "button", disabled: j < 0 || j >= list.length, onclick: () => select(list[j].key, null, false) }, label);
    return h("span.steps", step(i - 1, "← Newer"), step(i + 1, "Older →"));
  }

  function draftBox(o, version) {
    const waiting = o.state === "waiting";
    const mark = (state, said) => async (e) => {
      const r = await busy(e.currentTarget, () => api(`${path}/outputs/${enc(o.key)}/state`, { state }));
      if (!r) return;
      toast(said);
      shown = null;
      onChanged();
    };
    return h("div.draft-box",
      h("div.row.spread", h("span.label", "Draft to send — otto never sends it for you"),
        h("div.row",
          h("button.small", { type: "button", onclick: () => copyText(version.draft, "Copied — paste it where it goes") }, "Copy"),
          o.about && /^https?:\/\//.test(o.about) ? h("a.btn.small", { href: o.about, target: "_blank", rel: "noopener noreferrer" }, "Open the thread") : null)),
      h("div.draft-text", version.draft),
      waiting ? h("div.row",
        h("button.small.primary", { type: "button", onclick: mark("done", "Marked sent") }, "I sent it"),
        h("button.small.ghost", { type: "button", onclick: mark("dropped", "Marked not needed") }, "Not needed")) : null);
  }

  function inboxLines(o, d) {
    return d.messages.filter((m) => m.output && m.output.split("@")[0] === o.key).slice(0, 3).map((m) =>
      h("a.inbox-line", { href: `#/inbox/${enc(runId)}/m${m.id}` },
        m.open ? `In your inbox since ${clockTime(m.sentAt)} — “${m.title}”`
          : `Sent to your inbox ${relative(m.sentAt)} · ${m.reply ? `you replied: ${m.reply}` : "you marked it done"}`));
  }

  function tellAbout(o, n, d) {
    const k = `${o.key}@${n}`;
    const text = h("textarea", { rows: 2, placeholder: "What it got right, what it missed, what to do next…", value: typed[k] || "", oninput: (e) => { typed[k] = e.target.value; } });
    return h("div.tell-about",
      h("label", { for: `about-${o.key}` }, "Tell it about this"),
      Object.assign(text, { id: `about-${o.key}` }),
      h("div.row.spread",
        h("span.hint", `Goes to ${d.name} as a note about v${n} of “${o.title}”.`),
        h("button.small", {
          type: "button",
          onclick: async (e) => {
            if (!text.value.trim()) { toast("Write something first", true); return; }
            const r = await busy(e.currentTarget, () => api(`${path}/notes`, { text: text.value, standing: false, now: false, about: k }));
            if (!r) return;
            typed[k] = "";
            toast(`Sent — ${heard(r)}`);
            shown = null;
            onChanged();
          },
        }, "Send")));
  }

  function itemCard(o, d) {
    const open = sel && sel.key === o.key;
    const latest = latestOf(o);
    const card = h(`div.item-card${open ? ".open" : ""}`);
    const top = h("button.item-top", {
      type: "button", "aria-expanded": String(open),
      onclick: () => { sel = open ? null : { key: o.key, v: null }; showFull = false; shown = null; render(last); },
    },
    h("span.it-when", dayText(latest.at), " · ", clockTime(latest.at)),
    outputStateChip(o) || h("span"),
    h("span.it-title", o.title));
    card.append(top);
    if (open) {
      const body = h("div.item-body", h("p.faint", "Loading…"));
      card.append(body);
      load(o.key, latest.n).then((doc) => {
        const summary = doc.text != null && doc.summary;
        body.replaceChildren(
          summary ? summaryToggle(latest) : null,
          summary && !showFull ? md(doc.summary)
            : doc.text != null ? (/\.md$/.test(latest.file || "") ? md(doc.text) : h("pre.code", doc.text)) : latest.link ? h("a", { href: latest.link, target: "_blank", rel: "noopener noreferrer" }, latest.link) : null,
          latest.draft ? draftBox(o, latest) : null,
          ...inboxLines(o, d),
          tellAbout(o, latest.n, d));
      }).catch((err) => body.replaceChildren(h("div.banner.bad", err.message)));
    }
    return card;
  }

  return { el, render, select };
}

async function copyText(text, said) {
  try { await navigator.clipboard.writeText(text); toast(said); }
  catch { toast("Couldn't reach the clipboard; select the text and copy it", true); }
}

function download(text, name) {
  const a = h("a", { href: URL.createObjectURL(new Blob([text], { type: "text/plain" })), download: name });
  document.body.append(a);
  a.click();
  setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
}

// ---------------------------------------------------------------------------
// #/run/:id — one agent
// ---------------------------------------------------------------------------

function runView(id, opened) {
  const path = `/runs/${enc(id)}`;
  const work = workPanel(id, opened, () => refresh());
  const head = h("div");
  const ask = h("div");
  const alerts = h("div");
  const picture = h("div.panel");
  const lately = h("div.panel");
  const tell = h("div.panel");
  const chat = h("div.panel");
  const hood = h("div.hood");
  let last = null;

  view.replaceChildren(
    h("a.back", { href: "#/" }, "← Agents"),
    head, ask, alerts, work.el,
    h("div.agent-grid",
      h("div.col", picture, lately, tell, chat),
      h("aside.col", hood)));

  async function refresh() {
    let d;
    try {
      d = await api(path);
    } catch (err) {
      head.replaceChildren(h("div.banner.bad", err.message));
      return;
    }
    last = d;
    runNameCache[d.state.id] = d.name;
    renderHead(d);
    renderAsk(d);
    renderAlerts(d);
    work.render(d);
    renderPicture(d);
    renderLately(d);
    renderTell(d);
    renderChat(d);
    renderHood(d);
  }

  // ----- the top: who it is, what it's doing, and the controls
  let shownHead = null;
  function renderHead(d) {
    if (head.dataset.editing) return; // a rename is being typed; a refresh must not eat it
    if (head.querySelector(".menu:not([hidden])")) return; // nor a menu someone has open
    const s = d.state;
    const key = JSON.stringify([d.name, d.glance, d.running, s.status, s.paused, s.gate && s.gate.id, s.nextWakeAt,
      d.lastRun && d.lastRun.at, s.goal, s.policy.periodMinutes, Math.floor(Date.now() / 60000)]);
    if (key === shownHead) return;
    shownHead = key;
    const over = ["done", "stopped", "failed"].includes(s.status);
    const paused = !!s.paused && (!s.paused.until || new Date(s.paused.until) > Date.now());
    const controls = h("div.controls");
    if (!over) {
      controls.append(paused
        ? h("button.small", { onclick: (e) => resume(e.currentTarget, "The pause is lifted") }, "Resume")
        : pauseControl(s.id, refresh));
      controls.append(h("button.small", {
        disabled: d.running || !!s.gate,
        title: d.running ? "It's running now" : s.gate ? "It's waiting on your answer" : "Start a run now, outside its schedule",
        onclick: (e) => busy(e.currentTarget, async () => {
          const r = await api(path + "/wake", {});
          toast(r.note || "Running now");
          refresh();
        }),
      }, "Run now"));
      controls.append(stopControl());
    } else if (s.status !== "done") {
      controls.append(h("button.small.primary", { onclick: (e) => resume(e.currentTarget, "It's back") }, "Bring it back"));
    }
    const when = nextLine(d, paused);
    head.replaceChildren(h("div.agent-top",
      h("div.who",
        h("div.row.title", h("h1", d.name || s.id),
          h("button.ghost.small", { title: "Rename it", onclick: () => renameInPlace(d) }, "Rename"),
          deleteControl(d)),
        h("p.goal", s.goal),
        h("div.status-line", glyph(d.glance.mood), h("span", d.glance.line)),
        when ? h("div.when-line", when) : null),
      controls));
  }

  function nextLine(d, paused) {
    const s = d.state;
    const bits = [];
    if (d.running && s.wake) bits.push(`running since ${clockTime(s.wake.startedAt)}`);
    else if (!["done", "stopped", "failed"].includes(s.status) && !s.gate && !paused && s.nextWakeAt) {
      bits.push(`next run ${nextAt(s.nextWakeAt)}`);
    }
    if (!["done", "stopped", "failed"].includes(s.status)) bits.push(s.policy.perpetual ? `${everyText(s.policy.periodMinutes || 60)}, until you stop it` : everyText(s.policy.periodMinutes || 60));
    if (d.lastRun) bits.push(`last ran ${relative(d.lastRun.at)}`);
    return bits.join(" · ");
  }

  async function resume(button, said) {
    const r = await busy(button, () => api(path + "/resume", {}));
    if (!r) return;
    toast(`${said}${r.note ? " — " + r.note : ""}`);
    shownHead = null;
    refresh();
  }

  function stopControl() {
    const wrap = h("div.menu-wrap");
    const reason = h("input", { placeholder: "Why (optional)" });
    const menu = h("div.menu.wide", { hidden: true },
      h("b", "Stop it for good?"),
      h("p.faint", "It won't run again; anything running now is stopped and its repo locks released. You can bring it back later."),
      reason,
      h("div.row.end",
        h("button.ghost.small", { onclick: () => { menu.hidden = true; } }, "Cancel"),
        h("button.danger.small", {
          onclick: (e) => busy(e.currentTarget, async () => {
            const r = await api(path + "/stop", { reason: reason.value });
            toast(`Stopped${r.killedWake ? " — and the run going now" : ""}`);
            menu.hidden = true;
            refresh();
          }),
        }, "Stop it")));
    wrap.append(h("button.small.ghost", { onclick: () => { menu.hidden = !menu.hidden; if (!menu.hidden) reason.focus(); } }, "Stop…"), menu);
    return wrap;
  }

  // Gone for good, so it asks first, and says what goes and what stays.
  function deleteControl(d) {
    const wrap = h("div.menu-wrap");
    const live = !["done", "stopped", "failed"].includes(d.state.status);
    const withData = h("input", { type: "checkbox" });
    const name = d.name || d.state.id;
    const menu = h("div.menu.wide", { hidden: true },
      h("b", "Delete it for good?"),
      h("p.faint", `It's gone from your agents${live ? " — stopped first, and anything running now is stopped too" : ""}. Its working directory, and anything it did there, stay as they are.`),
      h("p.faint", "Its data — what it did, its questions and your answers, notes, messages and what it made — is kept until you clean it up on the otto itself page."),
      h("label.check", withData, "Delete its data now too"),
      h("div.row.end",
        h("button.ghost.small", { onclick: () => { menu.hidden = true; } }, "Cancel"),
        h("button.danger.small", {
          onclick: (e) => busy(e.currentTarget, async () => {
            const r = await api(path + "/delete", { data: withData.checked ? "delete" : "keep" });
            toast(r.kept ? `Deleted ${name} — its data is kept until you clean it up` : `Deleted ${name} and its data`);
            location.hash = "#/";
          }),
        }, "Delete it")));
    wrap.append(h("button.ghost.small", { title: "Delete it", onclick: () => { menu.hidden = !menu.hidden; } }, "Delete…"), menu);
    return wrap;
  }

  // The name is edited where it is shown. Nothing on disk is keyed by the name.
  function renameInPlace(d) {
    head.dataset.editing = "1";
    const input = h("input.rename", { value: d.name, "aria-label": "Name", spellcheck: false });
    const close = () => { delete head.dataset.editing; shownHead = null; refresh(); };
    let saveButton;
    const save = (button) => busy(button, async () => {
      if (input.value.trim() === d.name) { close(); return; }
      const r = await api(path + "/rename", { name: input.value });
      toast(r.from === r.to ? "Name unchanged" : `Renamed to ${r.to}`);
      close();
    });
    head.replaceChildren(h("div.agent-top", h("div.who",
      h("div.row.title", input,
        (saveButton = h("button.primary.small", { onclick: (e) => save(e.currentTarget) }, "Save")),
        h("button.ghost.small", { onclick: close }, "Cancel")),
      h("p.faint", "Letters, digits and hyphens — not just a number, and not another agent's name."))));
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") saveButton.click();
      if (e.key === "Escape") close();
    });
    input.focus();
    input.select();
  }

  // ----- an open question, answered right here
  let shownGate = null;
  function renderAsk(d) {
    const key = d.gate ? d.gate.id : "none";
    if (key === shownGate) return; // don't wipe a half-written answer on refresh
    shownGate = key;
    if (!d.gate) { ask.replaceChildren(); return; }
    ask.replaceChildren(h("div.panel.asking",
      h("p.label", "It's asking you"),
      questionCard(d.state.id, d.gate, () => { shownGate = null; refresh(); refreshChrome(); })));
  }

  // ----- what it would like, what's wrong, what it has sent
  function renderAlerts(d) {
    const parts = [];
    if (d.blockedExplanation && !d.gate) parts.push(h("div.banner.bad", d.blockedExplanation));
    if (d.todoLeftOpen) parts.push(h("div.banner.warn", "Marked done with items still open on its checklist, and nothing says whether they happened."));
    if (d.needs) parts.push(h("div.callout", h("strong", "It could use"), md(d.needs)));
    const open = d.messages.filter((m) => m.open);
    if (open.length) {
      parts.push(h("div.waiting-msgs", h("span.label", "Waiting in your inbox"),
        open.map((m) => h("a.msg-link", { href: `#/inbox/${enc(d.state.id)}/m${m.id}` }, kindChip(m.kind), m.title))));
    }
    alerts.replaceChildren(...parts);
  }

  // ----- the picture: its checklist when it keeps one, its week, what it has sent
  const marks = { open: ["○", "to do"], doing: ["◐", "doing"], done: ["✓", "done"], skipped: ["–", "skipped"], failed: ["✕", "failed"] };
  function renderPicture(d) {
    const parts = [];
    if (d.todo) {
      const t = d.todo;
      parts.push(h("p.label", "Progress"), progressBar(t.counts, d.todoLeftOpen),
        h(d.todoLeftOpen ? "div.todo.left-open" : "div.todo", t.lines.map((line) => {
          if (line.kind === "heading") return h(line.level === 1 ? "div.todo-title" : "div.todo-group", line.text);
          if (line.kind === "text") return h("p.muted.todo-text", line.text);
          const [g, name] = marks[line.mark];
          return h(`div.todo-item.${line.mark}`,
            h("span.mark", { title: name, "aria-label": name }, g),
            h("div", line.text, line.detail.length ? h("div.detail", line.detail.join("\n")) : null));
        })));
    }
    parts.push(h("p.label", "This week"), weekStrip(d.timeline, (key, v) => work.select(key, v)));
    const foot = [];
    if (d.check) foot.push(`${count(d.check.noChangeTotal, "free look")} found nothing, each a run it didn't have to spend`);
    const sent = d.messages.length;
    if (sent) foot.push(`${sent}${sent >= 20 ? "+" : ""} ${sent === 1 ? "message" : "messages"} sent`);
    if (foot.length) parts.push(h("p.foot-line", foot.join(" · ")));
    // An agent that keeps its work shows it under "Its work"; one that only sends messages, here.
    if (d.messages.length && !d.outputs.outputs.length) {
      parts.push(h("p.label", "What it has sent you"),
        h("div.tiles", d.messages.slice(0, 12).map((m) => h(`a.tile${m.open ? ".open" : ""}`, { href: `#/inbox/${enc(d.state.id)}/m${m.id}` },
          h("span.d", clockTime(m.sentAt)), h("span.t", m.title), kindChip(m.kind, m.open)))));
    }
    picture.replaceChildren(...parts);
  }

  // ----- what has happened lately, in words
  let latelyAll = false;
  function renderLately(d) {
    const shown = latelyAll ? d.lately : d.lately.slice(0, LATELY_FIRST);
    const more = d.lately.length - shown.length;
    lately.replaceChildren(h("p.label", "Lately"),
      d.lately.length
        ? h("ul.lately", shown.map((x) => h(`li.${x.kind}`, { title: localTime(x.at) },
          h("span.t", relative(x.at)),
          h("span",
            x.message ? h("a", { href: `#/inbox/${enc(d.state.id)}/m${x.message}` }, x.text)
              : x.output ? h("a", { href: `#/run/${enc(d.state.id)}/work/${enc(x.output)}`, onclick: (e) => { e.preventDefault(); work.select(x.output); } }, x.text)
                : x.text,
            x.times > 1 ? h("span.times", ` ×${x.times}`) : null))))
        : h("p.faint", "Nothing yet."),
      ...(more > 0 || latelyAll
        ? [h("button.ghost.small", { onclick: () => { latelyAll = !latelyAll; renderLately(d); } }, latelyAll ? "Show less" : `Show ${more} more`)]
        : []));
  }

  // ----- telling it something: the one box, built once so a refresh never wipes a draft
  const tellText = h("textarea", { rows: 3, placeholder: "Tell it something — how to do the work, what to look at, what to skip…" });
  const pin = h("button.small.toggle", { "aria-pressed": "false", title: "Pinned: given to every run until you unpin it", onclick: () => pin.setAttribute("aria-pressed", String(pin.getAttribute("aria-pressed") !== "true")) }, "Pin");
  const sendNote = (now) => async (e) => {
    if (!tellText.value.trim()) { toast("Write something first", true); return; }
    const pinned = pin.getAttribute("aria-pressed") === "true";
    await busy(e.currentTarget, async () => {
      const r = await api(path + "/notes", { text: tellText.value, standing: pinned, now });
      toast(`${pinned ? "Pinned" : "Sent"} — ${heard(r)}`);
      tellText.value = "";
      pin.setAttribute("aria-pressed", "false");
      shownNotes = null;
      refresh();
    });
  };
  const tellForm = h("div.tell-form", tellText,
    h("div.row.spread", h("span.hint", "It reads this the next time it runs. Pin it to apply every time."),
      h("div.row", pin, h("button.small", { onclick: sendNote(true) }, "Send and run now"), h("button.small.primary", { onclick: sendNote(false) }, "Send"))));
  let shownNotes = null;
  function renderTell(d) {
    const over = ["done", "failed", "stopped"].includes(d.state.status);
    const key = JSON.stringify([over, !!d.running, d.notes.map((n) => [n.id, n.givenToWake])]);
    if (key === shownNotes) return;
    shownNotes = key;
    const notes = d.notes.map((n) => h("div.note",
      h("div.row.spread",
        h("span.faint", n.standing ? "Pinned — every run gets it" : n.givenToWake ? (d.running ? "Being read now" : "Its last run didn't finish — it gets this again next time") : `Waiting to be read · ${relative(n.addedAt)}`),
        h("button.ghost.small", {
          onclick: (e) => busy(e.currentTarget, async () => {
            await api(`${path}/notes/${enc(n.id)}/drop`, {});
            toast(n.standing ? "Unpinned" : "Withdrawn");
            shownNotes = null;
            refresh();
          }),
        }, n.standing ? "Unpin" : "Withdraw")),
      md(n.text)));
    tell.replaceChildren(h("p.label", "Tell it something"), ...notes, over ? h("p.faint", "It has finished — nothing will read a new note.") : tellForm);
  }

  // ----- asking it something: a chat happens in a terminal, so this is the command for one
  let shownChat = null;
  function renderChat(d) {
    if (d.short === shownChat) return;
    shownChat = d.short;
    const command = `otto chat ${d.short}`;
    chat.replaceChildren(h("p.label", "Ask it something"),
      h("p.hint", "Why it did something, what it found, what's left — in a terminal, where it reads everything it has done and changes none of it. What you settle on reaches it the same way as what you type above."),
      h("div.command", h("code", command),
        h("button.small", { type: "button", onclick: () => copyText(command, "Copied — paste it in a terminal") }, "Copy")));
  }

  // ----- under the hood: everything the engine keeps, folded away
  function drill(key, title, body, onToggle) {
    const note = h("span.faint");
    const el = h("details.drill", h("summary", h("span.drill-title", title), note), body);
    el.open = store(`otto.drill.${key}`) === "1";
    el.addEventListener("toggle", () => {
      store(`otto.drill.${key}`, el.open ? "1" : "0");
      if (onToggle) onToggle(el.open);
    });
    return { el, note, body };
  }
  const handoff = drill("handoff", "Handoff", h("div"));
  const log = drill("log", "Log", h("div"), (open) => setStream("log", open));
  const live = drill("live", "Live", h("div"), (open) => setStream("live", open));
  const cost = drill("cost", "Cost & schedule", h("div"));
  const settings = drill("settings", "Settings", h("div"));
  log.note.textContent = "every event, filterable";
  hood.append(h("p.label", "Under the hood"), handoff.el, log.el, live.el, cost.el, settings.el);
  const settingsForm = h("div");
  const factsEl = h("div");
  settings.body.append(factsEl, settingsForm);

  let shownHandoff = null;
  function renderHood(d) {
    handoff.note.textContent = d.handoff ? "what each run leaves the next" : "none yet";
    if (d.handoff !== shownHandoff) {
      shownHandoff = d.handoff;
      handoff.body.replaceChildren(d.handoff ? h("pre.code", d.handoff) : h("p.muted", "No handoff yet."));
    }
    live.note.textContent = d.running ? "it's running — watch it work" : "nothing running";
    // Open on a run that had ended: a new one has started, so watch that instead.
    if (d.running && live.el.open && liveEnded) { setStream("live", false); setStream("live", true); }
    const s = d.state;
    const over = ["done", "failed", "stopped"].includes(s.status);
    cost.note.textContent = d.usage.wakes ? `${count(d.usage.wakes, "run")} · ${humaniseTokens(tokenTotal(d.usage))} tokens` : "nothing spent yet";
    cost.body.replaceChildren(
      h("dl.facts",
        h("dt", "Runs"), h("dd", `${s.budget.spentWakes}${s.budget.wakes ? " of " + s.budget.wakes : ""}${s.budget.hours ? `, ${s.budget.hours}h budget` : ""}`),
        d.usage.wakes ? [h("dt", "Tokens"), h("dd", tokenSummary(d.usage))] : null,
        d.wakeCost ? [h("dt", "A run costs"), h("dd", `~${d.wakeCost.avgTurns} turns, ${humaniseTokens(d.wakeCost.avgCacheCreation)} cache-write tokens (last ${d.wakeCost.wakes})`)] : null,
        over ? null : [h("dt", "How often"), h("dd", everyText(s.policy.periodMinutes || 60))]),
      over ? "" : renderCheck(d));
    factsEl.replaceChildren(h("dl.facts", settingRows(d).map(([k, v]) => [h("dt", k), h("dd", v)])));
    if (!settingsForm.dataset.open) {
      settingsForm.replaceChildren(over ? "" : h("div.row", { style: "margin-top:10px" },
        h("button.small", { onclick: () => periodForm(d) }, "Change how often…")));
    }
  }

  function settingRows(d) {
    const s = d.state;
    const rows = [
      ["Goal", s.goal],
      ["Done when", s.doneCondition || (s.policy.perpetual ? "never — it keeps going until you stop it" : "not decided yet — it proposes one and asks")],
      ["Follows", s.wraps.ref ? `${d.wrapsKind === "skill" ? "the skill" : "the instructions in"} ${s.wraps.ref}` : "just the goal"],
      ["Phase", s.phase],
      ["Status", d.statusLabel],
      ["Number", idText(s.id) + (s.formerId ? ` (was ${s.formerId})` : "")],
      ["Runs with", `${s.launcher.kind}, in ${s.launcher.detach === "tmux" ? "tmux" : "the background"}`],
    ];
    if (s.wake) rows.push(["Last run", `run ${s.wake.n}, started ${relative(s.wake.startedAt)}${s.wake.outcome ? ", " + s.wake.outcome : d.running ? ", running" : ""}`]);
    if (s.incompleteWakes) rows.push(["Stopped part-way", `${count(s.incompleteWakes, "time")} in a row`]);
    if (s.startFailures) rows.push(["Couldn't start", `${count(s.startFailures, "time")} in a row${s.startFailure ? ` — “${s.startFailure}”` : ""}`]);
    if (d.workdir && d.workdir.kind === "own") rows.push(["Works in", `${d.workdir.path} (not the default)`]);
    if (d.workdir && d.workdir.kind === "unset") rows.push(["Works in", "nowhere set — `otto config workdir <dir>`"]);
    return rows;
  }

  function periodForm(d) {
    settingsForm.dataset.open = "1";
    const current = formatMinutes(d.state.policy.periodMinutes || 60);
    const input = h("input", { value: current, placeholder: "30m, 4h, 1d" });
    const close = () => { delete settingsForm.dataset.open; refresh(); };
    let saveButton;
    settingsForm.replaceChildren(h("div.card", { style: "margin-top:10px" },
      h("p", `How often it runs when nothing else says otherwise — now ${everyText(d.state.policy.periodMinutes || 60)}.`),
      h("div.field", h("span", "Every"), input),
      h("div.row.end", { style: "margin-top:10px" },
        h("button.ghost.small", { onclick: close }, "Cancel"),
        (saveButton = h("button.primary.small", {
          onclick: (e) => busy(e.currentTarget, async () => {
            const r = await api(path + "/period", { period: input.value });
            toast(`Runs ${everyText(r.periodMinutes)}` + (r.rescheduled && r.nextWakeAt ? ` — next ${nextAt(r.nextWakeAt)}` : ""));
            close();
          }),
        }, "Save")))));
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") saveButton.click(); });
    input.focus();
    input.select();
  }

  // A free check is the cheapest run there is, so say plainly whether it has one.
  function renderCheck(d) {
    const c = d.check;
    if (!c) {
      return h("p.faint", { style: "margin-top:10px" },
        "No free check: every run is a full session. A script can look for news first and only spend a run when there is some — ",
        h("code", `otto check ${d.short} --script <file>`), ".");
    }
    const last = c.lastResult
      ? `${c.lastResult === "no-change" ? "nothing new" : c.lastResult === "changed" ? "found something" : "the check failed"}${c.lastAt ? " " + relative(c.lastAt) : ""}${c.lastNote ? " — " + c.lastNote : ""}`
      : "not run yet";
    return h("div", { style: "margin-top:12px" },
      h("p", h("strong", "A free check runs first. "), `${c.pinned ? (c.setByWake ? "It set this up itself" : "You set this up") : "Just for this one wait"}; last: ${last}. It has spared ${count(c.noChangeTotal, "run")}${c.wakeAfter ? `, and runs anyway after ${c.wakeAfter} quiet looks in a row` : ""}.`),
      c.text != null ? h("pre.code", c.text) : h("p.faint", `(${c.script} is missing — the next due run will happen anyway)`));
  }

  // ----- the log and the live view stream only while their section is open
  const streams = { log: null, live: null };
  let liveEnded = false;
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
          const cls = ev.startsWith("gate-") ? ".gate" : /incomplete|killed|abandoned|exhausted|lost|failed|refused/.test(ev) ? ".bad" : "";
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
      h("div.filters", since, events, h("label.check", decisions, "Turning points"), h("label.check", follow, "Follow")),
      out);
    load();
    return () => { if (source) source.close(); };
  }

  function livePanel(panel) {
    const status = h("p.muted", "Connecting…");
    const out = h("pre.term");
    const source = new EventSource(`/api${path}/live`);
    liveEnded = false;
    const show = (text, label) => {
      const atBottom = out.scrollTop + out.clientHeight >= out.scrollHeight - 30;
      out.replaceChildren(ansiToNodes(text));
      status.replaceChildren(label);
      if (atBottom) out.scrollTop = out.scrollHeight;
    };
    const glyphs = { said: "●", tool: "▸", result: "  ", error: "✗" };
    source.addEventListener("activity", (e) => {
      const atBottom = out.scrollTop + out.clientHeight >= out.scrollHeight - 30;
      const entries = JSON.parse(e.data);
      out.classList.add("wrap");
      out.replaceChildren(...entries.map((a) => h(`div.act.${a.kind}`,
        h("span.at", a.at ? new Date(a.at).toLocaleTimeString() : ""), " ",
        h("span.mark", glyphs[a.kind] || " "), " ",
        h("span.text", a.text))));
      if (!entries.length) out.append("It has started; nothing to show yet.");
      status.replaceChildren("What it is saying and doing — `otto attach` follows the same feed.");
      if (atBottom) out.scrollTop = out.scrollHeight;
    });
    source.addEventListener("pane", (e) => show(JSON.parse(e.data), "Its tmux pane (its transcript can't be read)"));
    source.addEventListener("log", (e) => show(JSON.parse(e.data), "wake.log (it isn't in tmux)"));
    // The server says so in the terminal's words; here it's said for the page, and an empty screen goes.
    source.addEventListener("ended", () => {
      source.close();
      liveEnded = true;
      const had = out.textContent.trim();
      status.replaceChildren(had
        ? "That run has finished — this is where it left off. What it did is under Lately."
        : "Not running right now. Press Run now, or leave this open for its next run.");
      if (!had) out.remove();
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
  const n = humaniseTokens;
  return `${n(tokenTotal(t))} over ${count(t.wakes, "run")}${t.chats ? ` and ${count(t.chats, "chat")}` : ""} — ` +
    `${n(t.inputTokens)} in, ${n(t.outputTokens)} out, ` +
    `${n(t.cacheCreation)} cache write, ${n(t.cacheRead)} cache read` +
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
    h("a.back", { href: "#/system" }, "← otto itself"),
    h("h1", "Usage"),
    h("p.muted", "What your agents' runs have used, in tokens — finished agents included. Read from each run's session transcript."),
    h("div.row", { style: "margin:14px 0;flex-wrap:wrap;gap:10px" },
      picker("since", windows, since, (v) => { since = v; }),
      picker("by", [["run", "By agent"], ["day", "By day"]], by, (v) => { by = v; })),
    out);

  async function load() {
    let r;
    try { r = await api(`/usage?since=${enc(since)}&by=${by}`); } catch (err) { out.replaceChildren(h("p", err.message)); return; }
    if (!r.rows.length) { out.replaceChildren(h("p.muted", "Nothing ran in this window.")); return; }
    const n = humaniseTokens;
    const cells = (label, t, link) => [
      h("td.id", link ? h("a", { href: link }, label) : label),
      h("td", { dataset: { label: "runs" } }, `${t.wakes}${t.unmeasured ? "*" : ""}`),
      h("td", { dataset: { label: "total" } }, h("strong", n(tokenTotal(t)))),
      h("td", { dataset: { label: "input" } }, n(t.inputTokens)),
      h("td", { dataset: { label: "output" } }, n(t.outputTokens)),
      h("td", { dataset: { label: "cache write" } }, n(t.cacheCreation)),
      h("td", { dataset: { label: "cache read" } }, n(t.cacheRead))];
    out.replaceChildren(
      h("table.table",
        h("thead", h("tr", [by === "run" ? "Agent" : "Day", "Runs", "Total", "Input", "Output", "Cache write", "Cache read"].map((c) => h("th", c)))),
        h("tbody",
          r.rows.map((row) => h("tr", cells(row.name || row.label, row, by === "run" ? `#/run/${enc(row.key)}` : null))),
          h("tr", cells("Total", r.totals, null)))),
      r.totals.unmeasured
        ? h("p.muted", { style: "margin-top:10px" }, `* includes ${count(r.totals.unmeasured, "run")} whose usage couldn't be read — usually a sandbox that hides ~/.claude. Counted, tokens unknown.`)
        : "");
  }
  load();
}

// ---------------------------------------------------------------------------
// #/new — describe the job
// ---------------------------------------------------------------------------

const TEMPLATES = [
  ["Watch for something", "Watch <where> for <what>. When it turns up, <do what>, and let me know.", "1h", true],
  ["A daily report", "Each day, <find or check what>, and send me a short report on what you found.", "1d", true],
  ["See a task through", "<The task>, through to <the end state>. Show me the plan before you start, and ask before anything that can't be undone.", "1h", false],
  ["Work through a list", "<Do what> for each of <the list>, one at a time, keeping a checklist. Tell me when it's done or stuck.", "1h", false],
];

function newRunView() {
  const f = {};
  const field = (name, label, input, help) => {
    f[name] = input;
    return h("label.field", h("span", label), input, help ? h("small", help) : null);
  };
  const lines = (el) => el.value.split("\n").map((s) => s.trim()).filter(Boolean);
  const select = (options, value) => h("select", options.map(([v, l]) => h("option", { value: v, selected: v === value }, l)));

  const goal = h("textarea", { rows: 4, required: true, placeholder: "What should it do? Say it the way you'd ask a colleague." });
  f.goal = goal;
  const keepsGoing = h("input", { type: "checkbox" });
  f.perpetual = keepsGoing;
  const until = h("input", { placeholder: "e.g. the PR is merged — or leave it, and it proposes one and asks" });
  f.until = until;
  const untilField = h("label.field", h("span", "It's done when"), until);
  keepsGoing.addEventListener("change", () => { untilField.hidden = keepsGoing.checked; });
  const period = h("input", { placeholder: "1h" });
  f.period = period;

  const templates = h("div.templates", TEMPLATES.map(([label, text, every, perpetual]) => h("button.small", {
    type: "button",
    onclick: () => {
      goal.value = text;
      period.value = every;
      keepsGoing.checked = perpetual;
      untilField.hidden = perpetual;
      goal.focus();
      const at = text.indexOf("<");
      if (at >= 0) goal.setSelectionRange(at, text.indexOf(">", at) + 1);
    },
  }, label)));

  const wrap = h("div.segmented", ["goal", "skill", "instructions"].map((kind) =>
    h("label", h("input", { type: "radio", name: "wrap", value: kind, checked: kind === "goal", onchange: syncWrap }),
      { goal: "Just what I wrote", skill: "A skill", instructions: "An instructions file" }[kind])));
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
    h("div.card.brief",
      h("h3", "What should it do?"),
      templates,
      goal,
      h("div.grid2",
        h("label.field", h("span", "How often it looks"), period, h("small", "30m, 1h, 1d. It only spends a full run when there's something to do, if it can check for free.")),
        h("div.field", h("span", "When it's finished"),
          h("label.check", keepsGoing, "It keeps going until I stop it"), untilField))),
    h("details.more",
      h("summary", "More options — what it follows, where it works, what it may do"),
      h("fieldset", h("legend", "What it follows"),
        field("name", "Name", h("input", { placeholder: "made up from what you wrote" }), "Short and unique — what you'll call it."),
        h("div.field", h("span", "Follows"), wrap),
        skillField, instrField,
        h("div.grid2",
          field("target", "About", h("input", { placeholder: "PROJ-123, a PR URL…" }), "Recorded as facts.target."),
          field("repos", "Repositories it may work in", h("textarea", { rows: 2, placeholder: "One directory per line" })))),
      h("fieldset", h("legend", "Limits"),
        h("div.grid2",
          field("budgetWakes", "Stop and ask after this many runs", h("input", { type: "number", min: 0, value: 0 }), "0: no limit."),
          field("budgetHours", "…or this many hours", h("input", { type: "number", min: 0, value: 0 }), "0: no limit."))),
      h("fieldset", h("legend", "Where it works"),
        h("div.grid2",
          field("launcher", "Runs with", select([["claude", "claude"]], "claude"),
            "A sandbox is the right choice for anything unattended. Add launchers in config.json under otto's home."),
          field("detach", "Each run happens in", select([["tmux", "a tmux session"], ["none", "the background"]], "tmux"),
            "tmux lets you watch a run live.")),
        field("workdir", "Working directory", h("input", { placeholder: "~/work" }), "Where every run starts. Empty: the default."),
        (saveDefault = h("label.check", { hidden: true },
          (f.saveWorkdir = h("input", { type: "checkbox", checked: true })), "Make this the default"))),
      h("fieldset", h("legend", "What it may do"),
        field("permissionMode", "Permission mode", select([
          ["bypassPermissions", "bypassPermissions (default)"], ["acceptEdits", "acceptEdits"], ["auto", "auto"],
          ["dontAsk", "dontAsk"], ["manual", "manual"], ["plan", "plan"]], "bypassPermissions"),
        "Nobody is there to answer a permission prompt; the real guardrail is the launcher."),
        h("div.grid2",
          field("allowedTools", "Allowed tools", h("textarea", { rows: 2, placeholder: "Read\nBash(git *)" })),
          field("disallowedTools", "Denied tools", h("textarea", { rows: 2, placeholder: "WebFetch" })))),
      h("fieldset", h("legend", "Advanced"),
        field("id", "Run id", h("input", { placeholder: "the next number" })),
        field("phase", "Starting phase", h("input", { value: "start" })),
        h("div.grid2",
          field("fact", "Facts", h("textarea", { rows: 2, placeholder: "branch=feat/x" }), "K=V, one per line."),
          field("policy", "Policy", h("textarea", { rows: 2, placeholder: "maxWakeMinutes=45" }), "K=V, one per line.")))),
    h("div.row",
      h("button.primary", { type: "button", onclick: (e) => submit(e.currentTarget, false) }, "Start it"),
      h("button", { type: "button", onclick: (e) => submit(e.currentTarget, true) }, "Show me what it would run")),
    result,
  );
  syncWrap();
  let defaultWorkdir = null;
  api("/meta").then((m) => {
    const current = f.launcher.value;
    f.launcher.replaceChildren(...m.launchers.map((name) => h("option", { value: name, selected: name === current }, name)));
    defaultWorkdir = m.workdir;
    if (m.workdir) {
      f.workdir.placeholder = m.workdir;
    } else {
      f.workdir.required = true;
      f.workdir.nextElementSibling.textContent = "Where every run starts. No default is set yet — give one.";
      saveDefault.hidden = false;
      form.querySelector("details.more").open = true;
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
      until: f.perpetual.checked ? null : text("until"),
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
    if (!b.goal) { toast("Say what it should do first", true); f.goal.focus(); return; }
    if (/<[^>]+>/.test(b.goal)) { toast("Fill in the <…> parts of the template first", true); f.goal.focus(); return; }
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
        h("h3", `It would be ${runLabel(r.planned.id, r.planned.name)} — nothing was created`),
        h("pre.code", r.planned.firstWake)));
      return;
    }
    if (r.wakeError) toast(`Created ${runLabel(r.id)}, but its first run did not start: ${r.wakeError}`, true);
    else toast(`Started ${runLabel(r.id)}`);
    location.hash = `#/run/${enc(r.id)}`;
  }

  view.replaceChildren(h("h1", "New agent"),
    h("p.muted", "Describe the job. Its first run reads what you wrote, looks around, and asks you anything it needs to settle — like when it's finished — before it gets going."),
    form);
}

// ---------------------------------------------------------------------------
// #/system — otto itself
// ---------------------------------------------------------------------------

function systemView() {
  const healthBox = h("div");
  const unusedBox = h("div");
  const card = h("div.card");
  const output = h("div");
  view.replaceChildren(
    h("h1", "otto itself"),
    h("p.muted", "The machinery that keeps your agents going. When it's working there's nothing to do here."),
    healthBox,
    h("h2", "The reviver"),
    h("p.muted", "The background job that wakes your agents on schedule, every five minutes. Without it, a sleeping agent never wakes on its own."),
    card, output,
    h("h2", "Data no agent uses"),
    h("p.muted", "What otto still keeps for agents you deleted, and anything else left over. No agent reads any of it."),
    unusedBox,
    h("p", { style: "margin-top:24px" }, h("a", { href: "#/usage" }, "What your agents have used →")));

  async function refreshUnused() {
    let list;
    try { list = await api("/cleanup"); } catch (err) { unusedBox.replaceChildren(h("p", err.message)); return; }
    if (!list.length) { unusedBox.replaceChildren(h("p.faint", "Nothing — everything otto keeps belongs to an agent.")); return; }
    const remove = (paths, button) => busy(button, async () => {
      const removed = await api("/cleanup", { paths });
      toast(`Deleted ${count(removed.length, "item")} — ${sizeText(removed.reduce((n, u) => n + u.bytes, 0))} freed`);
      refreshUnused();
    });
    const allLabel = `Delete all — ${sizeText(list.reduce((n, u) => n + u.bytes, 0))}`;
    unusedBox.replaceChildren(
      h("table.table",
        h("thead", h("tr", ["What", "Size", "Last used", ""].map((c) => h("th", c)))),
        h("tbody", list.map((u) => h("tr",
          h("td", h("div", u.what), h("div.mono.faint", u.path)),
          h("td", { dataset: { label: "size" } }, sizeText(u.bytes)),
          h("td", { dataset: { label: "last used" } }, u.lastUsed ? h("span", { title: localTime(u.lastUsed) }, relative(u.lastUsed)) : "never"),
          h("td", h("button.small.danger", { onclick: (e) => remove([u.path], e.currentTarget) }, "Delete")))))),
      h("div.row", { style: "margin-top:10px" },
        // Everything at once is the one click that could be a slip, so it takes two.
        h("button.danger", {
          onclick: (e) => {
            const button = e.currentTarget;
            if (!button.dataset.sure) {
              button.dataset.sure = "1";
              button.textContent = `Delete all ${list.length}? Click again`;
              setTimeout(() => { delete button.dataset.sure; button.textContent = allLabel; }, 4000);
              return;
            }
            remove(list.map((u) => u.path), button);
          },
        }, allLabel)));
  }
  refreshUnused();

  async function refresh() {
    try {
      const health = await api("/health");
      healthBox.replaceChildren(health.ok
        ? h("div.banner.ok", "All good — agents are being woken on time.")
        : h("div.banner.warn", health.problems.map((p) => h("div", p))));
    } catch (err) { healthBox.replaceChildren(h("div.banner.bad", err.message)); }
    let s;
    try { s = await api("/agent"); } catch (err) { card.replaceChildren(h("p", err.message)); return; }
    const badge = s.loaded === true ? h("span.pill.done", "running")
      : s.loaded === false ? h("span.pill.failed", "not running") : h("span.pill", "unknown");
    card.replaceChildren(
      h("div.row.spread", h("h3", "Status"), badge),
      h("dl.facts", { style: "margin-top:12px" },
        h("dt", "Last ran"), h("dd", s.lastPoke ? `${s.lastPokeRelative} (${localTime(s.lastPoke)})` : "never (no log yet)"),
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
          title: "Start the runs that are due now, as the reviver would",
          onclick: (e) => busy(e.currentTarget, async () => {
            const r = await api("/poke", {});
            output.replaceChildren(h("pre.term.wrap", [...r.lines, ...r.errors].join("\n")));
            refresh();
          }),
        }, "Run it now")),
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
  const run = hash.match(/^#\/run\/([^/]+)(?:\/work\/([^/]+)(?:\/v(\d+))?)?$/);
  const inbox = hash.match(/^#\/inbox(?:\/([^/]+\/[qm][^/]+))?$/);
  const section = run ? "agents" : inbox ? "inbox" : hash === "#/new" ? "new" : hash === "#/" ? "agents" : "";
  for (const a of document.querySelectorAll("[data-nav]")) a.classList.toggle("active", a.dataset.nav === section);
  if (run) runView(decodeURIComponent(run[1]), run[2] ? { key: decodeURIComponent(run[2]), v: run[3] ? Number(run[3]) : null } : null);
  else if (inbox) inboxView(inbox[1] ? decodeURIComponent(inbox[1]) : null);
  else if (hash === "#/new") newRunView();
  else if (hash === "#/system" || hash === "#/agent") systemView();
  else if (hash === "#/usage") usageView();
  else agentsView();
  window.scrollTo(0, 0);
}

window.addEventListener("hashchange", route);
route();
api("/meta").then((m) => {
  document.getElementById("home").textContent = `otto ${m.version} · ${m.home}`;
}).catch(() => {});
