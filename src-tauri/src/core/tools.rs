//! Tools for whoever makes packs: `--make-pack` builds a pack zip from a folder, `--dev-setup` builds the sample
//! game, the sample pack and the dev catalog.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::manifest::{self, MANIFEST_NAME};
use super::util::{self, sha256_file};

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out)?;
        } else {
            out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

pub struct MadePack {
    pub out: PathBuf,
    pub manifest: Value,
    /// The `pack` entry for catalog.json.
    pub entry: Value,
}

/// Builds a pack from `pack.config.json` (every manifest field except `files`, plus `filesDir`). A game-version
/// check with only a `path` gets its sha256 and size from `game_dir`, an untouched install of that game build.
pub fn make_pack(config: &Path, game_dir: Option<&Path>, out: Option<&Path>, url: Option<&str>) -> Result<MadePack, String> {
    let cfg_dir = config.parent().unwrap_or(Path::new("."));
    let text = fs::read_to_string(config).map_err(|e| format!("{}: {e}", config.display()))?;
    let mut cfg: Map<String, Value> = serde_json::from_str(util::strip_bom(&text)).map_err(|e| format!("{}: {e}", config.display()))?;
    let files_dir = cfg_dir.join(cfg.remove("filesDir").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "files".into()));

    let mut rels = Vec::new();
    walk(&files_dir, &files_dir, &mut rels).map_err(|e| format!("{}: {e}", files_dir.display()))?;
    rels.sort();
    let mut files = Vec::new();
    for rel in &rels {
        let abs = files_dir.join(rel);
        files.push(json!({ "path": rel, "sha256": sha256_file(&abs).map_err(|e| e.to_string())?, "size": fs::metadata(&abs).map_err(|e| e.to_string())?.len() }));
    }

    if let Some(Value::Array(gvs)) = cfg.get_mut("gameVersions") {
        for gv in gvs {
            if let Some(Value::Array(checks)) = gv.get_mut("checks") {
                for c in checks {
                    if c.get("sha256").is_some() || c.get("size").is_some() || c.get("exists").is_some() {
                        continue;
                    }
                    let p = c.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                    let gd = game_dir.ok_or(format!("Check \"{p}\" needs --game-dir to compute its hash"))?;
                    let abs = util::resolve_inside(gd, &p)?;
                    c["sha256"] = json!(sha256_file(&abs).map_err(|e| format!("{}: {e}", abs.display()))?);
                    c["size"] = json!(fs::metadata(&abs).map_err(|e| e.to_string())?.len());
                }
            }
        }
    }

    let mut m = Map::new();
    m.insert("schema".into(), json!(manifest::SCHEMA));
    m.extend(cfg);
    m.insert("files".into(), Value::Array(files));
    let manifest_json = Value::Object(m);
    let parsed: manifest::Manifest = serde_json::from_value(manifest_json.clone()).map_err(|e| format!("Invalid pack manifest: {e}"))?;
    let parsed = manifest::validate_manifest(parsed)?;

    let out = out.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(format!("{}-{}.zip", parsed.pack_id, parsed.version)));
    if let Some(d) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    {
        let f = fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file(MANIFEST_NAME, opt).map_err(|e| e.to_string())?;
        z.write_all(&serde_json::to_vec_pretty(&manifest_json).unwrap()).map_err(|e| e.to_string())?;
        for rel in &rels {
            z.start_file(format!("files/{rel}"), opt).map_err(|e| e.to_string())?;
            z.write_all(&fs::read(files_dir.join(rel)).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    let entry = json!({
        "version": parsed.version,
        "url": url.map(str::to_string).unwrap_or_else(|| out.file_name().unwrap().to_string_lossy().into_owned()),
        "sha256": sha256_file(&out).map_err(|e| e.to_string())?,
        "size": fs::metadata(&out).map_err(|e| e.to_string())?.len(),
    });
    Ok(MadePack { out, manifest: manifest_json, entry })
}

pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}

/// Builds a fresh `dev/sample-game`, the sample pack and `dev/catalog.json` (the sample game first, then the real
/// catalog's games).
pub fn dev_setup(root: &Path) -> Result<String, String> {
    let dev = root.join("dev");
    let game = dev.join("sample-game");
    let _ = fs::remove_dir_all(&game);
    copy_dir(&dev.join("sample-game-template"), &game).map_err(|e| e.to_string())?;
    let made = make_pack(
        &dev.join("sample-pack").join("pack.config.json"),
        Some(&dev.join("sample-game-template")),
        Some(&dev.join("packs").join("sample-game-he-0.1.0.zip")),
        Some("packs/sample-game-he-0.1.0.zip"),
    )?;
    let mut pack = made.entry.clone();
    pack["changelog"] = json!("חבילת בדיקה");
    let real: Value = serde_json::from_str(util::strip_bom(&fs::read_to_string(root.join("catalog").join("catalog.json")).map_err(|e| e.to_string())?))
        .map_err(|e| e.to_string())?;
    let mut games = vec![json!({
        "id": "sample-game", "name": "Sample Game", "nameHe": "משחק דוגמה", "engine": "custom",
        "aboutHe": "משחק בדיקה שמגיע עם המשגר.",
        "detect": { "markerFile": "SampleGame.exe", "processNames": ["SampleGame.exe"] },
        "devPath": game.to_string_lossy(),
        "pack": pack,
    })];
    games.extend(real["games"].as_array().cloned().unwrap_or_default());
    let catalog = json!({ "schema": 1, "updated": real["updated"], "games": games });
    let cat_path = dev.join("catalog.json");
    fs::write(&cat_path, serde_json::to_vec_pretty(&catalog).unwrap()).map_err(|e| e.to_string())?;
    Ok(format!("Sample game: {}\nPack: {}\nCatalog: {}", game.display(), made.out.display(), cat_path.display()))
}
