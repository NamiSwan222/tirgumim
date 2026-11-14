//! `--check-catalog`: the checks GitHub Actions runs on every change to `catalog/catalog.json`, so a broken list
//! never reaches players. It uses the same parser the launcher uses, plus rules for the published catalog.

use serde_json::Value;

use super::catalog::{parse_catalog, pick_release, Catalog, NO_RELEASE};
use super::util::wildcard;

/// Our own repository: launcher releases (`v*`) and the updater's `updater` release live here too.
pub const OWN_REPO: &str = "NamiSwan222/tirgumim";

#[derive(Debug, Default)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report {
    fn err(&mut self, id: &str, msg: impl AsRef<str>) {
        self.errors.push(format!("{id}: {}", msg.as_ref()));
    }
    fn warn(&mut self, id: &str, msg: impl AsRef<str>) {
        self.warnings.push(format!("{id}: {}", msg.as_ref()));
    }
}

fn https(u: &str) -> bool {
    u.starts_with("https://")
}

/// Checks the catalog without the network. Returns the parsed catalog (if it parses) and what is wrong.
pub fn check_offline(text: &str) -> (Option<Catalog>, Report) {
    let mut r = Report::default();
    // Parsed against the published URL, as players' launchers do (relative pack URLs are resolved against it).
    let base = format!("https://raw.githubusercontent.com/{OWN_REPO}/main/catalog/catalog.json");
    let c = match parse_catalog(text, &base) {
        Ok(c) => c,
        Err(e) => {
            r.errors.push(e);
            return (None, r);
        }
    };
    for g in &c.games {
        let id = g.id.as_str();
        if g.name.trim().is_empty() {
            r.err(id, "name is empty");
        }
        if g.name_he.as_deref().is_none_or(|n| n.trim().is_empty()) {
            r.warn(id, "no nameHe: the English name is shown");
        }
        if g.about_he.as_deref().is_none_or(|n| n.trim().is_empty()) {
            r.warn(id, "no aboutHe");
        }
        if g.dev_path.is_some() {
            r.err(id, "devPath is for the dev catalog only");
        }
        let d = &g.detect;
        if d.steam_app_id.is_none() && d.epic_app_name.is_none() && d.marker_file.is_none() {
            r.err(id, "detect needs steamAppId, epicAppName or markerFile");
        }
        if d.steam_app_id.as_deref().is_some_and(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit())) {
            r.err(id, "steamAppId must be digits");
        }
        if d.marker_file.as_deref().is_some_and(|m| m.contains("..") || m.contains('\\') || m.contains(':') || m.starts_with('/')) {
            r.err(id, "markerFile must be a relative path with forward slashes");
        }
        if d.process_names.is_empty() {
            r.warn(id, "no processNames: installing while the game runs can't be refused");
        }
        for u in [&g.poster, &g.hero].into_iter().flatten() {
            if !https(u) {
                r.err(id, format!("poster/hero must be https: {u}"));
            }
        }
        for q in &g.requires {
            if q.path.trim().is_empty() || q.message.trim().is_empty() {
                r.err(id, "every requires entry needs path and message");
            }
            if q.url.as_deref().is_some_and(|u| !https(u)) {
                r.err(id, "requires url must be https");
            }
        }
        if g.status.as_deref().is_some_and(|s| s != "inProgress") {
            r.err(id, format!("unknown status \"{}\" (only \"inProgress\")", g.status.as_deref().unwrap_or("")));
        }
        match &g.pack {
            None => {
                if g.status.is_none() {
                    r.warn(id, "no pack and not marked inProgress");
                }
            }
            Some(p) if p.github.is_some() => {
                let repo = p.github.as_deref().unwrap_or("");
                let tag = p.tag.as_deref().unwrap_or("*");
                if p.url.is_some() || p.sha256.is_some() {
                    r.err(id, "pack has both github and url/sha256; use one");
                }
                if repo.eq_ignore_ascii_case(OWN_REPO) {
                    if p.tag.is_none() {
                        r.err(id, format!("a pack in {OWN_REPO} needs a tag pattern like \"{id}-*\""));
                    }
                    if wildcard(tag, "v0.3.0") || wildcard(tag, "updater") {
                        r.err(id, format!("tag pattern \"{tag}\" also matches the launcher's own releases"));
                    }
                }
                if p.asset.is_none() {
                    r.warn(id, "no asset pattern: the first .zip in the release is used");
                }
            }
            Some(p) => {
                if p.version.is_none() || p.url.is_none() || p.sha256.is_none() {
                    r.err(id, "pack needs url, sha256 and version, or github");
                }
                if p.url.as_deref().is_some_and(|u| !https(u)) {
                    r.err(id, "pack url must be https");
                }
            }
        }
    }
    (Some(c), r)
}

/// For packs on GitHub releases: does a release match, with a checksum? `releases(repo)` returns GitHub's list.
pub fn check_releases(c: &Catalog, r: &mut Report, releases: &mut dyn FnMut(&str) -> Result<Vec<Value>, String>) {
    for g in &c.games {
        let Some(p) = g.pack.as_ref().filter(|p| p.github.is_some()) else { continue };
        let repo = p.github.as_deref().unwrap_or("");
        match releases(repo).and_then(|list| pick_release(&list, p)) {
            Ok(rp) => println!("  {}: {} {} ({})", g.id, repo, rp.version, rp.url),
            // Not published yet is fine while the translation is being made.
            Err(e) if e == NO_RELEASE && g.status.as_deref() == Some("inProgress") => r.warn(&g.id, "no release yet (inProgress)"),
            Err(e) if e == NO_RELEASE => r.err(&g.id, format!("no release in {repo} matches; mark it \"status\": \"inProgress\" until one exists")),
            Err(e) => r.err(&g.id, format!("{repo}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(v: Value) -> Report {
        check_offline(&v.to_string()).1
    }

    fn game(extra: Value) -> Value {
        let mut g = json!({
            "id": "my-game", "name": "My Game", "nameHe": "המשחק שלי", "aboutHe": "תרגום",
            "detect": { "steamAppId": "123", "processNames": ["MyGame.exe"] },
            "pack": { "github": "someone/my-game-he", "tag": "my-game-*", "asset": "my-game-he-*.zip" }
        });
        for (k, v) in extra.as_object().unwrap() {
            g[k] = v.clone();
        }
        json!({ "schema": 1, "games": [g] })
    }

    #[test]
    fn good_catalog_passes() {
        let r = check(game(json!({})));
        assert!(r.errors.is_empty() && r.warnings.is_empty(), "{r:?}");
        assert!(check(json!({ "schema": 1, "games": [] })).errors.is_empty());
    }

    #[test]
    fn catches_mistakes() {
        assert!(!check(json!({ "schema": 2, "games": [] })).errors.is_empty());
        assert!(!check(game(json!({ "detect": {} }))).errors.is_empty());
        assert!(!check(game(json!({ "devPath": "D:\\x" }))).errors.is_empty());
        assert!(!check(game(json!({ "status": "ready" }))).errors.is_empty());
        assert!(!check(game(json!({ "pack": { "url": "https://x/y.zip" } }))).errors.is_empty());
        assert!(!check(game(json!({ "pack": { "version": "1", "url": "http://x/y.zip", "sha256": "a".repeat(64) } }))).errors.is_empty());
        // A pack in our own repo must not pick up the launcher's releases.
        assert!(!check(game(json!({ "pack": { "github": OWN_REPO, "asset": "*.zip" } }))).errors.is_empty());
        assert!(!check(game(json!({ "pack": { "github": OWN_REPO, "tag": "*", "asset": "*.zip" } }))).errors.is_empty());
        assert!(check(game(json!({ "pack": { "github": OWN_REPO, "tag": "my-game-*", "asset": "my-game-he-*.zip" } }))).errors.is_empty());
    }

    #[test]
    fn releases_must_exist_unless_in_progress() {
        let (c, mut r) = check_offline(&game(json!({})).to_string());
        check_releases(&c.unwrap(), &mut r, &mut |_| Ok(vec![]));
        assert_eq!(r.errors.len(), 1, "{r:?}");

        let (c, mut r) = check_offline(&game(json!({ "status": "inProgress" })).to_string());
        check_releases(&c.unwrap(), &mut r, &mut |_| Ok(vec![]));
        assert!(r.errors.is_empty() && r.warnings.len() == 1, "{r:?}");
    }
}
