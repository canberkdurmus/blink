//! Blink developer tasks. Run with `cargo xtask <command>`.
//!
//! Commands:
//! - `verify [--full]`   fast tier (fmt, clippy, nextest, trace); `--full` adds the full-tier tests
//! - `trace [--gate Mn]` requirement coverage from SPEC.md, PROGRESS.md and nextest JUnit output
//! - `corpus fetch`      download and verify the test corpus
//! - `fixtures`          write synthetic fixtures to `corpus/synthetic/`
//! - `bench`             run perf scripts and compare with `perf/baseline-<target>.json`
//! - `report Mn`         generate `docs/reports/Mn.md`
//! - `guard --base REF`  CI guard checks for goldens/baselines and removed/ignored tests
//! - `interop`           darktable reads the sidecars Blink writes (Linux, nightly)

mod bench;
mod corpus;
mod fixtures;
mod guard;
mod interop;
mod report;
mod spec;
mod trace;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub type Result<T = ()> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let res = match args.as_slice() {
        ["verify"] => verify(false),
        ["verify", "--full"] => verify(true),
        ["trace"] => trace::run(trace::Mode::Full, None),
        ["trace", "--gate", m] => trace::run(trace::Mode::Full, Some(m)),
        ["corpus", "fetch"] => corpus::fetch(),
        ["fixtures"] => fixtures::run(),
        ["bench"] => bench::run(false),
        ["bench", "--allow-missing-baseline"] => bench::run(true),
        ["report", m] => report::run(m),
        ["guard", "--base", base] => guard::run(base),
        ["interop"] => interop::run(),
        ["soak", "--minutes", m] => soak(m),
        _ => Err(USAGE.to_string()),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage: cargo xtask <verify [--full] | trace [--gate Mn] | corpus fetch | fixtures | bench [--allow-missing-baseline] | report Mn | guard --base REF | interop | soak --minutes N>";

/// Random actions against the headless app for `minutes`, checking the decode cache cap on
/// every frame (PERF-8; nightly).
fn soak(minutes: &str) -> Result {
    let m: u64 = minutes
        .parse()
        .map_err(|_| format!("soak: minutes must be a whole number, not {minutes}"))?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let args = [
        "nextest",
        "run",
        "-p",
        "blink-ui",
        "--locked",
        "--no-capture",
        "-E",
        "test(/full_soak::/)",
    ];
    eprintln!("$ BLINK_SOAK_SECS={} {cargo} {}", m * 60, args.join(" "));
    let status = Command::new(&cargo)
        .args(args)
        .env("BLINK_SOAK_SECS", (m * 60).to_string())
        .current_dir(root())
        .status()
        .map_err(|e| format!("failed to start {cargo}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("soak failed ({status})"))
    }
}

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent dir")
        .to_path_buf()
}

/// Run a command in the workspace root, inheriting stdio; error if it fails.
pub fn run_cmd(program: &str, args: &[&str]) -> Result {
    eprintln!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root())
        .status()
        .map_err(|e| format!("failed to start {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{program} {}` failed ({status})", args.join(" ")))
    }
}

/// Run a command and capture stdout.
pub fn capture(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(root())
        .output()
        .map_err(|e| format!("failed to start {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`{program} {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// nextest filterset selecting full-tier tests (test names or modules starting with `full_`).
pub const FULL_FILTER: &str = "test(/(^|::)full_/)";

fn verify(full: bool) -> Result {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    run_cmd(&cargo, &["fmt", "--all", "--check"])?;
    run_cmd(
        &cargo,
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    let profile = std::env::var("NEXTEST_PROFILE").unwrap_or_else(|_| "default".into());
    let mut args = vec![
        "nextest",
        "run",
        "--workspace",
        "--locked",
        "--profile",
        profile.as_str(),
        "--no-tests=warn",
    ];
    let not_full = format!("not {FULL_FILTER}");
    if !full {
        args.extend(["-E", not_full.as_str()]);
    }
    run_cmd(&cargo, &args)?;
    trace::run(
        if full {
            trace::Mode::Full
        } else {
            trace::Mode::Fast
        },
        None,
    )
}
