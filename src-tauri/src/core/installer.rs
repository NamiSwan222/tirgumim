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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::manifest::{self, Manifest, VersionMatch, MANIFEST_NAME};
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
        let manifest = manifest::parse_manifest(&text).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
        Ok(OpenPack { manifest, archive })
    }

    /// Unpacks the files the manifest lists into a temporary folder and checks each one's hash.
    fn extract_verified(&self, pack: &mut OpenPack) -> Result<PathBuf> {
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
        let tmp = self.extract_verified(&mut pack)?;
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
            for f in &m.files {
                let abs = resolve_inside(o.game_dir, &f.path).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                let backup = if abs.exists() {
                    if abs.is_dir() {
                        return Err(PackError::new("IS_DIR", format!("{} is a folder in the game", f.path)));
                    }
                    let b = resolve_inside(&bdir, &f.path).map_err(|e| PackError::new("BAD_MANIFEST", e))?;
                    fs::create_dir_all(b.parent().unwrap())?;
                    fs::copy(&abs, &b)?;
                    Some(b)
                } else {
                    None
                };
                plan.push((f, abs, backup));
            }
            progress("copy");
            let mut files = Vec::new();
            for (f, abs, backup) in plan {
                mkdir_tracked(o.game_dir, abs.parent().unwrap(), &mut created)?;
                fs::copy(resolve_inside(tmp, &f.path).unwrap(), &abs)?;
                files.push(RecordFile { path: f.path.clone(), sha256: f.sha256.clone(), backed_up: backup.is_some() });
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
