# P15 — Workshop search and filtering

- **Pain point:** Poor search, NSFW/AI floods.
- **Fix:** Local library: search over titles, tags, descriptions and Workshop ids; kind, source and tag filters; questionable and mature items hidden until asked; sort by measured cost. Diorama never browses the Workshop, so only what the user subscribed to appears.
- **Automated:** `diorama-library` tests (listing, previews, settings)

## Steps
1. Open Diorama → Library; search a title word and a Workshop id.
2. Filter by kind and tag; toggle "Show mature".
3. Sort "Lightest first".

## Pass when
Each step narrows or orders the grid as named; mature/questionable items appear only with the toggle.

## Last run
2026-10-26 off-screen (mock UI over the real library, 99 wallpapers), then the desktop trial (2026-10-26 to 28). Author blocklist and AI filter from the plan are not possible from local files: Workshop folders carry no author, and none of the 99 items has an AI tag.
