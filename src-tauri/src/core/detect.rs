//! Finding games on this PC: Steam (registry → libraryfolders.vdf → appmanifest_*.acf), Epic (the launcher's
//! .item manifests), and Steam's own cached cover art. Also which programs are running (a game must be closed
//! before its files are changed).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde::Serialize;

use super::util::resolve_inside;

#[cfg(windows)]
fn hidden(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn hidden(cmd: &mut Command) -> &mut Command {
    cmd
}

pub fn reg_query(key: &str, value: &str) -> Option<String> {
    let out = hidden(Command::new("reg").args(["query", key, "/v", value])).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let t = line.trim();
        if t.len() > value.len() && t[..value.len()].eq_ignore_ascii_case(value) {
            let rest = t[value.len()..].trim_start();
            if let Some(i) = rest.find("REG_") {
                let after = &rest[i..];
                let v = after.split_once(char::is_whitespace).map(|(_, v)| v.trim()).unwrap_or("");
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// A Valve KeyValues (VDF/ACF) value: a string or a block.
#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Obj(BTreeMap<String, Vdf>),
}

impl Vdf {
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(m) => m.get(key).or_else(|| m.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)),
            Vdf::Str(_) => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }
}

/// Enough of the VDF format for libraryfolders.vdf and appmanifest files.
pub fn parse_vdf(text: &str) -> Vdf {
    #[derive(PartialEq)]
    enum Tok {
        S(String),
        Open,
        Close,
    }
    let mut toks = Vec::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(ch) = it.next() {
                    match ch {
                        '\\' => match it.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(o) => s.push(o),
                            None => {}
                        },
                        '"' => break,
                        o => s.push(o),
                    }
                }
                toks.push(Tok::S(s));
            }
            '{' => toks.push(Tok::Open),
            '}' => toks.push(Tok::Close),
            '/' if it.peek() == Some(&'/') => {
                for ch in it.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    fn obj(toks: &[Tok], i: &mut usize) -> BTreeMap<String, Vdf> {
        let mut m = BTreeMap::new();
        while *i < toks.len() {
            let t = &toks[*i];
            *i += 1;
            let key = match t {
                Tok::Close => return m,
                Tok::Open => continue,
                Tok::S(s) => s.clone(),
            };
            match toks.get(*i) {
                Some(Tok::Open) => {
                    *i += 1;
                    m.insert(key, Vdf::Obj(obj(toks, i)));
                }
                Some(Tok::S(v)) => {
                    *i += 1;
                    m.insert(key, Vdf::Str(v.clone()));
                }
                _ => {}
            }
        }
        m
    }
    let mut i = 0;
    Vdf::Obj(obj(&toks, &mut i))
}

pub fn steam_roots() -> &'static [PathBuf] {
    static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut roots: Vec<PathBuf> = Vec::new();
        let reg = reg_query("HKCU\\Software\\Valve\\Steam", "SteamPath")
            .or_else(|| reg_query("HKLM\\SOFTWARE\\WOW6432Node\\Valve\\Steam", "InstallPath"));
        if let Some(r) = reg {
            roots.push(PathBuf::from(r.replace('/', "\\")));
        }
        roots.push(PathBuf::from("C:\\Program Files (x86)\\Steam"));
        let mut seen = HashSet::new();
        roots.retain(|r| r.join("steamapps").is_dir() && seen.insert(r.to_string_lossy().to_lowercase()));
        roots
    })
}

pub fn steam_libraries() -> Vec<PathBuf> {
    let mut libs: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |p: PathBuf, libs: &mut Vec<PathBuf>| {
        if seen.insert(p.to_string_lossy().to_lowercase().trim_end_matches('\\').to_string()) {
            libs.push(p);
        }
    };
    for r in steam_roots() {
        add(r.clone(), &mut libs);
        let Ok(text) = fs::read_to_string(r.join("steamapps").join("libraryfolders.vdf")) else { continue };
        let data = parse_vdf(&text);
        if let Some(Vdf::Obj(lf)) = data.get("libraryfolders") {
            for v in lf.values() {
                let p = v.str().map(str::to_string).or_else(|| v.get("path").and_then(Vdf::str).map(str::to_string));
                if let Some(p) = p {
                    add(PathBuf::from(p.replace('/', "\\")), &mut libs);
                }
            }
        }
    }
    libs
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PcGame {
    /// "steam" or "epic"
    pub store: String,
    /// Steam app id or Epic AppName.
    pub id: String,
    pub name: String,
    pub dir: String,
}

// Steam tools and runtimes that aren't games.
fn not_a_game(name: &str) -> bool {
    let n = name.to_lowercase();
    ["redistributable", "proton", "steam linux runtime", "steamworks", "steamvr", "dedicated server", " sdk", "soundtrack"]
        .iter()
        .any(|w| n.contains(w))
        || n.ends_with(" sdk")
        || n == "sdk"
}

pub fn list_steam_games() -> Vec<PcGame> {
    let mut out: BTreeMap<String, PcGame> = BTreeMap::new();
    for lib in steam_libraries() {
        let sa = lib.join("steamapps");
        let Ok(rd) = fs::read_dir(&sa) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = fs::read_to_string(e.path()) else { continue };
            let v = parse_vdf(&text);
            let Some(st) = v.get("AppState") else { continue };
            let (Some(appid), Some(dir), Some(title)) =
                (st.get("appid").and_then(Vdf::str), st.get("installdir").and_then(Vdf::str), st.get("name").and_then(Vdf::str))
            else {
                continue;
            };
            if not_a_game(title) || appid == "228980" {
                continue;
            }
            let full = sa.join("common").join(dir);
            if full.is_dir() {
                out.insert(appid.into(), PcGame { store: "steam".into(), id: appid.into(), name: title.into(), dir: full.to_string_lossy().into() });
            }
        }
    }
    out.into_values().collect()
}

pub fn find_steam(app_id: &str) -> Option<PathBuf> {
    if app_id.is_empty() {
        return None;
    }
    for lib in steam_libraries() {
        let sa = lib.join("steamapps");
        let Ok(text) = fs::read_to_string(sa.join(format!("appmanifest_{app_id}.acf"))) else { continue };
        if let Some(dir) = parse_vdf(&text).get("AppState").and_then(|s| s.get("installdir")).and_then(Vdf::str) {
            let full = sa.join("common").join(dir);
            if full.is_dir() {
                return Some(full);
            }
        }
    }
    None
}

fn epic_manifests() -> Vec<serde_json::Value> {
    let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    let dir = Path::new(&base).join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests");
    let Ok(rd) = fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("item")))
        .filter_map(|e| super::util::read_json::<serde_json::Value>(&e.path()))
        .collect()
}

pub fn list_epic_games() -> Vec<PcGame> {
    let mut out = Vec::new();
    for j in epic_manifests() {
        let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let (app, name, dir) = (s("AppName"), s("DisplayName"), s("InstallLocation"));
        if app.is_empty() || name.is_empty() || dir.is_empty() || !Path::new(&dir).is_dir() {
            continue;
        }
        if j.get("bIsIncompleteInstall").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        if let Some(cats) = j.get("AppCategories").and_then(|v| v.as_array()) {
            if !cats.is_empty() && !cats.iter().any(|c| c.as_str() == Some("games")) {
                continue;
            }
        }
        let main = s("MainGameAppName");
        if !main.is_empty() && main != app {
            continue; // DLC
        }
        out.push(PcGame { store: "epic".into(), id: app, name, dir });
    }
    out
}

pub fn find_epic(app_name: &str) -> Option<PathBuf> {
    if app_name.is_empty() {
        return None;
    }
    epic_manifests().into_iter().find_map(|j| {
        let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let dir = s("InstallLocation");
        ((s("AppName") == app_name || s("MainGameAppName") == app_name) && Path::new(&dir).is_dir()).then(|| PathBuf::from(dir))
    })
}

/// Steam's own cached art for an app: `kind` is "cover" (600x900 portrait) or "hero" (wide banner).
pub fn find_steam_art_file(app_id: &str, kind: &str) -> Option<PathBuf> {
    if app_id.is_empty() || !app_id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let wanted: &[&str] = match kind {
        "hero" => &["library_hero.jpg", "library_hero_2x.jpg"],
        _ => &["library_600x900.jpg", "library_capsule.jpg", "library_600x900_2x.jpg"],
    };
    for root in steam_roots() {
        let base = root.join("appcache").join("librarycache");
        let mut cands: Vec<PathBuf> = wanted.iter().map(|w| base.join(format!("{app_id}_{w}"))).collect(); // older layout
        let app_dir = base.join(app_id);
        cands.extend(wanted.iter().map(|w| app_dir.join(w)));
        if let Ok(rd) = fs::read_dir(&app_dir) {
            for sub in rd.flatten().filter(|e| e.path().is_dir()) {
                cands.extend(wanted.iter().map(|w| sub.path().join(w)));
            }
        }
        if let Some(hit) = cands.into_iter().find(|c| c.is_file()) {
            return Some(hit);
        }
    }
    None
}

/// Does `dir` hold this game (its marker file)?
pub fn is_game_dir(dir: &Path, marker: Option<&str>) -> bool {
    if !dir.is_dir() {
        return false;
    }
    match marker.filter(|m| !m.is_empty()) {
        None => true,
        Some(m) => resolve_inside(dir, m).map(|p| p.exists()).unwrap_or(false),
    }
}

/// The names (lower case) of the programs running now.
pub fn running_processes() -> HashSet<String> {
    let Ok(out) = hidden(Command::new("tasklist").args(["/FO", "CSV", "/NH"])).output() else { return HashSet::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix('"').and_then(|r| r.split_once('"')).map(|(name, _)| name.to_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf() {
        let v = parse_vdf(
            r#""libraryfolders"
            {
                "0" { "path" "C:\\Program Files (x86)\\Steam" "apps" { "228980" "1" } }
                "1" { "path" "D:\\SteamLibrary" } // comment
            }"#,
        );
        let lf = v.get("libraryfolders").unwrap();
        assert_eq!(lf.get("0").unwrap().get("path").unwrap().str(), Some("C:\\Program Files (x86)\\Steam"));
        assert_eq!(lf.get("1").unwrap().get("path").unwrap().str(), Some("D:\\SteamLibrary"));
        let acf = parse_vdf("\"AppState\"\n{\n\t\"appid\"\t\t\"4115450\"\n\t\"installdir\"\t\t\"Phantom Blade Zero\"\n}");
        assert_eq!(acf.get("appstate").unwrap().get("installdir").unwrap().str(), Some("Phantom Blade Zero"));
    }

    #[test]
    fn tools_are_not_games() {
        assert!(not_a_game("Steamworks Common Redistributables"));
        assert!(not_a_game("Proton 9.0"));
        assert!(!not_a_game("Phantom Blade Zero"));
    }
}
