//! Swap mode: the Hebrew files wait in `<game>/.tirgumim/he/` and are put in place only while the game runs from
//! the launcher. Everything is a rename on the game's own drive (instant, even for big files), or, for an `append`
//! patch, bytes added at the end of the file and cut off again.
//!
//! Crash safety: `.tirgumim/active.json` is written before the first change and removed after the last restore.
//! Restoring looks only at what is on disk, never at how far activation got, so it can run after a crash at any
//! point, and again after that, and always ends with the original files back.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::installer::PackError;
use super::manifest::OWN_DIR;
use super::util::{self, resolve_inside, sha256_file, sha256_file_cached};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwapKind {
    /// The game has this file: the original is set aside and ours put in its place.
    Replace,
    /// The game doesn't have this file: ours is put there and taken away again.
    Add,
    /// Bytes added at the end of the game's file and cut off again.
    Append,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapItem {
    pub path: String,
    pub kind: SwapKind,
    /// Our file (replace, add) or the bytes to append.
    pub hebrew_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_size: Option<u64>,
    /// Add: folders the game does not have, made when ours goes in and removed (if empty) when it comes out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub new_dirs: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub restored: Vec<String>,
    /// Changed by something else while ours was in place (a game update): left as it is.
    pub left_changed: Vec<String>,
}

fn err(code: &str, msg: impl Into<String>) -> PackError {
    PackError::new(code, msg)
}

pub fn own_dir(game: &Path) -> PathBuf {
    game.join(OWN_DIR)
}
pub fn journal(game: &Path) -> PathBuf {
    own_dir(game).join("active.json")
}
pub fn is_active(game: &Path) -> bool {
    journal(game).is_file()
}

/// Where our version waits, relative to the game root (recorded as an installed file).
pub fn staged_rel(item_path: &str, kind: SwapKind) -> String {
    match kind {
        SwapKind::Append => format!("{OWN_DIR}/he/{item_path}.append"),
        _ => format!("{OWN_DIR}/he/{item_path}"),
    }
}

fn at(game: &Path, rel: &str) -> Result<PathBuf, PackError> {
    resolve_inside(game, rel).map_err(|e| err("BAD_STATE", e))
}
fn staged(game: &Path, it: &SwapItem) -> Result<PathBuf, PackError> {
    at(game, &staged_rel(&it.path, it.kind))
}
fn set_aside(game: &Path, it: &SwapItem) -> Result<PathBuf, PackError> {
    at(game, &format!("{OWN_DIR}/orig/{}", it.path))
}

fn rename(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(p) = to.parent() {
        fs::create_dir_all(p)?;
    }
    fs::rename(from, to)
}

fn len(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// Can the swap start? The game's files must be the originals the pack was prepared on, and ours must be waiting.
pub fn check_ready(game: &Path, items: &[SwapItem]) -> Result<(), PackError> {
    if is_active(game) {
        return Err(err("SWAP_ACTIVE", "The Hebrew files are already in place"));
    }
    let mut changed = Vec::new();
    for it in items {
        let target = at(game, &it.path)?;
        let ok = match it.kind {
            SwapKind::Replace => target.is_file() && it.original_sha256.as_deref() == sha256_file_cached(&target).ok().as_deref(),
            SwapKind::Add => !target.exists(),
            SwapKind::Append => {
                len(&target) == it.original_size && it.original_sha256.as_deref() == sha256_file_cached(&target).ok().as_deref()
            }
        };
        if !ok {
            changed.push(it.path.clone());
        }
        let s = staged(game, it)?;
        if sha256_file_cached(&s).ok().as_deref() != Some(it.hebrew_sha256.as_str()) {
            return Err(err("NEEDS_REPAIR", format!("Translation file missing or changed: {}", it.path)));
        }
    }
    if !changed.is_empty() {
        let mut e = err("GAME_CHANGED", format!("The game's files changed since the translation was prepared: {}", changed.join(", ")));
        e.details = changed;
        return Err(e);
    }
    Ok(())
}

/// Puts our files in place. On any failure everything is put back before the error is returned.
pub fn activate(game: &Path, game_id: &str, items: &[SwapItem]) -> Result<(), PackError> {
    check_ready(game, items)?;
    let j = journal(game);
    fs::create_dir_all(own_dir(game))?;
    {
        let mut f = fs::File::create(&j)?;
        f.write_all(serde_json::to_string(&serde_json::json!({ "gameId": game_id, "since": util::now_iso() })).unwrap().as_bytes())?;
        f.sync_all()?;
    }
    let res = (|| -> Result<(), PackError> {
        for it in items {
            let target = at(game, &it.path)?;
            let ours = staged(game, it)?;
            match it.kind {
                SwapKind::Replace => {
                    rename(&target, &set_aside(game, it)?)?;
                    rename(&ours, &target)?;
                }
                SwapKind::Add => rename(&ours, &target)?,
                SwapKind::Append => {
                    let mut out = OpenOptions::new().append(true).open(&target)?;
                    io::copy(&mut fs::File::open(&ours)?, &mut out)?;
                    out.sync_all()?;
                    let want = it.original_size.unwrap_or(0) + it.append_size.unwrap_or(0);
                    if len(&target) != Some(want) {
                        return Err(err("SWAP_FAILED", format!("{} has the wrong size after adding the translation", it.path)));
                    }
                }
            }
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = restore(game, items);
        return Err(e);
    }
    Ok(())
}

/// Puts the originals back, from whatever state the files are in. Safe to run any number of times.
pub fn restore(game: &Path, items: &[SwapItem]) -> Result<RestoreReport, PackError> {
    let mut r = RestoreReport::default();
    let mut failed = Vec::new();
    for it in items {
        match restore_one(game, it) {
            Ok(Some(true)) => r.restored.push(it.path.clone()),
            Ok(Some(false)) => r.left_changed.push(it.path.clone()),
            Ok(None) => {}
            Err(e) => failed.push(format!("{}: {e}", it.path)),
        }
    }
    if !failed.is_empty() {
        // Keep the journal: the next launcher start tries again.
        return Err(err("RESTORE_FAILED", failed.join("; ")));
    }
    let _ = fs::remove_file(journal(game));
    remove_empty_tree(&own_dir(game).join("orig"));
    Ok(r)
}

/// Some(true): put back. Some(false): left as the game has it now. None: nothing to do.
fn restore_one(game: &Path, it: &SwapItem) -> Result<Option<bool>, PackError> {
    let target = at(game, &it.path)?;
    let ours = staged(game, it)?;
    let is_ours = |p: &Path| p.is_file() && sha256_file(p).ok().as_deref() == Some(it.hebrew_sha256.as_str());
    match it.kind {
        SwapKind::Replace => {
            let aside = set_aside(game, it)?;
            if !aside.exists() {
                return Ok(None);
            }
            let mut changed = false;
            if target.exists() {
                if !ours.exists() && is_ours(&target) {
                    rename(&target, &ours)?;
                } else {
                    // Something else (a game update) replaced our file: the game's new file wins.
                    changed = true;
                }
            }
            if changed {
                fs::remove_file(&aside)?;
                Ok(Some(false))
            } else {
                rename(&aside, &target)?;
                Ok(Some(true))
            }
        }
        SwapKind::Add => {
            if target.exists() && !ours.exists() {
                if is_ours(&target) {
                    rename(&target, &ours)?;
                    remove_new_dirs(game, it);
                    return Ok(Some(true));
                }
                return Ok(Some(false));
            }
            Ok(None)
        }
        SwapKind::Append => {
            let (orig, add) = (it.original_size.unwrap_or(0), it.append_size.unwrap_or(0));
            match len(&target) {
                Some(n) if n == orig => Ok(None),
                // Ours (or a write a crash cut short): cut back to the original and check it.
                Some(n) if n > orig && n <= orig + add => {
                    let f = OpenOptions::new().write(true).open(&target)?;
                    f.set_len(orig)?;
                    f.sync_all()?;
                    drop(f);
                    if it.original_sha256.as_deref() != sha256_file_cached(&target).ok().as_deref() {
                        return Err(err("RESTORE_FAILED", format!("{} is not the original after restoring", it.path)));
                    }
                    Ok(Some(true))
                }
                _ => Ok(Some(false)),
            }
        }
    }
}

/// The folders an Add item made, deepest first, if they are empty.
fn remove_new_dirs(game: &Path, it: &SwapItem) {
    for d in it.new_dirs.iter().rev() {
        if let Ok(p) = resolve_inside(game, d) {
            let _ = fs::remove_dir(p);
        }
    }
}

/// The folders on the way to `rel` that the game does not have, outermost first.
pub fn missing_dirs(game: &Path, rel: &str) -> Vec<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    (1..parts.len()).map(|n| parts[..n].join("/")).filter(|d| !game.join(d).exists()).collect()
}

fn remove_empty_tree(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                remove_empty_tree(&e.path());
            }
        }
    }
    let _ = fs::remove_dir(dir);
}
