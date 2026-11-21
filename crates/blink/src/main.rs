//! `blink [folder]` — opens the GUI.
//! `blink --bench-script <script.json> <folder> [--out <dir>]` — replays a bench script headless
//! and writes JSON results (SPEC §7).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

fn bench(args: &[PathBuf]) -> Result<(), String> {
    let [script, folder, rest @ ..] = args else {
        return Err("usage: blink --bench-script <script.json> <folder> [--out <dir>]".into());
    };
    let out = match rest {
        [flag, dir] if flag.as_os_str() == "--out" => dir.clone(),
        [] => PathBuf::from("."),
        _ => return Err("usage: blink --bench-script <script.json> <folder> [--out <dir>]".into()),
    };
    let text = std::fs::read_to_string(script).map_err(|e| format!("{}: {e}", script.display()))?;
    let script = blink_ui::bench::Script::parse(&text)?;
    let report = blink_ui::bench::run(&script, folder, blink_ui::Options::default)?;
    blink_ui::bench::write(&report, &out).map_err(|e| e.to_string())?;
    for m in &report.metrics {
        print!("{}", m.to_json());
    }
    if report.stale_frames > 0 {
        return Err(format!(
            "{} frames showed a stale item",
            report.stale_frames
        ));
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args
        .first()
        .is_some_and(|a| a.as_os_str() == "--bench-script")
    {
        return match bench(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("blink: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match blink_ui::run(args.into_iter().next()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("blink: {e}");
            ExitCode::FAILURE
        }
    }
}
