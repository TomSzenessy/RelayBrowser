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
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(serde::Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    stream: bool,
}

pub fn request_body(model: &str, url: &str, title: &str, prompt: &str) -> String {
    let system = "You are Relay's concise browser assistant. Answer in plain text. The user's page URL and title are provided, but not the page body; do not claim to have read it.";
    let user = format!("Page: {title}\nURL: {url}\n\nQuestion: {prompt}");
    serde_json::to_string(&Request {
        model,
        messages: vec![Message { role: "system", content: system }, Message { role: "user", content: &user }],
        stream: false,
    })
    .unwrap_or_else(|_| "{}".into())
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
pub fn complete(config: &AiConfig, key: &str, url: &str, title: &str, prompt: &str) -> Result<String, String> {
    let endpoint = normalize_endpoint(&config.endpoint).ok_or("AI endpoint must be an http(s) URL")?;
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(60))).build(),
    );
    let body = request_body(&config.model, url, title, prompt);
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
        let body = request_body("m", "https://a.test/", "A \"page\"", "why?");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "m");
        assert_eq!(v["messages"][0]["role"], "system");
        assert!(v["messages"][1]["content"].as_str().unwrap().contains("A \"page\""));
        assert_eq!(v["stream"], false);
    }

    #[test]
    fn replies_and_errors_are_explained() {
        assert_eq!(parse_reply(r#"{"choices":[{"message":{"content":"hello"}}]}"#).unwrap(), "hello");
        assert!(parse_reply(r#"{"error":{"message":"bad key"}}"#).unwrap_err().contains("bad key"));
        assert!(parse_reply("not json").unwrap_err().contains("JSON"));
        assert!(parse_reply(r#"{"choices":[{"message":{"content":"  "}}]}"#).is_err());
    }
}
