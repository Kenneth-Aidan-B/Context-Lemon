# Lemonade Context Engine

**A folder-aware memory layer for [Lemonade](https://lemonade-server.ai/) — 100% offline.**

Point it at a folder. Ask questions about what's in it. Every answer cites the file and
line range it came from. Nothing leaves the machine.

> Why it matters: **it makes Lemonade dramatically easier to use.** Lemonade gives you
> fast local inference; this gives that inference *your files* as context, without a
> cloud vector database, an API key, or a single network request.

---

## What it actually is

Lemonade already exposes an OpenAI-compatible API. This app is the connective tissue
that turns it into a retrieval-augmented system over local folders:

```
  folder  →  walk  →  chunk  →  /v1/embeddings  →  int8 vector store
                                                          │
  question  →  /v1/embeddings  →  cosine top-k  ──────────┘
                                       │
                                       ▼
                        grounded prompt  →  /v1/chat/completions  →  answer + [Source N]
```

Both model calls go to Lemonade. This app owns the walking, chunking, storage,
retrieval, and grounding — the parts Lemonade deliberately leaves to you.

## Install & first run

1. Install [Lemonade Server](https://lemonade-server.ai/) and pull the two models:
   ```powershell
   lemonade pull nomic-embed-text-v1-GGUF   # embeddings, 768-dim, 74 MB
   lemonade pull Qwen3-1.7B-GGUF            # generation, 1.0 GB
   ```
2. Run the installer from Releases.
3. Launch it. On first run it registers a bundled 4-file sample corpus
   ("Project Nightingale") and indexes it automatically, so there is something to ask
   about immediately — no setup, no network.

Ask: *"What port does the Nightingale gateway listen on by default?"*

```
The Nightingale gateway listens on port 7913 by default [Source 1].

Sources
  sample\faq.md          (lines 1-21)
  sample\README.md       (lines 1-17)
  sample\architecture.md (lines 1-29)
  sample\changelog.md    (lines 1-23)
```

That is real output, printed by `cargo test --test rag_quality -- --nocapture`, not a
mock-up. `faq.md` ranks first because it is where the port is documented.

The citation is the product. An answer you can't trace isn't useful.

## Footprint

The design constraint was a RAG layer small enough to leave running all day.

| | Per chunk | At 50,000 chunks |
|---|---|---|
| Naive (f32 vectors + text on the heap) | ~5 KB | ~265 MB |
| **This implementation** | **917 B** | **~44 MB** |

Two decisions get it there:

- **int8-quantised embeddings.** Vectors are unit-normalised, then rescaled so the
  largest component lands on ±127. That rescaling is load-bearing: a raw 768-d unit
  vector has components around 0.036, which would occupy barely three of the eight
  bits available and quantise to mush. 3072 B → 768 B.
- **Chunk text lives on disk**, in a side blob addressed by (offset, length). Only the
  top-k are read back to build a prompt.

Quantisation is not free in principle, so it is tested rather than assumed:
`quantization_preserves_ranking_and_score` compares int8 against exact f32 cosine over
200 vectors and requires an identical top-1, an identical top-5 set, and every score
within 0.01.

The live figure is shown in the UI (`4 file(s) indexed · 4 chunk(s) · 4.1 KB RAM`), so
it is readable off the screen rather than taken on trust.

## Hardware

Lemonade picks the inference backend; this app only sizes its own work. Measured on the
development machine:

| Spec | Value |
|---|---|
| RAM | 16 GB |
| GPU | NVIDIA RTX 3050 6 GB (laptop) |
| NPU | none |
| Generation | ~104 tok/s (Qwen3-1.7B-GGUF) |

Model choice is currently fixed. Hardware-tiered model selection is not implemented —
see [Status](#status).

## Behaviour worth knowing

- **Incremental.** Re-indexing hashes content and skips unchanged files. Hashing is
  FNV-1a, spelled out rather than `DefaultHasher`, whose algorithm std documents as
  unstable across Rust releases — a toolchain upgrade would otherwise invalidate every
  fingerprint at once and silently force a full re-embed.
- **Reconciling.** Files deleted, renamed, newly gitignored, or grown past the 5 MB cap
  are purged from the index, so nothing is ever cited from a file that no longer says it.
- **Crash-safe.** The index is written to a temp file, fsynced, then renamed. An index
  that fails to load is preserved as `index.bin.corrupt` rather than silently replaced
  with an empty one — those embeddings cost real compute.
- **Cancellable.** Removing a folder calls off any in-flight index job for it, and the
  cancellation is rechecked under the store lock, so a job cannot write chunks for a
  folder after it was purged.
- **Model-aware.** Two embedding models can share a dimensionality while embedding into
  unrelated spaces, so the index records which model produced it and resets if that
  changes. Without this, swapping models yields confident citations ranked against a
  foreign vector space.
- **Live.** Watched folders are re-indexed automatically on change. Events are debounced
  for 1.5 s of quiet first, because editors don't write a file once — they write,
  rename, and touch it several times over a few hundred milliseconds, and re-indexing on
  the first event would mean re-embedding a file you're still typing into. Events from
  `node_modules`, `.git` and friends are discarded before they reach the queue, so a
  `git status` in a watched repo doesn't trigger a pass.
- **Ignores what you'd expect.** Honours `.gitignore`; skips `node_modules`, `.git`,
  build output, and binaries; indexes 30+ text extensions.

## Storage

```
%APPDATA%\lemonade-context-engine\
  config.json           watched folders + Lemonade URL
  index.bin             metadata + int8 vectors (format v3)
  chunks.<gen>.dat      chunk text, read on demand
  index.bin.corrupt     kept if an index ever fails to load
```

The text blob is append-only and generation-numbered: compaction publishes a new
generation and deletes the old one only after the index that references it is durably
on disk, so no crash point leaves a mismatched pair.

## Build from source

```powershell
cd lemonade-context-engine
npm install
npm run tauri dev      # or: npm run tauri build
```

Tests:

```powershell
cd src-tauri
cargo test --lib                        # store: quantisation, persistence, budget
cargo test --test rag_smoke             # end-to-end, needs Lemonade running
cargo test --test rag_quality           # grounding + refusal, needs Lemonade running
```

The live suites skip (rather than fail) when Lemonade isn't reachable or no index
exists yet.

> **Note for this development machine:** `~/.cargo` is owned by `Administrators` with
> only read+execute for `Users`, and the shell runs under a UAC-filtered token, so cargo
> cannot write its registry cache. Set `$env:CARGO_HOME = "D:\AMD_Lemonade\.cargo-home"`
> before any cargo/tauri command. Not needed on a normal install.

## Status

Implemented: tray app, folder registration, gitignore-aware walking, overlap chunking,
int8 disk-backed store, incremental + reconciling indexing, **live file watching**,
retrieval, grounded generation with citations, first-run offline demo.

Not yet implemented:

- Hardware detection and model tiering (model choice is fixed)
- CI
- Signed installer — see [Packaging](#packaging)

### Packaging

`npm run tauri build` produces a working `lemonade-context-engine.exe` (13.9 MB, inside
the 20 MB budget), but MSI bundling currently fails on this machine: Tauri downloads
the WiX toolset from GitHub on first bundle and the request exceeds its global timeout.
The application itself builds and runs; only the installer step is blocked.

## Licence

This project: MIT (see [LICENSE](LICENSE)).
Dependencies: 12 direct, 519 transitive, all permissive — see
[THIRD_PARTY.md](THIRD_PARTY.md).

Models and Lemonade Server are **not** redistributed; you install them yourself. The
bundled `sample/` corpus is fiction written for this project — "Project Nightingale"
and Aeroflux Systems do not exist.
