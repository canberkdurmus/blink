//! `blink [folder]` — opens the GUI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    let folder = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    match blink_ui::run(folder) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("blink: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
