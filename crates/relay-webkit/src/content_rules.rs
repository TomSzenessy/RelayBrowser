//! EasyList / uBlock filter syntax → WebKit content-blocker JSON, which
//! `WKContentRuleListStore` compiles to bytecode so blocked scripts never load.
//!
//! Supported subset (covers the bulk of EasyList/EasyPrivacy):
//! `||domain^`, `|https://…`, plain substrings, `*` wildcards, `@@` exceptions,
//! `$third-party`, `$~third-party`, `$script,image,stylesheet,font,media,
//! xmlhttprequest,subdocument,popup,ping,websocket`, `$domain=a|~b` (a mixed
//! scope compiles to a block plus an ordered `ignore-previous-rules`
//! exception, because WebKit allows one domain condition per trigger; a mixed
//! element-hiding scope is skipped), `$match-case`, and element hiding `##sel`
//! / `domain##sel`. Anything else (regex filters, `#?#`, `#@#`, `$redirect`,
//! `$csp`, scriptlets) is skipped and counted.

use serde_json::{json, Value};

/// WebKit refuses lists above 150k rules per list.
pub const WEBKIT_MAX_RULES: usize = 150_000;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Compiled {
    pub rules: Vec<Value>,
    pub skipped: usize,
}

/// A host `allow_sites` can express as a WebKit `if-domain` (DNS names; not
/// IPv6 literals or hosts with odd characters).
pub fn is_site_host(host: &str) -> bool {
    !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
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
            .filter(|h| is_site_host(h))
            .map(|h| format!("*{h}"))
            .collect();
        if !domains.is_empty() {
            // Stay within WebKit's cap, or the whole list fails to compile.
            if self.rules.len() >= WEBKIT_MAX_RULES {
                self.skipped += self.rules.len() + 1 - WEBKIT_MAX_RULES;
                self.rules.truncate(WEBKIT_MAX_RULES - 1);
            }
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
/// classes, so no site content shares the selector. Hiding a banner clicks
/// nothing, so it records no choice: the site's configured default applies
/// (opt-in regions usually keep non-essential cookies off; opt-out regions
/// may set them). Modal
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
            Some(rules) => {
                for (rule, kind) in rules {
                    match kind {
                        RuleKind::Block | RuleKind::InlineException => out.rules.push(rule),
                        RuleKind::Exception => exceptions.push(rule),
                    }
                }
            }
            None => out.skipped += 1,
        }
    }
    // Plain `@@` exceptions apply to every block rule before them, so they run last.
    out.rules.extend(exceptions);
    if out.rules.len() > WEBKIT_MAX_RULES {
        out.skipped += out.rules.len() - WEBKIT_MAX_RULES;
        out.rules.truncate(WEBKIT_MAX_RULES);
    }
    out
}

/// Where a compiled rule goes relative to the others. WebKit evaluates rules in
/// list order, and `ignore-previous-rules` cancels every earlier rule that
/// matches the request.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    Block,
    /// `@@` exception: kept to the end so it can cancel all block rules.
    Exception,
    /// Mixed `$domain` exception: paired with the block just before it, so it
    /// stays inline and cannot cancel unrelated rules compiled later.
    InlineException,
}

/// Returns the rules for one filter in evaluation order; an empty vector means
/// the filter is valid but its domain scope can never match. `None` means
/// unsupported syntax, counted in [`Compiled::skipped`].
fn compile_line(line: &str) -> Option<Vec<(Value, RuleKind)>> {
    if line.contains("#?#") || line.contains("#@#") || line.contains("#$#") || line.contains("##+js") {
        return None;
    }
    if let Some(idx) = line.find("##") {
        let (domains, selector) = (&line[..idx], &line[idx + 2..]);
        if selector.is_empty() {
            return None;
        }
        let scope = DomainScope::parse(domains.split(',').filter(|d| !d.is_empty()))?;
        let mut trigger = json!({ "url-filter": ".*" });
        match apply_domains(&mut trigger, &scope) {
            DomainApplication::Applied => {}
            DomainApplication::Never => return Some(vec![]),
            // A CSS exception would need url-filter `.*`, which would also cancel
            // network blocking on the excluded page. WebKit has no CSS-only undo.
            DomainApplication::Split(_) => return None,
        }
        let rule = json!({ "trigger": trigger, "action": { "type": "css-display-none", "selector": selector } });
        return Some(vec![(rule, RuleKind::Block)]);
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
    let mut scope = DomainScope::Any;
    if let Some(opts) = options {
        let mut types = vec![];
        for opt in opts.split(',') {
            match opt {
                "third-party" => trigger["load-type"] = json!(["third-party"]),
                "~third-party" | "first-party" => trigger["load-type"] = json!(["first-party"]),
                "match-case" => trigger["url-filter-is-case-sensitive"] = json!(true),
                "important" => {}
                o if o.starts_with("domain=") => scope = DomainScope::parse(o[7..].split('|'))?,
                o => types.push(resource_type(o)?),
            }
        }
        if !types.is_empty() {
            types.dedup();
            trigger["resource-type"] = json!(types);
        }
    }
    let action = if exception { "ignore-previous-rules" } else { "block" };
    let kind = if exception { RuleKind::Exception } else { RuleKind::Block };
    match apply_domains(&mut trigger, &scope) {
        DomainApplication::Applied => Some(vec![(json!({ "trigger": trigger, "action": { "type": action } }), kind)]),
        DomainApplication::Never => Some(vec![]),
        DomainApplication::Split(exception_trigger) => {
            // An allow rule narrowed to include-minus-exclude would need both
            // conditions on one trigger, and a second exception cannot undo the
            // first. Skip it rather than widening the allow to the excluded host.
            if exception {
                return None;
            }
            Some(vec![
                (json!({ "trigger": trigger, "action": { "type": "block" } }), RuleKind::Block),
                (json!({ "trigger": exception_trigger, "action": { "type": "ignore-previous-rules" } }), RuleKind::InlineException),
            ])
        }
    }
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

/// A `$domain` or `domain##` scope after resolving which sides can matter.
#[derive(Debug, PartialEq)]
enum DomainScope {
    Any,
    Include(Vec<String>),
    Exclude(Vec<String>),
    IncludeExcept { include: Vec<String>, except: Vec<String> },
    /// Every included host is excluded: the filter can never match.
    Never,
}

impl DomainScope {
    fn parse<'a>(domains: impl Iterator<Item = &'a str>) -> Option<Self> {
        let (mut include, mut except) = (vec![], vec![]);
        for d in domains {
            let d = d.trim().to_ascii_lowercase();
            if d.is_empty() || d.contains('*') {
                return None;
            }
            match d.strip_prefix('~') {
                Some(x) if x.is_empty() => return None,
                Some(x) => push_unique(&mut except, x),
                None => push_unique(&mut include, &d),
            }
        }
        Some(match (include.is_empty(), except.is_empty()) {
            (true, true) => Self::Any,
            (false, true) => Self::Include(include),
            (true, false) => Self::Exclude(except),
            (false, false) => Self::mixed(include, except),
        })
    }

    /// A WebKit include matches the host and its subdomains, so an include
    /// inside an excluded host can never match, while an exclusion outside
    /// every include cannot change a match. Keep only the domains that matter.
    fn mixed(include: Vec<String>, except: Vec<String>) -> Self {
        let include: Vec<String> = include.into_iter().filter(|i| !except.iter().any(|e| covers(e, i))).collect();
        if include.is_empty() {
            return Self::Never;
        }
        let except: Vec<String> = except.into_iter().filter(|e| include.iter().any(|i| covers(i, e))).collect();
        if except.is_empty() {
            Self::Include(include)
        } else {
            Self::IncludeExcept { include, except }
        }
    }
}

/// True when `domain` is `host` itself or a subdomain of it.
fn covers(host: &str, domain: &str) -> bool {
    domain == host || domain.strip_suffix(host).is_some_and(|prefix| prefix.ends_with('.'))
}

fn push_unique(domains: &mut Vec<String>, domain: &str) {
    if !domains.iter().any(|d| d == domain) {
        domains.push(domain.to_string());
    }
}

/// What one trigger can carry. WebKit rejects two domain conditions on one
/// trigger (`JSONMultipleConditions`), so a mixed scope returns a second
/// trigger for the paired `ignore-previous-rules` exception.
enum DomainApplication {
    Applied,
    Split(Value),
    Never,
}

fn apply_domains(trigger: &mut Value, scope: &DomainScope) -> DomainApplication {
    let webkit = |domains: &[String]| domains.iter().map(|d| format!("*{d}")).collect::<Vec<_>>();
    match scope {
        DomainScope::Any => DomainApplication::Applied,
        DomainScope::Include(include) => {
            trigger["if-domain"] = json!(webkit(include));
            DomainApplication::Applied
        }
        DomainScope::Exclude(except) => {
            trigger["unless-domain"] = json!(webkit(except));
            DomainApplication::Applied
        }
        DomainScope::IncludeExcept { include, except } => {
            trigger["if-domain"] = json!(webkit(include));
            let mut exception = trigger.clone();
            exception["if-domain"] = json!(webkit(except));
            DomainApplication::Split(exception)
        }
        DomainScope::Never => DomainApplication::Never,
    }
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

    fn domain_matches(patterns: &Value, page: &str) -> bool {
        patterns.as_array().unwrap().iter().any(|pattern| {
            let pattern = pattern.as_str().unwrap();
            let host = pattern.strip_prefix('*').unwrap_or(pattern);
            page == host || page.strip_suffix(host).is_some_and(|prefix| prefix.ends_with('.'))
        })
    }

    /// WebKit applies rules in order and `ignore-previous-rules` cancels every
    /// match before it. Every rule in these tests shares one url-filter, so the
    /// domain condition alone decides whether it matches.
    fn blocks_on(compiled: &Compiled, page: &str) -> bool {
        let mut blocked = false;
        for rule in &compiled.rules {
            let trigger = &rule["trigger"];
            let included = trigger.get("if-domain").is_none_or(|d| domain_matches(d, page));
            let not_excluded = trigger.get("unless-domain").is_none_or(|d| !domain_matches(d, page));
            if !included || !not_excluded {
                continue;
            }
            match rule["action"]["type"].as_str().unwrap() {
                "block" => blocked = true,
                "ignore-previous-rules" => blocked = false,
                _ => {}
            }
        }
        blocked
    }

    #[test]
    fn domain_anchor() {
        assert_eq!(regex_of("||ads.example.com^"), "^[^:]+://+([^:/]+\\.)?ads\\.example\\.com[:/?]");
    }

    #[test]
    fn options_map_to_webkit_triggers() {
        let c = compile("||tracker.test^$third-party,script,domain=news.test|~blog.news.test");
        assert_eq!((c.rules.len(), c.skipped), (2, 0));

        let block = &c.rules[0];
        assert_eq!(block["action"]["type"], "block");
        assert_eq!(block["trigger"]["load-type"], json!(["third-party"]));
        assert_eq!(block["trigger"]["resource-type"], json!(["script"]));
        assert_eq!(block["trigger"]["if-domain"], json!(["*news.test"]));
        assert!(block["trigger"].get("unless-domain").is_none());

        let exception = &c.rules[1];
        assert_eq!(exception["action"]["type"], "ignore-previous-rules");
        assert_eq!(exception["trigger"]["load-type"], json!(["third-party"]));
        assert_eq!(exception["trigger"]["resource-type"], json!(["script"]));
        assert_eq!(exception["trigger"]["if-domain"], json!(["*blog.news.test"]));
        assert_eq!(exception["trigger"]["url-filter"], block["trigger"]["url-filter"]);
        assert!(
            c.rules.iter().all(|r| r["trigger"].get("if-domain").is_none() || r["trigger"].get("unless-domain").is_none()),
            "WebKit rejects a trigger with both domain conditions"
        );

        assert!(blocks_on(&c, "news.test"));
        assert!(blocks_on(&c, "sub.news.test"));
        assert!(!blocks_on(&c, "blog.news.test"));
        assert!(!blocks_on(&c, "sub.blog.news.test"));
        assert!(!blocks_on(&c, "other.test"));
    }

    #[test]
    fn domain_scopes_keep_each_side() {
        // Exclusion only: the block applies everywhere except the excluded host.
        let c = compile("||tracker.test^$domain=~blog.news.test");
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0]["trigger"]["unless-domain"], json!(["*blog.news.test"]));
        assert!(blocks_on(&c, "news.test"));
        assert!(!blocks_on(&c, "blog.news.test"));

        // An exclusion outside the included host cannot change a match.
        let c = compile("||tracker.test^$domain=news.test|~other.test");
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0]["trigger"]["if-domain"], json!(["*news.test"]));
        assert!(blocks_on(&c, "news.test"));
        assert!(!blocks_on(&c, "other.test"));

        // The exclusion covers the include: the filter can never match.
        let c = compile("||tracker.test^$domain=blog.news.test|~news.test");
        assert!(c.rules.is_empty());
        assert_eq!(c.skipped, 0);
        assert!(!blocks_on(&c, "blog.news.test"));

        // A malformed exclusion is unsupported, not a bare `*` domain.
        let c = compile("||tracker.test^$domain=news.test|~");
        assert!(c.rules.is_empty());
        assert_eq!(c.skipped, 1);
    }

    #[test]
    fn mixed_domain_scopes_webkit_cannot_express_are_skipped() {
        // A narrowed allow would need both conditions on one trigger; a second
        // exception cannot undo the first.
        let c = compile("@@||tracker.test^$domain=news.test|~blog.news.test");
        assert!(c.rules.is_empty());
        assert_eq!(c.skipped, 1);

        // A CSS exception with url-filter `.*` would also cancel network
        // blocking on the excluded page, so the whole scope is unsupported.
        let c = compile("news.test,~blog.news.test##.promo");
        assert!(c.rules.is_empty());
        assert_eq!(c.skipped, 1);

        // Exclusion-only element hiding stays expressible.
        let c = compile("~blog.news.test##.promo");
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0]["trigger"]["unless-domain"], json!(["*blog.news.test"]));
        assert_eq!(c.rules[0]["action"], json!({"type": "css-display-none", "selector": ".promo"}));
    }

    #[test]
    fn mixed_domain_exception_stays_with_its_block() {
        let c = compile("||tracker.test^$domain=news.test|~blog.news.test\n||tracker.test^$domain=blog.news.test");
        assert_eq!(c.rules.len(), 3);
        assert_eq!(c.rules[1]["action"]["type"], "ignore-previous-rules");
        assert_eq!(c.rules[2]["action"]["type"], "block");
        assert_eq!(c.rules[2]["trigger"]["if-domain"], json!(["*blog.news.test"]));
        assert!(blocks_on(&c, "blog.news.test"), "the later block survives its inline exception");
        assert!(blocks_on(&c, "news.test"));
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
        let mut full = Compiled { rules: vec![json!({}); WEBKIT_MAX_RULES], skipped: 0 };
        full.allow_sites(&["a.test"]);
        assert_eq!(full.rules.len(), WEBKIT_MAX_RULES, "the exception fits under the cap");
        assert_eq!(full.rules.last().unwrap()["action"]["type"], "ignore-previous-rules");
        assert_eq!(full.skipped, 1);
        assert!(!is_site_host("[") && !is_site_host("::1") && is_site_host("news.example"));
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
