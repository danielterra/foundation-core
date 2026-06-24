# foundation-core

Local-first, append-only ontology store powering the [Foundation](https://github.com/danielterra/foundation) app.

## What is this

`foundation-core` is the persistence and reasoning kernel of Foundation. It provides:

- **EAVTO triple store** — an immutable, append-only SQLite-backed store where every fact is a `(subject, predicate, object, tx, origin)` tuple. Updates are new triples with a higher `tx`; the highest-`tx` triple per `(subject, predicate)` pair is the current fact. `retracted = 1` permanently removes a fact.
- **OWL primitives** — generic class/individual/property management, cardinality enforcement, inheritance, icon validation, and formula evaluation. All functions are parametric; domain IRIs are supplied by the caller.
- **Foundation base ontology** — the complete `foundation:*` vocabulary (classes, properties, individuals) embedded at compile time from `assets/ontology.sql`. The ontology is loaded into a fresh database on first boot and never re-imported.
- **Full-text search** — Tantivy-backed index initialized in a background thread on boot, stored in the platform app-data directory.

## Layers

```
Commands (app crate)
    └── Core-Ontology (app crate)
            └── OWL  (this crate — src/owl/)
                    └── EAVTO  (this crate — src/eavto/)
                                └── SQLite
```

Each layer imports only from the layer directly below it. `eavto/` contains no Foundation-specific IRIs. `owl/` contains no `foundation:*` or `anthropic:*` references.

## Configurable storage identity

Call `paths::configure()` once at startup, before any database initialization, to set the application directory name, database filename, and log/search namespace:

```rust
foundation_core::paths::configure(
    "Foundation",       // Documents/<this> directory
    "FOUNDATION.db",    // database filename inside that directory
    "org.w3id.foundation", // platform app-data namespace
);
```

Omitting this call uses the Foundation defaults above. The first call wins; subsequent calls are no-ops.

## Regenerating the icon asset

When `node_modules/material-symbols` is updated, regenerate `assets/material_symbols_names.txt`:

```sh
cargo run --manifest-path src-tauri/crates/foundation-core/Cargo.toml \
          --bin update_material_symbols
```

The build script (`build.rs`) reads this asset to produce `material_symbols.rs` in `OUT_DIR`, without requiring `node_modules` at build time.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
