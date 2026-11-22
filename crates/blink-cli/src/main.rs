//! `blink-cli` — headless commands over the same core and runtime as the GUI (CLI-1, CLI-2).

use blink_core::filter::Filter;
use blink_core::formats::{FileKind, Formats};
use blink_core::item::Item;
use blink_core::state::{Action, Effect, MarkChange, Marks, State, reduce};
use blink_decode::raw::RawBackend;
use blink_runtime::fs::{Fs, RealFs};
use blink_runtime::sidecar::{SidecarStyle, SidecarWriter, SystemClock};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage:
  blink-cli scan [-r] <folder>
  blink-cli list [-r] [--filter EXPR] <folder>
  blink-cli export [-r] [--filter EXPR] [--format txt|csv] <folder>
  blink-cli rate [--dry-run] [--style adobe|darktable] <file> <-1..5>
  blink-cli label [--dry-run] [--style adobe|darktable] <file> <Red|Yellow|Green|Blue|Purple|none>
  blink-cli doctor <file>
  blink-cli doctor --formats
  blink-cli --version

--style: name of new sidecars (default adobe: IMG.xmp; darktable: IMG.ARW.xmp).
-r: include sub-folders. Filter EXPR: comma-separated, AND-combined:
  rating>=N, rating=N, rejected, !rejected, picked, label=NAME, body=NAME, lens=NAME,
  focal=MIN..MAX, iso=MIN..MAX, date=YYYY-MM-DD..YYYY-MM-DD, kind=raw|dng|jpeg|heif
  (either end of a range may be left out; one value is a one-value range)";

/// Default pick value (D-2): pick = 1 star.
const PICK_VALUE: i8 = 1;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = String::new();
    let code = run(&args, &mut out, &RealFs);
    let _ = std::io::stdout().write_all(out.as_bytes());
    ExitCode::from(code)
}

struct Opts {
    recursive: bool,
    dry_run: bool,
    filter: Filter,
    format: String,
    formats_flag: bool,
    style: SidecarStyle,
    positional: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        recursive: false,
        dry_run: false,
        filter: Filter::default(),
        format: "txt".into(),
        formats_flag: false,
        style: SidecarStyle::default(),
        positional: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-r" | "--recursive" => o.recursive = true,
            "--dry-run" => o.dry_run = true,
            "--formats" => o.formats_flag = true,
            "--filter" => {
                let e = it.next().ok_or("--filter needs an expression")?;
                o.filter = Filter::parse(e)?;
            }
            "--style" => {
                o.style = match it.next().map(String::as_str) {
                    Some("adobe") => SidecarStyle::Adobe,
                    Some("darktable") => SidecarStyle::Darktable,
                    _ => return Err("--style needs adobe or darktable".into()),
                }
            }
            "--format" => o.format = it.next().ok_or("--format needs txt or csv")?.clone(),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => o.positional.push(a.clone()),
        }
    }
    Ok(o)
}

fn formats(backend: &dyn RawBackend) -> Formats {
    Formats::new(backend.extensions())
}

/// Run a command; output goes to `out`, errors to stderr. Returns the exit code.
fn run(args: &[String], out: &mut String, fs: &dyn Fs) -> u8 {
    let Some(cmd) = args.first() else {
        eprintln!("{USAGE}");
        return 2;
    };
    if cmd == "--version" || cmd == "-V" {
        let _ = writeln!(out, "blink-cli {}", blink_runtime::version());
        return 0;
    }
    if cmd == "--help" || cmd == "-h" {
        let _ = writeln!(out, "{USAGE}");
        return 0;
    }
    let opts = match parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("blink-cli: {e}\n{USAGE}");
            return 2;
        }
    };
    let backend = blink_decode::default_backend();
    let res = match cmd.as_str() {
        "scan" => cmd_scan(fs, &backend, &opts, out),
        "list" => cmd_list(fs, &backend, &opts, out),
        "export" => cmd_export(fs, &backend, &opts, out),
        "rate" | "label" => cmd_mark(fs, &backend, cmd, &opts, out),
        "doctor" => cmd_doctor(fs, &backend, &opts, out),
        _ => Err(format!("unknown command `{cmd}`")),
    };
    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("blink-cli: {e}");
            1
        }
    }
}

fn one_positional(o: &Opts, what: &str) -> Result<PathBuf, String> {
    match o.positional.as_slice() {
        [p] => Ok(PathBuf::from(p)),
        _ => Err(format!("expected one {what}\n{USAGE}")),
    }
}

/// Open a folder through the same reducer the GUI uses, then load marks (RATE-4).
fn open(
    fs: &dyn Fs,
    backend: &dyn RawBackend,
    folder: &Path,
    recursive: bool,
) -> Result<State, String> {
    let mut s = State::new(formats(backend));
    if recursive {
        s = reduce(&s, Action::SetRecursive(true)).0;
    }
    let (s, _) = reduce(&s, Action::OpenFolder(folder.to_path_buf()));
    let files = blink_runtime::scan::scan_folder(fs, folder, recursive)
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    let (s, _) = reduce(
        &s,
        Action::FolderScanned {
            folder: folder.to_path_buf(),
            recursive,
            files,
        },
    );
    let marks = blink_runtime::marks::load_marks(fs, &s.items);
    Ok(reduce(&s, Action::MarksLoaded(marks)).0)
}

fn rel(folder: &Path, p: &Path) -> String {
    p.strip_prefix(folder)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Items passing `f`, in path order. Capture metadata is read only when the filter needs it.
fn filtered<'a>(fs: &dyn Fs, s: &'a State, f: &Filter) -> Vec<&'a Item> {
    let meta: std::collections::BTreeMap<_, _> = if f.needs_meta() {
        blink_runtime::meta::load_meta(fs, &s.items)
            .into_iter()
            .collect()
    } else {
        Default::default()
    };
    s.items
        .iter()
        .filter(|i| {
            let p = &i.primary.path;
            f.matches_meta(i, s.marks.get(p), meta.get(p), PICK_VALUE)
        })
        .collect()
}

fn cmd_scan(
    fs: &dyn Fs,
    backend: &dyn RawBackend,
    o: &Opts,
    out: &mut String,
) -> Result<(), String> {
    let folder = one_positional(o, "folder")?;
    let s = open(fs, backend, &folder, o.recursive)?;
    let mut counts = [0usize; 4];
    for m in s.items.iter().flat_map(Item::members) {
        counts[m.kind as usize] += 1;
    }
    let total: usize = counts.iter().sum();
    let pairs = s.items.iter().filter(|i| !i.companions.is_empty()).count();
    let rated = s
        .marks
        .values()
        .filter(|m| m.rating.is_some_and(|r| r != 0))
        .count();
    let labelled = s.marks.values().filter(|m| m.label.is_some()).count();
    let _ = writeln!(out, "recursive: {}", if o.recursive { "yes" } else { "no" });
    let _ = writeln!(
        out,
        "files: {total} supported (raw {}, dng {}, jpeg {}, heif {})",
        counts[FileKind::Raw as usize],
        counts[FileKind::Dng as usize],
        counts[FileKind::Jpeg as usize],
        counts[FileKind::Heif as usize]
    );
    let _ = writeln!(out, "items: {} ({pairs} pairs)", s.items.len());
    let _ = writeln!(out, "rated: {rated}, labelled: {labelled}");
    Ok(())
}

fn rating_text(m: Option<&Marks>) -> String {
    match m.and_then(|m| m.rating) {
        Some(-1) => "rejected".into(),
        Some(r) => r.to_string(),
        None => "-".into(),
    }
}

fn cmd_list(
    fs: &dyn Fs,
    backend: &dyn RawBackend,
    o: &Opts,
    out: &mut String,
) -> Result<(), String> {
    let folder = one_positional(o, "folder")?;
    let s = open(fs, backend, &folder, o.recursive)?;
    for i in filtered(fs, &s, &o.filter) {
        let m = s.marks.get(&i.primary.path);
        let comp: Vec<String> = i
            .companions
            .iter()
            .map(|c| {
                c.path
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}",
            rel(&folder, &i.primary.path),
            i.primary.kind.label(),
            if comp.is_empty() {
                "-".into()
            } else {
                format!("+{}", comp.join(","))
            },
            rating_text(m),
            m.and_then(|m| m.label.as_deref()).unwrap_or("-")
        );
    }
    Ok(())
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn cmd_export(
    fs: &dyn Fs,
    backend: &dyn RawBackend,
    o: &Opts,
    out: &mut String,
) -> Result<(), String> {
    let folder = one_positional(o, "folder")?;
    let s = open(fs, backend, &folder, o.recursive)?;
    let csv = match o.format.as_str() {
        "txt" => false,
        "csv" => true,
        f => return Err(format!("unknown format `{f}` (txt or csv)")),
    };
    if csv {
        let _ = writeln!(out, "path,rating,label,pick");
    }
    for i in filtered(fs, &s, &o.filter) {
        let m = s.marks.get(&i.primary.path);
        for member in i.members() {
            let p = member.path.to_string_lossy();
            if csv {
                let rating = m
                    .and_then(|m| m.rating)
                    .map(|r| r.to_string())
                    .unwrap_or_default();
                let label = m.and_then(|m| m.label.clone()).unwrap_or_default();
                let pick = m.and_then(|m| m.rating) == Some(PICK_VALUE);
                let _ = writeln!(
                    out,
                    "{},{rating},{},{pick}",
                    csv_field(&p),
                    csv_field(&label)
                );
            } else {
                let _ = writeln!(out, "{p}");
            }
        }
    }
    Ok(())
}

/// `rate` / `label`: run the action through the reducer, like the GUI, and write the sidecar of
/// every pair member (FILE-4..6). `--dry-run` prints the planned changes instead.
fn cmd_mark(
    fs: &dyn Fs,
    backend: &dyn RawBackend,
    cmd: &str,
    o: &Opts,
    out: &mut String,
) -> Result<(), String> {
    let [file, value] = o.positional.as_slice() else {
        return Err(format!("expected <file> <value>\n{USAGE}"));
    };
    let file = PathBuf::from(file);
    // A bare file name has an empty parent: that's the current folder.
    let folder = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let name = file.file_name().ok_or("expected a file")?;
    let s = open(fs, backend, &folder, false)?;
    let idx = s
        .items
        .iter()
        .position(|i| i.members().any(|m| m.path.file_name() == Some(name)))
        .ok_or_else(|| format!("{}: not a supported file in its folder", file.display()))?;
    let action = if cmd == "rate" {
        let rating: i8 = value
            .parse()
            .ok()
            .filter(|r| (-1..=5).contains(r))
            .ok_or("rating must be -1..5")?;
        Action::SetRating { item: idx, rating }
    } else {
        const LABELS: [&str; 5] = ["Red", "Yellow", "Green", "Blue", "Purple"];
        let label = if value.eq_ignore_ascii_case("none") {
            None
        } else {
            Some(
                LABELS
                    .iter()
                    .find(|l| l.eq_ignore_ascii_case(value))
                    .ok_or("label must be Red, Yellow, Green, Blue, Purple or none")?
                    .to_string(),
            )
        };
        Action::SetLabel { item: idx, label }
    };
    // Same reducer as the GUI: exactly its sidecar effects are planned or written.
    let (_, effects) = reduce(&s, action);
    let clock = SystemClock;
    let mut writer = SidecarWriter::new(fs, &clock, o.style);
    for e in effects {
        if let Effect::WriteSidecar {
            source,
            change,
            owns_adobe_name,
        } = e
        {
            if o.dry_run {
                let change = match &change {
                    MarkChange::Rating(Some(r)) => format!("set xmp:Rating={r}"),
                    MarkChange::Rating(None) => "remove xmp:Rating".into(),
                    MarkChange::Label(Some(l)) => format!("set xmp:Label={l}"),
                    MarkChange::Label(None) => "remove xmp:Label".into(),
                    MarkChange::RevertRating => "revert xmp:Rating".into(),
                    MarkChange::RevertLabel => "revert xmp:Label".into(),
                };
                let _ = writeln!(
                    out,
                    "would update sidecar of {}: {change}",
                    name_of(&source)
                );
            } else {
                writer.queue(source, change, owns_adobe_name, std::time::Instant::now());
            }
        }
    }
    let mut failed = Vec::new();
    for rep in writer.flush_all() {
        match rep.result {
            Ok(()) => {
                for p in &rep.sidecars {
                    let _ = writeln!(out, "{}: updated {}", name_of(&rep.source), name_of(p));
                }
            }
            Err(e) => failed.push(e),
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("\n"))
    }
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn ms(d: std::time::Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

fn cmd_doctor(
    fs: &dyn Fs,
    backend: &dyn RawBackend,
    o: &Opts,
    out: &mut String,
) -> Result<(), String> {
    let f = formats(backend);
    if o.formats_flag {
        let raw: Vec<&str> = f.raw_extensions().collect();
        let _ = writeln!(out, "RAW ({}): {}", backend.name(), raw.join(", "));
        let _ = writeln!(
            out,
            "DNG: {}",
            blink_core::formats::DNG_EXTENSIONS.join(", ")
        );
        let _ = writeln!(
            out,
            "JPEG: {}",
            blink_core::formats::JPEG_EXTENSIONS.join(", ")
        );
        let _ = writeln!(
            out,
            "HEIF (listed and paired, not decoded in v1): {}",
            blink_core::formats::HEIF_EXTENSIONS.join(", ")
        );
        return Ok(());
    }
    let path = one_positional(o, "file")?;
    let i = blink_runtime::inspect::inspect(fs, &path, &f, backend).map_err(|e| e.to_string())?;
    let a = &i.analysis;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = writeln!(out, "file: {name}");
    let _ = writeln!(out, "size: {} bytes", i.len);
    let _ = writeln!(out, "kind: {}", i.kind.label());
    let _ = writeln!(out, "container: {}", a.container);
    let _ = writeln!(
        out,
        "camera: {}",
        blink_core::info::body(a.exif.make.as_deref(), a.exif.model.as_deref())
            .unwrap_or_else(|| "unknown".into())
    );
    match &a.raw {
        Some(Ok(p)) => {
            let _ = writeln!(
                out,
                "raw decoder: {}: ok ({} {}, sensor {} × {})",
                backend.name(),
                p.make,
                p.model,
                p.sensor_w,
                p.sensor_h
            );
        }
        Some(Err(e)) => {
            let _ = writeln!(out, "raw decoder: {}: FAILED: {e}", backend.name());
        }
        None => {}
    }
    if i.kind.is_raw_like() {
        let _ = writeln!(
            out,
            "sensor: {}",
            a.sensor
                .map(|(w, h)| format!("{w} × {h}"))
                .unwrap_or_else(|| "unknown".into())
        );
        let _ = writeln!(out, "embedded images: {}", a.embedded.len());
        for (n, e) in a.embedded.iter().enumerate() {
            let chosen = if Some(*e) == a.chosen {
                "  <- chosen"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  #{n} {} {} × {} at offset {}, {} bytes{chosen}",
                e.kind.label(),
                e.width,
                e.height,
                e.offset,
                e.len
            );
        }
        match a.chosen {
            Some(c) => {
                let _ = writeln!(
                    out,
                    "chosen preview: {} {} × {}",
                    c.kind.label(),
                    c.width,
                    c.height
                );
            }
            None => {
                let _ = writeln!(out, "chosen preview: none");
            }
        }
        let verdict = match (a.low_res(), a.chosen, a.sensor) {
            (Some(true), Some(c), Some((w, h))) => format!(
                "yes (preview long edge {} < 50% of sensor long edge {})",
                c.long_edge(),
                w.max(h)
            ),
            (Some(false), Some(c), Some((w, h))) => format!(
                "no (preview long edge {} >= 50% of sensor long edge {})",
                c.long_edge(),
                w.max(h)
            ),
            _ => "unknown (no preview or sensor size)".into(),
        };
        let _ = writeln!(out, "low-res: {verdict}");
    }
    let _ = writeln!(out, "badge: {}", i.source().badge());
    match &i.preview {
        Some(Ok(p)) => {
            let _ = writeln!(
                out,
                "preview decode: ok {} × {}, from {}, orientation {} ({})",
                p.image.width,
                p.image.height,
                p.space.label(),
                p.orientation,
                ms(i.decode_time)
            );
        }
        Some(Err(e)) => {
            let _ = writeln!(out, "preview decode: FAILED: {e} ({})", ms(i.decode_time));
        }
        None => {
            let _ = writeln!(out, "preview decode: not attempted");
        }
    }
    let _ = writeln!(
        out,
        "timing: read {}, analyze {}",
        ms(i.read_time),
        ms(i.analyze_time)
    );
    let _ = writeln!(out, "info:");
    for r in i.info().rows() {
        let _ = writeln!(out, "  {}: {}", r.label, r.value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Golden output tests (CLI-1, CLI-2, OPEN-4). Goldens live in `tests/snapshots/`; regenerate
    //! with `BLINK_UPDATE_SNAPSHOTS=1` and commit with a `golden-update:` trailer.
    use super::*;
    use blink_runtime::fs::MemFs;
    use blink_testkit::{fixtures, golden::mask_timings, sidecars};

    fn golden(name: &str, got: &str) {
        blink_testkit::golden::check(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/snapshots")
                .join(name),
            got,
        );
    }

    fn fixture() -> MemFs {
        let fs = MemFs::new();
        let preview = fixtures::orientation_jpeg(1);
        fs.add_file(
            "/f/a.ARW",
            fixtures::tiff_with_jpegs(std::slice::from_ref(&preview)),
        );
        fs.add_file("/f/a.JPG", preview.clone());
        fs.add_file("/f/a.xmp", sidecars::lightroom(4, Some("Red")));
        fs.add_file("/f/b.NEF", b"not really a raw".to_vec());
        fs.add_file("/f/b.HEIC", b"heif".to_vec());
        fs.add_file("/f/b.NEF.xmp", sidecars::darktable(-1));
        fs.add_file("/f/c.JPG", fixtures::orientation_jpeg(6));
        fs.add_file("/f/d.DNG", fixtures::tiff_with_jpegs(&[preview]));
        fs.add_file("/f/d.xmp", sidecars::bridge(5, Some("Green")));
        fs.add_file("/f/notes.txt", b"hello".to_vec());
        fs.add_file("/f/sub/e.ARW", b"raw".to_vec());
        fs.add_file("/f/sub/e.xmp", sidecars::digikam(2, Some("Purple")));
        fs
    }

    fn cli(fs: &MemFs, args: &[&str]) -> (u8, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = String::new();
        let code = run(&args, &mut out, fs);
        (code, mask_timings(&out))
    }

    #[test]
    fn req_cli_1_scan_golden() {
        let fs = fixture();
        let (code, out) = cli(&fs, &["scan", "/f"]);
        assert_eq!(code, 0);
        golden("scan.txt", &out);
        let (code, out) = cli(&fs, &["scan", "-r", "/f"]);
        assert_eq!(code, 0);
        golden("scan_recursive.txt", &out);
    }

    #[test]
    fn req_cli_1_list_and_filter_golden() {
        let fs = fixture();
        let (code, out) = cli(&fs, &["list", "-r", "/f"]);
        assert_eq!(code, 0);
        golden("list_recursive.txt", &out);
        let (code, out) = cli(
            &fs,
            &["list", "-r", "--filter", "rating>=2,!rejected", "/f"],
        );
        assert_eq!(code, 0);
        golden("list_filter_rating_ge_2.txt", &out);
        let (code, out) = cli(&fs, &["list", "--filter", "rejected", "/f"]);
        assert_eq!(code, 0);
        golden("list_filter_rejected.txt", &out);
        assert_eq!(cli(&fs, &["list", "--filter", "bogus", "/f"]).0, 2);
    }

    #[test]
    fn req_cli_1_export_golden() {
        let fs = fixture();
        let (code, out) = cli(&fs, &["export", "-r", "/f"]);
        assert_eq!(code, 0);
        golden("export.txt", &out);
        let (code, out) = cli(&fs, &["export", "-r", "--format", "csv", "/f"]);
        assert_eq!(code, 0);
        golden("export.csv", &out);
    }

    #[test]
    fn req_cli_1_rate_and_label_plan_writes_for_every_pair_member_without_writing() {
        let fs = fixture();
        let (code, out) = cli(&fs, &["rate", "--dry-run", "/f/a.ARW", "3"]);
        assert_eq!(code, 0);
        golden("rate_dry_run.txt", &out);
        let (code, out) = cli(&fs, &["label", "--dry-run", "/f/b.HEIC", "blue"]);
        assert_eq!(code, 0);
        golden("label_dry_run.txt", &out);
        let (code, out) = cli(&fs, &["label", "--dry-run", "/f/a.JPG", "none"]);
        assert_eq!(code, 0);
        golden("label_none_dry_run.txt", &out);
        // A bare file name means the current folder.
        let cwd = MemFs::new();
        cwd.add_file("./x.ARW", b"raw".to_vec());
        cwd.add_file("./x.JPG", b"jpg".to_vec());
        let (code, out) = cli(&cwd, &["rate", "--dry-run", "x.ARW", "5"]);
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            out,
            "would update sidecar of x.ARW: set xmp:Rating=5\nwould update sidecar of x.JPG: set xmp:Rating=5\n"
        );
        // Invalid values are rejected. (Writing without --dry-run: see the next test.)
        assert_ne!(cli(&fs, &["rate", "--dry-run", "/f/a.ARW", "7"]).0, 0);
        assert_ne!(cli(&fs, &["label", "--dry-run", "/f/a.ARW", "pink"]).0, 0);
        assert!(fs.calls().iter().all(|c| matches!(
            c,
            blink_runtime::fs::FsCall::Read(_)
                | blink_runtime::fs::FsCall::Metadata(_)
                | blink_runtime::fs::FsCall::ReadDir(_)
        )));
    }

    #[test]
    fn req_cli_1_req_open_2_rate_and_label_write_a_sidecar_for_every_pair_member() {
        let fs = fixture();
        let (code, out) = cli(&fs, &["rate", "/f/a.ARW", "3"]);
        assert_eq!(code, 0, "{out}");
        assert_eq!(out, "a.ARW: updated a.xmp\na.JPG: updated a.JPG.xmp\n");
        let (code, out) = cli(&fs, &["label", "--style", "darktable", "/f/a.JPG", "green"]);
        assert_eq!(code, 0, "{out}");
        for p in ["/f/a.xmp", "/f/a.JPG.xmp"] {
            let got = blink_xmp::read_owned(&fs.peek(p).unwrap()).unwrap();
            assert_eq!(got.rating, Some(3), "{p}");
            assert_eq!(got.label.as_deref(), Some("Green"), "{p}");
        }
        // Read back through `list`, like a second session.
        let (_, out) = cli(&fs, &["list", "--filter", "rating=3,label=Green", "/f"]);
        assert!(out.contains("a.ARW"), "{out}");
        // Only sidecars (and their temp files) were written.
        for c in fs.calls().iter().filter(|c| c.is_write()) {
            for p in c.changed_paths() {
                let n = p.to_string_lossy();
                assert!(n.ends_with(".xmp") || n.ends_with(".tmp"), "{c:?}");
            }
        }
        assert_ne!(cli(&fs, &["rate", "--style", "x", "/f/a.ARW", "1"]).0, 0);
    }

    #[test]
    fn req_cli_2_doctor_golden_for_synthetic_files() {
        let fs = fixture();
        for (file, golden_name) in [
            ("/f/a.ARW", "doctor_raw_container.txt"),
            ("/f/c.JPG", "doctor_jpeg_orientation_6.txt"),
            ("/f/b.NEF", "doctor_undecodable_raw.txt"),
            ("/f/b.HEIC", "doctor_heif.txt"),
        ] {
            let (_, out) = cli(&fs, &["doctor", file]);
            golden(golden_name, &out);
        }
        assert_eq!(cli(&fs, &["doctor", "/f/missing.ARW"]).0, 1);
    }

    #[test]
    fn req_open_4_doctor_lists_format_support() {
        let (code, out) = cli(&MemFs::new(), &["doctor", "--formats"]);
        assert_eq!(code, 0);
        golden("doctor_formats.txt", &out);
        for brand in [
            "arw", "cr2", "cr3", "nef", "raf", "orf", "rw2", "pef", "dng", "jpg", "heic",
        ] {
            assert!(out.contains(brand), "{brand} missing");
        }
    }
}
