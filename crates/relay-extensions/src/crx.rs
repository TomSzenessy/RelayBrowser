//! Chrome Web Store installer: download a `.crx`, check its CRX3 header,
//! and unpack the zip payload safely.
//!
//! Integrity: the download is HTTPS from Google, and the header's
//! `crx_id` must equal the requested id. RSA/ECDSA signature verification is
//! not implemented yet (tracked as a residual risk in docs/security.md).

use crate::Error;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// Chrome version reported to the update service (it serves MV3 builds to
/// modern versions only).
pub const PROD_VERSION: &str = "131.0.0.0";
const MAX_CRX_BYTES: u64 = 256 * 1024 * 1024;
/// Total decompressed bytes an extension may unpack to; also the per-entry
/// ceiling. Compressed CRX size is capped separately.
const MAX_UNPACK_BYTES: u64 = 256 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub fn is_extension_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// Extract a 32-character Chrome extension id from either a raw id or a
/// Chrome Web Store URL (e.g. `https://chromewebstore.google.com/detail/name/<id>`).
pub fn extract_extension_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if is_extension_id(trimmed) {
        return Some(trimmed.to_string());
    }
    let without_query = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    for segment in without_query.split('/').rev() {
        let lower = segment.to_ascii_lowercase();
        if is_extension_id(&lower) {
            return Some(lower);
        }
    }
    None
}

/// The Web Store's public update endpoint for one extension.
pub fn download_url(id: &str) -> Result<String, Error> {
    if !is_extension_id(id) {
        return Err(Error::Invalid(format!("{id:?} is not a Chrome extension id")));
    }
    Ok(format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion={PROD_VERSION}&acceptformat=crx2,crx3&x=id%3D{id}%26uc"
    ))
}

pub fn download(id: &str) -> Result<Vec<u8>, Error> {
    download_from(&download_url(id)?, DOWNLOAD_TIMEOUT)
}

fn download_from(url: &str, timeout: std::time::Duration) -> Result<Vec<u8>, Error> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .build(),
    );
    let mut res = agent.get(url).call().map_err(|e| Error::Http(format!("{url}: {e}")))?;
    if let Some(len) = res.body().content_length() {
        if len > MAX_CRX_BYTES {
            return Err(Error::Invalid(format!("package is {len} bytes, over the {MAX_CRX_BYTES}-byte cap")));
        }
    }
    let mut buf = vec![];
    res.body_mut().as_reader().take(MAX_CRX_BYTES + 1).read_to_end(&mut buf).map_err(|e| Error::Http(format!("{url}: {e}")))?;
    if buf.len() as u64 > MAX_CRX_BYTES {
        return Err(Error::Invalid(format!("package exceeds the {MAX_CRX_BYTES}-byte cap")));
    }
    Ok(buf)
}

/// Parsed CRX container: the zip payload and the id the header declares.
#[derive(Debug)]
pub struct Crx<'a> {
    pub version: u32,
    pub crx_id: Option<String>,
    pub zip: &'a [u8],
}

pub fn parse(bytes: &[u8]) -> Result<Crx<'_>, Error> {
    let bad = |m: &str| Error::Invalid(format!("not a CRX file: {m}"));
    if bytes.len() < 12 || &bytes[..4] != b"Cr24" {
        // Some mirrors serve the bare zip.
        if bytes.starts_with(b"PK\x03\x04") {
            return Ok(Crx { version: 0, crx_id: None, zip: bytes });
        }
        return Err(bad("missing Cr24 magic"));
    }
    let u32_at = |i: usize| bytes.get(i..i + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let version = u32_at(4).ok_or_else(|| bad("missing version"))?;
    match version {
        3 => {
            let header_len = u32_at(8).ok_or_else(|| bad("missing header length"))? as usize;
            let start = 12usize.checked_add(header_len).filter(|e| *e <= bytes.len()).ok_or_else(|| bad("header overruns file"))?;
            if start >= bytes.len() || !bytes[start..].starts_with(b"PK\x03\x04") {
                return Err(bad("missing zip payload"));
            }
            let crx_id = crx3_id(&bytes[12..start]);
            Ok(Crx { version, crx_id, zip: &bytes[start..] })
        }
        2 => {
            let key_len = u32_at(8).ok_or_else(|| bad("missing key length"))? as usize;
            let sig_len = u32_at(12).ok_or_else(|| bad("missing signature length"))? as usize;
            let start = 16usize.checked_add(key_len).and_then(|s| s.checked_add(sig_len)).filter(|e| *e <= bytes.len()).ok_or_else(|| bad("header overruns file"))?;
            if start >= bytes.len() || !bytes[start..].starts_with(b"PK\x03\x04") {
                return Err(bad("missing zip payload"));
            }
            Ok(Crx { version, crx_id: None, zip: &bytes[start..] })
        }
        v => Err(bad(&format!("unsupported version {v}"))),
    }
}

/// Minimal protobuf walk: CrxFileHeader.signed_header_data (field 10000)
/// → SignedData.crx_id (field 1, 16 bytes) → a–p encoded id.
fn crx3_id(header: &[u8]) -> Option<String> {
    let signed = proto_field(header, 10000)?;
    let id = proto_field(signed, 1)?;
    if id.len() != 16 {
        return None;
    }
    Some(id.iter().flat_map(|b| [b >> 4, b & 0xf]).map(|n| (b'a' + n) as char).collect())
}

fn varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn proto_field(buf: &[u8], want: u64) -> Option<&[u8]> {
    let mut pos = 0;
    while pos < buf.len() {
        let key = varint(buf, &mut pos)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                varint(buf, &mut pos)?;
            }
            1 => pos += 8,
            5 => pos += 4,
            2 => {
                let len = varint(buf, &mut pos)? as usize;
                let end = pos.checked_add(len).filter(|e| *e <= buf.len())?;
                if field == want {
                    return Some(&buf[pos..end]);
                }
                pos = end;
            }
            _ => return None,
        }
    }
    None
}

/// Unpack into `dest/<id>` (replacing an older copy). Entries that would
/// escape the directory are rejected.
pub fn unpack(bytes: &[u8], expected_id: &str, dest: &Path) -> Result<PathBuf, Error> {
    unpack_limited(bytes, expected_id, dest, MAX_UNPACK_BYTES)
}

fn unpack_limited(bytes: &[u8], expected_id: &str, dest: &Path, limit: u64) -> Result<PathBuf, Error> {
    let crx = parse(bytes)?;
    // The Web Store serves CRX3, which always declares its id; anything else
    // (CRX2, a bare zip, a header without an id) cannot be tied to the request.
    match &crx.crx_id {
        Some(id) if id == expected_id => {}
        Some(id) => return Err(Error::Invalid(format!("package declares id {id}, expected {expected_id}"))),
        None => return Err(Error::Invalid(format!("package for {expected_id} is not a CRX3 with an id; refusing to install"))),
    }
    let out = dest.join(expected_id);
    let staging = dest.join(format!(".{expected_id}.partial"));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| -> Result<PathBuf, Error> {
        std::fs::create_dir_all(&staging)?;
        let mut zip = zip::ZipArchive::new(Cursor::new(crx.zip))?;
        let mut total = 0u64;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            let Some(rel) = f.enclosed_name() else {
                return Err(Error::Invalid(format!("unsafe path in package: {}", f.name().map(|n| n.into_owned()).unwrap_or_default())));
            };
            let size = f.size();
            if size > limit || total.saturating_add(size) > limit {
                return Err(Error::Invalid(format!("package decompresses over the {limit}-byte cap")));
            }
            total += size;
            let path = staging.join(rel);
            if f.is_dir() {
                std::fs::create_dir_all(&path)?;
                continue;
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut w = std::fs::File::create(&path)?;
            std::io::copy(&mut f, &mut w)?;
        }
        // Chrome-only metadata that WebKit rejects.
        let _ = std::fs::remove_dir_all(staging.join("_metadata"));
        if !staging.join("manifest.json").is_file() {
            return Err(Error::Invalid("package has no manifest.json".into()));
        }
        let _ = std::fs::remove_dir_all(&out);
        std::fs::rename(&staging, &out)?;
        Ok(out)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(vec![]);
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, body) in entries {
                z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    fn crx3(id_bytes: [u8; 16], zip: &[u8]) -> Vec<u8> {
        // SignedData { crx_id = 1 }
        let mut signed = vec![0x0a, 16];
        signed.extend(id_bytes);
        // CrxFileHeader { signed_header_data = 10000 } ; key = 10000<<3|2 = 80002 → varint
        let mut header = vec![0x82, 0xf1, 0x04, signed.len() as u8];
        header.extend(signed);
        let mut out = b"Cr24".to_vec();
        out.extend(3u32.to_le_bytes());
        out.extend((header.len() as u32).to_le_bytes());
        out.extend(header);
        out.extend(zip);
        out
    }

    const ID: &str = "cjpalhdlnbpafiamejdnhcphjbkeiagm";

    /// Inverse of the a–p encoding, so the fixture cannot drift from ID.
    fn id_bytes() -> [u8; 16] {
        let n: Vec<u8> = ID.bytes().map(|c| c - b'a').collect();
        std::array::from_fn(|i| (n[2 * i] << 4) | n[2 * i + 1])
    }

    #[test]
    fn crx3_header_yields_the_store_id() {
        let bytes = crx3(id_bytes(), &zip_with(&[("manifest.json", "{}")]));
        assert_eq!(parse(&bytes).unwrap().crx_id.as_deref(), Some(ID));
    }

    #[test]
    fn unpacks_and_strips_metadata() {
        let d = tempfile::tempdir().unwrap();
        let bytes = crx3(id_bytes(), &zip_with(&[("manifest.json", "{\"name\":\"x\"}"), ("js/a.js", "1"), ("_metadata/verified_contents.json", "[]")]));
        let out = unpack(&bytes, ID, d.path()).unwrap();
        assert!(out.join("js/a.js").is_file());
        assert!(!out.join("_metadata").exists());
    }

    #[test]
    fn rejects_wrong_id_and_traversal() {
        let d = tempfile::tempdir().unwrap();
        let good = crx3(id_bytes(), &zip_with(&[("manifest.json", "{}")]));
        assert!(unpack(&good, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", d.path()).unwrap_err().to_string().contains("declares id"));
        let bare = zip_with(&[("manifest.json", "{}")]);
        assert!(unpack(&bare, ID, d.path()).unwrap_err().to_string().contains("not a CRX3"));
        let evil = crx3(id_bytes(), &zip_with(&[("manifest.json", "{}"), ("../../escape.txt", "x")]));
        assert!(unpack(&evil, ID, d.path()).unwrap_err().to_string().contains("unsafe path"));
        assert!(!d.path().parent().unwrap().join("escape.txt").exists());
    }

    #[test]
    fn truncated_crx_headers_are_errors_not_panics() {
        for len in 11..=16 {
            for version in [2u32, 3] {
                let mut bytes = vec![0u8; len];
                bytes[..4].copy_from_slice(b"Cr24");
                bytes[4..8].copy_from_slice(&version.to_le_bytes());
                assert!(parse(&bytes).is_err(), "len {len} version {version}");
            }
        }
    }

    #[test]
    fn unpack_limit_cleans_staging_and_rejects_a_bomb() {
        let d = tempfile::tempdir().unwrap();
        let payload = "x".repeat(50_000);
        let bytes = crx3(id_bytes(), &zip_with(&[("manifest.json", "{\"name\":\"x\"}"), ("big.bin", &payload)]));
        let err = unpack_limited(&bytes, ID, d.path(), 1024).unwrap_err().to_string();
        assert!(err.contains("cap"), "{err}");
        assert!(!d.path().join(format!(".{ID}.partial")).exists(), "partial staging must be removed");
        assert!(!d.path().join(ID).exists());
    }

    #[test]
    fn download_timeout_names_the_url() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\n\r\n");
            std::thread::sleep(std::time::Duration::from_secs(2));
        });
        let url = format!("http://127.0.0.1:{port}/stall");
        let started = std::time::Instant::now();
        let err = download_from(&url, std::time::Duration::from_millis(500)).unwrap_err();
        assert!(err.to_string().contains(&url), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "download did not time out");
        server.join().unwrap();
    }

    #[test]
    fn url_requires_a_real_id() {
        assert!(download_url("../../etc").is_err());
        assert!(download_url(ID).unwrap().contains("x=id%3Dcjpalhdlnbpafiamejdnhcphjbkeiagm%26uc"));
    }

    #[test]
    fn extract_extension_id_accepts_raw_and_cws_urls() {
        assert_eq!(extract_extension_id(ID).as_deref(), Some(ID));
        assert_eq!(
            extract_extension_id(&format!("https://chromewebstore.google.com/detail/ublock-origin/{ID}?hl=en")).as_deref(),
            Some(ID)
        );
        assert_eq!(
            extract_extension_id(&format!("https://chrome.google.com/webstore/detail/name/{ID}#top")).as_deref(),
            Some(ID)
        );
        assert_eq!(extract_extension_id("https://example.com/not-an-extension"), None);
        assert_eq!(extract_extension_id(""), None);
    }
}
