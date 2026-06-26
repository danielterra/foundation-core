# foundation-core

Local-first, append-only ontology store powering the [Foundation](https://github.com/danielterra/foundation) app.

> **Status:** early development (`0.1.0`). The EAVTO store and OWL primitives are extensively tested (650+ tests); the public API may still change.

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

## Building and testing

This is a standard Cargo library crate. SQLite is bundled (via `rusqlite`'s `bundled` feature), so no system SQLite is required.

```sh
cargo build              # build the library
cargo test               # run the test suite (650+ tests)
cargo doc --open         # generate and open API docs
```

The `update_material_symbols` binary (see [Regenerating the icon asset](#regenerating-the-icon-asset)) is the only extra build target. The `test-helpers` feature exposes `eavto::test_helpers` for use by downstream crates' tests.

## Usage

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
foundation-core = { git = "https://github.com/danielterra/foundation-core" }
```

### Open a store

`initialize_db` opens (or creates) the SQLite database, applies the schema, and — on first boot — loads the embedded Foundation base ontology:

```rust
use foundation_core::eavto::initialize_db;
use std::path::Path;

let mut conn = initialize_db(Path::new("FOUNDATION.db"))?;
```

### Assert facts

Every fact is a `(subject, predicate, object)` triple. `assert_raw_triples` retracts any existing value for the same `(subject, predicate)` and writes the new one under a fresh transaction; `batch_insert_triples` appends without retracting (use for multi-valued predicates):

```rust
use foundation_core::eavto::{Triple, Object};
use foundation_core::owl::assert_raw_triples;

let triples = vec![
    Triple::new("foundation:Computer", "rdf:type", Object::Iri("owl:Class".into())),
    Triple::new("foundation:Computer", "rdfs:label", Object::Literal {
        value: "Computer".into(),
        datatype: Some("xsd:string".into()),
        language: None,
    }),
];

let tx = assert_raw_triples(&mut conn, &triples, "my-app")?; // origin tag
```

Objects are typed: `Object::Iri`, `Object::Literal`, `Object::Integer`, `Object::Number`, `Object::Boolean`, and `Object::DateTime` (RFC 3339).

### Read classes and individuals

OWL primitives wrap the triple store with class/individual semantics, inheritance, and cardinality:

```rust
use foundation_core::owl::Class;

if let Some(class) = Class::get(&conn, "foundation:Computer")? {
    let instances = Class::get_instances(&conn, &class.iri)?;
    let descendants = Class::get_descendant_iris(&conn, &class.iri)?;
}
```

### Full-text search

The Tantivy index is initialized once from the database, then queried by IRI:

```rust
use foundation_core::search;
use std::path::Path;

search::init(Path::new("index_dir"), &conn);          // build/load the index
let hits: Vec<String> = search::search("computer", None, 20); // IRIs, ranked
```

> The snippets above are illustrative; see the module docs (`cargo doc`) and the in-crate tests for complete, compiling examples.

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
