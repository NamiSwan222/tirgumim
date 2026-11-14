//! The pack manifest (`tirgumim-pack.json`): its fields, its checks, and the game-version and required-file checks.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::util::{compare_versions, resolve_inside, safe_rel_path, sha256_file_cached};

pub const MANIFEST_NAME: &str = "tirgumim-pack.json";
pub const SCHEMA: u32 = 1;
/// The first launcher that applies `patches`. Older ones ignore the field, so a pack with patches must require this.
pub const PATCHES_MIN_LAUNCHER: &str = "0.3.0";
/// The first launcher with launch-only packs (`activation`) and `append` patches.
pub const LAUNCH_MIN_LAUNCHER: &str = "0.4.0";
/// The launcher's own folder inside a game (swap mode keeps the Hebrew and original files there). Packs can't use it.
pub const OWN_DIR: &str = ".tirgumim";

/// How a launch-only pack turns on. Without `activation` a pack is permanent (installed into the game for good).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivationMode {
    /// The pack only adds files; the game is started with arguments that point at them.
    Args,
    /// The Hebrew files are swapped in right before the game starts and back out when it exits.
    Swap,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    /// `steam.exe -applaunch <steamAppId> <args>`: Steam stays in charge (overlay, playtime, DRM).
    #[default]
    Steam,
    /// The game's exe straight away.
    Direct,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activation {
    pub mode: ActivationMode,
    /// The game's program, relative to the game root (started with `via: direct`; always checked to exist).
    pub exe: String,
    /// Passed to the game; `{gameDir}` becomes the game folder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// `{gameDir}` or a folder under it; default the exe's folder. Direct launches only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub via: Via,
    /// Environment variables for the game's process (`{gameDir}` filled in), e.g. `TIRGUMIM_LANG=he` for a proxy DLL
    /// that does nothing without it. Steam would drop them, so these packs start the game with `via: direct`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// `via: direct` only: Steam must be running first (the game uses Steam but doesn't start it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_steam: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchKind {
    /// A bsdiff (BSDIFF40) patch, applied in memory.
    #[default]
    Bsdiff,
    /// Bytes added at the end of the file (swap packs only), undone by cutting the file back to `fromSize`.
    Append,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// A game file changed in place with a bsdiff patch (BSDIFF40) instead of replaced, for big files such as a
/// GameMaker `data.win`. The game's file must be exactly `from_sha256`; the result must be `to_sha256`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackPatch {
    pub path: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub kind: PatchKind,
    pub from_sha256: String,
    pub to_sha256: String,
    /// The patched file's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// bsdiff: the patch inside the pack zip, under `patches/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// append: the bytes to add, inside the pack zip under `patches/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// append: the original file's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_size: Option<u64>,
    /// args packs: the patched file is written here (a new file) and `path` stays untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl PackPatch {
    /// Where the patched result goes in the game: `output` for args packs, else `path` itself.
    pub fn target(&self) -> &str {
        self.output.as_deref().unwrap_or(&self.path)
    }
    /// The zip entry this patch reads.
    pub fn zip_entry(&self) -> &str {
        self.patch.as_deref().or(self.data.as_deref()).unwrap_or("")
    }
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
    #[serde(default)]
    pub files: Vec<PackFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<PackPatch>,
    /// Launch-only: Hebrew only when the game is started from the launcher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<Activation>,
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
    if m.files.is_empty() && m.patches.is_empty() {
        errs.push("\"files\" and \"patches\" must not both be empty".into());
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
    let min = m.min_launcher_version.clone().unwrap_or_default();
    let needs = |v: &str| min.is_empty() || compare_versions(&min, v).is_lt();
    let launch_features = m.activation.is_some() || m.patches.iter().any(|p| p.kind == PatchKind::Append || p.output.is_some());
    if launch_features && needs(LAUNCH_MIN_LAUNCHER) {
        errs.push(format!("a pack with \"activation\", append patches or patch \"output\" needs \"minLauncherVersion\" {LAUNCH_MIN_LAUNCHER} or newer"));
    } else if !m.patches.is_empty() && needs(PATCHES_MIN_LAUNCHER) {
        errs.push(format!("a pack with \"patches\" needs \"minLauncherVersion\" {PATCHES_MIN_LAUNCHER} or newer"));
    }
    let mode = m.activation.as_ref().map(|a| a.mode);
    if let Some(a) = &mut m.activation {
        match safe_rel_path(&a.exe) {
            Ok(p) => a.exe = p,
            Err(e) => errs.push(format!("activation exe: {e}")),
        }
        if a.working_dir.as_deref().is_some_and(|w| !w.starts_with("{gameDir}") || w.contains("..")) {
            errs.push("activation workingDir must start with {gameDir}".into());
        }
        for arg in a.args.iter().chain(a.env.values()) {
            let rest = arg.replace("{gameDir}", "");
            if rest.contains('{') || rest.contains('}') {
                errs.push(format!("activation value {arg:?}: only {{gameDir}} can be filled in"));
            }
        }
        for k in a.env.keys() {
            if k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                errs.push(format!("activation env name {k:?} must be letters, digits and _"));
            }
        }
        if !a.env.is_empty() && a.via != Via::Direct {
            errs.push("activation env needs \"via\": \"direct\" (Steam does not pass environment variables to the game)".into());
        }
        if a.needs_steam && a.via != Via::Direct {
            errs.push("\"needsSteam\" is for \"via\": \"direct\" (via steam always has Steam)".into());
        }
        if a.mode == ActivationMode::Args && a.args.is_empty() && a.env.is_empty() {
            errs.push("an args pack needs \"args\" or \"env\" that point the game at its files".into());
        }
    }
    let mut zip_names = std::collections::HashSet::new();
    for p in &mut m.patches {
        match safe_rel_path(&p.path) {
            Ok(rel) => {
                // An args pack writes a new file and leaves `path` alone, so only its `output` is taken.
                if p.output.is_none() && !seen.insert(rel.to_lowercase()) {
                    errs.push(format!("duplicate path {rel} (a path is either in files or in patches, once)"));
                }
                p.path = rel;
            }
            Err(e) => errs.push(e),
        }
        if let Some(o) = p.output.clone() {
            match safe_rel_path(&o) {
                Ok(rel) => {
                    if rel.eq_ignore_ascii_case(&p.path) {
                        errs.push(format!("patch output for {} must be a new file", p.path));
                    } else if !seen.insert(rel.to_lowercase()) {
                        errs.push(format!("duplicate path {rel}"));
                    }
                    p.output = Some(rel);
                }
                Err(e) => errs.push(e),
            }
            if mode != Some(ActivationMode::Args) {
                errs.push(format!("patch \"output\" ({}) is for args packs", p.path));
            }
        } else if mode == Some(ActivationMode::Args) {
            errs.push(format!("in an args pack, the patch for {} needs \"output\" (a new file), so the original stays untouched", p.path));
        }
        let field = if p.kind == PatchKind::Bsdiff { "patch" } else { "data" };
        let other = if p.kind == PatchKind::Bsdiff { p.data.is_some() } else { p.patch.is_some() };
        if other {
            errs.push(format!("{field} patch for {} must use \"{field}\" only", p.path));
        }
        let entry = if p.kind == PatchKind::Bsdiff { &mut p.patch } else { &mut p.data };
        match entry.as_deref().map(safe_rel_path) {
            Some(Ok(rel)) if rel.starts_with("patches/") && rel.len() > "patches/".len() => {
                if !zip_names.insert(rel.to_lowercase()) {
                    errs.push(format!("patch file {rel} is used twice"));
                }
                *entry = Some(rel);
            }
            Some(Ok(rel)) => errs.push(format!("patch file {rel} must be under patches/")),
            Some(Err(e)) => errs.push(e),
            None => errs.push(format!("patch for {} needs \"{field}\"", p.path)),
        }
        if p.kind == PatchKind::Append {
            if p.from_size.is_none() {
                errs.push(format!("append patch for {} needs \"fromSize\"", p.path));
            }
            // Swap: added to the game's file during play. Args: a new file = the original + the data.
            if mode.is_none() {
                errs.push(format!("append patch for {} is for launch-only packs (swap, or args with \"output\")", p.path));
            }
        }
        for (k, h) in [("fromSha256", &mut p.from_sha256), ("toSha256", &mut p.to_sha256)] {
            if !is_sha256(h) {
                errs.push(format!("bad {k} for patch {}", p.path));
            }
            *h = h.to_lowercase();
        }
        if p.from_sha256 == p.to_sha256 {
            errs.push(format!("patch for {} changes nothing", p.path));
        }
    }
    let own = format!("{OWN_DIR}/");
    if m.files.iter().map(|f| f.path.as_str()).chain(m.patches.iter().map(|p| p.target())).any(|p| p.to_lowercase().starts_with(&own)) {
        errs.push(format!("{OWN_DIR}/ in the game folder belongs to the launcher"));
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
