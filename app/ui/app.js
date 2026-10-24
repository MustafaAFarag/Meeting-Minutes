"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const DONE_RESET_MS = 10000;
const DRAG_PX = 4;
const NOTICE_MS = 6000;
const DETAIL_CHARS = 120;
const MIN_DB = -60;
const COPIED_MS = 2500;
const COPIED_TEXT = "Copied - paste it into Claude";

const $ = (id) => document.getElementById(id);
const app = $("app");
const statusEl = $("status");

const TITLE_OPEN_MEETINGS = "Open My Meetings";

let doneTimer = null;
let noticeTimer = null;
let copiedTimer = null;
let updatePrompt = "";
// After Stop is clicked, late "recording" events must not flip the view back.
let stopping = false;
let statusDetail = "";
// The notes queue (mn-queue). It is separate from the recording view above: it only drives the
// badge on the My Meetings button, the Try again button, and the Done mark.
let queueBusy = false;
let queueError = "";
let queueText = "";

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
  $("retry").hidden = name !== "error" && !queueError;
}

function setView(name) {
  app.dataset.view = name;
  statusEl.title = STATUS_TITLE[name]();
  updateButtons(name);
  clearTimeout(doneTimer);
  if (name === "done") doneTimer = setTimeout(() => setView("idle"), DONE_RESET_MS);
}

// The column is always open (the user asked for no hover-to-unfold). The backend sizes the
// window and keeps it on its edge.
let dragging = false;
let press = null;

function openColumn() {
  app.dataset.fold = "open";
  invoke("set_size", { folded: false }).catch(console.error);
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

// The tooltip of the My Meetings button: what the notes queue is doing, then the button's job.
function meetingsTitle() {
  return queueText ? queueText + " - " + TITLE_OPEN_MEETINGS : TITLE_OPEN_MEETINGS;
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
    btn.title = meetingsTitle();
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
      // The recording is saved and the next Start is allowed; its notes are written in the
      // background (see onQueue).
      stopping = false;
      setView("idle");
      break;
    case "error":
      showError(status.message);
      break;
    default:
      // Anything else is not about the recording: keep what is on screen.
      break;
  }
}

// "Writing notes (2 waiting)". The stage names come from the backend.
function describeQueue(queue) {
  if (!queue.processing) return "";
  const waiting = queue.waiting > 0 ? " (" + queue.waiting + " waiting)" : "";
  return queue.processing + waiting;
}

function showQueueBadge(queue) {
  const state = queueError ? "error" : queueBusy ? "working" : "idle";
  $("queue-badge").dataset.state = state;
  $("queue-count").textContent = queueBusy && queue.waiting > 0 ? "+" + queue.waiting : "";
  queueText = queueError ? "Notes failed: " + queueError : describeQueue(queue);
  const btn = $("open-meetings");
  btn.title = meetingsTitle();
  btn.setAttribute("aria-label", btn.title);
}

function onQueue(queue) {
  const wasBusy = queueBusy;
  queueBusy = Boolean(queue.processing);
  queueError = queue.last_error ? clip(String(queue.last_error)) : "";
  showQueueBadge(queue);
  const view = app.dataset.view;
  updateButtons(view);
  // Everything is written and nothing else waits: show Done, unless a recording is on screen.
  const finished = wasBusy && !queueBusy && !queueError && queue.waiting === 0;
  if (finished && (view === "idle" || view === "done")) setView("done");
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
  // Until the "stopped" event: the recording is being closed and Start would still be refused.
  setWorking("Saving the recording…");
  // "Not recording" means it already stopped on its own; the status events carry on.
  run("stop", console.error);
});

// Failed notes are queued again by the backend. With no failed notes, this is a failed
// recording, and the backend starts a new one.
$("retry").addEventListener("click", () => {
  stopping = false;
  if (!queueError) resetMeters();
  run("retry", showError);
});

$("open-meetings").addEventListener("click", () => run("open_meetings", showNotice));

// The open column is 44px wide, so the button shows "Update" and, after a copy, as much of
// "Copied - paste it into Claude" as fits. The tooltip is the full sentence. The tray item
// is the path that shows the sentence in a notification. Folded (12px) hides this button.
function showUpdate(payload) {
  const prompt = payload && payload.prompt ? String(payload.prompt) : "";
  if (!prompt) return;
  updatePrompt = prompt;
  const btn = $("update");
  btn.hidden = false;
  btn.textContent = "Update";
  const version = payload.version ? " (" + payload.version + ")" : "";
  btn.title = "Update available" + version;
  btn.setAttribute("aria-label", "Update");
}

async function copyUpdate() {
  if (!updatePrompt) return;
  const btn = $("update");
  try {
    await navigator.clipboard.writeText(updatePrompt);
  } catch (err) {
    console.error(err);
    try {
      await invoke("copy_update_prompt", { text: updatePrompt });
    } catch (err2) {
      console.error(err2);
      return;
    }
  }
  btn.textContent = COPIED_TEXT;
  btn.title = COPIED_TEXT;
  btn.setAttribute("aria-label", COPIED_TEXT);
  clearTimeout(copiedTimer);
  copiedTimer = setTimeout(() => {
    btn.textContent = "Update";
    btn.title = "Update available";
    btn.setAttribute("aria-label", "Update");
  }, COPIED_MS);
}

$("update").addEventListener("click", () => {
  copyUpdate();
});

// A press-and-move on the empty area drags the column along the edge (and drops it on the
// nearest side edge).
app.addEventListener("mousedown", (event) => {
  if (event.button !== 0 || event.target.closest("button")) return;
  press = { x: event.clientX, y: event.clientY };
});

window.addEventListener("mousemove", (event) => {
  if (dragging && event.buttons === 0) dragging = false;
  if (!press || (event.buttons & 1) === 0) return;
  if (Math.hypot(event.clientX - press.x, event.clientY - press.y) < DRAG_PX) return;
  press = null;
  dragging = true;
  invoke("start_drag").catch((err) => {
    dragging = false;
    console.error(err);
  });
});

window.addEventListener("mouseup", () => {
  press = null;
  dragging = false;
});

function setEdge(edge) {
  app.dataset.edge = edge === "left" ? "left" : "right";
}

listen("mn-status", (event) => onStatus(event.payload));
listen("mn-queue", (event) => onQueue(event.payload));
listen("mn-edge", (event) => setEdge(event.payload));
listen("mn-update", (event) => showUpdate(event.payload));
invoke("current_edge").then(setEdge).catch(console.error);
// Sessions queued before the window loaded (restart recovery).
invoke("queue_status").then(onQueue).catch(console.error);
setView("idle");
openColumn();
