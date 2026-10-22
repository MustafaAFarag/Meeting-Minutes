"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const DONE_RESET_MS = 10000;
const HOVER_OPEN_MS = 150;
const HOVER_CLOSE_MS = 600;
// Error and Done unfold once by themselves, then fold again like any other state.
const AUTO_OPEN_MS = 8000;
const DRAG_PX = 4;
const NOTICE_MS = 6000;
const DETAIL_CHARS = 120;
const MIN_DB = -60;
const SELFTEST_STEP_MS = 2000;
const SELFTEST_CYCLES = 3;

const $ = (id) => document.getElementById(id);
const app = $("app");
const statusEl = $("status");

const TITLE_OPEN_MEETINGS = "Open My Meetings";

let doneTimer = null;
let noticeTimer = null;
// After Stop is clicked, late "recording" events must not flip the view back.
let stopping = false;
let statusDetail = "";

const STATUS_TITLE = {
  idle: () => "Ready",
  recording: () => "Recording",
  working: () => statusDetail,
  done: () => "Your notes are ready",
  error: () => statusDetail,
};

function updateButtons(name) {
  const canStart = name === "idle" || name === "done" || name === "error";
  $("start").disabled = !canStart;
  $("stop").disabled = name !== "recording";
  $("retry").hidden = name !== "error";
}

function setView(name) {
  app.dataset.view = name;
  statusEl.title = STATUS_TITLE[name]();
  updateButtons(name);
  clearTimeout(doneTimer);
  if (name === "done") doneTimer = setTimeout(() => setView("idle"), DONE_RESET_MS);
  if (name === "done" || name === "error") autoOpen();
}

// Fold / unfold. The window is resized by the backend, which keeps its edge and vertical centre.
let unfolded = false;
let pinned = false;
let hovering = false;
let dragging = false;
let press = null;
let openTimer = null;
let closeTimer = null;
let resizing = Promise.resolve();

function setUnfolded(open) {
  if (open === unfolded) return;
  unfolded = open;
  // Queued, so quick open/close changes cannot overtake each other. The content swaps only
  // once the window is big enough (opening) or before it shrinks (closing), so nothing clips.
  resizing = resizing.then(async () => {
    if (open) {
      await invoke("set_size", { folded: false }).catch(console.error);
      app.dataset.fold = "open";
    } else {
      app.dataset.fold = "closed";
      await invoke("set_size", { folded: true }).catch(console.error);
    }
  });
}

function foldIfIdle() {
  if (!pinned && !hovering && !dragging) setUnfolded(false);
}

function autoOpen() {
  setUnfolded(true);
  clearTimeout(closeTimer);
  closeTimer = setTimeout(foldIfIdle, AUTO_OPEN_MS);
}

function pointerEnter() {
  hovering = true;
  clearTimeout(closeTimer);
  clearTimeout(openTimer);
  if (!unfolded) openTimer = setTimeout(() => setUnfolded(true), HOVER_OPEN_MS);
}

function pointerLeave() {
  hovering = false;
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  closeTimer = setTimeout(foldIfIdle, HOVER_CLOSE_MS);
}

function togglePinned() {
  pinned = !pinned;
  app.dataset.pinned = String(pinned);
  if (pinned) {
    clearTimeout(closeTimer);
    setUnfolded(true);
  }
}

function pad(n) {
  return String(n).padStart(2, "0");
}

function formatTimer(seconds) {
  const s = Number.isFinite(seconds) ? Math.max(0, Math.floor(seconds)) : 0;
  return pad(Math.floor(s / 60)) + ":" + pad(s % 60);
}

// -60..0 dB -> 0..100 %
function levelPercent(db) {
  if (!Number.isFinite(db)) return 0;
  return Math.round(Math.min(1, Math.max(0, (db - MIN_DB) / -MIN_DB)) * 100);
}

function clip(text) {
  return text.length > DETAIL_CHARS ? text.slice(0, DETAIL_CHARS).trimEnd() + "…" : text;
}

function setWorking(text) {
  statusDetail = text;
  setView("working");
}

function showError(message) {
  const text = String(message || "");
  console.error(text);
  statusDetail = clip(text) || "Something went wrong";
  stopping = false;
  setView("error");
}

// "Open My Meetings" could not find a link: say so in its tooltip and ring it for a moment.
function showNotice(message) {
  const text = String(message || "");
  console.error(text);
  const btn = $("open-meetings");
  btn.title = clip(text);
  btn.classList.add("alert");
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    btn.title = TITLE_OPEN_MEETINGS;
    btn.classList.remove("alert");
  }, NOTICE_MS);
}

function resetMeters() {
  $("timer").textContent = "00:00";
  $("bar-me").style.height = "0%";
  $("bar-them").style.height = "0%";
}

function onStatus(status) {
  switch (status.state) {
    case "recording":
      if (stopping) return;
      $("timer").textContent = formatTimer(status.elapsed);
      $("bar-me").style.height = levelPercent(status.me_db) + "%";
      $("bar-them").style.height = levelPercent(status.them_db) + "%";
      if (app.dataset.view !== "recording") setView("recording");
      break;
    case "stopped":
    case "transcribing":
      setWorking("Transcribing…");
      break;
    case "summarizing":
      setWorking("Writing your notes…");
      break;
    case "publishing":
      setWorking("Updating My Meetings…");
      break;
    case "published":
      stopping = false;
      setView("done");
      break;
    case "error":
      showError(status.message);
      break;
    default:
      // transcribed, summarized and anything newer: keep what is on screen.
      break;
  }
}

async function run(command, onFail) {
  try {
    await invoke(command);
  } catch (err) {
    onFail(err);
  }
}

$("start").addEventListener("click", () => {
  stopping = false;
  resetMeters();
  // No second click until the first status event (or an error) has set the buttons again.
  $("start").disabled = true;
  run("start", showError);
});

$("stop").addEventListener("click", () => {
  stopping = true;
  $("stop").blur();
  setWorking("Transcribing…");
  // "Not recording" means it already stopped on its own; the status events carry on.
  run("stop", console.error);
});

$("retry").addEventListener("click", () => {
  stopping = false;
  setWorking("Transcribing…");
  run("retry", showError);
});

$("open-meetings").addEventListener("click", () => run("open_meetings", showNotice));

// Hover unfolds, a click on the empty area pins it open, a press-and-move drags the tab along
// the edge (and drops it on the nearest side edge).
app.addEventListener("mouseenter", pointerEnter);
app.addEventListener("mouseleave", () => {
  press = null;
  if (!dragging) pointerLeave();
});

app.addEventListener("mousedown", (event) => {
  if (event.button !== 0 || event.target.closest("button")) return;
  press = { x: event.clientX, y: event.clientY };
});

window.addEventListener("mousemove", (event) => {
  if (dragging && event.buttons === 0) {
    // The drag is over (the window manager ate the mouse-up). Back to normal hovering.
    dragging = false;
    pointerEnter();
  }
  if (!press || (event.buttons & 1) === 0) return;
  if (Math.hypot(event.clientX - press.x, event.clientY - press.y) < DRAG_PX) return;
  press = null;
  dragging = true;
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  invoke("start_drag").catch((err) => {
    dragging = false;
    console.error(err);
  });
});

window.addEventListener("mouseup", () => {
  if (press) togglePinned();
  press = null;
  dragging = false;
});

function setEdge(edge) {
  app.dataset.edge = edge === "left" ? "left" : "right";
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// MN_NOTCH_SELFTEST=1: unfold and fold on a timer, so the fold path can be checked without a
// mouse. The backend prints the window's size, position and visibility after each resize.
async function selftest() {
  if (!(await invoke("selftest_enabled").catch(() => false))) return;
  for (let i = 0; i < SELFTEST_CYCLES; i++) {
    await sleep(SELFTEST_STEP_MS);
    setUnfolded(true);
    await sleep(SELFTEST_STEP_MS);
    setUnfolded(false);
  }
}

listen("mn-status", (event) => onStatus(event.payload));
listen("mn-edge", (event) => setEdge(event.payload));
invoke("current_edge").then(setEdge).catch(console.error);
setView("idle");
selftest();
