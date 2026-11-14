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
