//! Finds translations straight from our GitHub repos, so a new game never needs a catalog commit or a launcher
//! update: every repo of `OWNER` with the topic `TOPIC` is a translation. Its game entry is `pack/catalog-entry.json`
//! in the repo (the same fields as a `catalog.json` game; `pack` defaults to the repo's own releases).
//!
//! GitHub calls: one search (its own limit, 10 a minute without a login) per refresh, and the entry files from
//! raw.githubusercontent.com (no API limit). The answer is kept for 10 minutes; Ctrl+F5 asks again. If GitHub can't
//! be reached, the last answer is used.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::catalog::{fetch_text, parse_catalog_tolerant, CatalogGame, GITHUB_CACHE_SECS};
use super::util;

/// Whose repos count (only theirs: anyone can put a topic on their own repo).
pub const OWNER: &str = "NamiSwan222";
pub const TOPIC: &str = "tirgumim-pack";
pub const ENTRY_FILE: &str = "pack/catalog-entry.json";
/// Even Ctrl+F5 doesn't search again sooner than this (the search limit is 10 a minute).
pub const FORCE_MIN_SECS: u64 = 10;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cached {
    fetched_at: u64,
    /// Raw entries, as the repos have them (with `pack.github` filled in).
    entries: Vec<Value>,
}

pub struct Discovery {
    file: PathBuf,
    /// Tests point these at file:// URLs.
    pub api_base: String,
    pub raw_base: String,
}

#[derive(Debug)]
pub struct Found {
    pub games: Vec<CatalogGame>,
    /// From GitHub just now (false: the kept answer).
    pub fresh: bool,
    /// Repos whose entry couldn't be read (shown in the log, never as an error to the player).
    pub skipped: Vec<String>,
}

impl Discovery {
    pub fn new(data_dir: &Path) -> Self {
        Discovery { file: data_dir.join("discovered.json"), api_base: "https://api.github.com".into(), raw_base: "https://raw.githubusercontent.com".into() }
    }

    pub fn find(&self, force: bool) -> Result<Found, String> {
        let cached: Option<Cached> = util::read_json(&self.file);
        let age = cached.as_ref().map(|c| util::now_secs().saturating_sub(c.fetched_at));
        if let (Some(c), Some(age)) = (&cached, age) {
            if age < if force { FORCE_MIN_SECS } else { GITHUB_CACHE_SECS } {
                return Ok(Found { games: to_games(&c.entries), fresh: false, skipped: vec![] });
            }
        }
        match self.ask_github() {
            Ok((entries, skipped)) => {
                let _ = util::write_json_atomic(&self.file, &Cached { fetched_at: util::now_secs(), entries: entries.clone() });
                Ok(Found { games: to_games(&entries), fresh: true, skipped })
            }
            Err(e) => match cached {
                Some(c) => Ok(Found { games: to_games(&c.entries), fresh: false, skipped: vec![] }),
                None => Err(e),
            },
        }
    }

    /// The repos' entries as they are (for the strict `--check-catalog --discover`), and the repos that failed.
    pub fn fetch_entries(&self) -> Result<(Vec<Value>, Vec<String>), String> {
        self.ask_github()
    }

    fn ask_github(&self) -> Result<(Vec<Value>, Vec<String>), String> {
        let url = format!("{}/search/repositories?q=topic:{TOPIC}+user:{OWNER}&per_page=100", self.api_base.trim_end_matches('/'));
        let text = fetch_text(&url)?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("GitHub search: {e}"))?;
        let items = v.get("items").and_then(Value::as_array).ok_or("GitHub search gave no items")?;
        Ok(self.entries_from_items(items))
    }

    /// Each found repo's entry file (skipping archived repos and anyone else's).
    fn entries_from_items(&self, items: &[Value]) -> (Vec<Value>, Vec<String>) {
        let mut entries = Vec::new();
        let mut skipped = Vec::new();
        for it in items {
            let s = |k: &str| it.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let (repo, branch) = (s("full_name"), s("default_branch"));
            let owner_ok = repo.split('/').next().is_some_and(|o| o.eq_ignore_ascii_case(OWNER));
            if repo.is_empty() || !owner_ok || it.get("archived").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let branch = if branch.is_empty() { "main".to_string() } else { branch };
            let raw = format!("{}/{repo}/{branch}/{ENTRY_FILE}", self.raw_base.trim_end_matches('/'));
            match fetch_text(&raw).and_then(|t| serde_json::from_str::<Value>(util::strip_bom(&t)).map_err(|e| e.to_string())) {
                Ok(mut e) if e.is_object() => {
                    fill_pack(&mut e, &repo);
                    entries.push(e);
                }
                Ok(_) => skipped.push(format!("{repo}: {ENTRY_FILE} is not an object")),
                Err(e) => skipped.push(format!("{repo}: {e}")),
            }
        }
        (entries, skipped)
    }
}

/// A repo's entry gets its pack from the repo's own releases unless it says otherwise. A paid entry doesn't: its
/// pack comes from the account server only (ENTITLEMENTS.md), never from public releases.
fn fill_pack(e: &mut Value, repo: &str) {
    if e.get("paid").and_then(Value::as_bool) == Some(true) {
        return;
    }
    match e.get_mut("pack") {
        Some(Value::Object(p)) => {
            if !p.contains_key("github") && !p.contains_key("url") {
                p.insert("github".into(), json!(repo));
            }
        }
        _ => e["pack"] = json!({ "github": repo }),
    }
}

/// Entries this launcher can read (one it can't is left out, as in the catalog).
fn to_games(entries: &[Value]) -> Vec<CatalogGame> {
    let doc = json!({ "schema": 1, "games": entries });
    parse_catalog_tolerant(&doc.to_string(), &format!("https://github.com/{OWNER}/")).map(|c| c.games).unwrap_or_default()
}

/// The catalog's games plus the discovered ones; a discovered entry replaces a catalog entry with the same id.
pub fn merge(catalog: Vec<CatalogGame>, found: Vec<CatalogGame>) -> Vec<CatalogGame> {
    let mut out: Vec<CatalogGame> = catalog.into_iter().filter(|g| !found.iter().any(|f| f.id == g.id)).collect();
    out.extend(found);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_tagged_repos_reads_their_entries_and_keeps_the_answer() {
        let tmp = tempfile::tempdir().unwrap();
        let api = tmp.path().join("api");
        let raw = tmp.path().join("raw");
        fs::create_dir_all(api.join("search")).unwrap();
        let items = json!({ "items": [
            { "full_name": format!("{OWNER}/game-a-he"), "default_branch": "main" },
            { "full_name": format!("{OWNER}/game-b-he"), "default_branch": "master" },
            { "full_name": format!("{OWNER}/old-he"), "default_branch": "main", "archived": true },
            { "full_name": "someone-else/fake-he", "default_branch": "main" },
            { "full_name": format!("{OWNER}/broken-he"), "default_branch": "main" }
        ] });
        // file:// can't carry a query string, so the test serves the search answer at the path the URL starts with.
        fs::write(api.join("search").join("repositories"), items.to_string()).unwrap();
        let entry = |dir: &str, branch: &str, v: Value| {
            let p = raw.join(OWNER).join(dir).join(branch).join("pack");
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("catalog-entry.json"), v.to_string()).unwrap();
        };
        entry("game-a-he", "main", json!({ "id": "game-a", "name": "Game A", "detect": { "steamAppId": "1" } }));
        entry("game-b-he", "master", json!({ "id": "game-b", "name": "Game B", "detect": { "steamAppId": "2" },
            "pack": { "tag": "game-b-*", "asset": "game-b-he-*.zip" } }));
        entry("broken-he", "main", json!(["not", "an", "entry"]));

        let mut d = Discovery::new(tmp.path());
        let url = |p: &Path| url::Url::from_file_path(p).unwrap().to_string();
        d.api_base = url(&api);
        d.raw_base = url(&raw);
        let items: Vec<Value> = serde_json::from_str::<Value>(&fs::read_to_string(api.join("search").join("repositories")).unwrap()).unwrap()["items"].as_array().unwrap().clone();
        let (entries, skipped) = d.entries_from_items(&items);
        util::write_json_atomic(&d.file, &Cached { fetched_at: util::now_secs(), entries: entries.clone() }).unwrap();
        let found = Found { games: to_games(&entries), fresh: true, skipped };
        let ids: Vec<_> = found.games.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["game-a", "game-b"]);
        assert_eq!(found.games[0].pack.as_ref().unwrap().github.as_deref(), Some(&*format!("{OWNER}/game-a-he")));
        assert_eq!(found.games[1].pack.as_ref().unwrap().tag.as_deref(), Some("game-b-*"));
        assert_eq!(found.skipped.len(), 1, "{:?}", found.skipped);
        assert!(found.fresh);

        // GitHub gone: the kept answer is used.
        d.api_base = "https://127.0.0.1:1/".into();
        let again = d.find(false).unwrap();
        assert!(!again.fresh);
        assert_eq!(again.games.len(), 2);

        let merged = merge(vec![CatalogGame { id: "game-a".into(), name: "Old A".into(), ..Default::default() }, CatalogGame { id: "c".into(), name: "C".into(), ..Default::default() }], found.games);
        assert_eq!(merged.iter().map(|g| (g.id.as_str(), g.name.as_str())).collect::<Vec<_>>(), [("c", "C"), ("game-a", "Game A"), ("game-b", "Game B")]);
    }

    #[test]
    fn paid_entries_get_no_public_pack() {
        let mut free = json!({ "id": "a", "name": "A" });
        fill_pack(&mut free, "o/a");
        assert_eq!(free["pack"]["github"], "o/a");
        let mut paid = json!({ "id": "b", "name": "B", "paid": true });
        fill_pack(&mut paid, "o/b");
        assert!(paid.get("pack").is_none());
    }
}
