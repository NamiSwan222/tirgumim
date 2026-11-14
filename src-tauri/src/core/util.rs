//! Small helpers shared by the rest of the core: hashing, safe paths, versions, JSON files.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_bytes(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Hashing a 500 MB game exe takes seconds, so hashes are remembered while size and modified time stay the same.
pub fn sha256_file_cached(path: &Path) -> io::Result<String> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let meta = fs::metadata(path)?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let key = format!("{}|{}|{}", path.to_string_lossy().to_lowercase(), meta.len(), mtime);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(h) = cache.lock().unwrap().get(&key) {
        return Ok(h.clone());
    }
    let h = sha256_file(path)?;
    cache.lock().unwrap().insert(key, h.clone());
    Ok(h)
}

/// Checks a pack-relative path and returns it with forward slashes. Absolute paths, drive letters, `..`, empty
/// segments and Windows' reserved characters are refused (zip-slip protection).
pub fn safe_rel_path(p: &str) -> Result<String, String> {
    let norm = p.trim().replace('\\', "/");
    if norm.is_empty() {
        return Err(format!("Invalid path: {p:?}"));
    }
    let b = norm.as_bytes();
    if norm.starts_with('/') || (b.len() >= 2 && b[1] == b':') {
        return Err(format!("Absolute path not allowed: {p}"));
    }
    for seg in norm.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(format!("Unsafe path: {p}"));
        }
        if seg.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (c as u32) < 32) {
            return Err(format!("Unsafe path: {p}"));
        }
    }
    Ok(norm)
}

/// Joins a checked relative path under `root`, making sure it stays inside.
pub fn resolve_inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = safe_rel_path(rel)?;
    let mut out = root.to_path_buf();
    for seg in rel.split('/') {
        out.push(seg);
    }
    // `rel` has no `..` or absolute parts (checked above), so only its own components can be odd.
    let added = out.components().count() - root.components().count();
    if added != rel.split('/').count() || out.components().rev().take(added).any(|c| !matches!(c, Component::Normal(_))) || !out.starts_with(root) {
        return Err(format!("Path escapes root: {rel}"));
    }
    Ok(out)
}

/// Compares dotted versions numerically: "0.10.0" > "0.9.1".
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| s.split('.').map(|n| n.trim().parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (pa, pb) = (parse(a), parse(b));
    for i in 0..pa.len().max(pb.len()) {
        let d = pa.get(i).unwrap_or(&0).cmp(pb.get(i).unwrap_or(&0));
        if d.is_ne() {
            return d;
        }
    }
    std::cmp::Ordering::Equal
}

/// Simple wildcard match: `*` is any run of characters, `?` one character. Case-insensitive.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(strip_bom(&text)).ok()
}

/// Writes JSON next to the target and renames it over, so a crash never leaves half a file.
pub fn write_json_atomic<T: serde::Serialize>(path: &Path, data: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(data)?)?;
    fs::rename(&tmp, path)
}

pub fn now_iso() -> String {
    // UTC, seconds precision, without a date crate.
    let secs = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A file or folder name no other call in this or another running launcher will get.
pub fn unique_name() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{}-{}", std::process::id(), nanos, N.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(safe_rel_path("a\\b/c.txt").unwrap(), "a/b/c.txt");
        for bad in ["", "/a", "C:/a", "a/../b", "a//b", "./a", "a/b:c"] {
            assert!(safe_rel_path(bad).is_err(), "{bad}");
        }
        let root = Path::new("C:\\game");
        assert_eq!(resolve_inside(root, "x/y.txt").unwrap(), Path::new("C:\\game\\x\\y.txt"));
    }

    #[test]
    fn versions_and_wildcards() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("0.10.0", "0.9.1"), Greater);
        assert_eq!(compare_versions("1.0", "1.0.0"), Equal);
        assert_eq!(compare_versions("1.2.3", "1.3"), Less);
        assert!(wildcard("pbz-he-*.zip", "PBZ-HE-0.2.0.zip"));
        assert!(wildcard("pbz-*", "pbz-0.1.0"));
        assert!(!wildcard("pbz-*.zip", "pbz-0.1.0.zip.sig"));
        assert!(wildcard("*", ""));
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_771), (2026, 11, 14));
    }
}
