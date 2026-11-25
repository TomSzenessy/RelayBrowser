//! Vimium-style link-hint label generator.
//!
//! Page inspection now uses Apple's native WebKit Web Inspector docked directly
//! inside each tab's rounded container (`⌘⌥I` / right-click → Inspect Element),
//! while local MCP agents inspect DOM, network, console, and cookies through
//! `relay-bridge`'s `SharedHub`.

/// Vimium-style labels: the fewest characters from a home-row alphabet, and
/// no label is a prefix of another, so typing resolves without a timeout.
pub fn hint_labels(n: usize) -> Vec<String> {
    const ALPHABET: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";
    let k = ALPHABET.len();
    if n == 0 {
        return vec![];
    }
    // Shortest length L such that k^L >= n; all labels have that length.
    let mut len = 1;
    while k.pow(len as u32) < n {
        len += 1;
    }
    (0..n)
        .map(|mut i| {
            let mut s = vec![b'a'; len];
            for slot in s.iter_mut().rev() {
                *slot = ALPHABET[i % k];
                i /= k;
            }
            String::from_utf8(s).expect("ascii")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn hint_labels_are_minimal_and_prefix_free() {
        assert_eq!(hint_labels(3), ["a", "s", "d"]);
        let many = hint_labels(100);
        assert!(many.iter().all(|l| l.len() == 2));
        let set: HashSet<_> = many.iter().collect();
        assert_eq!(set.len(), 100);
        assert!(hint_labels(0).is_empty());
    }
}
