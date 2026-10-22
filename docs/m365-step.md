# M365 step texts for the meeting-card prompt

The Rust core fills `{{M365_STEP}}` in `prompts/meeting-card.md` with one of these sections.

## claude
Find this meeting in the PM's Outlook calendar with the outlook_calendar_search tool: the event overlapping the start–end time (allow 15 minutes either side). Use its subject as the title (title_source "calendar") and its attendees. Then use chat_message_search (meeting subject, around that date) and sharepoint_search (files named in the transcript or chat) to find the related Teams chat and documents. Put only URLs those tools returned into related_links. Use what you find to fix name spellings and fill project/client. If no event matches, take the title from the transcript (title_source "transcript").

## codex
No calendar access in this run. Take the title from the transcript (title_source "transcript") and leave related_links empty.
