"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const DONE_RESET_MS = 10000;
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

function setWorking(text) {
  $("working-text").textContent = text;
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
      $("timer").textContent = formatTimer(status.elapsed);
      $("bar-me").style.width = levelPercent(status.me_db) + "%";
      $("bar-them").style.width = levelPercent(status.them_db) + "%";
      setView("recording");
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

function openMeetings() {
  run("open_meetings", showNotice);
}

$("start").addEventListener("click", () => {
  stopping = false;
  $("timer").textContent = "00:00";
  $("bar-me").style.width = "0%";
  $("bar-them").style.width = "0%";
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

$("dismiss").addEventListener("click", () => setView("idle"));

$("done-open").addEventListener("click", () => {
  openMeetings();
  setView("idle");
});

$("open-meetings").addEventListener("click", (event) => {
  event.preventDefault();
  openMeetings();
});

listen("mn-status", (event) => onStatus(event.payload));
