//! HTML builder for the native always-on-top video panel. Portable and
//! testable; `mac.rs` owns the NSPanel/WKWebView wrapper.

/// Build the panel document only for an http(s) video URL. The title and
/// poster are escaped; a `javascript:`/`data:` source is refused.
pub fn panel_html(src: &str, poster: &str, title: &str) -> Option<String> {
    if !(src.starts_with("http://") || src.starts_with("https://")) {
        return None;
    }
    let poster_ok = poster.starts_with("http://") || poster.starts_with("https://");
    let poster = if poster_ok { format!(r#" poster="{}""#, escape(poster)) } else { String::new() };
    let title = escape(title);
    let src = escape(src);
    Some(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{title}</title>
<style>
html,body{{margin:0;height:100%;background:#000;color:#eee;font:12px -apple-system,sans-serif;overflow:hidden}}
video{{width:100%;height:100%;object-fit:contain;background:#000}}
#t{{position:fixed;left:8px;bottom:6px;right:8px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;opacity:.75;pointer-events:none}}
</style></head><body><video src="{src}"{poster} controls autoplay playsinline></video><div id="t">{title}</div>
<script>const v=document.querySelector('video');v.addEventListener('canplay',()=>v.play().catch(()=>{{}}),{{once:true}});</script>
</body></html>"#
    ))
}

fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_sources_and_escaped_text() {
        let html = panel_html("https://cdn.test/v.mp4", "https://cdn.test/p.jpg", "A <b> & \"B\"").unwrap();
        assert!(html.contains(r#"src="https://cdn.test/v.mp4""#));
        assert!(html.contains(r#"poster="https://cdn.test/p.jpg""#));
        assert!(html.contains("A &lt;b&gt; &amp; &quot;B&quot;"));
        assert!(!html.contains("<b>"));
        assert!(panel_html("javascript:alert(1)", "", "x").is_none());
        assert!(panel_html("data:video/mp4;base64,AA", "", "x").is_none());
        let no_poster = panel_html("http://x.test/v", "file:///tmp/p.jpg", "t").unwrap();
        assert!(!no_poster.contains("poster="));
    }
}
