//! Installs and removes translation packs with a backup of every game file they replace.
//!
//! Everything the launcher keeps lives under `data_dir`:
//! - `state.json`: install records, folders the player chose, settings
//! - `backups/<gameId>/...`: the game's original files a pack replaced
//! - `tmp/`: where a pack is unpacked and checked before anything touches the game
//!
//! `state.json` has the same shape the Electron launcher wrote, so its installs carry over.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::manifest::{self, Activation, ActivationMode, Manifest, PatchKind, VersionMatch, MANIFEST_NAME};
use super::swap::{self, SwapItem, SwapKind};
use super::util::{self, compare_versions, resolve_inside, sha256_file, sha256_file_cached};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub allow_unknown: bool,
    /// For REQUIRES: the messages of what the game folder is missing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
}

impl PackError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        PackError { code: code.into(), message: message.into(), allow_unknown: false, details: vec![] }
    }
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl From<std::io::Error> for PackError {
    fn from(e: std::io::Error) -> Self {
        PackError::new("IO", e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, PackError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordFile {
    pub path: String,
    pub sha256: String,
    pub backed_up: bool,
    /// Changed in place by a bsdiff patch (the original is in the backup).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub patched: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRecord {
    pub pack_id: String,
    pub version: String,
    #[serde(default)]
    pub title: String,
    pub game_dir: String,
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub forced_unknown_version: bool,
    pub installed_at: String,
    pub files: Vec<RecordFile>,
    #[serde(default)]
    pub created_dirs: Vec<String>,
    /// Launch-only packs: how the game is started in Hebrew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<Activation>,
    /// Swap packs: what is put in place while the game runs from the launcher.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swap: Vec<SwapItem>,
    /// Installed from a paid pack (a download grant from the account server). Play and update stay gated by the
    /// account even if a later catalog says the game is free.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paid: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default)]
    pub installs: BTreeMap<String, InstallRecord>,
    #[serde(default)]
    pub custom_paths: BTreeMap<String, String>,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    pub restored: Vec<String>,
    pub deleted: Vec<String>,
    /// Files the game replaced since the install (a game update): left as the game has them.
    pub skipped_changed: Vec<String>,
    pub missing_backup: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verify {
    pub intact: bool,
    pub changed: Vec<String>,
}

pub struct InstallOptions<'a> {
    pub game_id: &'a str,
    pub game_dir: &'a Path,
    pub zip_path: &'a Path,
    pub expected_sha256: Option<&'a str>,
    /// Install on a game build the pack doesn't list, when the pack allows it.
    pub force: bool,
}

pub struct Installer {
    pub data_dir: PathBuf,
    pub launcher_version: String,
    state_file: PathBuf,
}

fn check_game_id(id: &str) -> Result<()> {
    if manifest::is_id(id) {
        Ok(())
    } else {
        Err(PackError::new("BAD_GAME_ID", format!("Invalid game id: {id}")))
    }
}

pub struct OpenPack {
    pub manifest: Manifest,
    archive: zip::ZipArchive<fs::File>,
}

impl Installer {
    pub fn new(data_dir: impl Into<PathBuf>, launcher_version: &str) -> Self {
        let data_dir = data_dir.into();
        let _ = fs::create_dir_all(&data_dir);
        Installer { state_file: data_dir.join("state.json"), data_dir, launcher_version: launcher_version.into() }
    }

    // ---------- state ----------
    pub fn load_state(&self) -> State {
        util::read_json(&self.state_file).unwrap_or_default()
    }
    pub fn save_state(&self, s: &State) -> Result<()> {
        Ok(util::write_json_atomic(&self.state_file, s)?)
    }
    pub fn record(&self, game_id: &str) -> Option<InstallRecord> {
        self.load_state().installs.get(game_id).cloned()
    }
    /// Marks an install as coming from a paid pack (see `InstallRecord::paid`).
    pub fn mark_paid(&self, game_id: &str) -> Result<()> {
        let mut s = self.load_state();
        if let Some(r) = s.installs.get_mut(game_id) {
            r.paid = true;
            self.save_state(&s)?;
        }
        Ok(())
    }
    pub fn set_custom_path(&self, game_id: &str, dir: Option<&str>) -> Result<()> {
        check_game_id(game_id)?;
        let mut s = self.load_state();
        match dir {
            Some(d) => s.custom_paths.insert(game_id.into(), d.into()),
            None => s.custom_paths.remove(game_id),
        };
        self.save_state(&s)
    }
    pub fn backup_dir(&self, game_id: &str) -> PathBuf {
        self.data_dir.join("backups").join(game_id)
    }

    // ---------- packs ----------
    /// Opens a pack zip and checks its manifest. Checks the zip's SHA-256 first when one is given.
    pub fn open_pack(&self, zip_path: &Path, expected_sha256: Option<&str>) -> Result<OpenPack> {
        if let Some(want) = expected_sha256 {
            if sha256_file(zip_path)? != want.to_lowercase() {
                return Err(PackError::new("ZIP_HASH", "Pack file is corrupt (hash mismatch)"));
            }
        }
        let file = fs::File::open(zip_path)?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| PackError::new("ZIP_READ", format!("Cannot read pack zip: {e}")))?;
        let text = {
            let mut entry = archive.by_name(MANIFEST_NAME).map_err(|_| PackError::new("NO_MANIFEST", format!("Pack has no {MANIFEST_NAME}")))?;
            let mut s = String::new();
            entry.read_to_string(&mut s).map_err(|e| PackError::new("BAD_MANIFEST", e.to_string()))?;
            s
        };
        // A pack for a newer launcher may use fields or modes this one doesn't know: say "update the launcher",
        // not "broken pack", so a future game never looks like an error.
        if let Some(min) = serde_json::from_str::<serde_json::Value>(util::strip_bom(&text))
            .ok()
            .and_then(|v| v.get("minLauncherVersion").and_then(|m| m.as_str()).map(str::to_string))
        {
            if compare_versions(&self.launcher_version, &min).is_lt() {
                return Err(PackError::new("LAUNCHER_TOO_OLD", format!("Pack needs launcher {min} or newer")));
            }
        }
        let manifest = manifest::parse_manifest(&text).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
        Ok(OpenPack { manifest, archive })
    }

    /// Unpacks the files the manifest lists into a temporary folder and checks each one's hash. Patched files are
    /// made there too, from the game's originals, and checked against `toSha256`. The game folder isn't touched.
    fn extract_verified(&self, pack: &mut OpenPack, game_dir: &Path, progress: &mut dyn FnMut(&str)) -> Result<PathBuf> {
        let tmp = self.data_dir.join("tmp").join(util::unique_name());
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp)?;
        let res = (|| {
            let mut by_name = std::collections::HashMap::new();
            for i in 0..pack.archive.len() {
                if let Some(Ok(name)) = pack.archive.name_for_index(i) {
                    by_name.insert(name.replace('\\', "/"), i);
                }
            }
            for f in &pack.manifest.files {
                let idx = by_name.get(&format!("files/{}", f.path)).copied();
                let mut entry = match idx.map(|i| pack.archive.by_index(i)) {
                    Some(Ok(e)) if !e.is_dir() => e,
                    _ => return Err(PackError::new("MISSING_FILE", format!("Pack is missing file {}", f.path))),
                };
                let out = resolve_inside(&tmp, &f.path).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                fs::create_dir_all(out.parent().unwrap())?;
                let mut w = fs::File::create(&out)?;
                let mut h = Sha256::new();
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let n = entry.read(&mut buf).map_err(|e| PackError::new("ZIP_READ", e.to_string()))?;
                    if n == 0 {
                        break;
                    }
                    h.update(&buf[..n]);
                    w.write_all(&buf[..n])?;
                }
                if util::hex(&h.finalize()) != f.sha256 {
                    return Err(PackError::new("FILE_HASH", format!("File {} in pack is corrupt", f.path)));
                }
            }
            if !pack.manifest.patches.is_empty() {
                progress("patch");
            }
            for p in &pack.manifest.patches {
                let mut entry = match by_name.get(p.zip_entry()).copied().map(|i| pack.archive.by_index(i)) {
                    Some(Ok(e)) if !e.is_dir() => e,
                    _ => return Err(PackError::new("MISSING_FILE", format!("Pack is missing patch {}", p.zip_entry()))),
                };
                let original = resolve_inside(game_dir, &p.path).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                if p.kind == PatchKind::Append && p.output.is_some() {
                    // Args pack: a new file = the original followed by the data, streamed (no RAM), hashed as written.
                    let dest = resolve_inside(&tmp, p.target()).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                    fs::create_dir_all(dest.parent().unwrap())?;
                    let mut w = HashWriter { inner: io::BufWriter::new(fs::File::create(&dest)?), hash: Sha256::new() };
                    io::copy(&mut fs::File::open(&original)?, &mut w)?;
                    io::copy(&mut entry, &mut w).map_err(|e| PackError::new("ZIP_READ", e.to_string()))?;
                    w.inner.flush()?;
                    if util::hex(&w.hash.finalize()) != p.to_sha256 {
                        return Err(PackError::new("PATCH_FAILED", format!("Patch for {} gave the wrong result", p.path)));
                    }
                    continue;
                }
                if p.kind == PatchKind::Append {
                    // Kept as it is (the bytes to add); the result is checked without writing a 1 GB file:
                    // the hash of the original followed by these bytes must be toSha256.
                    let dest = resolve_inside(&tmp, &swap::staged_rel(&p.path, SwapKind::Append)).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                    fs::create_dir_all(dest.parent().unwrap())?;
                    io::copy(&mut entry, &mut fs::File::create(&dest)?).map_err(|e| PackError::new("ZIP_READ", e.to_string()))?;
                    let mut h = Sha256::new();
                    io::copy(&mut fs::File::open(&original)?, &mut h)?;
                    io::copy(&mut fs::File::open(&dest)?, &mut h)?;
                    if util::hex(&h.finalize()) != p.to_sha256 {
                        return Err(PackError::new("PATCH_FAILED", format!("Patch for {} gave the wrong result", p.path)));
                    }
                    continue;
                }
                let mut diff = Vec::new();
                entry.read_to_end(&mut diff).map_err(|e| PackError::new("ZIP_READ", e.to_string()))?;
                drop(entry);
                let src = fs::read(&original)?;
                let out = apply_patch(&src, &diff).map_err(|e| PackError::new("PATCH_FAILED", format!("Patch for {} failed: {e}", p.path)))?;
                drop(src);
                if util::hex(&Sha256::digest(&out)) != p.to_sha256 {
                    return Err(PackError::new("PATCH_FAILED", format!("Patch for {} gave the wrong result", p.path)));
                }
                let dest = resolve_inside(&tmp, p.target()).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                fs::create_dir_all(dest.parent().unwrap())?;
                fs::write(&dest, &out)?;
            }
            Ok(())
        })();
        match res {
            Ok(()) => Ok(tmp),
            Err(e) => {
                let _ = fs::remove_dir_all(&tmp);
                Err(e)
            }
        }
    }

    fn check_pack(&self, m: &Manifest, game_id: &str, game_dir: &Path, force: bool) -> Result<VersionMatch> {
        if m.game_id != game_id {
            return Err(PackError::new("WRONG_GAME", format!("Pack is for \"{}\", not \"{game_id}\"", m.game_id)));
        }
        if !manifest::check_launcher_version(m, &self.launcher_version) {
            return Err(PackError::new("LAUNCHER_TOO_OLD", format!("Pack needs launcher {} or newer", m.min_launcher_version.as_deref().unwrap_or("?"))));
        }
        let gv = manifest::check_game_version(m, game_dir);
        if !gv.ok && !(gv.allow_unknown && force) {
            let mut e = PackError::new("GAME_VERSION", "This game version is not supported by the pack");
            e.allow_unknown = gv.allow_unknown;
            return Err(e);
        }
        // A patch only works on the exact original file, so this holds even for a forced install.
        let wrong: Vec<String> = m
            .patches
            .iter()
            .filter(|p| {
                let abs = resolve_inside(game_dir, &p.path).ok().filter(|a| a.is_file());
                let size_ok = p.from_size.is_none_or(|n| abs.as_ref().and_then(|a| fs::metadata(a).ok()).map(|m| m.len()) == Some(n));
                !size_ok || abs.and_then(|a| sha256_file_cached(&a).ok()).as_deref() != Some(p.from_sha256.as_str())
            })
            .map(|p| p.path.clone())
            .collect();
        if !wrong.is_empty() {
            let mut e = PackError::new("PATCH_SOURCE", format!("Not the original game file: {}", wrong.join(", ")));
            e.details = wrong;
            return Err(e);
        }
        let unmet = manifest::unmet_requires(&m.requires, game_dir);
        if !unmet.is_empty() {
            let mut e = PackError::new("REQUIRES", format!("Game folder is missing: {}", unmet.iter().map(|r| r.path.as_str()).collect::<Vec<_>>().join(", ")));
            e.details = unmet.iter().map(|r| if r.message.is_empty() { r.path.clone() } else { r.message.clone() }).collect();
            return Err(e);
        }
        Ok(gv)
    }

    /// Installs a pack. Backs up every file it replaces first; if anything fails after that, the game folder is put
    /// back as it was.
    pub fn install(&self, o: &InstallOptions, progress: &mut dyn FnMut(&str)) -> Result<InstallRecord> {
        check_game_id(o.game_id)?;
        if !o.game_dir.is_dir() {
            return Err(PackError::new("NO_GAME_DIR", format!("Game folder not found: {}", o.game_dir.display())));
        }
        if self.record(o.game_id).is_some() {
            return Err(PackError::new("ALREADY_INSTALLED", "A translation is already installed for this game"));
        }
        progress("verify");
        let mut pack = self.open_pack(o.zip_path, o.expected_sha256)?;
        let gv = self.check_pack(&pack.manifest, o.game_id, o.game_dir, o.force)?;
        let tmp = self.extract_verified(&mut pack, o.game_dir, progress)?;
        let res = self.copy_in(o, &pack.manifest, &gv, &tmp, progress);
        let _ = fs::remove_dir_all(&tmp);
        res
    }

    fn copy_in(&self, o: &InstallOptions, m: &Manifest, gv: &VersionMatch, tmp: &Path, progress: &mut dyn FnMut(&str)) -> Result<InstallRecord> {
        let bdir = self.backup_dir(o.game_id);
        if bdir.exists() {
            // Left over without an install record (a crash). Never delete originals: set them aside.
            fs::rename(&bdir, bdir.with_file_name(format!("{}.orphan-{}", o.game_id, util::now_secs())))?;
        }
        let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        let mut created: Vec<PathBuf> = Vec::new();
        let res = (|| {
            progress("backup");
            let mut plan = Vec::new();
            let (entries, swap_items) = plan_entries(o.game_dir, m, tmp)?;
            // Args packs only add files; one already there is byte-identical to ours (plan_entries checked), and
            // is adopted: no backup, recorded as ours, removed on uninstall.
            let args = m.activation.as_ref().is_some_and(|a| a.mode == ActivationMode::Args);
            for (path, src, sha256, patched) in &entries {
                let (path, sha256, patched) = (path, sha256, *patched);
                let abs = resolve_inside(o.game_dir, path).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                let backup = if abs.exists() && !args {
                    if abs.is_dir() {
                        return Err(PackError::new("IS_DIR", format!("{path} is a folder in the game")));
                    }
                    let b = resolve_inside(&bdir, path).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                    fs::create_dir_all(b.parent().unwrap())?;
                    fs::copy(&abs, &b)?;
                    Some(b)
                } else {
                    None
                };
                plan.push((path, src, sha256, patched, abs, backup));
            }
            progress("copy");
            let mut files = Vec::new();
            for (path, src, sha256, patched, abs, backup) in plan {
                files.push(RecordFile { path: path.clone(), sha256: sha256.clone(), backed_up: backup.is_some(), patched });
                if args && abs.is_file() {
                    // Adopted (identical): nothing to write, and a failed install leaves it where it was.
                    continue;
                }
                mkdir_tracked(o.game_dir, abs.parent().unwrap(), &mut created)?;
                // Move when the temp folder is on the game's drive (a 1 GB derived file isn't copied twice).
                let from = resolve_inside(tmp, src).unwrap();
                if fs::rename(&from, &abs).is_err() {
                    fs::copy(&from, &abs)?;
                }
                done.push((abs, backup));
            }
            let rec = InstallRecord {
                pack_id: m.pack_id.clone(),
                version: m.version.clone(),
                title: if m.title.is_empty() { m.pack_id.clone() } else { m.title.clone() },
                game_dir: o.game_dir.to_string_lossy().into_owned(),
                game_version: gv.matched.clone(),
                forced_unknown_version: !gv.ok,
                installed_at: util::now_iso(),
                files,
                created_dirs: created
                    .iter()
                    .map(|d| d.strip_prefix(o.game_dir).unwrap_or(d).to_string_lossy().replace('\\', "/"))
                    .collect(),
                activation: m.activation.clone(),
                swap: swap_items,
                paid: false,
            };
            let mut s = self.load_state();
            s.installs.insert(o.game_id.into(), rec.clone());
            self.save_state(&s)?;
            progress("done");
            Ok(rec)
        })();
        if res.is_err() {
            for (abs, backup) in done.iter().rev() {
                let _ = match backup {
                    Some(b) => fs::copy(b, abs).map(|_| ()),
                    None => fs::remove_file(abs),
                };
            }
            remove_empty_dirs(&created);
            let _ = fs::remove_dir_all(&bdir);
        }
        res
    }

    /// Puts the original files back. Files the game changed since the install (a game update) are left as they are.
    pub fn uninstall(&self, game_id: &str) -> Result<UninstallResult> {
        check_game_id(game_id)?;
        let rec = self.record(game_id).ok_or_else(|| PackError::new("NOT_INSTALLED", "No translation installed for this game"))?;
        let game_dir = PathBuf::from(&rec.game_dir);
        if !rec.swap.is_empty() && swap::is_active(&game_dir) {
            return Err(PackError::new("SWAP_ACTIVE", "The game is running in Hebrew; close it first"));
        }
        let bdir = self.backup_dir(game_id);
        let mut r = UninstallResult::default();
        for f in &rec.files {
            let abs = resolve_inside(&game_dir, &f.path).map_err(|e| PackError::new("BAD_STATE", e))?;
            let current = if abs.is_file() { Some(sha256_file(&abs)?) } else { None };
            if current.as_deref().is_some_and(|c| c != f.sha256) {
                r.skipped_changed.push(f.path.clone());
                continue;
            }
            if f.backed_up {
                let b = resolve_inside(&bdir, &f.path).map_err(|e| PackError::new("BAD_STATE", e))?;
                if !b.is_file() {
                    r.missing_backup.push(f.path.clone());
                    continue;
                }
                fs::create_dir_all(abs.parent().unwrap())?;
                fs::copy(&b, &abs)?;
                r.restored.push(f.path.clone());
            } else if current.is_some() {
                fs::remove_file(&abs)?;
                r.deleted.push(f.path.clone());
            }
        }
        let created: Vec<PathBuf> = rec.created_dirs.iter().filter_map(|d| resolve_inside(&game_dir, d).ok()).collect();
        remove_empty_dirs(&created);
        if !r.missing_backup.is_empty() {
            // Keep the record and backups so nothing is lost; the window says what to do.
            let mut e = PackError::new("MISSING_BACKUP", format!("Backup missing for: {}", r.missing_backup.join(", ")));
            e.details = r.missing_backup.clone();
            return Err(e);
        }
        let _ = fs::remove_dir_all(&bdir);
        let mut s = self.load_state();
        s.installs.remove(game_id);
        self.save_state(&s)?;
        Ok(r)
    }

    /// Update: the new pack is checked first, then the old one is removed and the new one installed. If the new
    /// install fails it rolls itself back, which leaves the game original.
    pub fn update(&self, o: &InstallOptions, progress: &mut dyn FnMut(&str)) -> Result<InstallRecord> {
        if self.record(o.game_id).is_some() {
            progress("verify");
            let pack = self.open_pack(o.zip_path, o.expected_sha256)?;
            self.check_pack(&pack.manifest, o.game_id, o.game_dir, o.force)?;
            drop(pack);
            self.uninstall(o.game_id)?;
        }
        self.install(o, progress)
    }

    /// Are the installed files still ours?
    pub fn verify(&self, game_id: &str) -> Option<Verify> {
        let rec = self.record(game_id)?;
        let dir = PathBuf::from(&rec.game_dir);
        let changed: Vec<String> = rec
            .files
            .iter()
            .filter(|f| {
                let abs = resolve_inside(&dir, &f.path).ok();
                abs.and_then(|a| sha256_file_cached(&a).ok()).as_deref() != Some(f.sha256.as_str())
            })
            .map(|f| f.path.clone())
            .collect();
        Some(Verify { intact: changed.is_empty(), changed })
    }

    pub fn update_available(&self, game_id: &str, latest: Option<&str>) -> bool {
        match (self.record(game_id), latest) {
            (Some(r), Some(v)) => compare_versions(v, &r.version).is_gt(),
            _ => false,
        }
    }
}

/// What goes where, for the pack's mode: (path in the game, file in tmp, its sha256, patched).
/// - permanent: files and patched files go in place (originals backed up);
/// - args: everything goes in as new files; replacing a game file is refused;
/// - swap: everything waits in `.tirgumim/he/`; the swap items say what is put in place at play time.
#[allow(clippy::type_complexity)]
fn plan_entries(game: &Path, m: &Manifest, tmp: &Path) -> Result<(Vec<(String, String, String, bool)>, Vec<SwapItem>)> {
    let mut out = Vec::new();
    let mut items = Vec::new();
    match m.activation.as_ref().map(|a| a.mode) {
        None => {
            out.extend(m.files.iter().map(|f| (f.path.clone(), f.path.clone(), f.sha256.clone(), false)));
            out.extend(m.patches.iter().map(|p| (p.path.clone(), p.path.clone(), p.to_sha256.clone(), true)));
        }
        Some(ActivationMode::Args) => {
            out.extend(m.files.iter().map(|f| (f.path.clone(), f.path.clone(), f.sha256.clone(), false)));
            out.extend(m.patches.iter().map(|p| (p.target().to_string(), p.target().to_string(), p.to_sha256.clone(), true)));
            // A file already there is fine if it is exactly ours (left by an earlier install of this translation,
            // e.g. by hand): it is adopted. Anything else would be replaced, which an args pack never does.
            let taken: Vec<String> = out
                .iter()
                .filter(|(p, _, sha, _)| {
                    let Ok(abs) = resolve_inside(game, p) else { return true };
                    abs.exists() && (!abs.is_file() || util::sha256_file_cached(&abs).ok().as_deref() != Some(sha.as_str()))
                })
                .map(|(p, ..)| p.clone())
                .collect();
            if !taken.is_empty() {
                let mut e = PackError::new("WOULD_REPLACE", format!("A launch-only pack may only add files, but these exist: {}", taken.join(", ")));
                e.details = taken;
                return Err(e);
            }
        }
        Some(ActivationMode::Swap) => {
            for f in &m.files {
                let existing = resolve_inside(game, &f.path).ok().filter(|a| a.is_file());
                let original = existing.as_ref().map(|a| util::sha256_file(a)).transpose()?;
                let kind = if original.is_some() { SwapKind::Replace } else { SwapKind::Add };
                out.push((swap::staged_rel(&f.path, kind), f.path.clone(), f.sha256.clone(), false));
                let new_dirs = if kind == SwapKind::Add { swap::missing_dirs(game, &f.path) } else { vec![] };
                items.push(SwapItem { path: f.path.clone(), kind, hebrew_sha256: f.sha256.clone(), original_sha256: original, original_size: None, append_size: None, new_dirs });
            }
            for p in &m.patches {
                let staged = swap::staged_rel(&p.path, if p.kind == PatchKind::Append { SwapKind::Append } else { SwapKind::Replace });
                if p.kind == PatchKind::Append {
                    let data = resolve_inside(tmp, &staged).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                    let sha = util::sha256_file(&data)?;
                    let size = fs::metadata(&data)?.len();
                    out.push((staged.clone(), staged, sha.clone(), true));
                    items.push(SwapItem { path: p.path.clone(), kind: SwapKind::Append, hebrew_sha256: sha, original_sha256: Some(p.from_sha256.clone()), original_size: p.from_size, append_size: Some(size), new_dirs: vec![] });
                } else {
                    out.push((staged, p.path.clone(), p.to_sha256.clone(), true));
                    items.push(SwapItem { path: p.path.clone(), kind: SwapKind::Replace, hebrew_sha256: p.to_sha256.clone(), original_sha256: Some(p.from_sha256.clone()), original_size: None, append_size: None, new_dirs: vec![] });
                }
            }
        }
    }
    Ok((out, items))
}

/// Writes through and hashes what passes.
struct HashWriter<W: Write> {
    inner: W,
    hash: Sha256,
}
impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(b)?;
        self.hash.update(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Applies a BSDIFF40 patch (classic bsdiff 4 with bzip2) to `src`.
pub fn apply_patch(src: &[u8], patch: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(src.len());
    qbsdiff::Bspatch::new(patch)?.apply(src, std::io::Cursor::new(&mut out))?;
    Ok(out)
}

fn mkdir_tracked(root: &Path, dir: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    let mut missing = Vec::new();
    let mut d = dir.to_path_buf();
    while d.starts_with(root) && d != root && !d.exists() {
        missing.push(d.clone());
        if !d.pop() {
            break;
        }
    }
    for m in missing.into_iter().rev() {
        fs::create_dir(&m)?;
        created.push(m);
    }
    Ok(())
}

fn remove_empty_dirs(dirs: &[PathBuf]) {
    let mut v: Vec<&PathBuf> = dirs.iter().collect();
    v.sort_by_key(|d| std::cmp::Reverse(d.as_os_str().len()));
    for d in v {
        if fs::read_dir(d).map(|mut r| r.next().is_none()).unwrap_or(false) {
            let _ = fs::remove_dir(d);
        }
    }
}
