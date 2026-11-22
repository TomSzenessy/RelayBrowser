//! Strip tracking parameters and redirect wrappers from a URL for sharing.
//! Pure string work: parameter order, other parameters and the fragment are
//! preserved byte for byte; non-http(s) input is returned untouched.

const TRACKING: &[&str] = &[
    "fbclid", "gclid", "dclid", "msclkid", "mc_eid", "mc_cid", "igshid", "ref_src", "ref_url", "_hsenc", "_hsmi", "yclid", "twclid", "vero_id", "oly_enc_id", "oly_anon_id",
];

/// Hosts where `si` is a share-tracking id (elsewhere it may be meaningful).
const SI_HOSTS: &[&str] = &["youtube.com", "youtu.be", "spotify.com"];

const MAX_UNWRAP: usize = 4;

fn is_http(url: &str) -> bool {
    let l = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

pub(crate) fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or("", |(_, r)| r);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or(auth);
    auth.split(':').next().unwrap_or(auth).to_ascii_lowercase()
}

fn host_is(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|p| p.ends_with('.'))
}

fn hex(b: u8) -> u8 {
    (b as char).to_digit(16).unwrap_or(0) as u8
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && b.get(i + 1).is_some_and(u8::is_ascii_hexdigit) && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit) {
            out.push((hex(b[i + 1]) << 4) | hex(b[i + 2]));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The wrapped destination of a google.com/url?q= or facebook l.php?u= link.
fn unwrap_redirect(url: &str) -> Option<String> {
    let host = host_of(url);
    let (before_frag, _) = url.split_once('#').unwrap_or((url, ""));
    let (base, query) = before_frag.split_once('?')?;
    let rest = base.split_once("://")?.1;
    let path = rest.find('/').map_or("/", |i| &rest[i..]);
    let labels: Vec<&str> = host.split('.').collect();
    let is_google = labels.iter().rposition(|l| *l == "google").is_some_and(|p| p + 2 == labels.len() || p + 3 == labels.len());
    let keys: &[&str] = if is_google {
        if path != "/url" {
            return None;
        }
        &["q", "url"]
    } else if host_is(&host, "facebook.com") {
        if path != "/l.php" {
            return None;
        }
        &["u"]
    } else {
        return None;
    };
    for key in keys {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=')
                && k == *key
            {
                let target = percent_decode(v);
                if is_http(&target) {
                    return Some(target);
                }
            }
        }
    }
    None
}

fn strip_params(url: &str) -> String {
    let (head, frag) = match url.split_once('#') {
        Some((h, f)) => (h, Some(f)),
        None => (url, None),
    };
    let Some((base, query)) = head.split_once('?') else {
        return url.to_string();
    };
    let host = host_of(base);
    let drop_si = SI_HOSTS.iter().any(|d| host_is(&host, d));
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            let key = pair.split_once('=').map_or(*pair, |(k, _)| k).to_ascii_lowercase();
            !(key.starts_with("utm_") || TRACKING.contains(&key.as_str()) || (drop_si && key == "si"))
        })
        .collect();
    let mut out = base.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// `url` without tracking parameters and redirect wrappers.
pub fn clean(url: &str) -> String {
    let mut cur = url.trim().to_string();
    if !is_http(&cur) {
        return url.to_string();
    }
    for _ in 0..MAX_UNWRAP {
        match unwrap_redirect(&cur) {
            Some(next) => cur = next,
            None => break,
        }
    }
    strip_params(&cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_utm_and_click_ids_keeping_the_rest() {
        assert_eq!(clean("https://a.com/p?utm_source=x&id=7&utm_medium=y&fbclid=abc&q=hi"), "https://a.com/p?id=7&q=hi");
        for k in ["gclid", "dclid", "msclkid", "mc_eid", "mc_cid", "igshid", "ref_src", "ref_url", "_hsenc", "_hsmi", "yclid", "twclid", "vero_id", "oly_enc_id", "oly_anon_id", "utm_campaign", "UTM_Source", "FBCLID"] {
            assert_eq!(clean(&format!("https://a.com/?{k}=1&keep=2")), "https://a.com/?keep=2", "{k}");
        }
    }

    #[test]
    fn drops_question_mark_when_nothing_left_and_keeps_fragment() {
        assert_eq!(clean("https://a.com/p?utm_source=x"), "https://a.com/p");
        assert_eq!(clean("https://a.com/p?utm_source=x&fbclid=1#frag"), "https://a.com/p#frag");
        assert_eq!(clean("https://a.com/p?utm_source=x&a=1#sec?b=2"), "https://a.com/p?a=1#sec?b=2");
        assert_eq!(clean("https://a.com/p#frag"), "https://a.com/p#frag");
    }

    #[test]
    fn order_and_untouched_urls_preserved_exactly() {
        for u in ["https://a.com/p?z=1&a=2&m=3", "https://a.com/", "https://a.com", "http://a.com/p?x=%20y&flag&b=", "https://a.com/p?utmx=1&sidx=2&ref=3"] {
            assert_eq!(clean(u), u);
        }
    }

    #[test]
    fn si_only_on_youtube_and_spotify() {
        assert_eq!(clean("https://youtu.be/abc?si=xyz&t=10"), "https://youtu.be/abc?t=10");
        assert_eq!(clean("https://www.youtube.com/watch?v=1&si=xyz"), "https://www.youtube.com/watch?v=1");
        assert_eq!(clean("https://m.youtube.com/watch?si=xyz&v=1"), "https://m.youtube.com/watch?v=1");
        assert_eq!(clean("https://open.spotify.com/track/1?si=abc"), "https://open.spotify.com/track/1");
        assert_eq!(clean("https://example.com/?si=keepme"), "https://example.com/?si=keepme");
        assert_eq!(clean("https://notyoutube.com/?si=keepme"), "https://notyoutube.com/?si=keepme");
    }

    #[test]
    fn non_http_input_is_untouched() {
        for u in ["", "about:blank", "file:///a?utm_source=x", "mailto:a@b.c?utm_source=x", "javascript:alert(1)", "not a url utm_source=1", "ftp://a.com/?utm_source=1", "  about:blank  ", "é"] {
            assert_eq!(clean(u), u);
        }
    }

    #[test]
    fn unwraps_google_redirect() {
        assert_eq!(clean("https://www.google.com/url?q=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1&sa=D&usg=x"), "https://example.com/a?b=1");
        assert_eq!(clean("https://www.google.co.uk/url?sa=t&url=https%3A%2F%2Fexample.com%2F"), "https://example.com/");
        assert_eq!(clean("https://www.google.com/url?q=https%3A%2F%2Fexample.com%2F%3Futm_source%3Dx%26k%3D1"), "https://example.com/?k=1");
    }

    #[test]
    fn unwraps_facebook_redirect() {
        assert_eq!(clean("https://l.facebook.com/l.php?u=https%3A%2F%2Fexample.com%2Fx%3Fa%3D1&h=AT0&fbclid=zz"), "https://example.com/x?a=1");
        assert_eq!(clean("https://www.facebook.com/l.php?u=http%3A%2F%2Fexample.com"), "http://example.com");
    }

    #[test]
    fn redirect_wrappers_only_when_target_is_http() {
        let js = "https://www.google.com/url?q=javascript%3Aalert(1)";
        assert_eq!(clean(js), js);
        let ordinary = "https://www.google.com/search?q=https%3A%2F%2Fexample.com";
        assert_eq!(clean(ordinary), ordinary);
        let missing = "https://www.google.com/url?sa=D";
        assert_eq!(clean(missing), missing);
        let other = "https://example.com/url?q=https%3A%2F%2Fevil.com";
        assert_eq!(clean(other), other);
        let fb = "https://www.facebook.com/somepage?u=https%3A%2F%2Fexample.com";
        assert_eq!(clean(fb), fb);
    }

    #[test]
    fn nested_wrappers_are_bounded() {
        let mut u = "https://example.com/final".to_string();
        for _ in 0..10 {
            u = format!("https://www.google.com/url?q={}", u.replace(':', "%3A").replace('/', "%2F").replace('?', "%3F").replace('=', "%3D").replace('&', "%26"));
        }
        let out = clean(&u);
        assert!(out.starts_with("https://"));
    }

    #[test]
    fn malformed_input_never_panics() {
        for u in ["https://", "https://?", "https://#", "https://a.com/?", "https://a.com/?&&&", "https://a.com/?=&=", "http://%", "https://www.google.com/url?q=%", "https://www.google.com/url?q=%zz", "https://www.google.com/url?q", "HTTPS://A.COM/?UTM_X=1", "https://a.com/\u{1F600}?utm_a=\u{1F600}", "https://www.google.com/url?q=https%3A%2F%2F%E2%82"] {
            let _ = clean(u);
        }
        assert_eq!(clean("HTTPS://A.COM/?UTM_X=1"), "HTTPS://A.COM/");
        assert_eq!(clean("https://a.com/?&&&"), "https://a.com/?&&&");
    }
}
