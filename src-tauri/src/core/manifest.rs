//! The pack manifest (`tirgumim-pack.json`): its fields, its checks, and the game-version and required-file checks.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::util::{compare_versions, resolve_inside, safe_rel_path, sha256_file_cached};

pub const MANIFEST_NAME: &str = "tirgumim-pack.json";
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Only that the file is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVersion {
    #[serde(default)]
    pub name: String,
    pub checks: Vec<Check>,
}

/// A file or folder the game folder must have (a mod loader the pack needs), or must not have (`absent`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Require {
    pub path: String,
    /// Shown to the player while the condition isn't met (Hebrew).
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
    /// An https page with the fix (a loader's download page), shown as a button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u32,
    pub pack_id: String,
    pub game_id: String,
    pub version: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_launcher_version: Option<String>,
    #[serde(default)]
    pub requires_account: bool,
    #[serde(default)]
    pub game_versions: Vec<GameVersion>,
    #[serde(default)]
    pub allow_unknown_game_version: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Require>,
    pub files: Vec<PackFile>,
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn is_id(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(ch) if ch.is_ascii_alphanumeric()) && c.all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch))
}

/// Parses and checks a manifest. Paths come back normalised and hashes lower-case.
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let m: Manifest = serde_json::from_str(super::util::strip_bom(text)).map_err(|e| format!("Manifest is not valid: {e}"))?;
    validate_manifest(m)
}

pub fn validate_manifest(mut m: Manifest) -> Result<Manifest, String> {
    let mut errs = Vec::new();
    if m.schema != SCHEMA {
        errs.push(format!("unsupported schema {} (expected {SCHEMA})", m.schema));
    }
    if !is_id(&m.pack_id) {
        errs.push("bad \"packId\"".into());
    }
    if !is_id(&m.game_id) {
        errs.push("bad \"gameId\"".into());
    }
    if m.version.trim().is_empty() {
        errs.push("missing \"version\"".into());
    }
    if m.files.is_empty() {
        errs.push("\"files\" must not be empty".into());
    }
    let mut seen = std::collections::HashSet::new();
    for f in &mut m.files {
        match safe_rel_path(&f.path) {
            Ok(p) => {
                // Windows paths are case-insensitive.
                if !seen.insert(p.to_lowercase()) {
                    errs.push(format!("duplicate path {p}"));
                }
                if p.eq_ignore_ascii_case(MANIFEST_NAME) {
                    errs.push("a pack file can't be named like the manifest".into());
                }
                f.path = p;
            }
            Err(e) => errs.push(e),
        }
        if !is_sha256(&f.sha256) {
            errs.push(format!("bad sha256 for {}", f.path));
        }
        f.sha256 = f.sha256.to_lowercase();
    }
    for gv in &mut m.game_versions {
        if gv.checks.is_empty() {
            errs.push(format!("gameVersion \"{}\" has no checks", gv.name));
        }
        for c in &mut gv.checks {
            match safe_rel_path(&c.path) {
                Ok(p) => c.path = p,
                Err(e) => errs.push(e),
            }
            if let Some(h) = &c.sha256 {
                if !is_sha256(h) {
                    errs.push(format!("bad sha256 in check for {}", c.path));
                }
                c.sha256 = Some(h.to_lowercase());
            }
        }
    }
    for r in &mut m.requires {
        match safe_rel_path(&r.path) {
            Ok(p) => r.path = p,
            Err(e) => errs.push(e),
        }
        if r.url.as_deref().is_some_and(|u| !u.starts_with("https://")) {
            errs.push(format!("requires url for {} must be https", r.path));
        }
    }
    if errs.is_empty() {
        Ok(m)
    } else {
        Err(format!("Invalid pack manifest: {}", errs.join("; ")))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMatch {
    pub ok: bool,
    pub matched: Option<String>,
    pub any_version: bool,
    pub allow_unknown: bool,
}

fn check_passes(game_dir: &Path, c: &Check) -> bool {
    let Ok(abs) = resolve_inside(game_dir, &c.path) else { return false };
    let Ok(meta) = std::fs::metadata(&abs) else { return false };
    if c.size.is_some_and(|s| s != meta.len()) {
        return false;
    }
    if let Some(h) = &c.sha256 {
        if meta.is_dir() || sha256_file_cached(&abs).ok().as_deref() != Some(h.as_str()) {
            return false;
        }
    }
    true
}

/// Which of the builds the pack lists this game folder is. A build matches when every one of its checks passes.
pub fn check_game_version(m: &Manifest, game_dir: &Path) -> VersionMatch {
    if m.game_versions.is_empty() {
        return VersionMatch { ok: true, matched: None, any_version: true, allow_unknown: true };
    }
    for gv in &m.game_versions {
        if gv.checks.iter().all(|c| check_passes(game_dir, c)) {
            return VersionMatch { ok: true, matched: Some(gv.name.clone()), any_version: false, allow_unknown: m.allow_unknown_game_version };
        }
    }
    VersionMatch { ok: false, matched: None, any_version: false, allow_unknown: m.allow_unknown_game_version }
}

pub fn check_launcher_version(m: &Manifest, launcher_version: &str) -> bool {
    m.min_launcher_version.as_deref().is_none_or(|min| compare_versions(launcher_version, min).is_ge())
}

/// The requirements the game folder doesn't meet.
pub fn unmet_requires<'a>(reqs: &'a [Require], game_dir: &Path) -> Vec<&'a Require> {
    reqs.iter()
        .filter(|r| {
            let there = resolve_inside(game_dir, &r.path).map(|p| p.exists()).unwrap_or(false);
            there == r.absent
        })
        .collect()
}
