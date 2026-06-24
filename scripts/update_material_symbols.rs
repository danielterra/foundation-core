//! Regenerates `assets/material_symbols_names.txt` from the local
//! `node_modules/material-symbols` package.
//!
//! Run from the project root:
//!   cargo run --manifest-path src-tauri/crates/foundation-core/Cargo.toml \
//!             --bin update_material_symbols
//!
//! The asset is then picked up by `build.rs` to produce `material_symbols.rs`
//! without any node_modules dependency at build time.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir
        .parent()
        .expect("foundation-core has no parent crates/")
        .parent()
        .expect("crates/ has no parent src-tauri/")
        .parent()
        .expect("src-tauri/ has no parent (project root)");

    let dts_path = project_root
        .join("node_modules")
        .join("material-symbols")
        .join("index.d.ts");
    let pkg_path = project_root
        .join("node_modules")
        .join("material-symbols")
        .join("package.json");
    let asset_path = manifest_dir
        .join("assets")
        .join("material_symbols_names.txt");

    let dts_content = std::fs::read_to_string(&dts_path).unwrap_or_else(|e| {
        eprintln!("ERROR: Cannot read {}: {}", dts_path.display(), e);
        eprintln!("  Run `npm install` first.");
        std::process::exit(1);
    });

    let pkg_content = std::fs::read_to_string(&pkg_path).unwrap_or_else(|e| {
        eprintln!("ERROR: Cannot read {}: {}", pkg_path.display(), e);
        std::process::exit(1);
    });

    let version = pkg_content
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("\"version\"") {
                trimmed
                    .split(':')
                    .nth(1)
                    .map(|v| v.trim().trim_matches(|c| c == '"' || c == ',').to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            eprintln!("ERROR: version field not found in package.json");
            std::process::exit(1);
        });

    let mut names: Vec<&str> = dts_content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('"') && trimmed.ends_with("\",") {
                Some(&trimmed[1..trimmed.len() - 2])
            } else {
                None
            }
        })
        .collect();

    names.sort_unstable();
    names.dedup();

    let mut output = String::new();
    output.push_str(&format!("# material-symbols version: {}\n", version));
    output.push_str("# source: node_modules/material-symbols/index.d.ts\n");
    for name in &names {
        output.push_str(name);
        output.push('\n');
    }

    std::fs::write(&asset_path, &output).unwrap_or_else(|e| {
        eprintln!("ERROR: Cannot write {}: {}", asset_path.display(), e);
        std::process::exit(1);
    });

    eprintln!(
        "Updated {} — {} icons, version {}",
        asset_path.display(),
        names.len(),
        version
    );
}
