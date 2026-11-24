//! Netscape bookmark HTML: export Relay's bookmarks and import the same
//! format from other browsers. The parser is deliberately small and
//! line-oriented; it accepts the common `<DL><p>`/`<DT>` layout.

use crate::chrome_import::Bookmark;
use crate::Error;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default)]
struct Node<'a> {
    folders: BTreeMap<String, Node<'a>>,
    bookmarks: Vec<&'a Bookmark>,
}

/// Build a Netscape bookmark file with nested folders.
pub fn export(bookmarks: &[Bookmark]) -> String {
    let mut root = Node::default();
    for b in bookmarks {
        let mut node = &mut root;
        for folder in &b.folder {
            node = node.folders.entry(folder.clone()).or_default();
        }
        node.bookmarks.push(b);
    }
    let mut out = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
    );
    write_node(&root, 1, &mut out);
    out.push_str("</DL><p>\n");
    out
}

fn write_node(node: &Node<'_>, depth: usize, out: &mut String) {
    let indent = "    ".repeat(depth);
    for b in &node.bookmarks {
        out.push_str(&format!("{indent}<DT><A HREF=\"{}\" >{}</A>\n", escape(&b.url), escape(&b.title)));
    }
    for (folder, child) in &node.folders {
        out.push_str(&format!("{indent}<DT><H3>{}</H3>\n{indent}<DL><p>\n", escape(folder)));
        write_node(child, depth + 1, out);
        out.push_str(&format!("{indent}</DL><p>\n"));
    }
}

/// Parse Netscape bookmark HTML. Malformed lines are skipped rather than
/// aborting the whole file.
pub fn parse(html: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let mut folders: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    let mut depth = 0usize;
    for line in html.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("<dl") {
            depth += 1;
            if depth > 1 {
                if let Some(folder) = pending.take() {
                    folders.push(folder);
                }
            }
        }
        if lower.contains("</dl") {
            if depth > 1 && folders.len() >= depth - 1 {
                folders.pop();
            }
            depth = depth.saturating_sub(1);
        }
        if let Some(title) = between(&lower, line, "<h3", "</h3>") {
            pending = Some(decode(&title));
            continue;
        }
        if let Some((href, title)) = anchor(line) {
            if href.starts_with("http://") || href.starts_with("https://") {
                out.push(Bookmark { title: if title.is_empty() { href.clone() } else { title }, url: href, folder: folders.clone() });
            }
        }
    }
    out
}

pub fn read(path: &Path) -> Result<Vec<Bookmark>, Error> {
    Ok(parse(&std::fs::read_to_string(path)?))
}

pub fn write(path: &Path, bookmarks: &[Bookmark]) -> Result<(), Error> {
    std::fs::write(path, export(bookmarks))?;
    Ok(())
}

fn between(lower: &str, original: &str, open: &str, close: &str) -> Option<String> {
    let start = lower.find(open)?;
    let after = lower[start..].find('>')? + start + 1;
    let end = lower[after..].find(close)? + after;
    Some(original[after..end].to_string())
}

fn anchor(line: &str) -> Option<(String, String)> {
    let lower = line.to_ascii_lowercase();
    let start = lower.find("<a ")?;
    let tag_end = lower[start..].find('>')? + start;
    let tag = &line[start..=tag_end];
    let href = attribute(tag, "href")?;
    let close = lower[tag_end..].find("</a>")? + tag_end;
    let title = decode(line[tag_end + 1..close].trim());
    Some((href, title))
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut pos = lower.find(name)? + name.len();
    pos = lower[pos..].find('=')? + pos + 1;
    let rest = tag[pos..].trim_start();
    let quote = rest.chars().next()?;
    let value = if quote == '"' || quote == '\'' {
        let after = &rest[1..];
        &after[..after.find(quote)?]
    } else {
        rest.split_whitespace().next()?
    };
    Some(decode(value))
}

fn decode(text: &str) -> String {
    text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_round_trip_preserves_folder_and_escapes() {
        let rows = vec![
            Bookmark { title: "A & B".into(), url: "https://a.test/?x=1&y=2".into(), folder: vec![] },
            Bookmark { title: "Work".into(), url: "https://work.test/".into(), folder: vec!["Folder".into(), "Sub".into()] },
        ];
        let html = export(&rows);
        assert!(html.contains("A &amp; B"));
        assert!(html.contains("https://a.test/?x=1&amp;y=2"));
        let parsed = parse(&html);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].title, "A & B");
        assert_eq!(parsed[0].url, "https://a.test/?x=1&y=2");
        assert_eq!(parsed[1].folder, vec!["Folder".to_string(), "Sub".to_string()]);
    }

    #[test]
    fn parses_common_browser_file_and_skips_bad_lines() {
        let html = r#"<DL><p>
<DT><H3>Reading</H3>
<DL><p>
<DT><A HREF="https://one.test/" ADD_DATE="1">One</A>
<DT><A HREF='javascript:alert(1)'>Bad</A>
</DL><p>
</DL><p>"#;
        let rows = parse(html);
        assert_eq!(rows, vec![Bookmark { title: "One".into(), url: "https://one.test/".into(), folder: vec!["Reading".into()] }]);
    }
}
