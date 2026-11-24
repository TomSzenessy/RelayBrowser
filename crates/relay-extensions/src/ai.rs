//! Minimal OpenAI-compatible chat client. The key never enters settings or
//! logs; the browser sends only the URL/title and the user's question.

pub const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";
const MAX_REPLY_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiConfig {
    pub endpoint: String,
    pub model: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self { endpoint: DEFAULT_ENDPOINT.to_string(), model: DEFAULT_MODEL.to_string() }
    }
}

/// Trim a base URL and refuse anything but http(s). A trailing `/` is removed
/// so `/chat/completions` is appended exactly once.
pub fn normalize_endpoint(raw: &str) -> Option<String> {
    let text = raw.trim().trim_end_matches('/');
    (text.starts_with("https://") || text.starts_with("http://")).then(|| text.to_string())
}

#[derive(serde::Serialize)]
struct Message {
    role: &'static str,
    content: String,
}

#[derive(serde::Serialize)]
struct Request {
    model: String,
    messages: Vec<Message>,
    stream: bool,
}

const MAX_HISTORY_TURNS: usize = 10;
const MAX_HISTORY_BYTES: usize = 24_000;

/// Build the chat-completions body. Prior turns are included (last 10, capped)
/// so follow-up questions keep context; the page text is attached only to the
/// current question.
pub fn request_body(model: &str, url: &str, title: &str, page: &str, prompt: &str, history: &[(bool, String)]) -> String {
    let system = "You are Relay's concise browser assistant. Answer in plain text. The user's page URL, title, and visible page text are provided.";
    let mut messages = vec![Message { role: "system", content: system.to_string() }];
    let mut used = 0usize;
    let start = history.len().saturating_sub(MAX_HISTORY_TURNS);
    for (user, text) in &history[start..] {
        if used >= MAX_HISTORY_BYTES {
            break;
        }
        let text = text.chars().take(8_000).collect::<String>();
        used += text.len();
        if text.trim().is_empty() {
            continue;
        }
        messages.push(Message { role: if *user { "user" } else { "assistant" }, content: text });
    }
    let user = format!("Page: {title}\nURL: {url}\n\nVisible page text:\n{page}\n\nQuestion: {prompt}");
    messages.push(Message { role: "user", content: user });
    serde_json::to_string(&Request { model: model.to_string(), messages, stream: false }).unwrap_or_else(|_| "{}".into())
}

pub fn parse_reply(json: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("AI reply was not JSON: {e}"))?;
    let content = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| "AI reply had no message content".into())
        })?;
    let text = content.trim();
    if text.is_empty() {
        return Err("AI returned an empty answer".into());
    }
    Ok(text.chars().take(MAX_REPLY_BYTES).collect())
}

/// One blocking completion. `endpoint` is a base URL like
/// `https://api.openai.com/v1`; the key is sent only in the Authorization
/// header. Runs off the main thread by the caller.
pub fn complete(
    config: &AiConfig,
    key: &str,
    url: &str,
    title: &str,
    page: &str,
    prompt: &str,
    history: &[(bool, String)],
) -> Result<String, String> {
    let endpoint = normalize_endpoint(&config.endpoint).ok_or("AI endpoint must be an http(s) URL")?;
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(60))).build(),
    );
    let body = request_body(&config.model, url, title, page, prompt, history);
    let mut res = agent
        .post(&format!("{endpoint}/chat/completions"))
        .header("Authorization", &format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .send(body)
        .map_err(|e| format!("AI request failed: {e}"))?;
    let status = res.status();
    let text = res.body_mut().read_to_string().map_err(|e| format!("AI response read failed: {e}"))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(300).collect();
        return Err(format!("AI endpoint returned {status}: {snippet}"));
    }
    parse_reply(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_and_body_are_bounded() {
        assert_eq!(normalize_endpoint(" https://api.test/v1/ ").as_deref(), Some("https://api.test/v1"));
        assert!(normalize_endpoint("file:///etc/passwd").is_none());
        let body = request_body("m", "https://a.test/", "A \"page\"", "visible body", "why?", &[]);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "m");
        assert_eq!(v["messages"][0]["role"], "system");
        let user = v["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("A \"page\"") && user.contains("visible body"));
        assert_eq!(v["stream"], false);
    }

    #[test]
    fn prior_turns_are_bounded_and_keep_roles() {
        let history: Vec<(bool, String)> = (0..20)
            .map(|i| (i % 2 == 0, format!("turn {i} {}", "x".repeat(1000))))
            .collect();
        let body = request_body("m", "https://a.test/", "T", "page", "now", &history);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let messages = v["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages.iter().filter(|m| m["role"] == "assistant").count(), 5, "last 10 turns only");
        assert!(messages.len() <= 12);
        assert_eq!(messages.last().unwrap()["role"], "user");
        assert!(messages.last().unwrap()["content"].as_str().unwrap().contains("Question: now"));
    }

    #[test]
    fn replies_and_errors_are_explained() {
        assert_eq!(parse_reply(r#"{"choices":[{"message":{"content":"hello"}}]}"#).unwrap(), "hello");
        assert!(parse_reply(r#"{"error":{"message":"bad key"}}"#).unwrap_err().contains("bad key"));
        assert!(parse_reply("not json").unwrap_err().contains("JSON"));
        assert!(parse_reply(r#"{"choices":[{"message":{"content":"  "}}]}"#).is_err());
    }
}
