//! `cargo xtask interop`: darktable reads what Blink writes (SPEC §6 interop oracle, nightly).
//!
//! Runs on Linux with `darktable` (built with Lua) and `xvfb-run` installed. For each case a
//! corpus RAW is copied into its own folder, `blink-cli` writes marks, and darktable imports the
//! folder into an in-memory library (`tests/interop/darktable_probe.lua`) and reports the rating
//! and colour labels it read. Cases:
//!
//! - `new-darktable-style`: no sidecar; Blink creates `IMG.ARW.xmp` (`--style darktable`).
//! - `darktable-roundtrip`: darktable imports first and writes its own sidecar (history and
//!   all); Blink updates that sidecar (FILE-4, FILE-5); a fresh darktable library reads it back,
//!   and darktable's own fields are still there.
//! - `reject`: `xmp:Rating=-1` reads as rejected.
//! - `both-styles`: a Lightroom-style `IMG.xmp` and a darktable-style sidecar exist; Blink
//!   updates both; darktable (which reads `IMG.ARW.xmp`) sees the change.
//!
//! The report goes to `target/interop/report.txt`.

use crate::{Result, root, run_cmd};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Case {
    name: &'static str,
    /// `blink-cli` argument lists; `{f}` is the RAW's path.
    blink: Vec<Vec<&'static str>>,
    /// Let darktable import (and write its sidecar) before Blink runs.
    darktable_first: bool,
    /// Add a Lightroom-style `IMG.xmp` before Blink runs.
    lightroom_sidecar: bool,
    want_rating: i32,
    want_label: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "new-darktable-style",
            blink: vec![
                vec!["rate", "--style", "darktable", "{f}", "3"],
                vec!["label", "--style", "darktable", "{f}", "red"],
            ],
            darktable_first: false,
            lightroom_sidecar: false,
            want_rating: 3,
            want_label: "red",
        },
        Case {
            name: "darktable-roundtrip",
            blink: vec![vec!["rate", "{f}", "5"], vec!["label", "{f}", "purple"]],
            darktable_first: true,
            lightroom_sidecar: false,
            want_rating: 5,
            want_label: "purple",
        },
        Case {
            name: "reject",
            blink: vec![vec!["rate", "--style", "darktable", "{f}", "-1"]],
            darktable_first: false,
            lightroom_sidecar: false,
            want_rating: -1,
            want_label: "none",
        },
        Case {
            name: "both-styles",
            blink: vec![vec!["rate", "{f}", "2"], vec!["label", "{f}", "green"]],
            darktable_first: true,
            lightroom_sidecar: true,
            want_rating: 2,
            want_label: "green",
        },
    ]
}

/// A RAW darktable supports, from the fetched corpus.
fn sample_raw() -> Result<PathBuf> {
    let cache = crate::corpus::cache_dir();
    for rel in ["canon/EOS-20D.CR2", "nikon/Z6_3.NEF"] {
        let p = cache.join(rel);
        if p.exists() {
            return Ok(p);
        }
    }
    Err("no sample RAW in the corpus cache: run `cargo xtask corpus fetch`".into())
}

/// Run darktable with the probe over `dirs`; returns the probe's lines.
fn darktable_probe(work: &Path, tag: &str, dirs: &[PathBuf]) -> Result<Vec<String>> {
    let probe = root().join("tests/interop/darktable_probe.lua");
    let out = work.join(format!("probe-{tag}.txt"));
    let list = dirs
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(";");
    let status = Command::new("timeout")
        .args([
            "600",
            "xvfb-run",
            "-a",
            "darktable",
            "--library",
            ":memory:",
            "--configdir",
        ])
        .arg(work.join(format!("dtconfig-{tag}")))
        .arg("--luacmd")
        .arg(format!("dofile('{}')", probe.display()))
        .env("BLINK_DT_OUT", &out)
        .env("BLINK_DT_DIRS", &list)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("failed to start darktable via xvfb-run: {e}"))?;
    let text = std::fs::read_to_string(&out)
        .map_err(|e| format!("darktable wrote no probe output ({status}): {e}"))?;
    Ok(text.lines().map(str::to_string).collect())
}

pub fn run() -> Result {
    crate::corpus::fetch()?;
    run_cmd("cargo", &["build", "-p", "blink-cli"])?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("target"));
    let cli = target
        .join("debug")
        .join(format!("blink-cli{}", std::env::consts::EXE_SUFFIX));
    let raw = sample_raw()?;
    let ext = raw.extension().unwrap().to_string_lossy().to_string();
    let work = std::env::temp_dir().join(format!("blink-interop-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let cases = cases();
    let mut files = Vec::new();
    for c in &cases {
        let dir = work.join(c.name);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let f = dir.join(format!("IMG_0001.{ext}"));
        std::fs::copy(&raw, &f).map_err(|e| e.to_string())?;
        files.push(f);
    }
    // Phase 1: darktable imports first where the case wants its own sidecar.
    let first: Vec<PathBuf> = cases
        .iter()
        .zip(&files)
        .filter(|(c, _)| c.darktable_first)
        .map(|(_, f)| f.parent().unwrap().to_path_buf())
        .collect();
    darktable_probe(&work, "1", &first)?;
    // Phase 2: Blink writes.
    for (c, f) in cases.iter().zip(&files) {
        let dt_sidecar = PathBuf::from(format!("{}.xmp", f.display()));
        if c.darktable_first && !dt_sidecar.exists() {
            return Err(format!("{}: darktable wrote no sidecar on import", c.name));
        }
        if c.lightroom_sidecar {
            let lr = f.with_extension("xmp");
            std::fs::write(&lr, LIGHTROOM).map_err(|e| e.to_string())?;
        }
        for args in &c.blink {
            let args: Vec<String> = args
                .iter()
                .map(|a| a.replace("{f}", &f.display().to_string()))
                .collect();
            let st = Command::new(&cli)
                .args(&args)
                .status()
                .map_err(|e| e.to_string())?;
            if !st.success() {
                return Err(format!("{}: blink-cli {} failed", c.name, args.join(" ")));
            }
        }
    }
    // Phase 3: a fresh darktable library reads everything back.
    let dirs: Vec<PathBuf> = files
        .iter()
        .map(|f| f.parent().unwrap().to_path_buf())
        .collect();
    let lines = darktable_probe(&work, "2", &dirs)?;
    let mut report = Vec::new();
    let mut fails = Vec::new();
    for (c, f) in cases.iter().zip(&files) {
        let want = format!(
            "{} rating={} label={}",
            f.display(),
            c.want_rating,
            c.want_label
        );
        let got = lines
            .iter()
            .find(|l| l.starts_with(&f.display().to_string()))
            .cloned()
            .unwrap_or_else(|| format!("{}: not imported", f.display()));
        let mut ok = got == want;
        let dt_sidecar =
            std::fs::read_to_string(format!("{}.xmp", f.display())).unwrap_or_default();
        if c.darktable_first && !dt_sidecar.contains("darktable:history") {
            ok = false;
            fails.push(format!("{}: darktable's history was lost", c.name));
        }
        if c.lightroom_sidecar {
            let lr = std::fs::read_to_string(f.with_extension("xmp")).unwrap_or_default();
            if !(lr.contains("xmp:Rating=\"2\"") && lr.contains("crs:WhiteBalance")) {
                ok = false;
                fails.push(format!(
                    "{}: the Adobe-style sidecar wasn't updated in place",
                    c.name
                ));
            }
        }
        report.push(format!(
            "{:<22} {} {got}",
            c.name,
            if ok { "PASS" } else { "FAIL" }
        ));
        if got != want {
            fails.push(format!("{}: darktable read `{got}`, want `{want}`", c.name));
        }
    }
    let text = report.join("\n") + "\n";
    print!("{text}");
    let dir = root().join("target/interop");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("report.txt"), &text).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&work);
    if fails.is_empty() {
        Ok(())
    } else {
        Err(fails.join("\n"))
    }
}

/// A Lightroom-style sidecar (attribute form, `crs:` settings).
const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="1"
   crs:WhiteBalance="As Shot">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
