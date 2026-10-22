# Meeting Notes: test call script

**Who:** Mustafa (the PM, the "Me" side) and one colleague (the "Them" side).
**How long:** about 4 minutes of talking, plus 1 minute of silence at the end.
**Date it was written for:** Thursday 22 October 2026. If you run it on another day, the "expected" dates below move with it.

## Before the call (2 minutes)

1. In Outlook, create a Teams meeting called **"Meeting notes pilot test"** for now, and invite your colleague. Claude uses this calendar entry to name the meeting and list the people in it.
2. Put on **headphones**. Without them the mic records your colleague a second time.
3. Join the call. Then, in a terminal, run:
   `"/mnt/ssd/CTCHealth/Project Management Aritfact-Skill-Github/core/target/debug/mn" run`
4. Wait until you see lines starting with `{"state":"recording"`, then start reading.

Read the lines naturally. You don't need to match them word for word.

## Part 1: what each feature needs to hear

| # | Line | What it tests |
|---|---|---|
| 1 | **Mustafa:** Hi, thanks for joining. Quick check-in on the onboarding guide for the new portal launch. | Meeting topic and summary |
| 2 | **Colleague:** Hi. So the first draft is done. I uploaded it next to the Q4 roadmap tracker on SharePoint. | Related links (SharePoint search) |
| 3 | **Mustafa:** Great. I'll review the draft and send you my comments by tomorrow. | **My to-do** with a date (Fri 23 Oct) |
| 4 | **Colleague:** Perfect. I still need the final screenshots from the product team. I'll have them by Monday. | **Waiting on others** with a date (Mon 26 Oct) |
| 5 | **Mustafa:** OK. Let's decide now: we send the guide to the client only after legal has checked it. | **Decision** |
| 6 | **Colleague:** Agreed. That means the client review moves to the 5th of November. | **Decision** + **deliverable** with an exact date |
| 7 | **Mustafa:** Then I'll update the timeline and email it to the client today. | **My to-do**, due today |
| 8 | **Colleague:** Can you also book the legal review call? | A request to me... |
| 9 | **Mustafa:** Yes, I'll book it for next Wednesday. | ...that I accept: **My to-do** (Wed 28 Oct) |
| 10 | **Colleague:** The German translation is with the agency. They said end of next week. | **Deliverable** owned by someone outside the call (Fri 30 Oct) |
| 11 | **Mustafa:** Fine. If nothing arrives, I'll chase the agency. | **My to-do** with no date (should show "no date", not an invented one) |

## Part 2: things that should NOT become tasks

| # | Line | Expected |
|---|---|---|
| 12 | **Colleague:** By the way, the weather is terrible today. | Ignored |
| 13 | **Mustafa:** Maybe one day we should redo the whole template, but not now. | Not a to-do: it's a "maybe" |
| 14 | **Colleague:** Actually, about the screenshots: you have better contact with the product team. Can you take that one? | Ownership change... |
| 15 | **Mustafa:** Sure, I'll get the screenshots myself by Monday. | ...the screenshots become **my to-do** (Mon 26 Oct), not "waiting on" |

## Part 3 (optional, 30 seconds): Arabic

| # | Line |
|---|---|
| 16 | **Mustafa:** تمام، هبعتلك الملف المعدل بكرة الصبح. |
| 17 | **Colleague:** ماشي، وأنا هراجعه وأرد عليك يوم الاتنين. |

Expected: line 16 becomes a to-do ("Send the updated file", Fri 23 Oct). Line 17 goes to waiting on others (review, Mon 26 Oct). The card is still written in English.
Warning: the transcriber picks one language per side from the first 30 seconds. A short Arabic part at the end of an English call may come out weak. That counts as a finding, not a failure.

## Ending the call

| # | Line |
|---|---|
| 18 | **Mustafa:** Great, thanks. Talk next week. |

Hang up, **stay silent**, and leave the terminal open.

## After the call: what should happen

| Time after hang-up | You should see in the terminal |
|---|---|
| about 60 s | `"state":"stopped" ... "reason":"silence"` |
| then 1 to 3 min | `transcribing` → `transcribed` |
| then about 60 s | `summarizing` with `"engine":"claude"` → `summarized` |
| then about 40 s | `publishing` → `published` with your page link |

Then open https://claude.ai/artifact/REDACTED. The new meeting appears within 60 seconds, or straight away if you reload.

## Checklist (tick while you check the page)

| ✓ | Check | Expected |
|---|---|---|
| ☐ | Meeting title | "Meeting notes pilot test" (from Outlook), **without** the "Name guessed" tag |
| ☐ | Attendees | You and your colleague |
| ☐ | Summary | At most 5 lines; it mentions legal first and the client review on 5 Nov |
| ☐ | Decisions | Legal before client · client review moves to 5 Nov |
| ☐ | My to-dos | Review draft (23 Oct) · update + email timeline (today) · book legal call (28 Oct) · chase agency (no date) · screenshots (26 Oct) |
| ☐ | Waiting on others | German translation from the agency (30 Oct). Screenshots should **not** be here |
| ☐ | Not tasks | Nothing about the weather or "redo the template" |
| ☐ | Today block | "Update + email timeline" shows as due today |
| ☐ | Related links | The SharePoint tracker link, if the search finds it |
| ☐ | Transcript | Opens; "Me" and "Them" lines are separate and not doubled |
| ☐ | Download | "Download transcript" saves a .md file |
| ☐ | Tick | Tick one to-do, reload: it stays ticked |
| ☐ | Audio | The session folder in `/mnt/data/audio-lab/MeetingNotes/` has no .wav files left |
| ☐ | Arabic (optional) | Lines 16–17 appear as tasks in English |

## If something goes wrong

- **It never stops by itself:** press Ctrl+C. It still transcribes and publishes. Tell me what `me_db` showed in the last status lines.
- **The terminal shows `"state":"error"`:** copy that line to me.
- **The page doesn't show the meeting:** run `mn publish` (same folder as the `mn run` command above), then reload.
