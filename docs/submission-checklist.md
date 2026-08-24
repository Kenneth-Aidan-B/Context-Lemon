# AMD x Lemonade submission checklist

Use this checklist from top to bottom. Repository-only work is already represented by
tracked files; account, media, and hardware steps need to be completed by the project
owner.

## 1. Verify the release candidate

From a clean checkout on Windows:

```powershell
npm ci
npm run build

cd src-tauri
cargo test --lib --locked
```

With Lemonade running (the tests create a disposable sample index):

```powershell
cargo test --test rag_smoke --locked -- --nocapture
cargo test --test rag_quality --locked -- --nocapture
```

A healthy run is 39 passing tests: 32 unit, 4 quality, 3 smoke, with the 4 benchmark
tests correctly reported as ignored.

Record the commit SHA and retain the full test output with the submission materials.

## 2. Build the release

Ensure these versions agree before tagging:

- `package.json` → `version`
- `src-tauri/Cargo.toml` → `package.version`
- `src-tauri/tauri.conf.json` → `version`

Build the release artifacts with the Tauri CLI. Do **not** run a bare
`cargo build --release` alongside it: that rebuilds the binary outside the CLI's
environment, so it points at the Vite dev server instead of embedding the frontend, and
it silently overwrites the bundler's output. The symptom is a packaged app that opens to
`localhost refused to connect`.

```powershell
npm ci
npm run tauri build
```

Expected Windows output locations:

```text
src-tauri\target\release\lemonade-context-engine.exe
src-tauri\target\release\bundle\msi\*.msi
src-tauri\target\release\bundle\nsis\*.exe
```

If MSI or NSIS bundling is blocked, publish the standalone executable in a ZIP and
name the archive `Context-Lemon-v0.3.0-windows-x64.zip`. Label it clearly as an
unsigned preview; do not imply that an unsigned artifact is signed. Test the exact
uploaded artifact on a second Windows account or machine.

Launch the packaged executable before uploading it and confirm the interface renders.
A build misconfigured to use the dev server looks byte-plausible and passes every test
— it only fails when a human opens it.

Create the release with:

- a two-sentence product pitch;
- supported Windows versions and CPU/GPU expectations;
- Lemonade and model prerequisites;
- SHA-256 checksums for every artifact;
- a link to the demo video;
- the known unsigned-installer or SmartScreen caveat, if applicable; and
- the exact source commit used for the build.

After publication, update the README source-only notice with the direct release link.

## 3. Capture submission media

Follow [demo-script.md](demo-script.md). Before recording, verify that no personal
paths, notifications, tokens, email addresses, or unrelated windows are visible.

Required outputs:

- 60–120 second demo video;
- folder/indexing screenshot;
- grounded answer and citations screenshot; and
- live update/re-index screenshot or short GIF.

Add the video link directly below the README pitch and place screenshots in
`docs/media/`.

## 4. Validate on AMD hardware, if available

On Ryzen AI or Radeon hardware, record:

```powershell
lemonade backends
lemonade bench Bonsai-8B-gguf --scenarios chat
```

Then run the port question in Context-Lemon and record:

| Field | Result |
| --- | --- |
| CPU/APU | |
| GPU | |
| NPU | |
| RAM | |
| Lemonade backend selected | |
| Generation tok/s | |
| Time to first token | |
| Correct answer (`7913`) | |
| Correct file citation | |

Report only observed results. AMD hardware is a stronger challenge story, not a
requirement for the application architecture.

## 5. Polish the GitHub project

Repository owner actions:

- add topics: `amd`, `lemonade`, `local-ai`, `rag`, `privacy`, `rust`, `tauri`;
- add the demo URL to the repository About section;
- verify the MIT license is detected by GitHub;
- confirm the CI badge is green on the release commit;
- verify every README link from a logged-out browser; and
- pin the release and any challenge submission post.

## 6. Final judge check

A new visitor should be able to answer each question within 30 seconds:

- What does the project do?
- Where is Lemonade used?
- What stays local?
- What is technically novel?
- Can I see it working?
- Can I install or build it?
- Can I trace an answer to a file and line range?

Do not submit until the demo link and downloadable artifact both work from a logged-out
browser.
