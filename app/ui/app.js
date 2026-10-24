"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const DONE_RESET_MS = 10000;
const DRAG_PX = 4;
const NOTICE_MS = 6000;
const DETAIL_CHARS = 120;
const MIN_DB = -60;
const UPDATE_NOTE_MS = 4000;

const $ = (id) => document.getElementById(id);
const app = $("app");
const statusEl = $("status");

const TITLE_OPEN_MEETINGS = "Open My Meetings";

let doneTimer = null;
let noticeTimer = null;
let updateNoteTimer = null;
let offerVersion = "";
let updateBusy = false;
let failedMode = false;
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
  updating: () => statusDetail,
  done: () => "Your notes are ready",
  error: () => statusDetail,
};

function updateButtons(name) {
  const canStart = !updateBusy && (name === "idle" || name === "done" || name === "error");
  $("start").disabled = !canStart;
  $("stop").disabled = name !== "recording";
  $("retry").hidden = name !== "error" && !queueError;
  const update = $("update");
  if (!failedMode && !update.classList.contains("note")) update.disabled = updateBusy;
}

function setView(name) {
  if (updateBusy && name !== "updating") return;
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

// The open column is 44px wide. The button is two short lines ("Update" / "v0.1.8"). A longer
// sentence ellipsizes and the tooltip is the full text. Folded (12px) hides this button.
function showOffer() {
  if (!offerVersion) return;
  clearTimeout(updateNoteTimer);
  failedMode = false;
  updateBusy = false;
  const btn = $("update");
  $("update-box").hidden = false;
  btn.hidden = false;
  btn.disabled = false;
  btn.classList.remove("failed", "note");
  $("update-kicker").textContent = "Update";
  $("update-detail").hidden = false;
  $("update-detail").textContent = offerVersion;
  $("update-help").hidden = true;
  const label = "Update to " + offerVersion;
  btn.title = label;
  btn.setAttribute("aria-label", label);
  if (app.dataset.view === "updating") setView("idle");
  updateButtons(app.dataset.view);
}

function showUpdating(message) {
  clearTimeout(updateNoteTimer);
  failedMode = false;
  updateBusy = true;
  const btn = $("update");
  $("update-box").hidden = false;
  btn.hidden = false;
  btn.disabled = true;
  btn.classList.remove("failed", "note");
  $("update-kicker").textContent = "Updating";
  const detail = $("update-detail");
  detail.hidden = true;
  $("update-help").hidden = true;
  btn.title = message || "Updating... Meeting Notes will restart";
  btn.setAttribute("aria-label", btn.title);
  statusDetail = btn.title;
  setView("updating");
}

function showFailed(message) {
  clearTimeout(updateNoteTimer);
  updateBusy = false;
  failedMode = true;
  const btn = $("update");
  $("update-box").hidden = false;
  btn.hidden = false;
  btn.disabled = false;
  btn.classList.add("failed");
  btn.classList.remove("note");
  $("update-kicker").textContent = "Update";
  $("update-detail").hidden = false;
  $("update-detail").textContent = "failed";
  $("update-help").hidden = false;
  btn.title = message || "Update failed";
  btn.setAttribute("aria-label", "Update failed");
  if (app.dataset.view === "updating") setView("idle");
  updateButtons(app.dataset.view);
}

function hideUpdate() {
  clearTimeout(updateNoteTimer);
  $("update-box").hidden = true;
  $("update-help").hidden = true;
  failedMode = false;
  updateBusy = false;
  offerVersion = "";
  if (app.dataset.view === "updating") setView("idle");
  updateButtons(app.dataset.view);
}

// `after` runs once the short confirmation has been on screen for a few seconds.
function showUpdateNote(message, after) {
  updateBusy = false;
  failedMode = false;
  const btn = $("update");
  $("update-box").hidden = false;
  btn.hidden = false;
  btn.disabled = true;
  btn.classList.remove("failed");
  btn.classList.add("note");
  $("update-kicker").textContent = message;
  $("update-detail").hidden = true;
  $("update-help").hidden = true;
  btn.title = message;
  btn.setAttribute("aria-label", message);
  if (app.dataset.view === "updating") setView("idle");
  updateButtons(app.dataset.view);
  clearTimeout(updateNoteTimer);
  updateNoteTimer = setTimeout(() => {
    btn.classList.remove("note");
    after();
  }, UPDATE_NOTE_MS);
}

function applyUpdate(payload) {
  if (!payload || !payload.phase) return;
  if (payload.version) offerVersion = String(payload.version);
  const message = payload.message ? String(payload.message) : "";
  switch (payload.phase) {
    case "offer":
      if (failedMode || updateBusy) return;
      showOffer();
      break;
    case "updating":
      showUpdating(message);
      break;
    case "up_to_date":
      showUpdateNote(message, hideUpdate);
      break;
    case "failed":
      showFailed(message);
      break;
    case "refused":
      showUpdateNote(message, () => {
        if (offerVersion) showOffer();
        else hideUpdate();
      });
      break;
    case "copied":
      showUpdateNote(message, () => {
        if (offerVersion) showOffer();
        else hideUpdate();
      });
      break;
    case "quiet":
      updateBusy = false;
      if (!offerVersion) hideUpdate();
      else showOffer();
      break;
    default:
      break;
  }
}

function dismissFailure() {
  invoke("dismiss_update_notice")
    .then(() => {
      failedMode = false;
      if (offerVersion) showOffer();
      else hideUpdate();
    })
    .catch((err) => console.error(err));
}

$("update").addEventListener("click", () => {
  if (failedMode) {
    dismissFailure();
    return;
  }
  if (updateBusy || $("update").classList.contains("note")) return;
  invoke("start_update").catch((err) => console.error(err));
});

$("update-help").addEventListener("click", () => {
  invoke("copy_update_help").catch((err) => console.error(err));
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
listen("mn-update", (event) => applyUpdate(event.payload));
invoke("startup_update").then(applyUpdate).catch(console.error);
invoke("current_edge").then(setEdge).catch(console.error);
// Sessions queued before the window loaded (restart recovery).
invoke("queue_status").then(onQueue).catch(console.error);
setView("idle");
openColumn();
