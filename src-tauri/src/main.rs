// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

fn arg_after(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // tirgumim --make-pack <pack.config.json> [--game-dir DIR] [--out FILE.zip] [--url URL]
    if args.first().map(String::as_str) == Some("--make-pack") {
        let Some(config) = args.get(1).filter(|a| !a.starts_with("--")) else {
            eprintln!("usage: --make-pack <pack.config.json> [--game-dir DIR] [--out FILE.zip] [--url URL]");
            std::process::exit(2);
        };
        let game_dir = arg_after(&args, "--game-dir").map(PathBuf::from);
        let out = arg_after(&args, "--out").map(PathBuf::from);
        match tirgumim_lib::core::tools::make_pack(Path::new(config), game_dir.as_deref(), out.as_deref(), arg_after(&args, "--url").as_deref()) {
            Ok(m) => {
                let n = m.manifest["files"].as_array().map(Vec::len).unwrap_or(0);
                println!("Built {} ({n} files)\nCatalog \"pack\" entry:\n{}", m.out.display(), serde_json::to_string_pretty(&m.entry).unwrap());
                if let Some(min) = m.manifest["minLauncherVersion"].as_str() {
                    println!(
                        "\nPut this line in the GitHub release notes (older launchers then keep the last release they can install):\n{}{min}",
                        tirgumim_lib::core::catalog::MIN_LAUNCHER_MARK
                    );
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // tirgumim --check-catalog <catalog.json> [--online]   (GitHub Actions runs this on every catalog change)
    if args.first().map(String::as_str) == Some("--check-catalog") {
        std::process::exit(check_catalog(&args));
    }

    if args.first().map(String::as_str) == Some("--dev-setup") {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        match tirgumim_lib::core::tools::dev_setup(&root) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let dev = args.iter().any(|a| a == "--dev-catalog") || std::env::var_os("TIRGUMIM_DEV_CATALOG").is_some();
    tirgumim_lib::run(dev);
}

fn report_errors_push(r: &mut tirgumim_lib::core::check::Report, e: String) {
    r.errors.push(e);
}

fn check_catalog(args: &[String]) -> i32 {
    use tirgumim_lib::core::{catalog, check};
    let Some(file) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: --check-catalog <catalog.json> [--online]");
        return 2;
    };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: {file}: {e}");
            return 2;
        }
    };
    let (parsed, mut report) = check::check_offline(&text);
    if let Some(c) = &parsed {
        println!("{file}: {} games", c.games.len());
        if args.iter().any(|a| a == "--discover") {
            // Our tagged repos: each entry checked with the same strict rules, plus its releases.
            let tmp = std::env::temp_dir().join(format!("tirgumim-check-{}", std::process::id()));
            match tirgumim_lib::core::discover::Discovery::new(&tmp).fetch_entries() {
                Ok((entries, failed)) => {
                    println!("{} tagged repos found", entries.len() + failed.len());
                    for f in failed {
                        report_errors_push(&mut report, format!("repo entry: {f}"));
                    }
                    let doc = serde_json::json!({ "schema": 1, "games": entries });
                    let (found, r) = check::check_offline(&doc.to_string());
                    report.errors.extend(r.errors);
                    report.warnings.extend(r.warnings);
                    if let Some(f) = found {
                        check::check_releases(&f, &mut report, &mut |repo| {
                            let url = format!("https://api.github.com/repos/{repo}/releases?per_page=50");
                            catalog::fetch_text(&url).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
                        });
                    }
                }
                Err(e) => report.errors.push(format!("GitHub search: {e}")),
            }
        }
        if args.iter().any(|a| a == "--online") {
            check::check_releases(c, &mut report, &mut |repo| {
                let url = format!("https://api.github.com/repos/{repo}/releases?per_page=50");
                catalog::fetch_text(&url).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
            });
        }
    }
    for w in &report.warnings {
        println!("warning: {w}");
    }
    for e in &report.errors {
        eprintln!("error: {e}");
    }
    if report.errors.is_empty() {
        println!("Catalog OK");
        0
    } else {
        eprintln!("{} error(s)", report.errors.len());
        1
    }
}
