use crate::indexing::chunker;
use crate::indexing::store::{hash_content, unix_time, NewChunk, VectorStore};
use crate::indexing::walker;
use crate::lemonade;
use crate::lemonade::LemonadeClient;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub const EMBEDDING_MODEL: &str = "nomic-embed-text-v1-GGUF";
/// unsloth/Qwen3-0.6B-GGUF:Q4_0 — Q4_0 is llama.cpp's direct 4-bit (INT4) quant, no
/// GPU required. Chosen over the 1.2B+ LFM2 family for footprint: it needs no extra
/// download on a machine that already has it, and it's under half the size of
/// Qwen3-1.7B-GGUF, which cuts both resident memory and cold-load time.
pub const CHAT_MODEL: &str = "Qwen3-0.6B-GGUF";
const EMBED_BATCH_SIZE: usize = 32;
const TOP_K: usize = 5;
/// Persist at most this often during a run, so a long index is not lost on a crash
/// but we still avoid rewriting the whole index once per file.
const FLUSH_EVERY_N_FILES: usize = 50;
/// How many times a single chunk may be halved before it's given up on rather than
/// embedded. 2^6 = 64-way split of a ~1400-char chunk bottoms out well under the
/// 200-char floor below, so this is never the binding constraint in practice.
const MAX_SPLIT_DEPTH: u32 = 6;
/// Below this, a chunk is judged unsplittable-usefully — a fragment this short is not
/// worth the extra round trip, and if it's *still* rejected as too large at this size,
/// something other than length is wrong, so retrying smaller pieces won't help.
const MIN_SPLIT_CHARS: usize = 200;

#[derive(Debug, Serialize, Clone)]
pub struct IndexStats {
    pub files_indexed: usize,
    pub chunks_indexed: usize,
    pub files_skipped_unchanged: usize,
    pub files_purged: usize,
    pub files_failed: usize,
    /// True when the job stopped early because its folder was removed or superseded.
    pub cancelled: bool,
}

pub async fn index_folder(
    store: &VectorStore,
    client: &LemonadeClient,
    folder: &str,
    cancel: &AtomicBool,
) -> Result<IndexStats, String> {
    // A model swap invalidates every stored vector, so catch it before doing any work.
    if store.reset_if_model_changed(EMBEDDING_MODEL) {
        store.flush().map_err(|e| e.to_string())?;
    }

    let root = Path::new(folder);
    let files = walker::walk_folder(root);

    // Anything previously indexed under this folder that the walk no longer yields
    // (deleted, renamed, now ignored, or grown past the size cap) must be dropped,
    // or ask() keeps citing files that are gone.
    let seen: HashSet<String> = files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let files_purged = store.retain_seen_files(folder, &seen);

    let mut files_indexed = 0usize;
    let mut chunks_indexed = 0usize;
    let mut files_skipped_unchanged = 0usize;
    let mut files_failed = 0usize;
    let mut since_flush = 0usize;

    for file_path in files {
        // Checked before each file, and again inside the store under its lock, so a
        // removed folder cannot have chunks written for it after the purge.
        if cancel.load(Ordering::SeqCst) {
            store.flush().map_err(|e| e.to_string())?;
            return Ok(IndexStats {
                files_indexed,
                chunks_indexed,
                files_skipped_unchanged,
                files_purged,
                files_failed,
                cancelled: true,
            });
        }

        let path_str = file_path.to_string_lossy().to_string();
        let metadata = match fs::metadata(&file_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = metadata.modified().map(unix_time).unwrap_or(0);

        let text = match fs::read_to_string(&file_path) {
            // Unreadable now (binary, non-UTF-8, locked). Drop any stale chunks so we
            // never serve content the file no longer has.
            Err(_) => {
                store.clear_file(&path_str);
                files_failed += 1;
                continue;
            }
            Ok(t) => t,
        };
        let file_hash = hash_content(&text);

        // The content is already read and hashed, so the hash alone decides. Comparing
        // mtime too would force a full re-embed after any touch that leaves bytes
        // identical (git checkout, backup restore, cloud resync).
        if let Some((_, existing_hash)) = store.file_fingerprint(&path_str) {
            if existing_hash == file_hash {
                files_skipped_unchanged += 1;
                continue;
            }
        }

        let spans = chunker::chunk_text(&text);
        if spans.is_empty() {
            // File became empty — clear its old chunks instead of leaving them behind.
            store.clear_file(&path_str);
            continue;
        }

        let mut new_chunks = Vec::with_capacity(spans.len());
        let mut dropped = 0usize;
        for batch in spans.chunks(EMBED_BATCH_SIZE) {
            let texts: Vec<String> = batch.iter().map(|s| s.text.clone()).collect();
            let embedded: Vec<(chunker::ChunkSpan, Vec<f32>)> =
                match client.embed(&texts, EMBEDDING_MODEL).await {
                    Ok(embeddings) => batch.iter().cloned().zip(embeddings).collect(),
                    // One dense chunk (e.g. escape-heavy JSON) tokenizing past the
                    // backend's limit must not sink the other 31 in this batch, or —
                    // since this whole function bails via `?` — every file behind it
                    // in the folder. Fall back to one request per chunk so only the
                    // oversized one needs the slower, splitting path.
                    Err(e) if e.starts_with(lemonade::EMBED_TOO_LARGE_PREFIX) => {
                        let mut resolved = Vec::with_capacity(batch.len());
                        for span in batch {
                            resolved.extend(embed_span_resilient(client, span.clone()).await?);
                        }
                        resolved
                    }
                    Err(e) => return Err(e),
                };
            for (span, embedding) in embedded {
                if embedding.is_empty() {
                    dropped += 1;
                    continue;
                }
                new_chunks.push(NewChunk {
                    start_line: span.start_line,
                    end_line: span.end_line,
                    text: span.text,
                    embedding,
                });
            }
        }

        if dropped > 0 {
            // Storing a partial file under its current hash would mark it "unchanged"
            // forever, leaving a permanent hole. Leave the fingerprint stale so the
            // next run retries it, and report the failure.
            files_failed += 1;
            continue;
        }

        let chunk_count = new_chunks.len();
        let written = store
            .upsert_file(&path_str, mtime, file_hash, new_chunks, cancel)
            .map_err(|e| e.to_string())?;
        if !written {
            // Cancelled while this file was being embedded; the next loop iteration
            // reports the cancellation.
            continue;
        }
        chunks_indexed += chunk_count;
        files_indexed += 1;

        since_flush += 1;
        if since_flush >= FLUSH_EVERY_N_FILES {
            store.flush().map_err(|e| e.to_string())?;
            since_flush = 0;
        }
    }

    store.flush().map_err(|e| e.to_string())?;

    Ok(IndexStats {
        files_indexed,
        chunks_indexed,
        files_skipped_unchanged,
        files_purged,
        files_failed,
        cancelled: false,
    })
}

/// Embed one chunk, halving and retrying it if Lemonade rejects it as too large,
/// until it fits or [`MAX_SPLIT_DEPTH`]/[`MIN_SPLIT_CHARS`] is reached. Returns one
/// entry per surviving fragment; a fragment that still won't fit even at the floor
/// comes back with an empty embedding, which the caller already treats as "drop this
/// one and leave the file's fingerprint stale so the next run retries it" — the same
/// path used for an embedding that came back empty for any other reason.
///
/// An explicit work stack rather than recursion: `async fn` can't call itself directly
/// (the resulting future would have unbounded size), and boxing each recursive call
/// is more ceremony than this needs.
async fn embed_span_resilient(
    client: &LemonadeClient,
    span: chunker::ChunkSpan,
) -> Result<Vec<(chunker::ChunkSpan, Vec<f32>)>, String> {
    let mut pending: Vec<(chunker::ChunkSpan, u32)> = vec![(span, 0)];
    let mut out = Vec::new();

    while let Some((piece, depth)) = pending.pop() {
        match client.embed(&[piece.text.clone()], EMBEDDING_MODEL).await {
            Ok(mut embeddings) => {
                let embedding = embeddings.pop().unwrap_or_default();
                out.push((piece, embedding));
            }
            Err(e) if e.starts_with(lemonade::EMBED_TOO_LARGE_PREFIX) => {
                if depth < MAX_SPLIT_DEPTH && piece.text.len() > MIN_SPLIT_CHARS {
                    let (left, right) = split_span(&piece);
                    pending.push((right, depth + 1));
                    pending.push((left, depth + 1));
                } else {
                    out.push((piece, Vec::new()));
                }
            }
            // A real connectivity/model/server failure — propagate it. Retrying a
            // smaller piece would not fix an unreachable server.
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// Bisect a chunk's text at a UTF-8-safe byte boundary near its midpoint. Both halves
/// keep the parent's line range — an acceptable loss of precision, since citations
/// already report whole-chunk line ranges rather than exact character offsets.
fn split_span(span: &chunker::ChunkSpan) -> (chunker::ChunkSpan, chunker::ChunkSpan) {
    let mut boundary = span.text.len() / 2;
    while boundary < span.text.len() && !span.text.is_char_boundary(boundary) {
        boundary += 1;
    }
    let (left_text, right_text) = span.text.split_at(boundary);
    (
        chunker::ChunkSpan {
            text: left_text.to_string(),
            start_line: span.start_line,
            end_line: span.end_line,
        },
        chunker::ChunkSpan {
            text: right_text.to_string(),
            start_line: span.start_line,
            end_line: span.end_line,
        },
    )
}

#[derive(Debug, Serialize, Clone)]
pub struct Source {
    pub file: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct AskResponse {
    pub answer: String,
    pub sources: Vec<Source>,
}

pub async fn ask(store: &VectorStore, client: &LemonadeClient, question: &str) -> Result<AskResponse, String> {
    let trimmed = question.trim();
    if trimmed.is_empty() {
        return Err("Question cannot be empty".to_string());
    }

    let query_embedding = client
        .embed(&[trimmed.to_string()], EMBEDDING_MODEL)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "empty embedding response".to_string())?;

    if query_embedding.is_empty() {
        return Err("Lemonade returned an empty embedding for the question".to_string());
    }

    let results = store.search(&query_embedding, TOP_K)?;

    if results.is_empty() {
        return Ok(AskResponse {
            answer: "No indexed files yet — add a folder first.".to_string(),
            sources: Vec::new(),
        });
    }

    let mut context = String::new();
    let mut sources = Vec::new();
    for (i, (chunk, _score)) in results.iter().enumerate() {
        context.push_str(&format!(
            "[Source {}: {} lines {}-{}]\n{}\n\n",
            i + 1,
            chunk.file_path,
            chunk.start_line,
            chunk.end_line,
            chunk.text
        ));
        sources.push(Source {
            file: chunk.file_path.clone(),
            start_line: chunk.start_line,
            end_line: chunk.end_line,
        });
    }

    let system = "You are a helpful assistant answering questions using ONLY the provided context. \
        Every claim must be grounded in the context below. If the answer isn't in the context, say so \
        plainly instead of guessing. Cite sources inline using the format [Source N].";
    let user = format!("Context:\n{context}\nQuestion: {trimmed}");

    let answer = client.chat(CHAT_MODEL, system, &user, true).await?;

    Ok(AskResponse { answer, sources })
}
