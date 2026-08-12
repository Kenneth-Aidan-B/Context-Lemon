use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub struct LemonadeClient {
    base_url: String,
    http: reqwest::Client,
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
        let resp = self
            .http
            .post(&url)
            .header("Authorization", "Bearer lemonade")
            .json(&json!({ "model": model, "input": texts }))
            .send()
            .await
            .map_err(|e| format!("embeddings request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("embeddings request returned {status}: {body}"));
        }

        let parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| format!("failed to parse embeddings response: {e}"))?;

        let mut ordered: Vec<Vec<f32>> = vec![Vec::new(); texts.len()];
        for (pos, item) in parsed.data.into_iter().enumerate() {
            let idx = item.index.unwrap_or(pos);
            if idx < ordered.len() {
                ordered[idx] = item.embedding;
            }
        }
        Ok(ordered)
    }

    pub async fn chat(&self, model: &str, system: &str, user: &str, no_think: bool) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.base_url);
        let user_content = if no_think {
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
}
