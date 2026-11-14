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
