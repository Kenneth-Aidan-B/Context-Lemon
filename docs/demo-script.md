# Context-Lemon 90-second demo script

One continuous recording, six beats, timed to 1:30. Built for `v0.3.0` with
`Bonsai-8B-gguf` as the answer model.

The beat that matters most is **05**: editing a source file on camera and watching the
answer change. Nothing else proves as directly that the answer is read from your files
rather than recalled from training data.

## Prepare once

1. Start Lemonade Server and confirm both models are installed:
   ```powershell
   lemonade pull nomic-embed-text-v1-GGUF
   lemonade pull Bonsai-8B-gguf
   ```
2. **Copy `sample/` to a disposable folder outside the repository.** Beat 05 edits a
   file, and the bundled corpus is a test fixture — editing it in place breaks the
   quality suite.
3. Open the disposable `faq.md` in an editor, positioned so one keystroke reaches it.
   Fumbling for the window on camera costs the beat.
4. Launch Context-Lemon, add the disposable folder, wait for indexing to finish.
5. Ask one throwaway question and clear it. A cold first answer costs 4.4s on
   Bonsai-8B; warm it is under a second.
6. Raise display scaling to 125–150%. The app's content column is capped at 560 CSS px,
   so widening the window only adds margin — scaling is what makes text survive
   video compression.
7. Turn on Do Not Disturb. Close everything not being demonstrated. Confirm the Status
   line reads "Connected to Lemonade server" — it is also your closing shot.
8. Check the tray for older Context-Lemon instances and quit them. Every version shares
   one config directory, so two running builds fight over the same index.

## Shot list

| Time | On screen | Narration |
| --- | --- | --- |
| 0:00–0:08 | Context-Lemon open and idle | "Asking an AI about your own files usually means uploading them first. Context-Lemon doesn't — it runs entirely on your machine, through AMD Lemonade." |
| 0:08–0:20 | Click **+ Add Folder**, pick the disposable folder, hold on the progress line as it counts files scanned, remaining, and chunks embedded | "Point it at a folder. It walks the files, respects your gitignore, and embeds them locally — showing exactly how many are left." |
| 0:20–0:40 | Ask the Talon question; let the answer render, then reveal the **Sources** panel with file names and line ranges | "Ask it something real. The answer comes back grounded, and every claim is traceable to a file and a line range. That's the whole point: an answer you can't check isn't worth much." |
| 0:40–0:56 | Ask the encryption question; hold on the refusal | "Now ask something that isn't in your files. A smaller model invents an answer here. This one says it doesn't know — refusing to guess is the feature." |
| 0:56–1:18 | Switch to the editor, change `7913` to `7921` in `faq.md`, save, return to the app, watch it re-index, ask the port question | "Change the source file, and the watcher re-indexes just what changed. Ask again — the answer follows the file. Nothing was rebuilt and nothing was uploaded." |
| 1:18–1:30 | Open the **Answer Model** dropdown briefly to show both VRAM groups, then rest on the Status line and cut to an end card | "You pick the model that fits your GPU. No cloud, no API keys, nothing uploaded. Rust and Tauri, MIT licensed, running on Lemonade." |

Narration totals 153 words across 90 seconds, deliberately leaving about a
third of the runtime silent so the interface has room to speak for itself.

## Questions with verified answers

Run against the bundled `sample/` corpus with `Bonsai-8B-gguf` during testing.

| Ask | Expect | Behaviour |
| --- | --- | --- |
| How does the Talon Cache decide what to evict, and what determines its capacity? | Cost-aware LRU weighing recency against compute cost; 512 MB per accelerator | Answers |
| What port does the Nightingale gateway listen on by default? | `7913`, citing `faq.md` | Answers |
| Who is the lead engineer on Project Nightingale? | Priya Raman | Answers |
| If an accelerator stops responding, walk through what happens and on what timeline. | 2s heartbeat, 3 missed ≈ 6s to `suspect`, 30s to `dead` | Answers |
| What encryption algorithm does Talon use to secure its cache entries? | States the context does not mention one | **Refuses** |
| What was Project Nightingale's annual marketing budget? | States the context does not contain it | **Refuses** |

The two refusals are the differentiator. `Qwen3-0.6B-GGUF` fails the encryption question
outright — it answers "SHA-256", inventing a security property from a hashing detail.

## Recording

Both tools ship with Windows 11; nothing needs installing.

- **Snipping Tool** — `Win + Shift + S`, switch to the record icon, drag a region around
  the app window, Start. Region capture means nothing outside the rectangle can wander
  into shot. Recommended.
- **Xbox Game Bar** — `Win + G`, Capture widget. Records the focused window and takes
  mic input if you want to narrate live in one pass.

Settings: record at the native 1920×1080 and 60 fps, export H.264 MP4. Record narration
separately if you want to click precisely without racing your own speech.

## Do not switch models inside a take

Committing a model switch stalls the next answer while Lemonade loads: 4.4s for
Bonsai-8B, 10.5s for Qwen3-0.6B, 18.8s for Qwen3-1.7B. Beat 06 opens the dropdown to
show the list, not to use it. If you do switch on camera, cut the pause in the edit —
nothing renders during it.

## Publishing the video

GitHub renders an inline player for attachment URLs, so the video can play at the top of
the README rather than sitting behind a link.

1. Open a new issue on the repository — **do not submit it**.
2. Drag the MP4 into the comment box. GitHub uploads it immediately and rewrites the box
   with a `https://github.com/user-attachments/assets/…` URL.
3. Copy that URL and close the tab. The upload is already permanent; the issue is never
   posted.
4. Put the bare URL in the README under the pitch.

Limits are 100 MB per video on a paid plan (10 MB on Free), MP4/MOV/WebM, H.264
recommended. Do not commit the MP4 to the repository: it would live in git history
permanently, and GitHub strips `<video>` tags from README markdown, so a committed file
only plays after a click into the blob viewer.

## Screenshots to capture afterwards

Save under `docs/media/` so the README can use stable relative paths.

1. `01-folder-indexed.png` — the full app with Lemonade connected, the folder listed,
   and index counts readable.
2. `02-grounded-answer.png` — an answer with its complete file-and-line citation list.
3. `03-model-picker.png` — the Answer Model dropdown open, showing both VRAM groups.
4. `04-live-reindex.png` — the indexing status right after the file edit, or the updated
   answer if the status is too brief to catch.

Crop only empty desktop space. Never crop away the Lemonade connection state, index
counts, question, answer, or citations. Write descriptive alt text when adding them.

Restore or delete the disposable corpus afterwards. Do not edit the bundled `sample/`
used by the test suite.
