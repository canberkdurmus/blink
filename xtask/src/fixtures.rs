//! `cargo xtask fixtures`: write the synthetic fixtures (SPEC §11.3) to `corpus/synthetic/` for
//! manual inspection. Tests generate the same bytes in memory via `blink-testkit`.

use crate::{Result, root};
use blink_testkit::fixtures;

pub fn run() -> Result {
    let dir = root().join("corpus").join("synthetic");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut files: Vec<(String, Vec<u8>)> = (1..=8u16)
        .map(|o| {
            (
                format!("orientation-{o}.jpg"),
                fixtures::orientation_jpeg(o),
            )
        })
        .collect();
    files.push(("adobe-rgb.jpg".into(), fixtures::adobe_rgb_jpeg()));
    for (name, bytes) in &files {
        std::fs::write(dir.join(name), bytes).map_err(|e| e.to_string())?;
    }
    println!("fixtures: {} files in {}", files.len(), dir.display());
    Ok(())
}
