//! The Tirgumim window: commands the React page calls, the `poster` protocol for game art, progress events.
//! The work itself is in `core` (no window, tested with `cargo test`).

pub mod core;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::core::catalog::{self, Catalog, CatalogGame, ReleaseCache, ResolvedPack};
use crate::core::detect::{self, PcGame};
use crate::core::installer::{InstallOptions, InstallRecord, Installer, PackError, UninstallResult};
use crate::core::manifest::{self, ActivationMode, Via};
use crate::core::{launch, swap};

/// The `poster` protocol as the window reaches it (Tauri serves custom protocols at http://<name>.localhost on
/// Windows).
#[cfg(windows)]
const POSTER_BASE: &str = "http://poster.localhost";
#[cfg(not(windows))]
const POSTER_BASE: &str = "poster://localhost";

/// Where the launcher reads our translations list (the NamiSwan222/tirgumim repository). Settings can change it.
pub const DEFAULT_CATALOG_URL: &str = "https://raw.githubusercontent.com/NamiSwan222/tirgumim/main/catalog/catalog.json";

/// The project folder at build time: the dev catalog and the sample game live there.
fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

struct CatalogState {
    catalog: Option<Catalog>,
    /// "github" | "bundled" | "dev" | "custom"
    source: Option<String>,
    error: Option<String>,
    /// Each game's pack as resolved (GitHub release, or written in the catalog).
    packs: HashMap<String, Result<ResolvedPack, String>>,
}

pub struct Shared {
    installer: Installer,
    releases: ReleaseCache,
    dev: bool,
    bundled_catalog: PathBuf,
    posters_dir: PathBuf,
    cat: Mutex<CatalogState>,
    scanned: Mutex<Option<Vec<PcGame>>>,
    busy: AtomicBool,
    poster_misses: Mutex<HashSet<String>>,
    steam_ids: Mutex<HashMap<String, Option<String>>>,
    /// Games running in Hebrew from the launcher right now (swap packs: a watcher restores them on exit).
    sessions: Mutex<HashSet<String>>,
    /// The window was closed while a game ran: quit once every game is back to normal.
    exit_after: AtomicBool,
}

fn err(code: &str, msg: impl Into<String>) -> PackError {
    PackError::new(code, msg)
}

/// Runs blocking work off the window's thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, PackError> + Send + 'static) -> Result<T, PackError> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| err("ERROR", e.to_string()))?
}

struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Shared {
    fn catalog_url(&self) -> String {
        if self.dev {
            return url::Url::from_file_path(project_root().join("dev").join("catalog.json").canonicalize().unwrap_or_default())
                .map(|u| u.to_string())
                .unwrap_or_default();
        }
        self.installer.load_state().settings.catalog_url.unwrap_or_else(|| DEFAULT_CATALOG_URL.into())
    }

    /// Reads the catalog (GitHub, or the copy that ships with the launcher when GitHub can't be reached) and
    /// resolves every game's pack.
    fn refresh_catalog(&self, force: bool) {
        let url = self.catalog_url();
        let (mut catalog, mut source, mut error) = (None, None, None);
        match catalog::load_catalog(&url) {
            Ok(c) => {
                catalog = Some(c);
                source = Some(if self.dev { "dev" } else if url == DEFAULT_CATALOG_URL { "github" } else { "custom" }.to_string());
            }
            Err(e) => error = Some(e),
        }
        if catalog.is_none() {
            let bundled = url::Url::from_file_path(&self.bundled_catalog).map(|u| u.to_string()).unwrap_or_default();
            if let Ok(c) = catalog::load_catalog(&bundled) {
                catalog = Some(c);
                source = Some("bundled".into());
            }
        }
        let mut packs = HashMap::new();
        if let Some(c) = &catalog {
            for g in &c.games {
                if let Some(p) = &g.pack {
                    packs.insert(g.id.clone(), self.releases.resolve(p, force));
                }
            }
        }
        *self.cat.lock().unwrap() = CatalogState { catalog, source, error, packs };
    }

    fn rescan(&self) {
        let mut all = detect::list_steam_games();
        all.extend(detect::list_epic_games());
        *self.scanned.lock().unwrap() = Some(all);
    }

    fn game(&self, id: &str) -> Result<CatalogGame, PackError> {
        let cat = self.cat.lock().unwrap();
        cat.catalog.as_ref().and_then(|c| c.games.iter().find(|g| g.id == id).cloned()).ok_or_else(|| err("UNKNOWN_GAME", format!("Unknown game {id}")))
    }

    fn pack(&self, id: &str) -> Option<Result<ResolvedPack, String>> {
        self.cat.lock().unwrap().packs.get(id).cloned()
    }

    fn scanned_for(&self, g: &CatalogGame) -> Option<PcGame> {
        let d = &g.detect;
        let scanned = self.scanned.lock().unwrap();
        scanned.as_ref()?.iter().find(|s| {
            (s.store == "steam" && d.steam_app_id.as_deref().is_some_and(|a| a == s.id))
                || (s.store == "epic" && d.epic_app_name.as_deref().is_some_and(|a| a == s.id))
        }).cloned()
    }

    /// The game's folder: where the translation is installed, else the folder the player chose, else Steam/Epic.
    fn locate(&self, g: &CatalogGame) -> Option<(PathBuf, &'static str)> {
        let marker = g.detect.marker_file.as_deref();
        let st = self.installer.load_state();
        if let Some(rec) = st.installs.get(&g.id) {
            let p = PathBuf::from(&rec.game_dir);
            if p.is_dir() {
                return Some((p, "installed"));
            }
        }
        let custom = st.custom_paths.get(&g.id).cloned().or_else(|| if self.dev { g.dev_path.clone() } else { None });
        if let Some(c) = custom.map(PathBuf::from).filter(|c| detect::is_game_dir(c, marker)) {
            return Some((c, "custom"));
        }
        if let Some(s) = self.scanned_for(g) {
            let p = PathBuf::from(&s.dir);
            if detect::is_game_dir(&p, marker) {
                return Some((p, if s.store == "steam" { "steam" } else { "epic" }));
            }
        }
        if self.scanned.lock().unwrap().is_none() {
            if let Some(p) = g.detect.steam_app_id.as_deref().and_then(detect::find_steam).filter(|p| detect::is_game_dir(p, marker)) {
                return Some((p, "steam"));
            }
            if let Some(p) = g.detect.epic_app_name.as_deref().and_then(detect::find_epic).filter(|p| detect::is_game_dir(p, marker)) {
                return Some((p, "epic"));
            }
        }
        None
    }

    fn running(&self, g: &CatalogGame, procs: &HashSet<String>) -> Option<String> {
        g.detect.process_names.iter().find(|p| procs.contains(&p.to_lowercase())).cloned()
    }

    fn game_view(&self, g: &CatalogGame, procs: &HashSet<String>) -> Value {
        let loc = self.locate(g);
        let rec = self.installer.record(&g.id);
        // While a swap pack's files are in the game, its waiting copies are away: not a repair case.
        let swapped = rec.as_ref().is_some_and(|r| !r.swap.is_empty() && swap::is_active(Path::new(&r.game_dir)));
        let playing = swapped || self.sessions.lock().unwrap().contains(&g.id);
        let verify = rec.as_ref().filter(|_| !swapped).and_then(|_| self.installer.verify(&g.id));
        let pack = self.pack(&g.id);
        let available = pack.as_ref().and_then(|p| p.as_ref().ok()).cloned();
        // No matching release yet just means the translation isn't published: shown as "in progress".
        let pack_error = pack.as_ref().and_then(|p| p.as_ref().err()).filter(|e| e.as_str() != catalog::NO_RELEASE).cloned();
        let status = if rec.is_some() {
            if playing {
                "playing"
            } else if verify.as_ref().is_some_and(|v| !v.intact) {
                "needsRepair"
            } else if self.installer.update_available(&g.id, available.as_ref().map(|a| a.version.as_str())) {
                "updateAvailable"
            } else {
                "installed"
            }
        } else if g.min_launcher_version.as_deref().is_some_and(|m| crate::core::util::compare_versions(&self.installer.launcher_version, m).is_lt()) {
            "needsLauncher"
        } else if loc.is_none() {
            "notFound"
        } else if available.is_some() {
            "ready"
        } else if pack_error.is_some() {
            "unavailable"
        } else {
            "noPack"
        };
        let needs: Vec<Value> = loc
            .as_ref()
            .map(|(dir, _)| manifest::unmet_requires(&g.requires, dir).into_iter().map(|r| json!({ "message": r.message, "url": r.url })).collect())
            .unwrap_or_default();
        let id = urlencode(&g.id);
        json!({
            "id": g.id, "name": g.name, "nameHe": g.name_he, "aboutHe": g.about_he, "engine": g.engine,
            "inProgress": g.status.as_deref() == Some("inProgress"),
            "cover": format!("{}/cover/game/{id}", POSTER_BASE),
            "hero": format!("{}/hero/game/{id}", POSTER_BASE),
            "status": status,
            "location": loc.as_ref().map(|(d, s)| json!({ "dir": d.to_string_lossy(), "source": s })),
            "available": available,
            "packError": pack_error,
            "running": self.running(g, procs),
            "launchOnly": rec.as_ref().and_then(|r| r.activation.as_ref()).map(|a| match a.mode { ActivationMode::Args => "args", ActivationMode::Swap => "swap" }),
            "needs": needs,
            "minLauncherVersion": g.min_launcher_version,
            "installed": rec.as_ref().map(|r| json!({
                "version": r.version, "installedAt": r.installed_at, "gameVersion": r.game_version,
                "forced": r.forced_unknown_version, "changed": verify.as_ref().map(|v| v.changed.clone()).unwrap_or_default(),
                "files": r.files.len(),
            })),
        })
    }

    fn other_games(&self, catalog_games: &[CatalogGame]) -> Vec<Value> {
        let matched: HashSet<(String, String)> =
            catalog_games.iter().filter_map(|g| self.scanned_for(g)).map(|s| (s.store, s.id)).collect();
        let scanned = self.scanned.lock().unwrap();
        let mut v: Vec<&PcGame> = scanned.iter().flatten().filter(|s| !matched.contains(&(s.store.clone(), s.id.clone()))).collect();
        v.sort_by_key(|s| s.name.to_lowercase());
        v.into_iter()
            .map(|s| {
                let id = urlencode(&s.id);
                json!({ "store": s.store, "id": s.id, "name": s.name, "dir": s.dir,
                        "cover": format!("{}/cover/{}/{id}", POSTER_BASE, s.store),
                        "hero": format!("{}/hero/{}/{id}", POSTER_BASE, s.store) })
            })
            .collect()
    }

    fn list(&self, reload: bool) -> Value {
        if reload || self.cat.lock().unwrap().catalog.is_none() {
            self.refresh_catalog(reload);
        }
        if reload || self.scanned.lock().unwrap().is_none() {
            self.rescan();
        }
        let (games, source, error, updated) = {
            let c = self.cat.lock().unwrap();
            (
                c.catalog.as_ref().map(|c| c.games.clone()).unwrap_or_default(),
                c.source.clone(),
                c.error.clone(),
                c.catalog.as_ref().and_then(|c| c.updated.clone()),
            )
        };
        let procs = detect::running_processes();
        json!({
            "translations": games.iter().map(|g| self.game_view(g, &procs)).collect::<Vec<_>>(),
            "others": self.other_games(&games),
            "catalog": { "source": source, "error": error, "updated": updated },
        })
    }

    fn check_closed(&self, g: &CatalogGame) -> Result<(), PackError> {
        if let Some(p) = self.running(g, &detect::running_processes()) {
            return Err(err("GAME_RUNNING", format!("{p} is running")));
        }
        Ok(())
    }

    fn do_install(&self, app: &AppHandle, game_id: &str, force: bool, update: bool) -> Result<InstallRecord, PackError> {
        let g = self.game(game_id)?;
        let pack = match self.pack(game_id) {
            Some(Ok(p)) => p,
            Some(Err(e)) => return Err(err("PACK_UNAVAILABLE", e)),
            None => return Err(err("NO_PACK", "No translation pack yet for this game")),
        };
        if let Some(m) = g.min_launcher_version.as_deref().filter(|m| crate::core::util::compare_versions(&self.installer.launcher_version, m).is_lt()) {
            return Err(err("LAUNCHER_TOO_OLD", format!("This game needs launcher {m} or newer")));
        }
        let (dir, _) = self.locate(&g).ok_or_else(|| err("NO_GAME_DIR", "Game folder not found"))?;
        self.check_closed(&g)?;
        let unmet = manifest::unmet_requires(&g.requires, &dir);
        if !unmet.is_empty() {
            let mut e = err("REQUIRES", "Game folder is missing what the translation needs");
            e.details = unmet.iter().map(|r| r.message.clone()).collect();
            return Err(e);
        }
        let emit = |step: &str, fraction: Option<f64>| {
            let _ = app.emit("progress", json!({ "gameId": game_id, "step": step, "fraction": fraction }));
        };
        emit("download", Some(0.0));
        let dl = self.installer.data_dir.join("downloads");
        let zip = catalog::download(&pack.url, &dl, &mut |f| emit("download", f)).map_err(|e| err("DOWNLOAD", e))?;
        let o = InstallOptions { game_id, game_dir: &dir, zip_path: &zip, expected_sha256: Some(&pack.sha256), force };
        let mut progress = |step: &str| emit(step, None);
        let res = if update { self.installer.update(&o, &mut progress) } else { self.installer.install(&o, &mut progress) };
        if zip.starts_with(&dl) {
            let _ = std::fs::remove_file(&zip);
        }
        res
    }

    /// "Play in Hebrew": checks, puts a swap pack's files in place, starts the game (through Steam or directly) and,
    /// for swap packs, watches it so the originals come back when it exits.
    fn play(&self, app: &AppHandle, game_id: &str) -> Result<Value, PackError> {
        let g = self.game(game_id)?;
        let rec = self.installer.record(game_id).ok_or_else(|| err("NOT_INSTALLED", "The translation is not prepared yet"))?;
        let act = rec.activation.clone().ok_or_else(|| err("NOT_LAUNCH_ONLY", "This translation is always on; start the game as usual"))?;
        let dir = PathBuf::from(&rec.game_dir);
        if self.sessions.lock().unwrap().contains(game_id) {
            return Err(err("SESSION_ACTIVE", "The game is already running in Hebrew"));
        }
        // Started some other way (Steam): never swap files under a running game.
        let procs = detect::running_processes();
        if let Some(p) = self.running(&g, &procs) {
            return Err(err("GAME_ALREADY_RUNNING", format!("{p} is already running")));
        }
        if act.mode == ActivationMode::Args && self.installer.verify(game_id).is_some_and(|v| !v.intact) {
            return Err(err("NEEDS_REPAIR", "Some translation files changed"));
        }
        let steam = launch::steam_exe();
        let cmd = launch::build(&act, &dir, g.detect.steam_app_id.as_deref(), steam.as_deref(), &procs)?;
        let watch = act.mode == ActivationMode::Swap;
        if watch {
            self.with_busy(|| swap::activate(&dir, game_id, &rec.swap))?;
        }
        if let Err(e) = launch::spawn(&cmd) {
            if watch {
                let _ = swap::restore(&dir, &rec.swap);
            }
            return Err(e);
        }
        if watch {
            self.sessions.lock().unwrap().insert(game_id.to_string());
            spawn_watcher(app.clone(), game_id.to_string(), false);
        }
        let _ = app.emit("session", json!({ "gameId": game_id, "state": "started" }));
        Ok(json!({ "startingSteam": cmd.starting_steam, "via": if act.via == Via::Steam { "steam" } else { "direct" }, "watched": watch }))
    }

    fn with_busy<T>(&self, f: impl FnOnce() -> Result<T, PackError>) -> Result<T, PackError> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(err("BUSY", "Another operation is running"));
        }
        let _g = BusyGuard(&self.busy);
        f()
    }
}

/// Waits for the game to start and to exit (all of its processes gone for 15 s, so a game that restarts itself
/// through Steam is still covered), then puts the original files back. `already_running`: adopted at launcher
/// start after the window had been closed or the launcher had crashed.
fn spawn_watcher(app: AppHandle, game_id: String, already_running: bool) {
    std::thread::spawn(move || {
        use std::time::{Duration, Instant};
        let st = app.state::<Shared>();
        let names: Vec<String> = st.game(&game_id).map(|g| g.detect.process_names.iter().map(|p| p.to_lowercase()).collect()).unwrap_or_default();
        let start = Instant::now();
        let (mut seen, mut last) = (already_running, Instant::now());
        let mut told = false;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let procs = detect::running_processes();
            if names.iter().any(|n| procs.contains(n)) {
                seen = true;
                last = Instant::now();
                if !told {
                    told = true;
                    let _ = app.emit("session", json!({ "gameId": game_id, "state": "running" }));
                }
            } else if seen && last.elapsed() > Duration::from_secs(15) {
                break;
            } else if !seen && start.elapsed() > Duration::from_secs(180) {
                let _ = app.emit("session", json!({ "gameId": game_id, "state": "neverStarted" }));
                break;
            }
        }
        let res = st.installer.record(&game_id).map(|r| swap::restore(Path::new(&r.game_dir), &r.swap));
        st.sessions.lock().unwrap().remove(&game_id);
        let _ = match res {
            Some(Err(e)) => app.emit("session", json!({ "gameId": game_id, "state": "restoreFailed", "error": e })),
            Some(Ok(r)) => app.emit("session", json!({ "gameId": game_id, "state": "restored", "leftChanged": r.left_changed })),
            None => Ok(()),
        };
        if st.exit_after.load(Ordering::SeqCst) && st.sessions.lock().unwrap().is_empty() {
            app.exit(0);
        }
    });
}

/// At launcher start: a swap left in place (the launcher crashed or was killed) is restored, or, if the game is
/// still running, watched again.
fn recover_swaps(app: &AppHandle) {
    let st = app.state::<Shared>();
    let procs = detect::running_processes();
    for (game_id, rec) in st.installer.load_state().installs {
        let dir = PathBuf::from(&rec.game_dir);
        if rec.swap.is_empty() || !swap::is_active(&dir) {
            continue;
        }
        let names: Vec<String> = st.game(&game_id).map(|g| g.detect.process_names.iter().map(|p| p.to_lowercase()).collect()).unwrap_or_default();
        if names.iter().any(|n| procs.contains(n)) {
            st.sessions.lock().unwrap().insert(game_id.clone());
            spawn_watcher(app.clone(), game_id, true);
        } else if let Err(e) = swap::restore(&dir, &rec.swap) {
            eprintln!("restore {game_id}: {e}");
        }
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ---------- commands ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: String,
    dev: bool,
}

#[tauri::command]
fn app_info(app: AppHandle, st: State<'_, Shared>) -> AppInfo {
    AppInfo { version: app.package_info().version.to_string(), dev: st.dev }
}

#[tauri::command]
async fn list(app: AppHandle, reload: bool) -> Result<Value, PackError> {
    blocking(move || Ok(app.state::<Shared>().list(reload))).await
}

#[tauri::command]
async fn play(app: AppHandle, game_id: String) -> Result<Value, PackError> {
    blocking(move || app.state::<Shared>().play(&app, &game_id)).await
}

/// The window was closed while a game runs in Hebrew: hide it and quit after the game is back to normal.
#[tauri::command]
fn close_when_game_exits(app: AppHandle, st: State<'_, Shared>) {
    st.exit_after.store(true, Ordering::SeqCst);
    if st.sessions.lock().unwrap().is_empty() {
        app.exit(0);
    } else if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

#[tauri::command]
async fn install(app: AppHandle, game_id: String, force: bool) -> Result<InstallRecord, PackError> {
    blocking(move || {
        let st = app.state::<Shared>();
        st.with_busy(|| st.do_install(&app, &game_id, force, false))
    })
    .await
}

#[tauri::command]
async fn update(app: AppHandle, game_id: String, force: bool) -> Result<InstallRecord, PackError> {
    blocking(move || {
        let st = app.state::<Shared>();
        st.with_busy(|| st.do_install(&app, &game_id, force, true))
    })
    .await
}

#[tauri::command]
async fn uninstall(app: AppHandle, game_id: String) -> Result<UninstallResult, PackError> {
    blocking(move || {
        let st = app.state::<Shared>();
        st.with_busy(|| {
            let g = st.game(&game_id)?;
            st.check_closed(&g)?;
            st.installer.uninstall(&game_id)
        })
    })
    .await
}

#[tauri::command]
async fn pick_folder(app: AppHandle, game_id: String) -> Result<Option<String>, PackError> {
    use tauri_plugin_dialog::DialogExt;
    blocking(move || {
        let st = app.state::<Shared>();
        let g = st.game(&game_id)?;
        if st.installer.record(&game_id).is_some() {
            return Err(err("ALREADY_INSTALLED", "Uninstall the translation before changing the folder"));
        }
        let title = format!("בחרו את תיקיית ההתקנה של {}", g.name_he.as_deref().unwrap_or(&g.name));
        let Some(picked) = app.dialog().file().set_title(title).blocking_pick_folder() else { return Ok(None) };
        let dir = picked.into_path().map_err(|e| err("ERROR", e.to_string()))?;
        if !detect::is_game_dir(&dir, g.detect.marker_file.as_deref()) {
            return Err(err("NOT_GAME_DIR", format!("The folder does not contain {}", g.detect.marker_file.as_deref().unwrap_or("?"))));
        }
        let s = dir.to_string_lossy().into_owned();
        st.installer.set_custom_path(&game_id, Some(&s))?;
        Ok(Some(s))
    })
    .await
}

#[tauri::command]
async fn open_folder(app: AppHandle, game_id: String) -> Result<(), PackError> {
    blocking(move || {
        let st = app.state::<Shared>();
        let g = st.game(&game_id)?;
        let (dir, _) = st.locate(&g).ok_or_else(|| err("NO_GAME_DIR", "Game folder not found"))?;
        std::process::Command::new("explorer").arg(dir).spawn().map_err(|e| err("ERROR", e.to_string()))?;
        Ok(())
    })
    .await
}

/// Opens an https page (a loader's download page from `requires`) in the default browser.
#[tauri::command]
fn open_url(url: String) -> Result<(), PackError> {
    let parsed = url::Url::parse(&url).map_err(|e| err("BAD_URL", e.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(err("BAD_URL", "Only https pages"));
    }
    std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", parsed.as_str()]).spawn().map_err(|e| err("ERROR", e.to_string()))?;
    Ok(())
}

#[tauri::command]
fn get_settings(st: State<'_, Shared>) -> Value {
    json!({ "catalogUrl": st.installer.load_state().settings.catalog_url.unwrap_or_default(), "defaultCatalogUrl": DEFAULT_CATALOG_URL })
}

#[tauri::command]
fn set_settings(st: State<'_, Shared>, catalog_url: String) -> Result<(), PackError> {
    let url = catalog_url.trim();
    if !url.is_empty() && !url.starts_with("https://") {
        return Err(err("BAD_URL", "Must start with https://"));
    }
    let mut s = st.installer.load_state();
    s.settings.catalog_url = (!url.is_empty()).then(|| url.to_string());
    st.installer.save_state(&s)
}

// ---------- posters ----------

fn steam_cdn(app_id: &str, kind: &str) -> Vec<String> {
    let file = if kind == "hero" { "library_hero.jpg" } else { "library_600x900.jpg" };
    vec![
        format!("https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/{app_id}/{file}"),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{app_id}/{file}"),
    ]
}

impl Shared {
    fn cached_download(&self, urls: &[String], name: &str) -> Option<PathBuf> {
        let file = self.posters_dir.join(name);
        if file.is_file() {
            return Some(file);
        }
        if self.poster_misses.lock().unwrap().contains(name) {
            return None;
        }
        for u in urls.iter().filter(|u| u.starts_with("https://")) {
            let Ok(mut resp) = ureq::get(u).header("User-Agent", catalog::USER_AGENT).call() else { continue };
            let is_image = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("image/"));
            if !is_image {
                continue;
            }
            let Ok(bytes) = resp.body_mut().with_config().limit(15 * 1024 * 1024).read_to_vec() else { continue };
            let _ = std::fs::create_dir_all(&self.posters_dir);
            let tmp = file.with_extension("part");
            if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &file).is_ok() {
                return Some(file);
            }
        }
        self.poster_misses.lock().unwrap().insert(name.into());
        None
    }

    fn steam_art(&self, app_id: &str, kind: &str) -> Option<PathBuf> {
        if app_id.is_empty() || !app_id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        detect::find_steam_art_file(app_id, kind).or_else(|| self.cached_download(&steam_cdn(app_id, kind), &format!("steam-{app_id}-{kind}.jpg")))
    }

    /// Most Epic games are on Steam too: find the Steam page by name for the art.
    fn steam_id_by_name(&self, name: &str) -> Option<String> {
        if let Some(v) = self.steam_ids.lock().unwrap().get(name) {
            return v.clone();
        }
        let norm = |s: &str| {
            s.to_lowercase().replace(['®', '™', '©'], "").chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let url = format!("https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US", urlencode(name));
        let found = catalog::fetch_text(&url).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| {
            let items = v.get("items")?.as_array()?.clone();
            let n = norm(name);
            let name_of = |i: &Value| norm(i.get("name").and_then(Value::as_str).unwrap_or(""));
            items.iter().find(|i| name_of(i) == n).or_else(|| items.iter().find(|i| name_of(i).starts_with(&n))).and_then(|i| i.get("id")).map(|id| id.to_string())
        });
        self.steam_ids.lock().unwrap().insert(name.into(), found.clone());
        found
    }

    fn resolve_art(&self, kind: &str, store: &str, id: &str) -> Option<PathBuf> {
        match store {
            "steam" => self.steam_art(id, kind),
            "game" => {
                let g = self.game(id).ok()?;
                let custom = if kind == "hero" { g.hero.clone() } else { g.poster.clone() };
                if let Some(u) = custom {
                    if u.starts_with("file:") {
                        return url::Url::parse(&u).ok()?.to_file_path().ok();
                    }
                    let ext = Path::new(url::Url::parse(&u).ok()?.path()).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or("jpg".into());
                    if let Some(p) = self.cached_download(&[u], &format!("game-{}-{kind}.{ext}", g.id)) {
                        return Some(p);
                    }
                }
                self.steam_art(g.detect.steam_app_id.as_deref()?, kind)
            }
            "epic" => {
                let name = self.scanned.lock().unwrap().iter().flatten().find(|s| s.store == "epic" && s.id == id).map(|s| s.name.clone())?;
                let app_id = self.steam_id_by_name(&name)?;
                self.steam_art(&app_id, kind)
            }
            _ => None,
        }
    }
}

fn poster_response(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    let parts: Vec<String> = path.trim_start_matches('/').split('/').map(|p| url::form_urlencoded::parse(format!("x={p}").as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default()).collect();
    let file = match parts.as_slice() {
        [kind, store, id] => app.state::<Shared>().resolve_art(kind, store, id),
        _ => None,
    };
    let bytes = file.and_then(|f| std::fs::read(f).ok());
    match bytes {
        Some(b) => {
            let mime = if b.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else if b.starts_with(b"RIFF") { "image/webp" } else { "image/jpeg" };
            tauri::http::Response::builder().header("Content-Type", mime).header("Cache-Control", "max-age=86400").body(b).unwrap()
        }
        None => tauri::http::Response::builder().status(404).body(Vec::new()).unwrap(),
    }
}

fn data_root() -> PathBuf {
    // %APPDATA%\Tirgumim belongs to another app on this PC, so ours is "Tirgumim Launcher" (as the Electron launcher).
    let appdata = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    appdata.join("Tirgumim Launcher")
}

pub fn run(dev: bool) {
    tauri::Builder::default()
        // One launcher at a time (two would both watch a swap); a second start shows the first window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            app.state::<Shared>().exit_after.store(false, Ordering::SeqCst);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        // Self-update from our GitHub releases (see `.github/workflows/release.yml`), then restart.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .register_asynchronous_uri_scheme_protocol("poster", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            std::thread::spawn(move || responder.respond(poster_response(&app, &path)));
        })
        .setup(move |app| {
            let root = data_root();
            let data = root.join(if dev { "dev-data" } else { "data" });
            let resource = app.path().resource_dir().map(|d| d.join("catalog.json")).ok().filter(|p| p.is_file());
            let bundled = resource.unwrap_or_else(|| project_root().join("catalog").join("catalog.json"));
            let version = app.package_info().version.to_string();
            app.manage(Shared {
                releases: ReleaseCache::new(&data),
                installer: Installer::new(&data, &version),
                dev,
                bundled_catalog: bundled,
                posters_dir: root.join("posters"),
                cat: Mutex::new(CatalogState { catalog: None, source: None, error: None, packs: HashMap::new() }),
                scanned: Mutex::new(None),
                busy: AtomicBool::new(false),
                poster_misses: Mutex::new(HashSet::new()),
                steam_ids: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashSet::new()),
                exit_after: AtomicBool::new(false),
            });
            // Needs the catalog (process names), so after a first load, off the window's thread.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                handle.state::<Shared>().refresh_catalog(false);
                recover_swaps(&handle);
            });
            Ok(())
        })
        .on_window_event(|w, e| {
            // Never quit while a swap is in place without someone to restore it.
            if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                let st = w.app_handle().state::<Shared>();
                if !st.sessions.lock().unwrap().is_empty() && !st.exit_after.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = w.emit("close-blocked", ());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![app_info, list, play, close_when_game_exits, install, update, uninstall, pick_folder, open_folder, open_url, get_settings, set_settings])
        .run(tauri::generate_context!())
        .expect("error while running the launcher");
}
