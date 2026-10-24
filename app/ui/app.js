"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const DRAG_PX = 4;
const MIN_DB = -60;

const $ = (id) => document.getElementById(id);
const app = $("app");
const stage = $("stage");
const statusEl = $("status");
const cardEl = $("card");

const TITLE_OPEN_MEETINGS = "Open My Meetings";

// What each part of the app is doing. What the column shows (the "view") is worked out from
// these in computeView(), so no single event can leave it stuck in an old state.
let rec = "idle"; // "idle" | "recording" | "saving"
let recordError = false;
let queue = { processing: null, waiting: 0, last_error: null, failed: 0, ready: 0 };
let updateBusy = false;
let offerVersion = "";
let failedMode = false;
// After Stop is clicked, late "recording" events must not flip the view back.
let stopping = false;
// The message card that is open now (the payload of mn-card), or null.
let card = null;

// The status mark, in words (its tooltip and its label for screen readers). Everything a PM must
// know is on the card; this is only the short form.
const STATUS_WORDS = {
  idle: "Ready",
  recording: "Recording",
  working: "Saving the recording",
  updating: "Updating Meeting Notes. Click for details",
  notes: "Writing your notes. Click for details",
  error: "Something went wrong. Click for details",
  done: "Notes ready. Click for details",
};
// Views where a click on the status mark has something to say.
const EXPLAINED = new Set(["updating", "notes", "error", "done"]);

function computeView() {
  if (updateBusy) return "updating";
  if (rec === "recording") return "recording";
  if (rec === "saving") return "working";
  if (recordError || queue.failed > 0) return "error";
  if (queue.processing) return "notes";
  // The green check stays until My Meetings is opened or a new recording starts.
  if (queue.ready > 0) return "done";
  return "idle";
}

function updateButtons(name) {
  const canStart = !updateBusy && name !== "recording" && name !== "working";
  $("start").disabled = !canStart;
  $("stop").disabled = name !== "recording";
  const retry = $("retry");
  retry.hidden = name !== "error";
  // Failed notes are tried again; a failed recording is started again (a new recording).
  const label = queue.failed > 0 ? "Try again" : "Start again";
  retry.title = label;
  retry.setAttribute("aria-label", label);
  if (!failedMode) $("update").disabled = updateBusy;
}

function showQueueBadge() {
  const busy = Boolean(queue.processing);
  $("queue-badge").dataset.state = queue.failed > 0 ? "error" : busy ? "working" : "idle";
  $("queue-count").textContent = busy && queue.waiting > 0 ? "+" + queue.waiting : "";
  const btn = $("open-meetings");
  btn.title = TITLE_OPEN_MEETINGS;
  btn.setAttribute("aria-label", TITLE_OPEN_MEETINGS);
}

function render() {
  const name = computeView();
  app.dataset.view = name;
  const words = STATUS_WORDS[name];
  const clickable = EXPLAINED.has(name);
  statusEl.title = words;
  statusEl.setAttribute("aria-label", words);
  statusEl.setAttribute("aria-disabled", clickable ? "false" : "true");
  statusEl.tabIndex = clickable ? 0 : -1;
  updateButtons(name);
  showQueueBadge();
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

function resetMeters() {
  $("timer").textContent = "00:00";
  $("bar-me").style.height = "0%";
  $("bar-them").style.height = "0%";
}

function onStatus(status) {
  switch (status.state) {
    case "recording":
      if (stopping) return;
      recordError = false;
      rec = "recording";
      $("timer").textContent = formatTimer(status.elapsed);
      $("bar-me").style.height = levelPercent(status.me_db) + "%";
      $("bar-them").style.height = levelPercent(status.them_db) + "%";
      if (app.dataset.view !== "recording") render();
      break;
    case "stopped":
      // The recording is saved and the next Start is allowed; its notes are written in the
      // background (see onQueue).
      stopping = false;
      rec = "idle";
      render();
      break;
    case "error":
      // The words are on the card (the app sends it); this is only the red mark.
      console.error(status.kind + ": " + status.message);
      stopping = false;
      rec = "idle";
      recordError = true;
      render();
      break;
    default:
      // Anything else is not about the recording: keep what is on screen.
      break;
  }
}

function onQueue(next) {
  queue = next;
  render();
}

$("start").addEventListener("click", () => {
  stopping = false;
  recordError = false;
  resetMeters();
  // No second click until the first status event (or a refusal) has set the buttons again.
  $("start").disabled = true;
  invoke("start").catch((err) => {
    console.error(err);
    render();
  });
});

$("stop").addEventListener("click", () => {
  stopping = true;
  $("stop").blur();
  // Until the "stopped" event: the recording is being closed and Start would still be refused.
  rec = "saving";
  render();
  // "Not recording" means it already stopped on its own; the status events carry on.
  invoke("stop").catch(console.error);
});

// Failed notes are queued again by the backend. With no failed notes, this is a failed
// recording, and the button starts a new one.
$("retry").addEventListener("click", () => {
  stopping = false;
  if (queue.failed > 0) {
    invoke("retry").catch(console.error);
    return;
  }
  recordError = false;
  resetMeters();
  invoke("start").catch((err) => {
    console.error(err);
    render();
  });
});

$("open-meetings").addEventListener("click", () => {
  // A problem is shown on the card by the app.
  invoke("open_meetings").catch(console.error);
});

// A click on the status mark opens the card for what is going on (writing, failed, ready).
statusEl.addEventListener("click", () => {
  if (statusEl.getAttribute("aria-disabled") === "true") return;
  invoke("show_status_card").catch(console.error);
});

// The open column is 44px wide. The update button is two short lines ("Update" / "v0.1.8").
// Anything longer is said on the card. Folded (12px) hides this button.
function showOffer() {
  if (!offerVersion) return;
  failedMode = false;
  updateBusy = false;
  const btn = $("update");
  $("update-box").hidden = false;
  btn.hidden = false;
  btn.disabled = false;
  btn.classList.remove("failed");
  $("update-kicker").textContent = "Update";
  $("update-detail").hidden = false;
  $("update-detail").textContent = offerVersion;
  $("update-help").hidden = true;
  const label = "Update to " + offerVersion;
  btn.title = label;
  btn.setAttribute("aria-label", label);
  render();
}

function showUpdating(message) {
  failedMode = false;
  updateBusy = true;
  const btn = $("update");
  $("update-box").hidden = false;
  btn.hidden = false;
  btn.disabled = true;
  btn.classList.remove("failed");
  $("update-kicker").textContent = "Updating";
  $("update-detail").hidden = true;
  $("update-help").hidden = true;
  btn.title = message || "Updating... Meeting Notes will restart";
  btn.setAttribute("aria-label", btn.title);
  render();
}

function showFailed() {
  updateBusy = false;
  failedMode = true;
  const btn = $("update");
  $("update-box").hidden = false;
  btn.hidden = false;
  btn.disabled = false;
  btn.classList.add("failed");
  $("update-kicker").textContent = "Update";
  $("update-detail").hidden = false;
  $("update-detail").textContent = "failed";
  $("update-help").hidden = false;
  btn.title = "Update failed. Click to dismiss";
  btn.setAttribute("aria-label", "Update failed. Dismiss");
  render();
}

function hideUpdate() {
  $("update-box").hidden = true;
  $("update-help").hidden = true;
  failedMode = false;
  updateBusy = false;
  offerVersion = "";
  render();
}

function showOfferOrHide() {
  if (offerVersion) showOffer();
  else hideUpdate();
}

// The words for every update step (failed, refused, up to date, copied, too slow) are on the
// card; the button only changes shape.
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
      hideUpdate();
      break;
    case "failed":
      showFailed();
      break;
    case "refused":
    case "copied":
    case "timeout":
      showOfferOrHide();
      break;
    case "quiet":
      updateBusy = false;
      showOfferOrHide();
      break;
    default:
      break;
  }
}

function dismissFailure() {
  invoke("dismiss_update_notice")
    .then(() => {
      failedMode = false;
      showOfferOrHide();
    })
    .catch((err) => console.error(err));
}

$("update").addEventListener("click", () => {
  if (failedMode) {
    dismissFailure();
    return;
  }
  if (updateBusy) return;
  invoke("start_update").catch((err) => console.error(err));
});

$("update-help").addEventListener("click", () => {
  invoke("copy_update_help").catch((err) => console.error(err));
});

// ---- The message card ----

function makeCardButton(spec) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "card-btn";
  btn.dataset.action = spec.action;
  btn.dataset.primary = spec.primary ? "true" : "false";
  btn.textContent = spec.label;
  btn.addEventListener("click", () => cardAction(spec.action));
  return btn;
}

function closeCardView() {
  const hadFocus = cardEl.contains(document.activeElement);
  stage.dataset.card = "closed";
  cardEl.hidden = true;
  $("card-title").textContent = "";
  $("card-body").replaceChildren();
  $("card-buttons").replaceChildren();
  if (hadFocus && document.activeElement) document.activeElement.blur();
}

// `view` is the mn-card payload: { card, column_top }. column_top keeps the column where it was
// on screen while the window is taller.
function renderCard(view) {
  stage.style.setProperty("--col-top", ((view && view.column_top) || 0) + "px");
  card = view && view.card ? view.card : null;
  if (!card) {
    closeCardView();
    return;
  }
  const alert = card.role === "alertdialog";
  cardEl.setAttribute("role", card.role);
  if (alert) cardEl.removeAttribute("aria-live");
  else cardEl.setAttribute("aria-live", "polite");
  cardEl.dataset.tone = card.tone;
  cardEl.dataset.topic = card.topic;
  $("card-title").textContent = card.title;
  $("card-body").replaceChildren(
    ...card.lines.map((text) => {
      const line = document.createElement("p");
      line.textContent = text;
      return line;
    }),
  );
  $("card-buttons").replaceChildren(...card.buttons.map(makeCardButton));
  cardEl.hidden = false;
  stage.dataset.card = "open";
  // An error takes the focus, so the keyboard reaches its buttons at once.
  const first = $("card-buttons").querySelector("button");
  if (alert && first) first.focus({ preventScroll: true });
}

function cardAction(action) {
  // The red mark of a failed recording goes away with its card.
  if (action === "dismiss" && card && card.topic === "record_failed") {
    recordError = false;
    render();
  }
  invoke("card_action", { action }).catch(console.error);
}

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape" || !card) return;
  event.preventDefault();
  cardAction("dismiss");
});

// "Notes ready" goes away on a click anywhere on it (its button also opens My Meetings).
cardEl.addEventListener("click", (event) => {
  if (!card || card.topic !== "notes_ready" || event.target.closest("button")) return;
  cardAction("dismiss");
});

// A press-and-move on the empty area drags the column along the edge (and drops it on the
// nearest side edge). A button that does something keeps its click.
const PRESSABLE = 'button:not(:disabled):not([aria-disabled="true"])';

app.addEventListener("mousedown", (event) => {
  if (event.button !== 0 || event.target.closest(PRESSABLE)) return;
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
  const side = edge === "left" ? "left" : "right";
  app.dataset.edge = side;
  stage.dataset.edge = side;
}

listen("mn-status", (event) => onStatus(event.payload));
listen("mn-queue", (event) => onQueue(event.payload));
listen("mn-edge", (event) => setEdge(event.payload));
listen("mn-update", (event) => applyUpdate(event.payload));
listen("mn-card", (event) => renderCard(event.payload));
invoke("startup_update").then(applyUpdate).catch(console.error);
invoke("current_edge").then(setEdge).catch(console.error);
// Sessions queued before the window loaded (restart recovery), and a card shown before it loaded.
invoke("queue_status").then(onQueue).catch(console.error);
invoke("current_card").then(renderCard).catch(console.error);
render();
openColumn();
