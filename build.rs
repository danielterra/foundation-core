fn main() {
    generate_material_symbols_list();
}

fn generate_material_symbols_list() {
    use std::fmt::Write as _;
    use std::path::PathBuf;

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let asset_path = manifest_dir.join("assets").join("material_symbols_names.txt");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = PathBuf::from(&out_dir).join("material_symbols.rs");

    println!("cargo:rerun-if-changed={}", asset_path.display());

    let content = std::fs::read_to_string(&asset_path)
        .expect("assets/material_symbols_names.txt not found — run scripts/update_material_symbols");

    let version = content
        .lines()
        .find(|l| l.starts_with("# material-symbols version:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|v| v.trim().to_string())
        .expect("version header missing from assets/material_symbols_names.txt");

    let mut names: Vec<&str> = content
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();

    names.sort_unstable();

    let mut buf = String::new();
    writeln!(buf, "pub static MATERIAL_SYMBOLS_VERSION: &str = \"{version}\";").unwrap();
    writeln!(buf, "pub static MATERIAL_SYMBOLS: &[&str] = &[").unwrap();
    for name in &names {
        writeln!(buf, "    \"{name}\",").unwrap();
    }
    writeln!(buf, "];").unwrap();

    std::fs::write(&out_path, buf).unwrap();
}
