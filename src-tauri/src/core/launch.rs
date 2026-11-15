//! Starting a game for a launch-only pack: through Steam (`steam.exe -applaunch <appId> <args>`) or straight away.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::detect;
use super::installer::PackError;
use super::manifest::{Activation, Via};
use super::util::resolve_inside;

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchCmd {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Set for the game only (never for Steam, which would not pass them on).
    pub env: Vec<(String, String)>,
    /// Steam wasn't running: `-applaunch` starts it first, which takes a while.
    pub starting_steam: bool,
}

fn err(code: &str, msg: impl Into<String>) -> PackError {
    PackError::new(code, msg)
}

/// `{gameDir}` → the game folder (Windows separators).
pub fn expand(s: &str, game_dir: &Path) -> String {
    let dir = game_dir.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
    s.replace("{gameDir}", &dir).replace('/', "\\")
}

pub fn steam_exe() -> Option<PathBuf> {
    detect::steam_roots().iter().map(|r| r.join("steam.exe")).find(|p| p.is_file())
}

/// The command that starts the game. `steam` is steam.exe if installed; `procs` the running programs.
pub fn build(a: &Activation, game_dir: &Path, steam_app_id: Option<&str>, steam: Option<&Path>, procs: &HashSet<String>) -> Result<LaunchCmd, PackError> {
    let exe = resolve_inside(game_dir, &a.exe).map_err(|e| err("BAD_MANIFEST", e))?;
    if !exe.is_file() {
        return Err(err("EXE_MISSING", format!("{} not found in the game folder", a.exe)));
    }
    let args: Vec<String> = a.args.iter().map(|x| expand(x, game_dir)).collect();
    match a.via {
        Via::Steam => {
            let id = steam_app_id.filter(|s| !s.is_empty()).ok_or_else(|| err("STEAM_NO_APPID", "The game has no Steam app id in the catalog"))?;
            let steam = steam.ok_or_else(|| err("STEAM_MISSING", "Steam is not installed"))?;
            let mut all = vec!["-applaunch".to_string(), id.to_string()];
            all.extend(args);
            Ok(LaunchCmd { program: steam.to_path_buf(), args: all, cwd: None, env: vec![], starting_steam: !procs.contains("steam.exe") })
        }
        Via::Direct => {
            if a.needs_steam {
                if steam.is_none() {
                    return Err(err("STEAM_MISSING", "Steam is not installed"));
                }
                if !procs.contains("steam.exe") {
                    return Err(err("STEAM_NOT_RUNNING", "Open Steam first"));
                }
            }
            let env = a.env.iter().map(|(k, v)| (k.clone(), expand(v, game_dir))).collect();
            let cwd = match &a.working_dir {
                Some(w) => {
                    let rel = w.trim_start_matches("{gameDir}").trim_start_matches(['/', '\\']);
                    if rel.is_empty() { game_dir.to_path_buf() } else { resolve_inside(game_dir, rel).map_err(|e| err("BAD_MANIFEST", e))? }
                }
                None => exe.parent().unwrap_or(game_dir).to_path_buf(),
            };
            Ok(LaunchCmd { program: exe, args, cwd: Some(cwd), env, starting_steam: false })
        }
    }
}

pub fn spawn(c: &LaunchCmd) -> Result<(), PackError> {
    let mut cmd = Command::new(&c.program);
    cmd.args(&c.args);
    cmd.envs(c.env.iter().map(|(k, v)| (k, v)));
    if let Some(d) = &c.cwd {
        cmd.current_dir(d);
    }
    cmd.spawn().map(|_| ()).map_err(|e| err("LAUNCH_FAILED", format!("{}: {e}", c.program.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::manifest::ActivationMode;

    fn act(via: Via) -> Activation {
        Activation { mode: ActivationMode::Args, exe: "UNDERTALE.exe".into(), args: vec!["-game".into(), if via == Via::Steam { "data_he.win" } else { "{gameDir}/data_he.win" }.into()], working_dir: None, via, env: Default::default(), needs_steam: false }
    }

    #[test]
    fn steam_and_direct_command_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("UNDERTALE");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("UNDERTALE.exe"), "x").unwrap();
        let steam = tmp.path().join("steam.exe");
        let none = HashSet::new();
        let he = format!("{}\\data_he.win", game.to_string_lossy());

        let c = build(&act(Via::Steam), &game, Some("391540"), Some(&steam), &none).unwrap();
        assert_eq!(c.program, steam);
        assert_eq!(c.args, ["-applaunch", "391540", "-game", "data_he.win"]);
        assert!(c.starting_steam);
        let running: HashSet<String> = ["steam.exe".to_string()].into();
        assert!(!build(&act(Via::Steam), &game, Some("391540"), Some(&steam), &running).unwrap().starting_steam);
        assert_eq!(build(&act(Via::Steam), &game, Some("391540"), None, &none).unwrap_err().code, "STEAM_MISSING");
        assert_eq!(build(&act(Via::Steam), &game, None, Some(&steam), &none).unwrap_err().code, "STEAM_NO_APPID");

        let d = build(&act(Via::Direct), &game, None, None, &none).unwrap();
        assert_eq!(d.program, game.join("UNDERTALE.exe"));
        assert_eq!(d.args, ["-game", he.as_str()]);
        assert_eq!(d.cwd.as_deref(), Some(game.as_path()));

        // A proxy-DLL pack: env for the game, Steam must already run.
        let mut proxy = act(Via::Direct);
        proxy.args.clear();
        proxy.env.insert("TIRGUMIM_LANG".into(), "he".into());
        proxy.needs_steam = true;
        assert_eq!(build(&proxy, &game, None, Some(&steam), &none).unwrap_err().code, "STEAM_NOT_RUNNING");
        assert_eq!(build(&proxy, &game, None, None, &none).unwrap_err().code, "STEAM_MISSING");
        let p = build(&proxy, &game, None, Some(&steam), &running).unwrap();
        assert_eq!(p.env, [("TIRGUMIM_LANG".to_string(), "he".to_string())]);
        assert!(p.args.is_empty());

        let mut missing = act(Via::Direct);
        missing.exe = "nope.exe".into();
        assert_eq!(build(&missing, &game, None, None, &none).unwrap_err().code, "EXE_MISSING");
    }
}
