use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

pub struct LemonadeClient {
    base_url: String,
    http: reqwest::Client,
}

/// A running model needs room for its KV cache and compute buffers on top of the
/// weights Lemonade reports, so the checkpoint size alone understates what picking a
/// model actually costs.
///
/// Measured directly as GPU memory, by unloading everything, reading the card's used
/// bytes as a baseline, loading one model and taking the delta (llama.cpp Vulkan
/// backend, ctx_size 4096): Qwen3-0.6B +0.47 GB, Qwen3-1.7B +0.49 GB, Bonsai-8B
/// +0.58 GB. Rounded up from the worst case so the estimate errs high — promising a
/// model will fit and then thrashing is far worse than being slightly pessimistic.
///
/// Deliberately *not* derived from the host process's private bytes. Those run ~0.9 GB
/// above the checkpoint, but on a GPU backend the weights are counted twice that way —
/// once mapped in host memory and once resident on the card — which overstates the real
/// constraint. On a CPU-only backend the same cost simply lands in system RAM instead.
const RUNTIME_OVERHEAD_GB: f64 = 0.6;

/// The ceiling a model may be estimated to occupy at runtime and still be offered.
/// Above this the app would be handing users a choice that stalls their machine.
pub const MAX_MODEL_RAM_GB: f64 = 6.0;

/// Under this, a model is flagged as leaving the machine comfortably usable alongside
/// it — the tier you can pick without thinking about what else is running.
pub const LIGHT_MODEL_RAM_GB: f64 = 2.0;

/// Lemonade serves embedding, transcription, reranking and generation models from the
/// same `/models` list, so the chat picker has to exclude everything that cannot answer
/// a chat completion. Matching on what a model *is not* keeps new generation models
/// working without a code change, which the reverse (an allow-list) would not.
const NON_CHAT_LABELS: &[&str] = &[
    "embeddings",
    "embedding",
    "reranking",
    "transcription",
    "realtime-transcription",
    "tts",
    "classification",
    "image-generation",
    "audio-generation",
];

/// One model Lemonade reports. Every field beyond `id` is optional: the OpenAI
/// `/v1/models` shape guarantees only the id, and these extras are Lemonade's own.
#[derive(Debug, Deserialize)]
struct RawModel {
    id: String,
    #[serde(default)]
    labels: Vec<String>,
    /// Checkpoint size in GB.
    #[serde(default)]
    size: Option<f64>,
    #[serde(default)]
    downloaded: Option<bool>,
    #[serde(default)]
    max_context_window: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<RawModel>,
}

/// A generation model the user is allowed to pick, with the sizing the UI shows.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ChatModel {
    pub id: String,
    /// Weights on disk, in GB, exactly as Lemonade reports them.
    ///
    /// For a model with several checkpoint components this covers all of them, which
    /// can overstate what a chat actually loads — `Gemma-4-E2B-it-GGUF` reports 3.81 GB
    /// including an `mmproj` vision projector, but measures 3.17 GB resident for text.
    /// The estimate below is therefore conservative for multi-component models, which
    /// is the safe direction: it may withhold a model that would have fit, but it never
    /// offers one that will not.
    pub size_gb: f64,
    /// `size_gb` plus the measured runtime allowance — what to expect it to cost while
    /// loaded, which is the number that actually matters when choosing.
    pub estimated_ram_gb: f64,
    /// True when this model stays under [`LIGHT_MODEL_RAM_GB`] while running.
    pub light: bool,
    pub max_context_window: Option<u64>,
}

/// Narrows Lemonade's full model list to the generation models a user may select.
///
/// Pulled out as a pure function so the filtering rules — which decide what a user is
/// even offered — can be tested against Lemonade's real payload shapes without a
/// server. A model whose size Lemonade does not report is dropped rather than shown:
/// the whole point of the cap is that it is honoured, and an unknown size cannot be.
fn select_chat_models(raw: Vec<RawModel>) -> Vec<ChatModel> {
    let mut models: Vec<ChatModel> = raw
        .into_iter()
        .filter(|m| m.downloaded != Some(false))
        .filter(|m| {
            !m.labels
                .iter()
                .any(|l| NON_CHAT_LABELS.contains(&l.to_lowercase().as_str()))
        })
        .filter_map(|m| {
            let size_gb = m.size?;
            let estimated_ram_gb = size_gb + RUNTIME_OVERHEAD_GB;
            (estimated_ram_gb <= MAX_MODEL_RAM_GB).then(|| ChatModel {
                id: m.id,
                size_gb,
                estimated_ram_gb,
                light: estimated_ram_gb < LIGHT_MODEL_RAM_GB,
                max_context_window: m.max_context_window,
            })
        })
        .collect();

    // Cheapest first, so the models that leave the machine usable are the ones a user
    // sees without scrolling. Ties broken by id to keep the order stable across calls.
    models.sort_by(|a, b| {
        a.estimated_ram_gb
            .partial_cmp(&b.estimated_ram_gb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    models
}

#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    #[serde(default)]
    index: Option<usize>,
}

/// Lemonade sometimes reports an application-level failure — e.g. a chunk that
/// tokenizes past the backend's batch-size limit — with an HTTP 200 and an
/// `{"error": {...}}` body instead of a non-2xx status. A 200 is therefore not proof
/// the body matches `EmbeddingsResponse`; this shape has to be checked for either way.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    message: String,
}

/// Marks an embed() error as "this specific input was rejected for being too large" —
/// as opposed to a connectivity/model/server failure — so callers can retry with a
/// smaller input instead of giving up. A plain string prefix (rather than a proper
/// error enum) matches the rest of this codebase's convention of `Result<_, String>`.
pub const EMBED_TOO_LARGE_PREFIX: &str = "EMBED_TOO_LARGE:";

/// Pulled out of `embed()` as a pure function so the exact response shapes Lemonade is
/// known to send — success, error-in-a-200, and a genuine non-2xx — can be tested
/// directly without spinning up an HTTP server.
fn parse_embeddings_response(
    body: &str,
    status: reqwest::StatusCode,
    expected_len: usize,
) -> Result<Vec<Vec<f32>>, String> {
    match serde_json::from_str::<EmbeddingsResponse>(body) {
        Ok(parsed) => {
            let mut ordered: Vec<Vec<f32>> = vec![Vec::new(); expected_len];
            for (pos, item) in parsed.data.into_iter().enumerate() {
                let idx = item.index.unwrap_or(pos);
                if idx < ordered.len() {
                    ordered[idx] = item.embedding;
                }
            }
            Ok(ordered)
        }
        Err(parse_err) => match serde_json::from_str::<ErrorEnvelope>(body) {
            Ok(envelope) if envelope.error.message.to_lowercase().contains("too large") => {
                Err(format!("{EMBED_TOO_LARGE_PREFIX}{}", envelope.error.message))
            }
            Ok(envelope) => Err(format!(
                "embeddings request failed ({status}): {}",
                envelope.error.message
            )),
            Err(_) if status.is_success() => Err(format!(
                "failed to parse embeddings response ({status}): {parse_err}"
            )),
            Err(_) => Err(format!("embeddings request returned {status}: {body}")),
        },
    }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

impl LemonadeClient {
    pub fn new(base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build reqwest client");
        LemonadeClient { base_url, http }
    }

    pub async fn embed(&self, texts: &[String], model: &str) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url);

        // A transient send failure (observed in practice: an isolated dropped loopback
        // connection during a long run, with the server otherwise healthy throughout)
        // must not abort an entire folder's indexing job. Retried a couple of times
        // with a short backoff before surfacing it as a real failure; a genuinely
        // unreachable or crashed server still fails after these attempts.
        const MAX_SEND_ATTEMPTS: u32 = 3;
        let mut resp = None;
        let mut send_err = String::new();
        for attempt in 1..=MAX_SEND_ATTEMPTS {
            match self
                .http
                .post(&url)
                .header("Authorization", "Bearer lemonade")
                .json(&json!({ "model": model, "input": texts }))
                .send()
                .await
            {
                Ok(r) => {
                    resp = Some(r);
                    break;
                }
                Err(e) => {
                    send_err = format!("embeddings request failed: {e}");
                    if attempt < MAX_SEND_ATTEMPTS {
                        tokio::time::sleep(Duration::from_millis(300 * attempt as u64)).await;
                    }
                }
            }
        }
        let resp = resp.ok_or(send_err)?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read embeddings response: {e}"))?;

        parse_embeddings_response(&body, status, texts.len())
    }

    /// The generation models installed in Lemonade that this app will let a user pick,
    /// smallest first. Non-generation models and anything estimated to exceed
    /// [`MAX_MODEL_RAM_GB`] while loaded are filtered out.
    pub async fn list_chat_models(&self) -> Result<Vec<ChatModel>, String> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", "Bearer lemonade")
            .send()
            .await
            .map_err(|e| format!("could not reach Lemonade to list models: {e}"))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read the model list: {e}"))?;
        if !status.is_success() {
            return Err(format!("listing models returned {status}: {body}"));
        }

        let parsed: ModelsResponse = serde_json::from_str(&body)
            .map_err(|e| format!("failed to parse the model list: {e}"))?;
        Ok(select_chat_models(parsed.data))
    }

    pub async fn chat(&self, model: &str, system: &str, user: &str) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.base_url);
        let user_content = if understands_no_think(model) {
            format!("{user}\n/no_think")
        } else {
            user.to_string()
        };

        let resp = self
            .http
            .post(&url)
            .header("Authorization", "Bearer lemonade")
            .json(&json!({
                "model": model,
                "stream": false,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user_content },
                ],
            }))
            .send()
            .await
            .map_err(|e| format!("chat request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("chat request returned {status}: {body}"));
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| format!("failed to parse chat response: {e}"))?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();

        let answer = strip_thinking(&content);
        if answer.is_empty() {
            // Better an explicit error than a blank answer box sitting under five
            // citations, which reads as a broken app rather than a failed request.
            return Err("the model returned an empty response".to_string());
        }
        Ok(answer)
    }

    pub async fn is_reachable(&self) -> bool {
        let url = format!("{}/models", self.base_url);
        self.http
            .get(&url)
            .header("Authorization", "Bearer lemonade")
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

/// `/no_think` is a Qwen3 control token that suppresses its `<think>` block. Other
/// families do not implement it, so appending it there just tacks a stray line onto the
/// prompt — harmless with the models tested, but it is still an instruction aimed at a
/// model that cannot act on it, so it is only sent where it means something.
fn understands_no_think(model: &str) -> bool {
    model.to_lowercase().contains("qwen3")
}

/// Qwen3 wraps its reasoning in `<think>…</think>`. Returns the visible answer with
/// every such block removed, alongside the reasoning that was removed.
///
/// Taking everything after the *last* `</think>` — the obvious implementation — returns
/// an empty string whenever the model keeps its whole reply inside the block or the
/// response is truncated mid-reasoning, which surfaces as a blank answer.
fn split_thinking(content: &str) -> (String, String) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";

    let mut visible = String::new();
    let mut thought = String::new();
    let mut rest = content;

    loop {
        let Some(start) = rest.find(OPEN) else {
            visible.push_str(rest);
            break;
        };
        visible.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        match after.find(CLOSE) {
            Some(end) => {
                thought.push_str(&after[..end]);
                rest = &after[end + CLOSE.len()..];
            }
            None => {
                // Unclosed block: the reply was cut off while still reasoning.
                thought.push_str(after);
                break;
            }
        }
    }

    (visible.trim().to_string(), thought.trim().to_string())
}

/// The visible answer, falling back to the reasoning text when the model left nothing
/// outside the block. Showing its reasoning is imperfect; showing nothing is worse.
fn strip_thinking(content: &str) -> String {
    let (visible, thought) = split_thinking(content);
    if visible.is_empty() {
        thought
    } else {
        visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_a_normal_thinking_block() {
        assert_eq!(
            strip_thinking("<think>weighing options</think>\n\nPort 7913."),
            "Port 7913."
        );
    }

    #[test]
    fn empty_think_block_from_no_think_leaves_the_answer() {
        assert_eq!(strip_thinking("<think>\n\n</think>\n\nPort 7913."), "Port 7913.");
    }

    #[test]
    fn plain_content_is_untouched() {
        assert_eq!(strip_thinking("Port 7913."), "Port 7913.");
    }

    /// The regression that produced a blank answer box under five citations.
    #[test]
    fn answer_left_entirely_inside_the_block_is_not_lost() {
        assert_eq!(
            strip_thinking("<think>The context does not mention a budget.</think>"),
            "The context does not mention a budget."
        );
    }

    /// A reply cut off mid-reasoning still yields something rather than nothing.
    #[test]
    fn unclosed_block_falls_back_to_the_reasoning() {
        assert_eq!(
            strip_thinking("<think>The context does not mention it"),
            "The context does not mention it"
        );
    }

    #[test]
    fn text_around_multiple_blocks_is_preserved_in_order() {
        assert_eq!(
            strip_thinking("A<think>x</think>B<think>y</think>C"),
            "ABC"
        );
    }

    #[test]
    fn genuinely_empty_content_stays_empty_so_the_caller_can_error() {
        assert_eq!(strip_thinking(""), "");
        assert_eq!(strip_thinking("<think></think>"), "");
    }

    #[test]
    fn parses_a_normal_successful_response() {
        let body = r#"{"data":[{"embedding":[0.1,0.2],"index":0,"object":"embedding"}],"model":"m","object":"list","usage":{}}"#;
        let result = parse_embeddings_response(body, reqwest::StatusCode::OK, 1).unwrap();
        assert_eq!(result, vec![vec![0.1, 0.2]]);
    }

    /// The exact regression: Lemonade rejected a 2093-char / 888-token config file
    /// with an HTTP 200 and this body, which `resp.json::<EmbeddingsResponse>()`
    /// used to fail on with an opaque "error decoding response body" — no indication
    /// anywhere that the real problem was chunk size, not a malformed response.
    #[test]
    fn error_wrapped_in_http_200_is_detected_and_labelled_too_large() {
        let body = r#"{"error":{"code":500,"details":{"backend":"llama-server","response":{"error":{"code":500,"message":"input (888 tokens) is too large to process. increase the physical batch size (current batch size: 512)","type":"server_error"}}},"message":"input (888 tokens) is too large to process. increase the physical batch size (current batch size: 512)","status_code":500,"type":"server_error"}}"#;
        let err = parse_embeddings_response(body, reqwest::StatusCode::OK, 1).unwrap_err();
        assert!(
            err.starts_with(EMBED_TOO_LARGE_PREFIX),
            "expected the too-large marker, got: {err}"
        );
        assert!(err.contains("888 tokens"), "expected the real message preserved, got: {err}");
    }

    #[test]
    fn error_in_200_that_is_not_a_size_problem_is_not_mislabelled() {
        let body = r#"{"error":{"message":"model not found","code":404}}"#;
        let err = parse_embeddings_response(body, reqwest::StatusCode::OK, 1).unwrap_err();
        assert!(!err.starts_with(EMBED_TOO_LARGE_PREFIX));
        assert!(err.contains("model not found"));
    }

    #[test]
    fn genuine_non_2xx_without_json_body_still_reports_the_raw_body() {
        let err = parse_embeddings_response(
            "internal server error",
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            1,
        )
        .unwrap_err();
        assert!(err.contains("internal server error"));
        assert!(!err.starts_with(EMBED_TOO_LARGE_PREFIX));
    }

    fn select_from(json: &str) -> Vec<ChatModel> {
        let parsed: ModelsResponse =
            serde_json::from_str(json).expect("fixture should parse as a model list");
        select_chat_models(parsed.data)
    }

    /// Trimmed from a real `GET /v1/models` against Lemonade — the field names and the
    /// mix of model kinds are exactly what the server sends, so the picker is tested
    /// against the payload it will actually meet rather than an idealised one.
    const REAL_PAYLOAD: &str = r#"{"data":[
        {"id":"Bonsai-8B-gguf","labels":["llamacpp","tool-calling"],"size":1.08,"downloaded":true,"max_context_window":65536,"object":"model"},
        {"id":"Gemma-4-E2B-it-GGUF","labels":["tool-calling","vision","llamacpp"],"size":3.81,"downloaded":true,"max_context_window":131072,"object":"model"},
        {"id":"Moonshine-Medium-Streaming","labels":["transcription","realtime-transcription","hot"],"size":1.08,"downloaded":true,"object":"model"},
        {"id":"Qwen3-0.6B-GGUF","labels":["reasoning","tool-calling"],"size":0.356,"downloaded":true,"max_context_window":40960,"object":"model"},
        {"id":"Qwen3-1.7B-GGUF","labels":["reasoning","tool-calling"],"size":0.984,"downloaded":true,"max_context_window":40960,"object":"model"},
        {"id":"nomic-embed-text-v1-GGUF","labels":["embeddings"],"size":0.073,"downloaded":true,"max_context_window":2048,"object":"model"}
    ],"object":"list"}"#;

    #[test]
    fn embedding_and_transcription_models_are_not_offered_as_chat_models() {
        let ids: Vec<String> = select_from(REAL_PAYLOAD).into_iter().map(|m| m.id).collect();
        assert!(
            !ids.iter().any(|id| id.contains("nomic-embed")),
            "the embedding model must not be selectable for chat: {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.contains("Moonshine")),
            "the transcription model must not be selectable for chat: {ids:?}"
        );
        assert_eq!(
            ids,
            vec![
                "Qwen3-0.6B-GGUF",
                "Qwen3-1.7B-GGUF",
                "Bonsai-8B-gguf",
                "Gemma-4-E2B-it-GGUF",
            ],
            "expected only generation models, cheapest first"
        );
    }

    /// The default ships in the light tier deliberately: a loaded Bonsai-8B measures
    /// 1.66 GB of GPU memory, which this estimate (1.68 GB) tracks to within 20 MB.
    #[test]
    fn the_light_tier_tracks_measured_memory_for_the_default_model() {
        let models = select_from(REAL_PAYLOAD);
        let bonsai = models
            .iter()
            .find(|m| m.id == "Bonsai-8B-gguf")
            .expect("Bonsai should be offered");
        assert!(
            bonsai.light,
            "Bonsai-8B is measured at 1.66 GB of GPU memory and must count as light, got {:?}",
            bonsai
        );
        assert!(bonsai.estimated_ram_gb < LIGHT_MODEL_RAM_GB);

        let gemma = models
            .iter()
            .find(|m| m.id == "Gemma-4-E2B-it-GGUF")
            .expect("Gemma should be offered");
        assert!(
            !gemma.light,
            "a 3.81 GB checkpoint cannot be in the under-2 GB tier: {gemma:?}"
        );
    }

    #[test]
    fn models_that_would_exceed_the_cap_are_not_offered() {
        let json = r#"{"data":[
            {"id":"Huge-70B","labels":["llamacpp"],"size":40.0,"downloaded":true},
            {"id":"JustOver","labels":["llamacpp"],"size":5.5,"downloaded":true},
            {"id":"JustUnder","labels":["llamacpp"],"size":5.4,"downloaded":true}
        ]}"#;
        let ids: Vec<String> = select_from(json).into_iter().map(|m| m.id).collect();
        // 5.4 + 0.6 = 6.0 is exactly at the inclusive cap; 5.5 + 0.6 = 6.1 is over it.
        assert_eq!(ids, vec!["JustUnder"], "the cap must be applied to the runtime estimate");
    }

    /// An unreported size cannot be checked against the cap, and showing it anyway
    /// would turn the cap into a promise the app cannot keep.
    #[test]
    fn a_model_with_no_reported_size_is_not_offered() {
        let json = r#"{"data":[{"id":"Mystery","labels":["llamacpp"],"downloaded":true}]}"#;
        assert!(select_from(json).is_empty());
    }

    #[test]
    fn a_model_that_is_not_downloaded_is_not_offered() {
        let json = r#"{"data":[{"id":"NotHere","labels":["llamacpp"],"size":1.0,"downloaded":false}]}"#;
        assert!(select_from(json).is_empty());
    }

    /// Lemonade labels vary in case across registry sources; the filter must not be
    /// fooled into offering an embedding model as a chat model by capitalisation.
    #[test]
    fn label_matching_is_case_insensitive() {
        let json = r#"{"data":[{"id":"Embed","labels":["Embeddings"],"size":0.1,"downloaded":true}]}"#;
        assert!(select_from(json).is_empty());
    }

    #[test]
    fn no_think_is_sent_only_to_the_family_that_implements_it() {
        assert!(understands_no_think("Qwen3-0.6B-GGUF"));
        assert!(understands_no_think("qwen3-1.7b-gguf"));
        assert!(!understands_no_think("Bonsai-8B-gguf"));
        assert!(!understands_no_think("Gemma-4-E2B-it-GGUF"));
    }
}
