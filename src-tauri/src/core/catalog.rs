//! The catalog (`catalog.json`: our games, how to find them, where their pack is), the GitHub releases a pack can
//! come from, and pack downloads.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::manifest::{is_id, is_sha256, Require};
use super::util::{self, compare_versions, wildcard};

pub const USER_AGENT: &str = concat!("Tirgumim-Launcher/", env!("CARGO_PKG_VERSION"));
/// A GitHub answer is reused this long (GitHub allows 60 questions an hour without a login).
pub const GITHUB_CACHE_SECS: u64 = 600;
/// Even "refresh now" doesn't ask GitHub again sooner than this.
pub const GITHUB_MIN_SECS: u64 = 30;
/// No release matches yet: the translation simply isn't published (not an error to show).
pub const NO_RELEASE: &str = "No release matches";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detect {
    #[serde(default)]
    pub steam_app_id: Option<String>,
    #[serde(default)]
    pub epic_app_name: Option<String>,
    /// A path under the game root that must exist: how a folder is confirmed to be this game.
    #[serde(default)]
    pub marker_file: Option<String>,
    /// The game's programs; while one runs, its files are not changed.
    #[serde(default)]
    pub process_names: Vec<String>,
}

/// Where a game's pack is. Either written out (`url` + `sha256` + `version`), or a GitHub repository whose newest
/// matching release is used (`github` + `tag` + `asset`), with the checksum GitHub publishes for the file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackSource {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub changelog: Option<String>,
    /// "owner/repo"
    #[serde(default)]
    pub github: Option<String>,
    /// Release tag pattern, like "pbz-*". Default: every tag.
    #[serde(default)]
    pub tag: Option<String>,
    /// Asset name pattern, like "pbz-he-*.zip". Default: "*.zip".
    #[serde(default)]
    pub asset: Option<String>,
    /// Also use pre-releases.
    #[serde(default)]
    pub prerelease: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogGame {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub name_he: Option<String>,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub detect: Detect,
    /// "inProgress" while the translation is being made.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub pack: Option<PackSource>,
    #[serde(default)]
    pub poster: Option<String>,
    #[serde(default)]
    pub hero: Option<String>,
    /// A short Hebrew line about the translation.
    #[serde(default)]
    pub about_he: Option<String>,
    /// Shown, and checked, before the pack is even downloaded.
    #[serde(default)]
    pub requires: Vec<Require>,
    /// Dev catalog only: the sample game's folder.
    #[serde(default)]
    pub dev_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    #[serde(default)]
    pub updated: Option<String>,
    pub games: Vec<CatalogGame>,
}

/// A pack ready to download: where, which version, and its checksum.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPack {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: Option<u64>,
    pub changelog: Option<String>,
    pub published_at: Option<String>,
    /// "catalog" or "github"
    pub source: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// Fetches an https:// or file:// URL as text.
pub fn fetch_text(url: &str) -> Result<String, String> {
    if url.starts_with("file:") {
        let p = url::Url::parse(url).map_err(|e| e.to_string())?.to_file_path().map_err(|_| format!("Bad file URL {url}"))?;
        return fs::read_to_string(p).map_err(|e| e.to_string());
    }
    if !url.starts_with("https://") {
        return Err(format!("Only https:// is allowed: {url}"));
    }
    agent()
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("Cache-Control", "no-cache")
        .call()
        .map_err(|e| format!("{url}: {e}"))?
        .body_mut()
        .with_config()
        .limit(20 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

pub fn parse_catalog(text: &str, base_url: &str) -> Result<Catalog, String> {
    let mut c: Catalog = serde_json::from_str(util::strip_bom(text)).map_err(|e| format!("Catalog is not valid: {e}"))?;
    if c.schema != 1 {
        return Err(format!("Catalog schema {} not supported", c.schema));
    }
    let base = url::Url::parse(base_url).ok();
    let join = |u: &mut Option<String>| {
        if let (Some(s), Some(b)) = (u.as_ref(), base.as_ref()) {
            if let Ok(j) = b.join(s) {
                *u = Some(j.to_string());
            }
        }
    };
    let mut ids = std::collections::HashSet::new();
    for g in &mut c.games {
        if !is_id(&g.id) || !ids.insert(g.id.clone()) {
            return Err(format!("Bad or repeated game id in catalog: {}", g.id));
        }
        if let Some(p) = &mut g.pack {
            join(&mut p.url);
            if p.sha256.as_deref().is_some_and(|h| !is_sha256(h)) {
                return Err(format!("Bad sha256 for {}", g.id));
            }
            if p.github.as_deref().is_some_and(|r| r.split('/').count() != 2 || r.contains(char::is_whitespace)) {
                return Err(format!("Bad github repository for {}", g.id));
            }
        }
        join(&mut g.poster);
        join(&mut g.hero);
    }
    Ok(c)
}

pub fn load_catalog(url: &str) -> Result<Catalog, String> {
    parse_catalog(&fetch_text(url)?, url)
}

// ---------- GitHub releases ----------

/// The rightmost run of digits and dots in a tag: "pbz-0.2.1" → "0.2.1", "v1.0" → "1.0".
pub fn version_from_tag(tag: &str) -> Option<String> {
    let b = tag.as_bytes();
    let mut end = b.len();
    while end > 0 && !b[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let mut start = end;
    while start > 0 && (b[start - 1].is_ascii_digit() || b[start - 1] == b'.') {
        start -= 1;
    }
    let v = tag[start..end].trim_matches('.');
    (!v.is_empty()).then(|| v.to_string())
}

/// A release can say which launcher it needs with a line `tirgumim-min-launcher: 0.4.0` in its notes. Launchers
/// older than that skip it and take the newest release they can install (from launcher 0.4.0 on; 0.3.0 takes the
/// newest and says "update the launcher").
pub const MIN_LAUNCHER_MARK: &str = "tirgumim-min-launcher:";

pub fn release_min_launcher(body: &str) -> Option<String> {
    body.lines().find_map(|l| {
        let l = l.trim().trim_start_matches(['-', '*', '`']).trim();
        l.to_lowercase().strip_prefix(MIN_LAUNCHER_MARK).map(|v| v.trim().trim_end_matches('`').trim().to_string()).filter(|v| !v.is_empty())
    })
}

/// Picks the newest release that matches `src` from GitHub's release list, among those this launcher can install.
pub fn pick_release(releases: &[Value], src: &PackSource) -> Result<ResolvedPack, String> {
    pick_release_for(releases, src, env!("CARGO_PKG_VERSION"))
}

pub fn pick_release_for(releases: &[Value], src: &PackSource, launcher_version: &str) -> Result<ResolvedPack, String> {
    let tag_pat = src.tag.as_deref().unwrap_or("*");
    let asset_pat = src.asset.as_deref().unwrap_or("*.zip");
    let mut best: Option<(String, &Value, &Value)> = None;
    let mut too_new: Option<String> = None;
    for r in releases {
        let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("");
        let b = |k: &str| r.get(k).and_then(Value::as_bool).unwrap_or(false);
        if b("draft") || (b("prerelease") && !src.prerelease) || !wildcard(tag_pat, s("tag_name")) {
            continue;
        }
        if let Some(min) = release_min_launcher(s("body")).filter(|m| compare_versions(launcher_version, m).is_lt()) {
            too_new.get_or_insert(min);
            continue;
        }
        let Some(version) = version_from_tag(s("tag_name")) else { continue };
        let Some(asset) = r.get("assets").and_then(Value::as_array).and_then(|a| {
            a.iter().find(|x| wildcard(asset_pat, x.get("name").and_then(Value::as_str).unwrap_or("")))
        }) else {
            continue;
        };
        if best.as_ref().is_none_or(|(v, _, _)| compare_versions(&version, v).is_gt()) {
            best = Some((version, r, asset));
        }
    }
    let (version, rel, asset) = match best {
        Some(b) => b,
        None => return Err(too_new.map(|m| format!("Needs launcher {m} or newer")).unwrap_or_else(|| NO_RELEASE.into())),
    };
    let url = asset.get("browser_download_url").and_then(Value::as_str).unwrap_or("").to_string();
    if !url.starts_with("https://") {
        return Err("Release file has no https download".into());
    }
    let sha256 = asset
        .get("digest")
        .and_then(Value::as_str)
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| is_sha256(h))
        .map(str::to_lowercase)
        .ok_or("GitHub gave no checksum for the release file")?;
    let body = rel.get("body").and_then(Value::as_str).map(|b| b.trim().chars().take(600).collect::<String>()).filter(|b| !b.is_empty());
    Ok(ResolvedPack {
        version,
        url,
        sha256,
        size: asset.get("size").and_then(Value::as_u64),
        changelog: src.changelog.clone().or(body),
        published_at: rel.get("published_at").and_then(Value::as_str).map(str::to_string),
        source: "github".into(),
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedReleases {
    fetched_at: u64,
    releases: Vec<Value>,
}

/// GitHub's release lists, kept in `sources.json` so each repository is asked at most every ten minutes.
pub struct ReleaseCache {
    file: PathBuf,
    /// Tests point this at a local file:// URL.
    pub api_base: String,
}

impl ReleaseCache {
    pub fn new(data_dir: &Path) -> Self {
        ReleaseCache { file: data_dir.join("sources.json"), api_base: "https://api.github.com".into() }
    }

    fn trim(r: &Value) -> Value {
        let pick = |v: &Value, keys: &[&str]| {
            Value::Object(keys.iter().filter_map(|k| v.get(*k).map(|x| (k.to_string(), x.clone()))).collect())
        };
        let mut t = pick(r, &["tag_name", "name", "draft", "prerelease", "body", "published_at"]);
        let assets: Vec<Value> = r
            .get("assets")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| pick(x, &["name", "size", "browser_download_url", "digest"])).collect())
            .unwrap_or_default();
        t["assets"] = Value::Array(assets);
        t
    }

    /// The release list for `repo`: from the cache while it is fresh (or `force` and it is very fresh), else from
    /// GitHub; if GitHub can't be reached, the last list we had.
    pub fn releases(&self, repo: &str, force: bool) -> Result<Vec<Value>, String> {
        let mut all: BTreeMap<String, CachedReleases> = util::read_json(&self.file).unwrap_or_default();
        let now = util::now_secs();
        if let Some(c) = all.get(repo) {
            let age = now.saturating_sub(c.fetched_at);
            if age < if force { GITHUB_MIN_SECS } else { GITHUB_CACHE_SECS } {
                return Ok(c.releases.clone());
            }
        }
        let url = format!("{}/repos/{repo}/releases?per_page=50", self.api_base.trim_end_matches('/'));
        let fetched = fetch_text(&url).and_then(|t| serde_json::from_str::<Vec<Value>>(&t).map_err(|e| e.to_string()));
        match fetched {
            Ok(list) => {
                let releases: Vec<Value> = list.iter().map(Self::trim).collect();
                all.insert(repo.into(), CachedReleases { fetched_at: now, releases: releases.clone() });
                let _ = util::write_json_atomic(&self.file, &all);
                Ok(releases)
            }
            Err(e) => all.get(repo).map(|c| c.releases.clone()).ok_or(e),
        }
    }

    /// Where a game's pack is, its version and checksum.
    pub fn resolve(&self, src: &PackSource, force: bool) -> Result<ResolvedPack, String> {
        if let Some(repo) = &src.github {
            return pick_release(&self.releases(repo, force)?, src);
        }
        match (&src.url, &src.sha256, &src.version) {
            (Some(url), Some(sha), Some(v)) => Ok(ResolvedPack {
                version: v.clone(),
                url: url.clone(),
                sha256: sha.to_lowercase(),
                size: src.size,
                changelog: src.changelog.clone(),
                published_at: None,
                source: "catalog".into(),
            }),
            _ => Err("Pack entry needs url, sha256 and version, or github".into()),
        }
    }
}

// ---------- downloads ----------

/// Downloads a pack into `dest_dir` and returns its path. A file:// pack (the dev catalog) is used where it is.
pub fn download(url: &str, dest_dir: &Path, progress: &mut dyn FnMut(Option<f64>)) -> Result<PathBuf, String> {
    if url.starts_with("file:") {
        return url::Url::parse(url).map_err(|e| e.to_string())?.to_file_path().map_err(|_| format!("Bad file URL {url}"));
    }
    if !url.starts_with("https://") {
        return Err("Packs are downloaded only over https".into());
    }
    fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let out = dest_dir.join(format!("{}.zip.part", util::unique_name()));
    let res = (|| {
        let resp = agent().get(url).call().map_err(|e| format!("Download failed: {e}"))?;
        let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
        let mut reader = resp.into_body().into_reader();
        let mut w = fs::File::create(&out).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut got = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            got += n as u64;
            progress(total.filter(|t| *t > 0).map(|t| got as f64 / t as f64));
        }
        Ok(())
    })();
    match res {
        Ok(()) => {
            let done = out.with_extension("");
            fs::rename(&out, &done).map_err(|e| e.to_string())?;
            Ok(done)
        }
        Err(e) => {
            let _ = fs::remove_file(&out);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tags() {
        assert_eq!(version_from_tag("pbz-0.2.1").as_deref(), Some("0.2.1"));
        assert_eq!(version_from_tag("v1.0").as_deref(), Some("1.0"));
        assert_eq!(version_from_tag("pbz2-0.3-final").as_deref(), Some("0.3"));
        assert_eq!(version_from_tag("latest"), None);
    }

    fn rel(tag: &str, asset: &str, digest: Option<&str>, pre: bool) -> Value {
        let mut a = json!({ "name": asset, "size": 10, "browser_download_url": format!("https://github.com/x/y/releases/download/{tag}/{asset}") });
        if let Some(d) = digest {
            a["digest"] = json!(format!("sha256:{d}"));
        }
        json!({ "tag_name": tag, "draft": false, "prerelease": pre, "body": "notes", "assets": [a] })
    }

    #[test]
    fn picks_newest_matching_release() {
        let h = "a".repeat(64);
        let list = vec![
            rel("pbz-0.9.0", "pbz-he-0.9.0.zip", Some(&h), false),
            rel("pbz-0.10.0", "pbz-he-0.10.0.zip", Some(&h), false),
            rel("pbz-0.11.0", "pbz-he-0.11.0.zip", Some(&h), true),
            rel("other-5.0", "other-5.0.zip", Some(&h), false),
        ];
        let src = PackSource { github: Some("x/y".into()), tag: Some("pbz-*".into()), asset: Some("pbz-he-*.zip".into()), ..Default::default() };
        let p = pick_release(&list, &src).unwrap();
        assert_eq!(p.version, "0.10.0");
        assert_eq!(p.sha256, h);
        assert_eq!(p.changelog.as_deref(), Some("notes"));
        let pre = PackSource { prerelease: true, ..src.clone() };
        assert_eq!(pick_release(&list, &pre).unwrap().version, "0.11.0");
        // no checksum from GitHub: refused
        let nodigest = vec![rel("pbz-1.0.0", "pbz-he-1.0.0.zip", None, false)];
        assert!(pick_release(&nodigest, &src).is_err());

        // A release that needs a newer launcher is skipped: the newest one this launcher can install is used.
        let mut needs = rel("pbz-0.12.0", "pbz-he-0.12.0.zip", Some(&h), false);
        needs["body"] = json!("מה חדש\n\n`tirgumim-min-launcher: 0.4.0`");
        let mixed = vec![rel("pbz-0.11.0", "pbz-he-0.11.0.zip", Some(&h), false), needs];
        assert_eq!(pick_release_for(&mixed, &src, "0.3.0").unwrap().version, "0.11.0");
        assert_eq!(pick_release_for(&mixed, &src, "0.4.0").unwrap().version, "0.12.0");
        let only_new = vec![mixed[1].clone()];
        assert_eq!(pick_release_for(&only_new, &src, "0.3.0").unwrap_err(), "Needs launcher 0.4.0 or newer");
        assert_eq!(release_min_launcher("x\ntirgumim-min-launcher: 0.4.0\ny").as_deref(), Some("0.4.0"));
        assert_eq!(release_min_launcher("nothing here"), None);
    }

    #[test]
    fn release_cache_reuses_answers() {
        let dir = tempfile::tempdir().unwrap();
        let api = dir.path().join("api");
        let list_file = api.join("repos").join("x").join("y");
        fs::create_dir_all(&list_file).unwrap();
        let h = "b".repeat(64);
        let body = serde_json::to_string(&vec![rel("pbz-0.1.0", "pbz-he-0.1.0.zip", Some(&h), false)]).unwrap();
        fs::write(list_file.join("releases"), &body).unwrap();
        // file:// stands in for api.github.com (the "?per_page=50" query isn't part of the file path)
        let mut cache = ReleaseCache::new(dir.path());
        cache.api_base = url::Url::from_directory_path(&api).unwrap().to_string();
        let src = PackSource { github: Some("x/y".into()), ..Default::default() };
        assert_eq!(cache.releases("x/y", false).unwrap().len(), 1);
        // GitHub now "down": the cached list is still used
        fs::remove_file(list_file.join("releases")).unwrap();
        assert_eq!(cache.resolve(&src, false).unwrap().version, "0.1.0");
        assert_eq!(cache.resolve(&src, true).unwrap().version, "0.1.0");
    }
}
