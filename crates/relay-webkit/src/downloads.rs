//! Download destination policy: turn a server-suggested filename into one
//! safe component under `~/Downloads/Relay`, and add a numeric suffix rather
//! than overwrite an existing file. Portable and tested everywhere; `mac.rs`
//! is the only caller (the WKDownload delegate), and the pure functions take
//! an `exists` predicate so tests need no filesystem.

use std::path::{Path, PathBuf};

/// Where Relay saves downloads, relative to the home directory.
pub const FOLDER: &str = "Downloads/Relay";

/// The same folder as chrome shows it before a destination exists.
pub const FOLDER_LABEL: &str = "~/Downloads/Relay";

/// Last path component of a server-suggested filename, whichever separator
/// the server used.
fn basename(suggested: &str) -> &str {
    suggested.rsplit(['/', '\\']).next().unwrap_or("")
}

/// A single safe path component. Directory components are dropped, control
/// characters and `:` become `_`, leading and trailing dots are trimmed (so
/// `.` and `..` cannot name a directory), and an empty result becomes
/// `download`. The output never contains `/` or `\`, so it cannot leave the
/// directory it is joined to.
pub fn safe_filename(suggested: &str) -> String {
    let mut name: String = basename(suggested).chars().map(|c| if c.is_control() || c == ':' { '_' } else { c }).collect();
    name = name.trim_matches('.').to_string();
    if name.is_empty() {
        name = "download".into();
    }
    name
}

/// Pick a collision-free destination under `dir`; `exists` decides whether a
/// candidate is taken. Returns the final file name and the path to save to.
pub fn destination(dir: &Path, suggested: &str, exists: impl Fn(&Path) -> bool) -> (String, PathBuf) {
    let name = safe_filename(suggested);
    let mut path = dir.join(&name);
    if exists(&path) {
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
            _ => (name.clone(), String::new()),
        };
        for n in 1..10_000 {
            let candidate = dir.join(format!("{stem} ({n}){ext}"));
            if !exists(&candidate) {
                path = candidate;
                break;
            }
        }
    }
    let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
    (filename, path)
}

/// The user's download folder, `~/Downloads/Relay`.
pub fn folder() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(FOLDER)
}

/// Create the download folder and return the sanitized filename plus the
/// collision-free absolute path the delegate should save to.
pub fn download_destination(suggested: &str) -> (String, String) {
    download_destination_in(&folder(), suggested)
}

/// The same, under a caller-chosen directory (Settings → General).
pub fn download_destination_in(dir: &Path, suggested: &str) -> (String, String) {
    let _ = std::fs::create_dir_all(dir);
    let (filename, path) = destination(dir, suggested, Path::exists);
    (filename, path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(suggested: &str) -> String {
        safe_filename(suggested)
    }

    #[test]
    fn a_chosen_directory_keeps_the_same_safety_rules() {
        let dir = std::env::temp_dir().join(format!("relay-downloads-{}", std::process::id()));
        let (name, path) = download_destination_in(&dir, "../../etc/passwd");
        assert_eq!(name, "passwd");
        assert_eq!(Path::new(&path).parent(), Some(dir.as_path()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keeps_a_plain_name_and_only_its_last_component() {
        assert_eq!(name("report.pdf"), "report.pdf");
        assert_eq!(name("my file (final).zip"), "my file (final).zip");
        assert_eq!(name("https://x.test/a/b/report.pdf"), "report.pdf");
        assert_eq!(name("/etc/passwd"), "passwd");
        assert_eq!(name("C:\\Windows\\System32\\cmd.exe"), "cmd.exe");
        assert_eq!(name("../../etc/passwd"), "passwd");
        assert_eq!(name("..\\..\\boot.ini"), "boot.ini");
    }

    #[test]
    fn strips_hidden_and_dot_only_names() {
        assert_eq!(name(".hidden"), "hidden");
        assert_eq!(name(".."), "download");
        assert_eq!(name("."), "download");
        assert_eq!(name("..."), "download");
        assert_eq!(name(""), "download");
        assert_eq!(name("/"), "download");
    }

    #[test]
    fn replaces_control_characters_and_the_hfs_separator() {
        assert_eq!(name("a:b"), "a_b");
        assert_eq!(name("a\tb\nc"), "a_b_c");
        assert_eq!(name("nul\0byte"), "nul_byte");
        assert_eq!(name(":"), "_", "a reserved name is still one usable component");
    }

    #[test]
    fn destinations_always_stay_inside_the_download_folder() {
        let dir = Path::new("/home/u/Downloads/Relay");
        for suggested in ["../../etc/passwd", "/etc/passwd", "..", ".", "", "a/b", "..\\..\\x", "C:\\x"] {
            let (filename, path) = destination(dir, suggested, |_| false);
            assert_eq!(path.parent(), Some(dir), "{suggested:?} escaped to {path:?}");
            assert_eq!(path.file_name().and_then(|n| n.to_str()), Some(filename.as_str()));
            assert!(!filename.contains(['/', '\\']), "{suggested:?} kept a path separator in {filename:?}");
            assert!(!filename.is_empty() && filename != "." && filename != "..", "{suggested:?} kept a directory alias");
        }
    }

    #[test]
    fn collisions_get_a_numeric_suffix_and_never_overwrite() {
        let dir = Path::new("/home/u/Downloads/Relay");
        let taken = |paths: &[&str]| {
            let set: std::collections::HashSet<PathBuf> = paths.iter().map(|p| dir.join(p)).collect();
            move |candidate: &Path| set.contains(candidate)
        };

        let taken_names = taken(&["a.zip", "a (1).zip"]);
        let (filename, path) = destination(dir, "a.zip", &taken_names);
        assert_eq!((filename.as_str(), path.as_path()), ("a (2).zip", dir.join("a (2).zip").as_path()));

        let taken_plain = taken(&["archive", "archive (1)"]);
        let (filename, _) = destination(dir, "archive", &taken_plain);
        assert_eq!(filename, "archive (2)", "a name without an extension still gets a suffix");

        let taken_double = taken(&["bundle.tar.gz"]);
        let (filename, _) = destination(dir, "bundle.tar.gz", &taken_double);
        assert_eq!(filename, "bundle.tar (1).gz", "only the last extension moves");

        let (filename, _) = destination(dir, "a.zip", &taken(&["other.zip"]));
        assert_eq!(filename, "a.zip", "an unused name is kept as-is");
    }

    #[test]
    fn a_collision_on_a_sanitized_name_uses_the_sanitized_stem() {
        let dir = Path::new("/home/u/Downloads/Relay");
        let taken = |candidate: &Path| candidate == dir.join("passwd");
        let (filename, path) = destination(dir, "../../etc/passwd", taken);
        assert_eq!(filename, "passwd (1)");
        assert_eq!(path.parent(), Some(dir));
    }

    #[test]
    fn folder_is_under_home() {
        let dir = folder();
        assert!(dir.ends_with(FOLDER), "{dir:?} should end with {FOLDER}");
        assert!(FOLDER_LABEL.ends_with(FOLDER));
    }
}
