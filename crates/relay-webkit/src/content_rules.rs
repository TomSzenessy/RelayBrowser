//! EasyList / uBlock filter syntax → WebKit content-blocker JSON, which
//! `WKContentRuleListStore` compiles to bytecode so blocked scripts never load.
//!
//! Supported subset (covers the bulk of EasyList/EasyPrivacy):
//! `||domain^`, `|https://…`, plain substrings, `*` wildcards, `@@` exceptions,
//! `$third-party`, `$~third-party`, `$script,image,stylesheet,font,media,
//! xmlhttprequest,subdocument,popup,ping,websocket`, `$domain=a|~b`,
//! `$match-case`, and element hiding `##sel` / `domain##sel`.
//! Anything else (regex filters, `#?#`, `#@#`, `$redirect`, `$csp`, scriptlets)
//! is skipped and counted.

use serde_json::{json, Value};

/// WebKit refuses lists above 150k rules per list.
pub const WEBKIT_MAX_RULES: usize = 150_000;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Compiled {
    pub rules: Vec<Value>,
    pub skipped: usize,
}

impl Compiled {
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.rules).expect("rules serialize")
    }

    /// Turn blocking off on these sites (and their subdomains): one final
    /// `ignore-previous-rules` rule keyed on the page's domain, so requests
    /// and element hiding on those pages are left alone. Must run last.
    pub fn allow_sites<S: AsRef<str>>(&mut self, hosts: &[S]) {
        let domains: Vec<String> = hosts
            .iter()
            .map(|h| h.as_ref().trim().trim_start_matches("www.").to_ascii_lowercase())
            .filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
            .map(|h| format!("*{h}"))
            .collect();
        if !domains.is_empty() {
            self.rules.push(json!({ "trigger": { "url-filter": ".*", "if-domain": domains }, "action": { "type": "ignore-previous-rules" } }));
        }
    }
}

/// A tiny built-in list so blocking works before any list is downloaded.
pub const BUILTIN_FILTERS: &str = "\
||doubleclick.net^
||googlesyndication.com^
||googleadservices.com^
||google-analytics.com^$third-party
||googletagmanager.com^$third-party
||adservice.google.com^
||amazon-adsystem.com^
||adnxs.com^
||criteo.com^$third-party
||taboola.com^
||outbrain.com^
||scorecardresearch.com^
||quantserve.com^
||hotjar.com^$third-party
||facebook.net^$third-party,script
##.ad-banner
##.adsbygoogle
";

/// Conservative cookie-consent (CMP) banner hiding: only the outer containers
/// of widely deployed consent platforms, by their vendor-specific ids and
/// classes, so no site content shares the selector. Hiding a banner gives no
/// consent; compliant CMPs then keep their non-essential cookies off. Modal
/// CMPs that lock page scrolling while open (Sourcepoint, Didomi popups,
/// Quantcast) are left out: hiding them would leave the page unscrollable.
/// Applied with the built-in blocklist (off when blocking is off).
pub const CMP_FILTERS: &str = "\
###onetrust-consent-sdk
###CybotCookiebotDialog
###usercentrics-root
###truste-consent-track
###iubenda-cs-banner
###cmplz-cookiebanner-container
##.osano-cm-window
##.cky-consent-container
##.klaro
";

pub fn compile(filters: &str) -> Compiled {
    let mut out = Compiled::default();
    let mut exceptions = vec![];
    for raw in filters.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
            continue;
        }
        match compile_line(line) {
            Some((rule, true)) => exceptions.push(rule),
            Some((rule, false)) => out.rules.push(rule),
            None => out.skipped += 1,
        }
    }
    // ignore-previous-rules only affects rules listed before it.
    out.rules.extend(exceptions);
    if out.rules.len() > WEBKIT_MAX_RULES {
        out.skipped += out.rules.len() - WEBKIT_MAX_RULES;
        out.rules.truncate(WEBKIT_MAX_RULES);
    }
    out
}

/// Returns the rule and whether it is an exception.
fn compile_line(line: &str) -> Option<(Value, bool)> {
    if line.contains("#?#") || line.contains("#@#") || line.contains("#$#") || line.contains("##+js") {
        return None;
    }
    if let Some(idx) = line.find("##") {
        let (domains, selector) = (&line[..idx], &line[idx + 2..]);
        if selector.is_empty() {
            return None;
        }
        let mut trigger = json!({ "url-filter": ".*" });
        apply_domains(&mut trigger, domains.split(',').filter(|d| !d.is_empty()))?;
        return Some((json!({ "trigger": trigger, "action": { "type": "css-display-none", "selector": selector } }), false));
    }

    let (exception, body) = match line.strip_prefix("@@") {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let (pattern, options) = match body.rfind('$') {
        Some(i) if !body[i + 1..].contains('/') => (&body[..i], Some(&body[i + 1..])),
        _ => (body, None),
    };
    if pattern.starts_with('/') && pattern.ends_with('/') && pattern.len() > 1 {
        return None; // raw regex filters use syntax WebKit does not support
    }
    let mut trigger = json!({ "url-filter": url_filter(pattern)? });
    if let Some(opts) = options {
        let mut types = vec![];
        for opt in opts.split(',') {
            match opt {
                "third-party" => trigger["load-type"] = json!(["third-party"]),
                "~third-party" | "first-party" => trigger["load-type"] = json!(["first-party"]),
                "match-case" => trigger["url-filter-is-case-sensitive"] = json!(true),
                "important" => {}
                o if o.starts_with("domain=") => apply_domains(&mut trigger, o[7..].split('|'))?,
                o => types.push(resource_type(o)?),
            }
        }
        if !types.is_empty() {
            types.dedup();
            trigger["resource-type"] = json!(types);
        }
    }
    let action = if exception { "ignore-previous-rules" } else { "block" };
    Some((json!({ "trigger": trigger, "action": { "type": action } }), exception))
}

fn resource_type(opt: &str) -> Option<&'static str> {
    Some(match opt {
        "script" => "script",
        "image" => "image",
        "stylesheet" => "style-sheet",
        "font" => "font",
        "media" => "media",
        "xmlhttprequest" | "websocket" | "ping" | "other" => "raw",
        "subdocument" => "document",
        "popup" => "popup",
        "document" => "document",
        _ => return None,
    })
}

fn apply_domains<'a>(trigger: &mut Value, domains: impl Iterator<Item = &'a str>) -> Option<()> {
    let (mut inc, mut exc) = (vec![], vec![]);
    for d in domains {
        let d = d.trim().to_ascii_lowercase();
        if d.is_empty() || d.contains('*') {
            return None;
        }
        match d.strip_prefix('~') {
            Some(x) => exc.push(format!("*{x}")),
            None => inc.push(format!("*{d}")),
        }
    }
    // WebKit allows only one of if-domain / unless-domain per trigger.
    match (inc.is_empty(), exc.is_empty()) {
        (false, true) => trigger["if-domain"] = json!(inc),
        (true, false) => trigger["unless-domain"] = json!(exc),
        (true, true) => {}
        (false, false) => trigger["if-domain"] = json!(inc),
    }
    Some(())
}

/// Translate an ABP pattern to WebKit's regex subset (no `|`, `{}`, `\d`).
fn url_filter(pattern: &str) -> Option<String> {
    let mut p = pattern;
    let mut out = String::new();
    if let Some(rest) = p.strip_prefix("||") {
        out.push_str("^[^:]+://+([^:/]+\\.)?");
        p = rest;
    } else if let Some(rest) = p.strip_prefix('|') {
        out.push('^');
        p = rest;
    }
    let anchored_end = p.ends_with('|');
    if anchored_end {
        p = &p[..p.len() - 1];
    }
    if p.is_empty() && out.is_empty() {
        return None;
    }
    for c in p.chars() {
        match c {
            '*' => out.push_str(".*"),
            '^' => out.push_str("[^a-zA-Z0-9_.%-]"),
            '.' | '+' | '?' | '(' | ')' | '[' | ']' | '\\' | '$' => {
                out.push('\\');
                out.push(c);
            }
            '{' | '}' | '|' => return None,
            c if c.is_ascii() => out.push(c),
            _ => return None, // WebKit url-filter is ASCII only
        }
    }
    // A trailing `^` after a host means "end of host". WebKit normalizes URLs
    // so a path always follows the host, and has no `|` alternation, so the
    // separator class `[:/?]` is exact there.
    if let Some(stripped) = out.strip_suffix("[^a-zA-Z0-9_.%-]") {
        out = format!("{stripped}[:/?]");
    }
    if anchored_end {
        out.push('$');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regex_of(filter: &str) -> String {
        compile(filter).rules[0]["trigger"]["url-filter"].as_str().unwrap().to_string()
    }

    #[test]
    fn domain_anchor() {
        assert_eq!(regex_of("||ads.example.com^"), "^[^:]+://+([^:/]+\\.)?ads\\.example\\.com[:/?]");
    }

    #[test]
    fn options_map_to_webkit_triggers() {
        let c = compile("||tracker.test^$third-party,script,domain=news.test|~blog.news.test");
        let t = &c.rules[0]["trigger"];
        assert_eq!(t["load-type"], json!(["third-party"]));
        assert_eq!(t["resource-type"], json!(["script"]));
        assert_eq!(t["if-domain"], json!(["*news.test"]));
    }

    #[test]
    fn exceptions_come_last_and_cosmetics_hide() {
        let c = compile("@@||good.test^\n||bad.test^\nnews.test##.promo\n! comment\n[Adblock Plus 2.0]");
        assert_eq!(c.rules.len(), 3);
        assert_eq!(c.rules[0]["action"]["type"], "block");
        assert_eq!(c.rules[1]["action"], json!({"type":"css-display-none","selector":".promo"}));
        assert_eq!(c.rules[1]["trigger"]["if-domain"], json!(["*news.test"]));
        assert_eq!(c.rules[2]["action"]["type"], "ignore-previous-rules");
    }

    #[test]
    fn unsupported_syntax_is_skipped_not_mangled() {
        let c = compile("/ads[0-9]+/\nexample.com#?#div:has(.ad)\n||x.test^$redirect=noop.js\n||ok.test^");
        assert_eq!((c.rules.len(), c.skipped), (1, 3));
    }

    #[test]
    fn builtin_list_compiles_fully() {
        let mut allowed = compile(BUILTIN_FILTERS);
        let before = allowed.rules.len();
        allowed.allow_sites(&["www.News.example", "bad host", "", "shop.test"]);
        assert_eq!(allowed.rules.len(), before + 1);
        assert_eq!(
            allowed.rules.last().unwrap(),
            &json!({"trigger":{"url-filter":".*","if-domain":["*news.example","*shop.test"]},"action":{"type":"ignore-previous-rules"}})
        );
        let mut none = compile(BUILTIN_FILTERS);
        none.allow_sites::<&str>(&[]);
        assert_eq!(none.rules.len(), before, "no sites, no extra rule");

        let cmp = compile(CMP_FILTERS);
        assert_eq!(cmp.skipped, 0);
        assert_eq!(cmp.rules.len(), 9);
        assert!(cmp.rules.iter().all(|r| r["action"]["type"] == "css-display-none" && r["trigger"]["url-filter"] == ".*"));
        let selectors: Vec<&str> = cmp.rules.iter().map(|r| r["action"]["selector"].as_str().unwrap()).collect();
        assert!(selectors.contains(&"#onetrust-consent-sdk") && selectors.contains(&"#CybotCookiebotDialog"), "{selectors:?}");
        assert!(selectors.iter().all(|s| s.starts_with('#') || s.starts_with('.')), "{selectors:?}");
        let c = compile(BUILTIN_FILTERS);
        assert_eq!(c.skipped, 0);
        assert!(c.rules.len() >= 15);
        serde_json::from_str::<Value>(&c.to_json()).unwrap();
    }

    #[test]
    fn regexes_avoid_constructs_webkit_rejects() {
        for r in compile(BUILTIN_FILTERS).rules {
            let f = r["trigger"]["url-filter"].as_str().unwrap();
            assert!(!f.contains('|') && !f.contains('{'), "{f}");
        }
    }
}
