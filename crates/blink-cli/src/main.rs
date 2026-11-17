//! `blink-cli` — headless commands over the same core.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("blink-cli {}", blink_runtime::version());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: blink-cli [--version]");
            ExitCode::from(2)
        }
    }
}
