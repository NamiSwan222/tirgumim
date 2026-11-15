//! Install / update / uninstall against a copy of the sample game (dev/sample-game-template).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::installer::{InstallOptions, Installer};
use super::manifest::{self, Require};
use super::tools::{copy_dir, make_pack};
use super::util::sha256_file;

fn dev() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf().join("dev")
}

struct Setup {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    game: PathBuf,
    inst: Installer,
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let game = root.join("game");
    copy_dir(&dev().join("sample-game-template"), &game).unwrap();
    let inst = Installer::new(root.join("data"), "0.2.0");
    Setup { _tmp: tmp, root, game, inst }
}

/// Builds the sample pack, after `mutate` has changed its source folder.
fn build_pack(root: &Path, name: &str, mutate: impl FnOnce(&Path)) -> (PathBuf, String) {
    let src = root.join(format!("packsrc-{name}"));
    copy_dir(&dev().join("sample-pack"), &src).unwrap();
    mutate(&src);
    let made = make_pack(&src.join("pack.config.json"), Some(&dev().join("sample-game-template")), Some(&root.join(format!("{name}.zip"))), None).unwrap();
    (made.out, made.entry["sha256"].as_str().unwrap().to_string())
}

fn edit_config(dir: &Path, f: impl FnOnce(&mut serde_json::Value)) {
    let p = dir.join("pack.config.json");
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    f(&mut v);
    fs::write(p, serde_json::to_string(&v).unwrap()).unwrap();
}

fn opts<'a>(s: &'a Setup, zip: &'a Path, sha: Option<&'a str>, force: bool) -> InstallOptions<'a> {
    InstallOptions { game_id: "sample-game", game_dir: &s.game, zip_path: zip, expected_sha256: sha, force }
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

/// Every file under `dir` and its hash: what "byte-identical" is checked against.
fn tree(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn walk(d: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
        for e in fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                out.insert(format!("{rel}/"), String::new());
                walk(&p, base, out);
            } else {
                out.insert(rel, sha256_file(&p).unwrap());
            }
        }
    }
    walk(dir, dir, &mut out);
    out
}

#[test]
fn install_then_uninstall_leaves_the_game_byte_identical() {
    let s = setup();
    let before = tree(&s.game);
    let (zip, sha) = build_pack(&s.root, "v1", |_| {});
    let rec = s.inst.install(&opts(&s, &zip, Some(&sha), false), &mut |_| {}).unwrap();
    assert_eq!(rec.version, "0.1.0");
    assert_eq!(rec.game_version.as_deref(), Some("1.0"));
    assert!(read(&s.game.join("data/strings.txt")).contains("התחל משחק"));
    assert!(s.game.join("data/fonts/hebrew.ttf").is_file());
    assert!(s.inst.verify("sample-game").unwrap().intact);

    let r = s.inst.uninstall("sample-game").unwrap();
    assert_eq!(r.restored, ["data/strings.txt"]);
    assert_eq!(r.deleted, ["data/fonts/hebrew.ttf"]);
    assert_eq!(tree(&s.game), before, "game folder must be exactly as it was");
    assert!(s.inst.record("sample-game").is_none());
    assert!(!s.inst.backup_dir("sample-game").exists());
}

#[test]
fn rejects_wrong_hash_and_wrong_game() {
    let s = setup();
    let (zip, _) = build_pack(&s.root, "v1", |_| {});
    let zeros = "0".repeat(64);
    assert_eq!(s.inst.install(&opts(&s, &zip, Some(&zeros), false), &mut |_| {}).unwrap_err().code, "ZIP_HASH");
    let other = InstallOptions { game_id: "other-game", ..opts(&s, &zip, None, false) };
    assert_eq!(s.inst.install(&other, &mut |_| {}).unwrap_err().code, "WRONG_GAME");
    assert!(read(&s.game.join("data/strings.txt")).contains("Start Game"));
}

#[test]
fn unknown_game_version_blocked_unless_forced_and_allowed() {
    let s = setup();
    fs::write(s.game.join("SampleGame.exe"), "patched exe v1.1").unwrap();
    let (zip, _) = build_pack(&s.root, "v1", |_| {});
    let e = s.inst.install(&opts(&s, &zip, None, false), &mut |_| {}).unwrap_err();
    assert_eq!((e.code.as_str(), e.allow_unknown), ("GAME_VERSION", true));
    assert!(s.inst.install(&opts(&s, &zip, None, true), &mut |_| {}).unwrap().forced_unknown_version);

    let s2 = setup();
    fs::write(s2.game.join("SampleGame.exe"), "patched exe v1.1").unwrap();
    let (strict, _) = build_pack(&s2.root, "strict", |d| edit_config(d, |v| v["allowUnknownGameVersion"] = false.into()));
    let e = s2.inst.install(&opts(&s2, &strict, None, true), &mut |_| {}).unwrap_err();
    assert_eq!((e.code.as_str(), e.allow_unknown), ("GAME_VERSION", false));
}

#[test]
fn uninstall_after_a_game_update_keeps_the_games_new_file() {
    let s = setup();
    let (zip, _) = build_pack(&s.root, "v1", |_| {});
    s.inst.install(&opts(&s, &zip, None, false), &mut |_| {}).unwrap();
    fs::write(s.game.join("data/strings.txt"), "menu.start=Start Game (v1.1)\n").unwrap();
    assert_eq!(s.inst.verify("sample-game").unwrap().changed, ["data/strings.txt"]);
    let r = s.inst.uninstall("sample-game").unwrap();
    assert_eq!(r.skipped_changed, ["data/strings.txt"]);
    assert!(read(&s.game.join("data/strings.txt")).contains("v1.1"));
}

#[test]
fn update_installs_the_new_version_and_keeps_the_true_originals() {
    let s = setup();
    let before = tree(&s.game);
    let (v1, _) = build_pack(&s.root, "v1", |_| {});
    s.inst.install(&opts(&s, &v1, None, false), &mut |_| {}).unwrap();
    let (v2, _) = build_pack(&s.root, "v2", |d| {
        edit_config(d, |v| v["version"] = "0.2.0".into());
        fs::write(d.join("files/data/strings.txt"), "menu.start=התחלה\n").unwrap();
    });
    assert!(s.inst.update_available("sample-game", Some("0.2.0")));
    s.inst.update(&opts(&s, &v2, None, false), &mut |_| {}).unwrap();
    assert_eq!(s.inst.record("sample-game").unwrap().version, "0.2.0");
    assert!(read(&s.game.join("data/strings.txt")).contains("התחלה"));
    s.inst.uninstall("sample-game").unwrap();
    assert_eq!(tree(&s.game), before);
}

#[test]
fn update_to_a_bad_pack_keeps_the_old_translation() {
    let s = setup();
    let (v1, _) = build_pack(&s.root, "v1", |_| {});
    s.inst.install(&opts(&s, &v1, None, false), &mut |_| {}).unwrap();
    let (other, _) = build_pack(&s.root, "other", |d| edit_config(d, |v| v["gameId"] = "other-game".into()));
    assert_eq!(s.inst.update(&opts(&s, &other, None, false), &mut |_| {}).unwrap_err().code, "WRONG_GAME");
    assert_eq!(s.inst.record("sample-game").unwrap().version, "0.1.0");
    assert!(read(&s.game.join("data/strings.txt")).contains("התחל משחק"));
}

fn raw_zip(root: &Path, manifest: &serde_json::Value, files: &[(&str, &[u8])]) -> PathBuf {
    let p = root.join(format!("{}.zip", super::util::unique_name()));
    let mut z = zip::ZipWriter::new(fs::File::create(&p).unwrap());
    let o = zip::write::SimpleFileOptions::default();
    z.start_file(manifest::MANIFEST_NAME, o).unwrap();
    z.write_all(manifest.to_string().as_bytes()).unwrap();
    for (name, data) in files {
        z.start_file(*name, o).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
    p
}

#[test]
fn rejects_path_traversal() {
    let s = setup();
    let m = serde_json::json!({ "schema": 1, "packId": "x", "gameId": "sample-game", "version": "1.0.0",
        "files": [{ "path": "../evil.txt", "sha256": "a".repeat(64) }] });
    let zip = raw_zip(&s.root, &m, &[("files/../evil.txt", b"evil")]);
    assert_eq!(s.inst.install(&opts(&s, &zip, None, false), &mut |_| {}).unwrap_err().code, "BAD_MANIFEST");
    assert!(!s.root.join("evil.txt").exists());
}

#[test]
fn rejects_a_file_whose_hash_is_wrong() {
    let s = setup();
    let m = serde_json::json!({ "schema": 1, "packId": "x", "gameId": "sample-game", "version": "1.0.0",
        "files": [{ "path": "data/strings.txt", "sha256": "a".repeat(64) }] });
    let zip = raw_zip(&s.root, &m, &[("files/data/strings.txt", b"tampered")]);
    assert_eq!(s.inst.install(&opts(&s, &zip, None, false), &mut |_| {}).unwrap_err().code, "FILE_HASH");
    assert!(read(&s.game.join("data/strings.txt")).contains("Start Game"));
}

#[test]
fn failed_install_rolls_back() {
    let s = setup();
    let before = tree(&s.game);
    let (zip, _) = build_pack(&s.root, "v1", |_| {});
    // Files go in order data/fonts/hebrew.ttf, data/strings.txt: a read-only strings.txt fails the second write
    // after the first has landed.
    let target = s.game.join("data/strings.txt");
    let mut perm = fs::metadata(&target).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&target, perm.clone()).unwrap();
    let res = s.inst.install(&opts(&s, &zip, None, false), &mut |_| {});
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    fs::set_permissions(&target, perm).unwrap();
    assert!(res.is_err());
    assert_eq!(tree(&s.game), before);
    assert!(s.inst.record("sample-game").is_none());
    assert!(!s.inst.backup_dir("sample-game").exists());
}

#[test]
fn required_files_block_the_install_until_they_are_there() {
    let s = setup();
    let (zip, _) = build_pack(&s.root, "req", |d| {
        edit_config(d, |v| v["requires"] = serde_json::json!([{ "path": "loader/core.dll", "message": "צריך להתקין את ה-loader" }]))
    });
    let e = s.inst.install(&opts(&s, &zip, None, false), &mut |_| {}).unwrap_err();
    assert_eq!(e.code, "REQUIRES");
    assert_eq!(e.details, ["צריך להתקין את ה-loader"]);
    fs::create_dir_all(s.game.join("loader")).unwrap();
    fs::write(s.game.join("loader/core.dll"), "x").unwrap();
    s.inst.install(&opts(&s, &zip, None, false), &mut |_| {}).unwrap();

    let absent = [Require { path: "loader/core.dll".into(), message: "remove it".into(), absent: true, url: None }];
    assert_eq!(manifest::unmet_requires(&absent, &s.game).len(), 1);
}

// ---------- binary patches (bsdiff) ----------

/// Bytes that look like a game data file (not compressible).
fn noise(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

struct PatchSetup {
    s: Setup,
    clean: PathBuf,
    original: Vec<u8>,
    changed: Vec<u8>,
}

/// A clean game (the sample game plus a 300 KB `data.win`), a copy to install into, and our changed `data.win`.
fn patch_setup() -> PatchSetup {
    let mut s = setup();
    s.inst = Installer::new(s.root.join("data"), manifest::PATCHES_MIN_LAUNCHER);
    let clean = s.root.join("clean");
    copy_dir(&dev().join("sample-game-template"), &clean).unwrap();
    let original = noise(300_000, 7);
    let mut changed = original.clone();
    for i in (1000..changed.len() - 64).step_by(4096) {
        changed[i..i + 64].copy_from_slice(&[0xd7; 64]);
    }
    changed.extend_from_slice("עברית".as_bytes());
    fs::write(clean.join("data.win"), &original).unwrap();
    fs::write(s.game.join("data.win"), &original).unwrap();
    PatchSetup { s, clean, original, changed }
}

fn build_patch_pack(p: &PatchSetup, name: &str, patch: serde_json::Value) -> Result<(PathBuf, serde_json::Value), String> {
    let src = p.s.root.join(format!("packsrc-{name}"));
    let _ = fs::remove_dir_all(&src);
    copy_dir(&dev().join("sample-pack"), &src).unwrap();
    fs::write(src.join("data.win.changed"), &p.changed).unwrap();
    edit_config(&src, |v| v["patches"] = serde_json::json!([patch]));
    let made = make_pack(&src.join("pack.config.json"), Some(&p.clean), Some(&p.s.root.join(format!("{name}.zip"))), None)?;
    Ok((made.out, made.manifest))
}

#[test]
fn patch_install_then_uninstall_leaves_the_game_byte_identical() {
    let p = patch_setup();
    let before = tree(&p.s.game);
    let (zip, m) = build_patch_pack(&p, "patch", serde_json::json!({ "path": "data.win", "patched": "data.win.changed" })).unwrap();
    assert_eq!(m["minLauncherVersion"], manifest::PATCHES_MIN_LAUNCHER);
    assert_eq!(m["patches"][0]["patch"], "patches/data.win.bsdiff");
    assert_eq!(m["patches"][0]["size"], p.changed.len());

    let mut steps = Vec::new();
    let rec = p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |st| steps.push(st.to_string())).unwrap();
    assert!(steps.iter().any(|s| s == "patch"));
    assert_eq!(fs::read(p.s.game.join("data.win")).unwrap(), p.changed, "the patch must give exactly our file");
    assert!(read(&p.s.game.join("data/strings.txt")).contains("התחל משחק"), "files and patches go in together");
    let f = rec.files.iter().find(|f| f.path == "data.win").unwrap();
    assert!(f.patched && f.backed_up);
    assert!(p.s.inst.verify("sample-game").unwrap().intact);

    let r = p.s.inst.uninstall("sample-game").unwrap();
    assert!(r.restored.iter().any(|x| x == "data.win"));
    assert_eq!(tree(&p.s.game), before, "game folder must be exactly as it was");
    assert_eq!(fs::read(p.s.game.join("data.win")).unwrap(), p.original);
}

#[test]
fn patch_refuses_a_file_that_is_not_the_original_even_forced() {
    let p = patch_setup();
    let (zip, _) = build_patch_pack(&p, "patch", serde_json::json!({ "path": "data.win", "patched": "data.win.changed" })).unwrap();
    let mut other = p.original.clone();
    other[10] ^= 1;
    fs::write(p.s.game.join("data.win"), &other).unwrap();
    let before = tree(&p.s.game);
    for force in [false, true] {
        let e = p.s.inst.install(&opts(&p.s, &zip, None, force), &mut |_| {}).unwrap_err();
        assert_eq!((e.code.as_str(), e.details.clone()), ("PATCH_SOURCE", vec!["data.win".to_string()]));
    }
    assert_eq!(tree(&p.s.game), before, "nothing in the game may change");
    // Already patched counts as "not the original" too.
    fs::write(p.s.game.join("data.win"), &p.changed).unwrap();
    assert_eq!(p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap_err().code, "PATCH_SOURCE");
    assert!(p.s.inst.record("sample-game").is_none());
}

#[test]
fn patch_from_a_ready_bsdiff_file_and_bad_ones_are_refused() {
    let p = patch_setup();
    let mut diff = Vec::new();
    qbsdiff::Bsdiff::new(&p.original, &p.changed).compare(std::io::Cursor::new(&mut diff)).unwrap();
    let dir = p.s.root.join("ready");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("x.bsdiff"), &diff).unwrap();
    fs::write(dir.join("bad.bsdiff"), b"not a bsdiff").unwrap();
    // patchFile is relative to the config; an absolute path works too.
    let pf = dir.join("x.bsdiff").to_string_lossy().into_owned();
    let bad = dir.join("bad.bsdiff").to_string_lossy().into_owned();
    let (zip, _) = build_patch_pack(&p, "ready", serde_json::json!({ "path": "data.win", "patchFile": pf })).unwrap();
    p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap();
    assert_eq!(fs::read(p.s.game.join("data.win")).unwrap(), p.changed);

    assert!(build_patch_pack(&p, "bad", serde_json::json!({ "path": "data.win", "patchFile": bad })).is_err());
    assert!(build_patch_pack(&p, "both", serde_json::json!({ "path": "data.win", "patched": "data.win.changed", "patchFile": pf })).is_err());
    assert!(build_patch_pack(&p, "missing", serde_json::json!({ "path": "nope.win", "patched": "data.win.changed" })).is_err());
}

#[test]
fn a_corrupt_patch_in_the_pack_changes_nothing() {
    use std::io::Read;
    let p = patch_setup();
    let (good, m) = build_patch_pack(&p, "patch", serde_json::json!({ "path": "data.win", "patched": "data.win.changed" })).unwrap();
    // Same manifest, but the patch inside the zip is cut short.
    let mut diff = Vec::new();
    zip::ZipArchive::new(fs::File::open(&good).unwrap()).unwrap().by_name("patches/data.win.bsdiff").unwrap().read_to_end(&mut diff).unwrap();
    diff.truncate(diff.len() / 2);
    let mut entries: Vec<(String, Vec<u8>)> = m["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let path = f["path"].as_str().unwrap();
            (format!("files/{path}"), fs::read(dev().join("sample-pack/files").join(path)).unwrap())
        })
        .collect();
    entries.push(("patches/data.win.bsdiff".into(), diff));
    let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let zip = raw_zip(&p.s.root, &m, &refs);
    let before = tree(&p.s.game);
    assert_eq!(p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap_err().code, "PATCH_FAILED");
    assert_eq!(tree(&p.s.game), before);
    assert!(p.s.inst.record("sample-game").is_none());
}

#[test]
fn patch_manifest_rules() {
    let h = |c: char| c.to_string().repeat(64);
    let base = |patch: &serde_json::Value, min: &str| {
        serde_json::json!({
            "schema": 1, "packId": "p", "gameId": "g", "version": "1", "minLauncherVersion": min,
            "files": [], "patches": [patch]
        })
    };
    let parse = |v: serde_json::Value| manifest::parse_manifest(&v.to_string());
    let ok = serde_json::json!({ "path": "data.win", "fromSha256": h('a'), "toSha256": h('b'), "patch": "patches/data.win.bsdiff" });
    assert!(parse(base(&ok, "0.3.0")).is_ok(), "patches alone are a valid pack");
    assert!(parse(base(&ok, "0.2.0")).is_err(), "older launchers would skip the patch");
    for (k, v) in [("patch", "data.win.bsdiff"), ("path", "../x"), ("toSha256", h('a').as_str())] {
        let mut bad = ok.clone();
        bad[k] = v.into();
        assert!(parse(base(&bad, "0.3.0")).is_err(), "{k} = {v} must be refused");
    }
    let mut both = base(&ok, "0.3.0");
    both["files"] = serde_json::json!([{ "path": "DATA.win", "sha256": h('c') }]);
    assert!(parse(both).is_err(), "a path is in files or patches, not both");

    // A launcher that is too old says so, before anything else.
    let p = patch_setup();
    let (zip, _) = build_patch_pack(&p, "patch", serde_json::json!({ "path": "data.win", "patched": "data.win.changed" })).unwrap();
    let old = Installer::new(p.s.root.join("old"), "0.2.0");
    assert_eq!(old.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap_err().code, "LAUNCHER_TOO_OLD");
}

// ---------- launch-only packs (activation: args / swap) ----------

use super::swap;

/// The game folder without the launcher's own `.tirgumim` folder: what Steam (or the player) sees.
fn game_tree(dir: &Path) -> BTreeMap<String, String> {
    tree(dir).into_iter().filter(|(k, _)| !k.starts_with(".tirgumim")).collect()
}

/// A launch-only pack built with make-pack on the patch setup (sample pack files + data.win).
fn launch_pack(p: &PatchSetup, name: &str, activation: serde_json::Value, patches: serde_json::Value) -> Result<(PathBuf, serde_json::Value), String> {
    let src = p.s.root.join(format!("packsrc-{name}"));
    let _ = fs::remove_dir_all(&src);
    copy_dir(&dev().join("sample-pack"), &src).unwrap();
    fs::write(src.join("data.win.changed"), &p.changed).unwrap();
    fs::write(src.join("big.append"), noise(50_000, 99)).unwrap();
    edit_config(&src, |v| {
        v["activation"] = activation;
        v["patches"] = patches;
    });
    let made = make_pack(&src.join("pack.config.json"), Some(&p.clean), Some(&p.s.root.join(format!("{name}.zip"))), None)?;
    Ok((made.out, made.manifest))
}

fn launch_setup() -> PatchSetup {
    let mut p = patch_setup();
    p.s.inst = Installer::new(p.s.root.join("data"), manifest::LAUNCH_MIN_LAUNCHER);
    let big = noise(400_000, 3);
    fs::write(p.clean.join("big.dat"), &big).unwrap();
    fs::write(p.s.game.join("big.dat"), &big).unwrap();
    p
}

#[test]
fn args_pack_only_adds_files_and_leaves_every_original_alone() {
    let p = launch_setup();
    let act = serde_json::json!({ "mode": "args", "exe": "SampleGame.exe", "args": ["-game", "data_he.win"], "via": "steam" });
    let (zip, m) = launch_pack(&p, "args", act.clone(), serde_json::json!([{ "path": "data.win", "patched": "data.win.changed", "output": "data_he.win" }])).unwrap();
    assert_eq!(m["minLauncherVersion"], manifest::LAUNCH_MIN_LAUNCHER);

    // The sample pack replaces data/strings.txt, which an args pack may not do.
    assert_eq!(p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap_err().code, "WOULD_REPLACE");

    let src = p.s.root.join("packsrc-args");
    fs::rename(src.join("files/data/strings.txt"), src.join("files/data/strings_he.txt")).unwrap();
    let made = make_pack(&src.join("pack.config.json"), Some(&p.clean), Some(&p.s.root.join("args2.zip")), None).unwrap();
    let before = tree(&p.s.game);
    let rec = p.s.inst.install(&opts(&p.s, &made.out, None, false), &mut |_| {}).unwrap();
    assert_eq!(rec.activation.as_ref().unwrap().mode, manifest::ActivationMode::Args);
    let after = tree(&p.s.game);
    for (k, v) in &before {
        assert_eq!(after.get(k), Some(v), "{k} must not change");
    }
    assert_eq!(fs::read(p.s.game.join("data_he.win")).unwrap(), p.changed);
    assert_eq!(fs::read(p.s.game.join("data.win")).unwrap(), p.original);
    p.s.inst.uninstall("sample-game").unwrap();
    assert_eq!(tree(&p.s.game), before);
}

#[test]
fn swap_pack_is_hebrew_only_while_swapped_in() {
    let p = launch_setup();
    let original_game = tree(&p.s.game);
    let act = serde_json::json!({ "mode": "swap", "exe": "SampleGame.exe" });
    let (zip, _) = launch_pack(&p, "swap", act, serde_json::json!([
        { "path": "data.win", "patched": "data.win.changed" },
        { "path": "big.dat", "kind": "append", "appendFile": "big.append" }
    ]))
    .unwrap();
    let rec = p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap();
    assert_eq!(game_tree(&p.s.game), original_game, "preparing a swap pack changes no game file");
    let kinds: Vec<_> = rec.swap.iter().map(|i| (i.path.as_str(), i.kind)).collect();
    assert!(kinds.contains(&("data/strings.txt", swap::SwapKind::Replace)));
    assert!(kinds.contains(&("data/fonts/hebrew.ttf", swap::SwapKind::Add)));
    assert!(kinds.contains(&("big.dat", swap::SwapKind::Append)));
    assert!(p.s.inst.verify("sample-game").unwrap().intact);

    swap::activate(&p.s.game, "sample-game", &rec.swap).unwrap();
    assert!(swap::is_active(&p.s.game));
    assert!(read(&p.s.game.join("data/strings.txt")).contains("התחל משחק"));
    assert!(p.s.game.join("data/fonts/hebrew.ttf").is_file());
    assert_eq!(fs::read(p.s.game.join("data.win")).unwrap(), p.changed);
    assert_eq!(fs::metadata(p.s.game.join("big.dat")).unwrap().len(), 450_000);
    assert_eq!(p.s.inst.uninstall("sample-game").unwrap_err().code, "SWAP_ACTIVE");
    assert_eq!(swap::activate(&p.s.game, "sample-game", &rec.swap).unwrap_err().code, "SWAP_ACTIVE");

    let r = swap::restore(&p.s.game, &rec.swap).unwrap();
    assert_eq!(r.restored.len(), 4);
    assert!(!swap::is_active(&p.s.game));
    assert_eq!(game_tree(&p.s.game), original_game, "after the game exits it is the original again");
    assert!(swap::restore(&p.s.game, &rec.swap).unwrap().restored.is_empty(), "restoring twice is harmless");

    // And it can be played again, then removed.
    swap::activate(&p.s.game, "sample-game", &rec.swap).unwrap();
    swap::restore(&p.s.game, &rec.swap).unwrap();
    p.s.inst.uninstall("sample-game").unwrap();
    assert_eq!(tree(&p.s.game), original_game, "nothing of ours is left");
}

#[test]
fn swap_recovers_from_a_crash_at_any_step() {
    let p = launch_setup();
    let original_game = tree(&p.s.game);
    let (zip, _) = launch_pack(&p, "swap", serde_json::json!({ "mode": "swap", "exe": "SampleGame.exe" }), serde_json::json!([
        { "path": "data.win", "patched": "data.win.changed" },
        { "path": "big.dat", "kind": "append", "appendFile": "big.append" }
    ]))
    .unwrap();
    let rec = p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap();
    let g = &p.s.game;
    let own = g.join(".tirgumim");

    // Crash 1: journal written, the original set aside, ours not yet in place; half the bytes appended.
    fs::write(swap::journal(g), "{}").unwrap();
    fs::create_dir_all(own.join("orig")).unwrap();
    fs::rename(g.join("data.win"), own.join("orig/data.win")).unwrap();
    {
        use std::io::Write as _;
        let mut f = fs::OpenOptions::new().append(true).open(g.join("big.dat")).unwrap();
        f.write_all(&noise(50_000, 99)[..20_000]).unwrap();
    }
    swap::restore(g, &rec.swap).unwrap();
    assert_eq!(game_tree(g), original_game);
    assert!(!swap::is_active(g));

    // Crash 2: fully active, the launcher killed. The next start restores.
    swap::activate(g, "sample-game", &rec.swap).unwrap();
    swap::restore(g, &rec.swap).unwrap();
    assert_eq!(game_tree(g), original_game);
    assert!(p.s.inst.verify("sample-game").unwrap().intact, "our files are back waiting for the next play");
}

#[test]
fn swap_keeps_a_file_the_game_updated_while_playing_and_refuses_a_changed_game() {
    let p = launch_setup();
    let (zip, _) = launch_pack(&p, "swap", serde_json::json!({ "mode": "swap", "exe": "SampleGame.exe" }), serde_json::json!([])).unwrap();
    let rec = p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap();
    let g = &p.s.game;
    swap::activate(g, "sample-game", &rec.swap).unwrap();
    // A Steam update replaces the file while ours is in place.
    fs::write(g.join("data/strings.txt"), "menu.start=Start Game (v1.1)\n").unwrap();
    let r = swap::restore(g, &rec.swap).unwrap();
    assert_eq!(r.left_changed, ["data/strings.txt"]);
    assert!(read(&g.join("data/strings.txt")).contains("v1.1"), "the game's new file wins");
    assert!(!g.join(".tirgumim/orig/data/strings.txt").exists());

    // Next play: the game is not what the translation was prepared on.
    let e = swap::activate(g, "sample-game", &rec.swap).unwrap_err();
    assert!(e.code == "GAME_CHANGED" || e.code == "NEEDS_REPAIR", "{e:?}");
    assert!(!swap::is_active(g));
}

/// The Yu-Gi-Oh shape: a proxy DLL that only acts with an env var, and Hebrew copies of the data files made on the
/// player's PC from their originals (append into a new file), so no original is ever touched.
#[test]
fn env_pack_with_derived_copies_never_touches_originals() {
    let p = launch_setup();
    let act = serde_json::json!({ "mode": "args", "exe": "SampleGame.exe", "env": { "TIRGUMIM_LANG": "he" }, "via": "direct", "needsSteam": true });
    let (_, _) = launch_pack(&p, "env", act, serde_json::json!([
        { "path": "big.dat", "kind": "append", "appendFile": "big.append", "output": "tirgumim/he/big.dat" },
        { "path": "data.win", "patched": "data.win.changed", "output": "tirgumim/he/data.win" }
    ]))
    .unwrap();
    // The sample pack's files replace game files; this pack only adds a "DLL".
    let src = p.s.root.join("packsrc-env");
    fs::remove_dir_all(src.join("files")).unwrap();
    fs::create_dir_all(src.join("files")).unwrap();
    fs::write(src.join("files/xinput9_1_0.dll"), b"proxy").unwrap();
    let made = make_pack(&src.join("pack.config.json"), Some(&p.clean), Some(&p.s.root.join("env2.zip")), None).unwrap();
    assert_eq!(made.manifest["patches"][0]["output"], "tirgumim/he/big.dat");

    let before = tree(&p.s.game);
    let rec = p.s.inst.install(&opts(&p.s, &made.out, None, false), &mut |_| {}).unwrap();
    let after = tree(&p.s.game);
    for (k, v) in &before {
        assert_eq!(after.get(k), Some(v), "{k} must not change");
    }
    let mut want = fs::read(p.clean.join("big.dat")).unwrap();
    want.extend(noise(50_000, 99));
    assert_eq!(fs::read(p.s.game.join("tirgumim/he/big.dat")).unwrap(), want);
    assert_eq!(fs::read(p.s.game.join("tirgumim/he/data.win")).unwrap(), p.changed);
    assert!(p.s.game.join("xinput9_1_0.dll").is_file());
    let a = rec.activation.unwrap();
    assert_eq!(a.env.get("TIRGUMIM_LANG").map(String::as_str), Some("he"));
    assert!(a.needs_steam);
    assert!(p.s.inst.verify("sample-game").unwrap().intact);

    p.s.inst.uninstall("sample-game").unwrap();
    assert_eq!(tree(&p.s.game), before, "uninstall removes the DLL and the copies, nothing else");
}

/// The whole play loop with a real program standing in for the game: swap in, start it directly, see it in the
/// process list, see it gone, restore.
#[cfg(windows)]
#[test]
fn play_loop_with_a_real_process() {
    use std::time::{Duration, Instant};
    let p = launch_setup();
    let original_game = tree(&p.s.game);
    let (zip, _) = launch_pack(&p, "play", serde_json::json!({ "mode": "swap", "exe": "SampleGame.exe", "via": "direct", "args": ["-n", "3", "127.0.0.1"] }), serde_json::json!([])).unwrap();
    let rec = p.s.inst.install(&opts(&p.s, &zip, None, false), &mut |_| {}).unwrap();
    // A unique exe name, so other tests and programs can't be mistaken for it.
    let exe_name = format!("tirgumimtest{}.exe", std::process::id());
    let mut act = rec.activation.clone().unwrap();
    act.exe = exe_name.clone();
    fs::copy(std::path::Path::new(&std::env::var("SystemRoot").unwrap()).join("System32/PING.EXE"), p.s.game.join(&exe_name)).unwrap();

    swap::activate(&p.s.game, "sample-game", &rec.swap).unwrap();
    let cmd = super::launch::build(&act, &p.s.game, None, None, &Default::default()).unwrap();
    super::launch::spawn(&cmd).unwrap();
    let name = exe_name.to_lowercase();
    let t = Instant::now();
    let mut seen = false;
    while t.elapsed() < Duration::from_secs(30) {
        let running = super::detect::running_processes().contains(&name);
        if running {
            seen = true;
        } else if seen {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    assert!(seen, "the started program shows in the process list");
    assert!(!super::detect::running_processes().contains(&name), "and is gone after it exits");
    swap::restore(&p.s.game, &rec.swap).unwrap();
    fs::remove_file(p.s.game.join(&exe_name)).unwrap();
    assert_eq!(game_tree(&p.s.game), original_game);
}

#[test]
fn launch_manifest_rules() {
    let h = |c: char| c.to_string().repeat(64);
    let m = |act: serde_json::Value, patches: serde_json::Value, min: &str| {
        serde_json::json!({ "schema": 1, "packId": "p", "gameId": "g", "version": "1", "minLauncherVersion": min,
            "files": [{ "path": "a.txt", "sha256": h('c') }], "patches": patches, "activation": act }).to_string()
    };
    let args = serde_json::json!({ "mode": "args", "exe": "Game.exe", "args": ["-game", "he.win"] });
    let swap_act = serde_json::json!({ "mode": "swap", "exe": "Game.exe" });
    let bs = |out: Option<&str>| {
        let mut v = serde_json::json!({ "path": "data.win", "fromSha256": h('a'), "toSha256": h('b'), "patch": "patches/data.win.bsdiff" });
        if let Some(o) = out { v["output"] = o.into(); }
        v
    };
    let ap = serde_json::json!({ "path": "big.dat", "kind": "append", "fromSha256": h('a'), "fromSize": 10, "toSha256": h('b'), "data": "patches/big.append" });
    let ok = |s: String| manifest::parse_manifest(&s).map(|_| ()).map_err(|e| e);
    assert!(ok(m(args.clone(), serde_json::json!([bs(Some("he.win"))]), "0.4.0")).is_ok());
    assert!(ok(m(swap_act.clone(), serde_json::json!([bs(None), ap.clone()]), "0.4.0")).is_ok());
    assert!(ok(m(args.clone(), serde_json::json!([bs(Some("he.win"))]), "0.3.0")).is_err(), "0.3.0 would install it for good");
    assert!(ok(m(args.clone(), serde_json::json!([bs(None)]), "0.4.0")).is_err(), "args patches must write a new file");
    assert!(ok(m(args.clone(), serde_json::json!([ap.clone()]), "0.4.0")).is_err(), "append changes the original");
    assert!(ok(m(swap_act.clone(), serde_json::json!([bs(Some("he.win"))]), "0.4.0")).is_err());
    assert!(ok(m(serde_json::Value::Null, serde_json::json!([ap]), "0.4.0")).is_err(), "append is for swap packs");
    // Steam drops quotes: no {gameDir} or spaces in Steam args; a direct launch may use {gameDir}.
    for bad in ["{gameDir}/he.win", "my he.win"] {
        let mut a = args.clone();
        a["args"] = serde_json::json!(["-game", bad]);
        assert!(ok(m(a, serde_json::json!([bs(Some("he.win"))]), "0.4.0")).is_err(), "{bad} with via steam");
    }
    let mut direct = args.clone();
    direct["args"] = serde_json::json!(["-game", "{gameDir}/he.win"]);
    direct["via"] = "direct".into();
    assert!(ok(m(direct, serde_json::json!([bs(Some("he.win"))]), "0.4.0")).is_ok());
    let mut bad_arg = args.clone();
    bad_arg["args"] = serde_json::json!(["{appData}/x"]);
    assert!(ok(m(bad_arg, serde_json::json!([bs(Some("he.win"))]), "0.4.0")).is_err());
    let own = serde_json::json!({ "schema": 1, "packId": "p", "gameId": "g", "version": "1", "files": [{ "path": ".tirgumim/x", "sha256": h('c') }] });
    assert!(manifest::parse_manifest(&own.to_string()).is_err());
}
