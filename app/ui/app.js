"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const DONE_RESET_MS = 10000;
const HOVER_OPEN_MS = 150;
const HOVER_CLOSE_MS = 600;
// Error and Done unfold once by themselves, then fold again like any other state.
const AUTO_OPEN_MS = 8000;
const DRAG_PX = 4;
const NOTICE_MS = 6000;
const DETAIL_CHARS = 120;
const MIN_DB = -60;

const $ = (id) => document.getElementById(id);
const app = $("app");

let doneTimer = null;
let noticeTimer = null;
// After Stop is clicked, late "recording" events must not flip the view back.
let stopping = false;

function setView(name) {
  app.dataset.view = name;
  clearTimeout(doneTimer);
  if (name === "done") doneTimer = setTimeout(() => setView("idle"), DONE_RESET_MS);
  if (name === "done" || name === "error") autoOpen();
}

// Fold / unfold. The window is resized by the backend, which keeps its top-centre point.
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

function setWorking(text, short) {
  $("working-text").textContent = text;
  $("working-mini").textContent = short;
  setView("working");
}

function showError(message) {
  const text = String(message || "");
  console.error(text);
  $("error-detail").textContent =
    text.length > DETAIL_CHARS ? text.slice(0, DETAIL_CHARS).trimEnd() + "…" : text;
  stopping = false;
  setView("error");
}

function showNotice(message) {
  const text = String(message || "");
  console.error(text);
  const el = $("notice");
  el.textContent = text.length > DETAIL_CHARS ? text.slice(0, DETAIL_CHARS).trimEnd() + "…" : text;
  el.hidden = false;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => { el.hidden = true; }, NOTICE_MS);
}

function onStatus(status) {
  switch (status.state) {
    case "recording":
      if (stopping) return;
      $("timer").textContent = $("timer-mini").textContent = formatTimer(status.elapsed);
      $("bar-me").style.width = levelPercent(status.me_db) + "%";
      $("bar-them").style.width = levelPercent(status.them_db) + "%";
      setView("recording");
      break;
    case "stopped":
    case "transcribing":
      setWorking("Transcribing…", "Transcribing…");
      break;
    case "summarizing":
      setWorking("Writing your notes…", "Writing…");
      break;
    case "publishing":
      setWorking("Updating My Meetings…", "Updating…");
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

function openMeetings() {
  run("open_meetings", showNotice);
}

$("start").addEventListener("click", () => {
  stopping = false;
  $("timer").textContent = $("timer-mini").textContent = "00:00";
  $("bar-me").style.width = "0%";
  $("bar-them").style.width = "0%";
  run("start", showError);
});

$("stop").addEventListener("click", () => {
  stopping = true;
  $("stop").blur();
  setWorking("Transcribing…", "Transcribing…");
  // "Not recording" means it already stopped on its own; the status events carry on.
  run("stop", console.error);
});

$("retry").addEventListener("click", () => {
  stopping = false;
  setWorking("Transcribing…", "Transcribing…");
  run("retry", showError);
});

$("dismiss").addEventListener("click", () => setView("idle"));

$("done-open").addEventListener("click", () => {
  openMeetings();
  setView("idle");
});

$("open-meetings").addEventListener("click", (event) => {
  event.preventDefault();
  openMeetings();
});

// Hover unfolds, a click pins it open, a press-and-move drags the pill along the top edge.
app.addEventListener("mouseenter", pointerEnter);
app.addEventListener("mouseleave", () => {
  press = null;
  if (!dragging) pointerLeave();
});

app.addEventListener("mousedown", (event) => {
  if (event.button !== 0 || event.target.closest("button, a, .error-detail")) return;
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
  getCurrentWindow().startDragging().catch((err) => {
    dragging = false;
    console.error(err);
  });
});

window.addEventListener("mouseup", () => {
  if (press) togglePinned();
  press = null;
  dragging = false;
});

listen("mn-status", (event) => onStatus(event.payload));
