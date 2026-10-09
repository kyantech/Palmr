use std::path::{Path, PathBuf};

use anyhow::{ensure, Result};

const BUDGET: u64 = 8 * 1024;
const MAX_DEPTH: usize = 6;

fn tests_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn walk(dir: &Path, depth: usize, visit: &mut dyn FnMut(&Path, u64)) -> Result<()> {
    ensure!(
        depth <= MAX_DEPTH,
        "directory nesting deeper than {MAX_DEPTH}"
    );
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            "node_modules" | "target" | "test-results" | "playwright-report"
        ) {
            continue;
        }
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            walk(&path, depth + 1, visit)?;
        } else if meta.is_file() {
            visit(&path, meta.len());
        }
    }
    Ok(())
}

#[test]
fn unit_no_generated_payload_fixtures_are_committed() -> Result<()> {
    let fixtures = tests_root().join("fixtures");
    let mut oversized = Vec::new();
    walk(&fixtures, 0, &mut |path, len| {
        if len >= BUDGET {
            oversized.push(format!("{} ({len} bytes)", path.display()));
        }
    })?;
    ensure!(
        oversized.is_empty(),
        "fixtures over the 8 KiB budget: {oversized:?}"
    );

    let mut payloads = Vec::new();
    walk(&tests_root(), 0, &mut |path, _| {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let archive = [".zip", ".tar", ".gz"]
            .iter()
            .any(|suffix| name.ends_with(suffix));
        let raw = [".bin", ".dat", ".img", ".iso"]
            .iter()
            .any(|suffix| name.ends_with(suffix));
        if raw || (archive && !path.starts_with(&fixtures)) {
            payloads.push(path.display().to_string());
        }
    })?;
    ensure!(
        payloads.is_empty(),
        "generated-payload files under tests/: {payloads:?}"
    );
    Ok(())
}
