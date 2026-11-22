//! Offline `!bang` shortcuts: `!gh rust lang` or `rust lang !gh` jumps straight
//! to a site's own search. No network lookup; the table is built in, and
//! custom bangs (settings) override it.

/// (trigger, search template with `%s`, homepage for an empty query).
const BUILTIN: &[(&str, &str, &str)] = &[
    ("gh", "https://github.com/search?q=%s", "https://github.com"),
    ("rs", "https://doc.rust-lang.org/std/index.html?search=%s", "https://doc.rust-lang.org/std/"),
    ("crates", "https://crates.io/search?q=%s", "https://crates.io"),
    ("docs", "https://docs.rs/releases/search?query=%s", "https://docs.rs"),
    ("mdn", "https://developer.mozilla.org/en-US/search?q=%s", "https://developer.mozilla.org"),
    ("yt", "https://www.youtube.com/results?search_query=%s", "https://www.youtube.com"),
    ("so", "https://stackoverflow.com/search?q=%s", "https://stackoverflow.com"),
    ("npm", "https://www.npmjs.com/search?q=%s", "https://www.npmjs.com"),
    ("w", "https://en.wikipedia.org/w/index.php?search=%s", "https://en.wikipedia.org"),
    ("k", "https://kagi.com/search?q=%s", "https://kagi.com"),
    ("g", "https://www.google.com/search?q=%s", "https://www.google.com"),
    ("ddg", "https://duckduckgo.com/?q=%s", "https://duckduckgo.com"),
    ("maps", "https://www.google.com/maps/search/%s", "https://www.google.com/maps"),
];

const MAX_TRIGGER: usize = 16;

fn encode(q: &str) -> String {
    let mut out = String::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn valid_trigger(t: &str) -> bool {
    !t.is_empty() && t.len() <= MAX_TRIGGER && t.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `scheme://host` of a template, used as its empty-query destination.
fn origin(template: &str) -> String {
    match template.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            format!("{scheme}://{host}")
        }
        None => template.to_string(),
    }
}

/// Parse the `bangs` setting: whitespace-separated `trigger=template` pairs,
/// e.g. `gh=https://example.com/s?q=%s w=https://wiki.test/?q=%s`. Triggers
/// are letters, digits, `-` or `_` (a leading `!` is allowed); templates must
/// be http(s) URLs. Returns the pairs, or the first invalid entry.
pub fn parse_custom(setting: &str) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = vec![];
    for entry in setting.split_whitespace() {
        let (trigger, template) = entry.split_once('=').ok_or_else(|| entry.to_string())?;
        let trigger = trigger.trim_start_matches('!').to_ascii_lowercase();
        let trigger_ok = !trigger.is_empty() && trigger.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let lower = template.to_ascii_lowercase();
        let template_ok = (lower.starts_with("https://") || lower.starts_with("http://")) && template.len() > "https://".len();
        if !trigger_ok || !template_ok {
            return Err(entry.to_string());
        }
        out.retain(|(t, _)| *t != trigger); // the last definition wins
        out.push((trigger, template.to_string()));
    }
    Ok(out)
}

fn lookup(trigger: &str, query: &str, custom: &[(String, String)]) -> Option<String> {
    let trig = trigger.to_ascii_lowercase();
    let custom_hit = custom.iter().find(|(t, _)| t.trim().trim_start_matches('!').eq_ignore_ascii_case(&trig));
    if let Some((_, template)) = custom_hit {
        if query.is_empty() {
            return Some(origin(template));
        }
        return Some(template.replace("%s", &encode(query)));
    }
    let (_, template, home) = BUILTIN.iter().find(|(t, _, _)| *t == trig)?;
    Some(if query.is_empty() { (*home).to_string() } else { template.replace("%s", &encode(query)) })
}

/// Target URL for `!bang query` / `query !bang`, or `None` when the input is
/// not a known bang. Plain URLs and text like `hello!` never match.
pub fn resolve(input: &str, custom: &[(String, String)]) -> Option<String> {
    let words: Vec<&str> = input.split_whitespace().collect();
    let first = *words.first()?;
    if let Some(trigger) = first.strip_prefix('!')
        && valid_trigger(trigger)
        && let Some(url) = lookup(trigger, &words[1..].join(" "), custom)
    {
        return Some(url);
    }
    let last = *words.last()?;
    if words.len() > 1
        && let Some(trigger) = last.strip_prefix('!')
        && valid_trigger(trigger)
    {
        return lookup(trigger, &words[..words.len() - 1].join(" "), custom);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Option<String> {
        resolve(s, &[])
    }

    #[test]
    fn every_builtin_resolves_with_query() {
        let cases = [
            ("!gh rust lang", "https://github.com/search?q=rust+lang"),
            ("!rs serde", "https://doc.rust-lang.org/std/index.html?search=serde"),
            ("!mdn fetch", "https://developer.mozilla.org/en-US/search?q=fetch"),
            ("!yt cats", "https://www.youtube.com/results?search_query=cats"),
            ("!so x", "https://stackoverflow.com/search?q=x"),
            ("!npm x", "https://www.npmjs.com/search?q=x"),
            ("!w x", "https://en.wikipedia.org/w/index.php?search=x"),
            ("!k x", "https://kagi.com/search?q=x"),
            ("!g x", "https://www.google.com/search?q=x"),
            ("!ddg x", "https://duckduckgo.com/?q=x"),
            ("!maps x", "https://www.google.com/maps/search/x"),
            ("!crates x", "https://crates.io/search?q=x"),
            ("!docs x", "https://docs.rs/releases/search?query=x"),
        ];
        for (input, want) in cases {
            assert_eq!(r(input).as_deref(), Some(want), "{input}");
        }
    }

    #[test]
    fn suffix_form_and_case() {
        assert_eq!(r("rust lang !gh").as_deref(), Some("https://github.com/search?q=rust+lang"));
        assert_eq!(r("!GH Rust").as_deref(), Some("https://github.com/search?q=Rust"));
        assert_eq!(r("  !gh   a    b  ").as_deref(), Some("https://github.com/search?q=a+b"));
    }

    #[test]
    fn query_is_percent_encoded() {
        assert_eq!(r("!g a&b=c/d?é").as_deref(), Some("https://www.google.com/search?q=a%26b%3Dc%2Fd%3F%C3%A9"));
        assert_eq!(r("!g 100%").as_deref(), Some("https://www.google.com/search?q=100%25"));
        assert_eq!(r("!g a#b").as_deref(), Some("https://www.google.com/search?q=a%23b"));
    }

    #[test]
    fn empty_query_goes_home() {
        assert_eq!(r("!gh").as_deref(), Some("https://github.com"));
        assert_eq!(r("!docs  ").as_deref(), Some("https://docs.rs"));
        assert_eq!(r("!yt").as_deref(), Some("https://www.youtube.com"));
    }

    #[test]
    fn unknown_and_non_bangs_are_none() {
        for s in ["!nope x", "!", "!!", "!!gh x", "hello!", "hello !", "https://x.com/a!b", "x !nope", "gh rust", "", "   ", "!gh!x y", "a!gh b", "! gh x", "!toolongtriggername1234 x"] {
            assert_eq!(r(s), None, "{s:?}");
        }
    }

    #[test]
    fn bang_in_middle_is_not_a_bang() {
        assert_eq!(r("rust !gh lang"), None);
    }

    #[test]
    fn prefix_wins_over_suffix() {
        assert_eq!(r("!gh a !yt").as_deref(), Some("https://github.com/search?q=a+%21yt"));
    }

    #[test]
    fn unknown_prefix_can_still_use_known_suffix() {
        assert_eq!(r("!zzz a !gh").as_deref(), Some("https://github.com/search?q=%21zzz+a"));
    }

    #[test]
    fn custom_bangs_override_and_extend() {
        let custom = vec![("gh".to_string(), "https://example.com/s/%s".to_string()), ("!wiki".to_string(), "https://wiki.test/find?term=%s&x=1".to_string())];
        assert_eq!(resolve("!gh a b", &custom).as_deref(), Some("https://example.com/s/a+b"));
        assert_eq!(resolve("a b !WIKI", &custom).as_deref(), Some("https://wiki.test/find?term=a+b&x=1"));
        assert_eq!(resolve("!wiki", &custom).as_deref(), Some("https://wiki.test"));
        assert_eq!(resolve("!gh", &custom).as_deref(), Some("https://example.com"));
        assert_eq!(resolve("!yt cats", &custom).as_deref(), Some("https://www.youtube.com/results?search_query=cats"));
    }

    #[test]
    fn custom_setting_parses_pairs_and_rejects_bad_entries() {
        let parsed = parse_custom("  !GH=https://example.com/s?q=%s\nwiki=https://wiki.test/?q=%s gh=https://b.test/%s ").unwrap();
        assert_eq!(parsed, vec![("wiki".into(), "https://wiki.test/?q=%s".into()), ("gh".into(), "https://b.test/%s".into())]);
        assert_eq!(parse_custom("").unwrap(), vec![]);
        assert_eq!(parse_custom("x=javascript:alert(1)"), Err("x=javascript:alert(1)".into()));
        assert_eq!(parse_custom("noequals"), Err("noequals".into()));
        assert_eq!(parse_custom("b@d=https://a.test/%s"), Err("b@d=https://a.test/%s".into()));
        assert_eq!(parse_custom("=https://a.test/%s"), Err("=https://a.test/%s".into()));
        assert_eq!(parse_custom("x=https://"), Err("x=https://".into()));
    }

    #[test]
    fn custom_template_without_placeholder_is_used_verbatim() {
        let custom = vec![("fixed".to_string(), "https://a.test/page".to_string())];
        assert_eq!(resolve("!fixed q", &custom).as_deref(), Some("https://a.test/page"));
    }

    #[test]
    fn never_panics_on_odd_input() {
        for s in ["!\u{0}", "!é q", "\u{1F600} !gh", "!gh \u{1F600}", "!\n\t", "\u{200B}!gh"] {
            let _ = r(s);
        }
    }
}
