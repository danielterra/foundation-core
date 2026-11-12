//! Versioned semantic renderer contracts shipped as a curated ontology overlay.
//!
//! This overlay is deliberately separate from the historical full ontology SQL
//! asset. It is applied through EAVTO append-only writes on database startup.

use crate::eavto::{store, Connection, Object, Triple};
use serde::Deserialize;

const OVERLAY_JSON: &str = include_str!("../assets/semantic_widget_catalog.json");
const OVERLAY_ORIGIN: &str = "foundation_core_semantic_catalog_overlay";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverlayPayload {
    format_version: u32,
    manifest_version: u64,
    triples: Vec<OverlayTriple>,
}

#[derive(Debug, Deserialize)]
struct OverlayTriple {
    subject: String,
    predicate: String,
    object: OverlayObject,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum OverlayObject {
    Iri {
        value: String,
    },
    Literal {
        value: String,
        datatype: Option<String>,
        language: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySyncSummary {
    pub manifest_version: u64,
    pub triple_count: usize,
    pub changed: bool,
}

/// Applies the packaged semantic widget catalog to a local ontology.
///
/// Repeated calls are idempotent: identical current facts are no-ops, while
/// changed facts are appended by the EAVTO store with a higher transaction.
/// The complete overlay is committed atomically and never writes directly to
/// SQLite tables.
pub fn sync(conn: &mut Connection) -> Result<OverlaySyncSummary, String> {
    let payload: OverlayPayload = serde_json::from_str(OVERLAY_JSON)
        .map_err(|error| format!("Embedded semantic catalog overlay is invalid: {error}"))?;
    if payload.format_version != 1 || payload.manifest_version == 0 || payload.triples.is_empty() {
        return Err("Embedded semantic catalog overlay has an unsupported or empty format".into());
    }

    let mut triples = payload
        .triples
        .into_iter()
        .map(OverlayTriple::into_triple)
        .collect::<Result<Vec<_>, _>>()?;
    if triples
        .iter()
        .any(|triple| !triple.subject.starts_with("foundation:"))
    {
        return Err("Semantic catalog overlay contains a non-Foundation subject".into());
    }
    add_missing_contract_types(&mut triples);

    let changed = store::with_transaction(conn, |conn| {
        let triples_to_write = preserve_existing_catalog_class_label(conn, triples.clone())?;
        let triples_to_write = omit_existing_contract_individuals(conn, triples_to_write)?;
        let triples_to_write = validate_existing_facts(conn, &triples_to_write)?;
        store::append_triples(conn, &triples_to_write, OVERLAY_ORIGIN)
            .map(|tx| tx != 0)
            .map_err(|error| format!("Failed to append semantic catalog overlay: {error}"))
    })?;

    Ok(OverlaySyncSummary {
        manifest_version: payload.manifest_version,
        triple_count: triples.len(),
        changed,
    })
}

fn add_missing_contract_types(triples: &mut Vec<Triple>) {
    let contract_subjects = triples
        .iter()
        .filter(|triple| triple.predicate == "foundation:componentContractVersion")
        .map(|triple| triple.subject.clone())
        .collect::<std::collections::BTreeSet<_>>();

    for subject in contract_subjects {
        let contract_type = Object::Iri("foundation:SemanticComponentDefinition".into());
        if !triples.iter().any(|triple| {
            triple.subject == subject
                && triple.predicate == "rdf:type"
                && triple.object == contract_type
        }) {
            triples.push(Triple::new(&subject, "rdf:type", contract_type));
        }
    }
}

fn semantic_objects_equal(existing: &Object, packaged: &Object) -> bool {
    match (existing, packaged) {
        (
            Object::Literal {
                value: existing_value,
                datatype: existing_datatype,
                language: existing_language,
            },
            Object::Literal {
                value: packaged_value,
                datatype: packaged_datatype,
                language: packaged_language,
            },
        ) => {
            existing_value == packaged_value
                && equivalent_string_datatype(existing_datatype.as_deref())
                    == equivalent_string_datatype(packaged_datatype.as_deref())
                && existing_language == packaged_language
        }
        _ => existing == packaged,
    }
}

fn equivalent_string_datatype(datatype: Option<&str>) -> Option<&str> {
    match datatype {
        Some("xsd:string") | Some("http://www.w3.org/2001/XMLSchema#string") => Some("xsd:string"),
        Some(other) => Some(other),
        None => None,
    }
}

/// Versioned contract individuals are owned by the app's catalog sync on an
/// existing installation. The base overlay seeds each individual atomically
/// only when its IRI has no current facts; it never merges or validates metadata
/// on a preexisting individual, leaving version/diff decisions to app-side sync.
fn omit_existing_contract_individuals(
    conn: &Connection,
    triples: Vec<Triple>,
) -> Result<Vec<Triple>, String> {
    let subjects = triples
        .iter()
        .filter(|triple| {
            (triple.predicate == "rdf:type"
                && triple.object
                    == Object::Iri("foundation:SemanticComponentDefinition".into()))
                // Historical contract versions in the generated overlay may
                // predate their rdf:type assertion. Their version predicate is
                // the stable marker that makes them part of the contract set.
                || triple.predicate == "foundation:componentContractVersion"
        })
        .map(|triple| triple.subject.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut existing_subjects = std::collections::BTreeSet::new();
    for subject in subjects {
        // Do not use `get_by_entity_predicate(..., "rdf:type")` here. The
        // contract importer can persist a contract's immutable metadata before
        // its type assertion (or on a partially upgraded local ontology). A
        // type-only test would then miss the existing IRI and preflight would
        // reject/attempt to rewrite the old hash. Looking at all current facts
        // also avoids depending on any ontology functional-property metadata
        // for rdf:type, which is inherently multi-valued.
        let existing = crate::eavto::query::get_by_entity(conn, &subject).map_err(|error| {
            format!("Could not inspect existing contract individual {subject}: {error}")
        })?;
        if !existing.triples.is_empty() {
            existing_subjects.insert(subject);
        }
    }
    Ok(triples
        .into_iter()
        .filter(|triple| {
            if triple.subject == "foundation:CoreOntologyRegistry"
                && triple.predicate == "foundation:includesIndividual"
                && matches!(&triple.object, Object::Iri(iri) if existing_subjects.contains(iri))
            {
                return false;
            }
            !existing_subjects.contains(&triple.subject) || triple.predicate == "rdf:type"
        })
        .collect())
}

fn preserve_existing_catalog_class_label(
    conn: &Connection,
    triples: Vec<Triple>,
) -> Result<Vec<Triple>, String> {
    let class_iri = "foundation:SemanticComponentDefinition";
    let labels = crate::eavto::query::get_by_entity_predicate(conn, class_iri, "rdfs:label")
        .map_err(|error| format!("Could not inspect semantic catalog class labels: {error}"))?;
    if labels.triples.is_empty() {
        return Ok(triples);
    }
    Ok(triples
        .into_iter()
        .filter(|triple| !(triple.subject == class_iri && triple.predicate == "rdfs:label"))
        .collect())
}

fn validate_existing_facts(conn: &Connection, expected: &[Triple]) -> Result<Vec<Triple>, String> {
    let mut groups: std::collections::BTreeMap<(&str, &str), Vec<&Triple>> =
        std::collections::BTreeMap::new();
    for triple in expected {
        groups
            .entry((&triple.subject, &triple.predicate))
            .or_default()
            .push(triple);
    }

    let mut triples_to_write = Vec::new();
    for ((subject, predicate), expected_objects) in groups {
        let existing = crate::eavto::query::get_by_entity_predicate(conn, subject, predicate)
            .map_err(|error| {
                format!("Could not inspect existing overlay fact {subject} {predicate}: {error}")
            })?;
        if existing.triples.is_empty() {
            triples_to_write.extend(expected_objects.into_iter().cloned());
            continue;
        }

        let existing_objects: Vec<&Object> = existing
            .triples
            .iter()
            .map(|triple| &triple.object)
            .collect();
        let is_append_only_relation = matches!(
            (subject, predicate),
            (
                "foundation:CoreOntologyRegistry",
                "foundation:includesIndividual"
            ) | (_, "rdf:type")
                | (_, "rdfs:subClassOf")
                | (_, "rdfs:comment")
                | ("foundation:SemanticComponentDefinition", "rdfs:label")
        );
        let compatible = is_append_only_relation || {
            existing_objects.len() == expected_objects.len()
                && expected_objects.iter().all(|triple| {
                    existing_objects
                        .iter()
                        .any(|object| semantic_objects_equal(object, &triple.object))
                })
        };
        if !compatible {
            let existing_diagnostics = existing
                .triples
                .iter()
                .map(|triple| persisted_fact_diagnostic(conn, triple))
                .collect::<Result<Vec<_>, _>>()?;
            let expected_diagnostics = expected_objects
                .iter()
                .map(|triple| {
                    format!(
                        "subject={subject:?}, predicate={predicate:?}, {}, tx=<not persisted>, origin={OVERLAY_ORIGIN:?}",
                        object_diagnostic(&triple.object)
                    )
                })
                .collect::<Vec<_>>();
            return Err(format!(
                "Existing ontology fact conflicts with immutable catalog overlay: {subject} {predicate} (existing: {}; expected: {})",
                existing_diagnostics.join("; "),
                expected_diagnostics.join("; ")
            ));
        }

        triples_to_write.extend(expected_objects.into_iter().filter_map(|expected| {
            let is_already_present = existing_objects
                .iter()
                .any(|existing| semantic_objects_equal(existing, &expected.object));
            (!is_already_present).then(|| expected.clone())
        }));
    }
    Ok(triples_to_write)
}

fn persisted_fact_diagnostic(conn: &Connection, triple: &Triple) -> Result<String, String> {
    let origin = crate::eavto::query::get_origin_name(conn, triple.origin_id)
        .map_err(|error| format!("Could not inspect overlay conflict provenance: {error}"))?
        .unwrap_or_else(|| format!("<unknown origin id {}>", triple.origin_id));
    Ok(format!(
        "subject={:?}, predicate={:?}, {}, tx={}, origin={origin:?}",
        triple.subject,
        triple.predicate,
        object_diagnostic(&triple.object),
        triple.tx
    ))
}

fn object_diagnostic(object: &Object) -> String {
    match object {
        Object::Iri(value) => {
            format!("object_type=iri, object={value:?}, datatype=None, lang=None")
        }
        Object::Blank(value) => {
            format!("object_type=blank, object={value:?}, datatype=None, lang=None")
        }
        Object::Literal {
            value,
            datatype,
            language,
        } => format!(
            "object_type=literal, object={value:?}, datatype={datatype:?}, lang={language:?}"
        ),
        Object::Integer(value) => format!(
            "object_type=literal, object={value:?}, datatype={:?}, lang=None",
            object.datatype()
        ),
        Object::Number(value) => format!(
            "object_type=literal, object={value:?}, datatype={:?}, lang=None",
            object.datatype()
        ),
        Object::Boolean(value) => format!(
            "object_type=literal, object={value:?}, datatype={:?}, lang=None",
            object.datatype()
        ),
        Object::DateTime(value) => format!(
            "object_type=literal, object={value:?}, datatype={:?}, lang=None",
            object.datatype()
        ),
    }
}

impl OverlayTriple {
    fn into_triple(self) -> Result<Triple, String> {
        if !self.subject.starts_with("foundation:")
            || !(self.predicate.starts_with("foundation:")
                || self.predicate.starts_with("rdf:")
                || self.predicate.starts_with("rdfs:"))
        {
            return Err("Semantic catalog overlay contains an invalid subject or predicate".into());
        }
        let object = match self.object {
            OverlayObject::Iri { value } => Object::Iri(value),
            OverlayObject::Literal {
                value,
                datatype,
                language,
            } => Object::Literal {
                value,
                datatype: datatype.map(|datatype| match datatype.as_str() {
                    "xsd:string" => "http://www.w3.org/2001/XMLSchema#string".into(),
                    _ => datatype,
                }),
                language,
            },
        };
        Ok(Triple::new(self.subject, self.predicate, object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eavto::test_helpers::setup_test_db;

    fn current_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM triples_current", [], |row| row.get(0))
            .unwrap()
    }

    fn current_value(conn: &Connection, subject: &str, predicate: &str) -> Option<String> {
        conn.query_row(
            "SELECT COALESCE(object, object_value) FROM triples_current
             WHERE subject = ?1 AND predicate = ?2 LIMIT 1",
            [subject, predicate],
            |row| row.get(0),
        )
        .ok()
    }

    #[test]
    fn applies_versioned_catalog_contracts_to_an_empty_ontology() {
        let mut conn = setup_test_db();
        let summary = sync(&mut conn).unwrap();

        assert!(summary.changed);
        assert!(summary.manifest_version > 0);
        assert!(summary.triple_count >= 100);
        assert_eq!(
            current_value(
                &conn,
                "foundation:SemanticComponentDefinition_Details_v2",
                "foundation:componentContractVersion"
            ),
            Some("2".into())
        );
        assert_eq!(
            current_value(
                &conn,
                "foundation:SemanticComponentDefinition_Action_v1",
                "foundation:componentContractVersion"
            ),
            Some("1".into())
        );
        let action_is_registered: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM triples_current
                 WHERE subject = 'foundation:CoreOntologyRegistry'
                   AND predicate = 'foundation:includesIndividual'
                   AND object = 'foundation:SemanticComponentDefinition_Action_v1')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(action_is_registered);
    }

    #[test]
    fn fresh_database_imports_versioned_catalog_before_app_setup_and_reopens_idempotently() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("base-ontology-catalog-test.sqlite");
        let conn = crate::eavto::connection::initialize_db(&db_path).unwrap();

        assert!(crate::owl::Class::exists(
            &conn,
            "foundation:SemanticComponentDefinition"
        ));
        for component in ["Decision", "ValueEntry", "Information"] {
            let contract = format!("foundation:SemanticComponentDefinition_{component}_v1");
            assert!(
                crate::owl::Individual::get(&conn, &contract)
                    .unwrap()
                    .is_some(),
                "fresh database must include the versioned {component} contract before app setup"
            );
            let version: String = conn
                .query_row(
                    "SELECT object_value FROM triples_current
                     WHERE subject = ?1 AND predicate = 'foundation:componentContractVersion'",
                    [&contract],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(version, "1");
        }
        let details_v2 = "foundation:SemanticComponentDefinition_Details_v2";
        assert!(
            crate::owl::Individual::get(&conn, details_v2)
                .unwrap()
                .is_some(),
            "clean seed must publish the current Details v2 contract"
        );
        let details_version = crate::eavto::query::get_by_entity_predicate(
            &conn,
            details_v2,
            "foundation:componentContractVersion",
        )
        .unwrap();
        assert_eq!(
            details_version.triples[0].object.as_literal().as_deref(),
            Some("2")
        );
        assert!(
            crate::owl::Individual::get(&conn, "foundation:CoreOntologyRegistry")
                .unwrap()
                .is_some()
        );
        let before_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM triples_current", [], |row| row.get(0))
            .unwrap();
        drop(conn);

        let reopened = crate::eavto::connection::initialize_db(&db_path).unwrap();
        let after_count: i64 = reopened
            .query_row("SELECT COUNT(*) FROM triples_current", [], |row| row.get(0))
            .unwrap();
        assert_eq!(after_count, before_count);
    }

    #[test]
    fn repeated_sync_is_idempotent_and_preserves_existing_local_facts() {
        let mut conn = setup_test_db();
        store::append_triples(
            &mut conn,
            &[Triple::new(
                "foundation:CoreOntologyRegistry",
                "foundation:includesIndividual",
                Object::Iri("foundation:ExistingCoreIndividual".into()),
            )],
            "test_fixture",
        )
        .unwrap();
        sync(&mut conn).unwrap();
        store::assert_triples(
            &mut conn,
            &[Triple::new(
                "foundation:LocalSentinel",
                "foundation:sentinelValue",
                Object::Literal {
                    value: "preserve-me".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            )],
            "test_fixture",
        )
        .unwrap();
        let before_count = current_count(&conn);

        let repeated = sync(&mut conn).unwrap();

        assert!(!repeated.changed);
        assert_eq!(current_count(&conn), before_count);
        assert_eq!(
            current_value(
                &conn,
                "foundation:LocalSentinel",
                "foundation:sentinelValue"
            ),
            Some("preserve-me".into())
        );
        let existing_registry_link: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM triples_current
                 WHERE subject = 'foundation:CoreOntologyRegistry'
                   AND predicate = 'foundation:includesIndividual'
                   AND object = 'foundation:ExistingCoreIndividual')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(existing_registry_link);
    }

    #[test]
    fn leaves_core_schema_labels_to_the_base_ontology_pipeline() {
        let mut conn = setup_test_db();
        store::append_triples(
            &mut conn,
            &[Triple::new(
                "foundation:componentActionsSchema",
                "rdfs:label",
                Object::Literal {
                    value: "Actions Schema (legacy)".into(),
                    datatype: Some("http://www.w3.org/2001/XMLSchema#string".into()),
                    language: None,
                },
            )],
            "core_ontology_sync",
        )
        .unwrap();
        store::append_triples(
            &mut conn,
            &[Triple::new(
                "foundation:LocalSentinel",
                "foundation:sentinelValue",
                Object::Literal {
                    value: "preserve-me".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            )],
            "user_data",
        )
        .unwrap();

        let first_sync = sync(&mut conn).unwrap();

        assert!(first_sync.changed);
        assert_eq!(
            current_value(&conn, "foundation:componentActionsSchema", "rdfs:label"),
            Some("Actions Schema (legacy)".into())
        );
        assert_eq!(
            current_value(
                &conn,
                "foundation:LocalSentinel",
                "foundation:sentinelValue"
            ),
            Some("preserve-me".into())
        );
        let label_history_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM triples
                 WHERE subject = 'foundation:componentActionsSchema'
                   AND predicate = 'rdfs:label'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(label_history_count, 1);

        let second_sync = sync(&mut conn).unwrap();
        assert!(!second_sync.changed);
        let final_label_history_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM triples
                 WHERE subject = 'foundation:componentActionsSchema'
                   AND predicate = 'rdfs:label'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(final_label_history_count, label_history_count);
    }

    #[test]
    fn leaves_user_owned_core_schema_label_untouched() {
        let mut conn = setup_test_db();
        store::append_triples(
            &mut conn,
            &[Triple::new(
                "foundation:componentActionsSchema",
                "rdfs:label",
                Object::Literal {
                    value: "User-owned schema label".into(),
                    datatype: Some("http://www.w3.org/2001/XMLSchema#string".into()),
                    language: None,
                },
            )],
            "user_data",
        )
        .unwrap();

        sync(&mut conn).unwrap();

        assert_eq!(
            current_value(&conn, "foundation:componentActionsSchema", "rdfs:label"),
            Some("User-owned schema label".into())
        );
    }

    #[test]
    fn accepts_semantically_equal_schema_label_with_ai_provenance_without_rewriting_it() {
        let mut conn = setup_test_db();
        sync(&mut conn).unwrap();
        store::assert_triples(
            &mut conn,
            &[Triple::new(
                "foundation:componentActionsSchema",
                "rdfs:label",
                Object::Literal {
                    value: "Component Actions Schema".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            )],
            "ai",
        )
        .unwrap();
        let before = crate::eavto::query::get_by_entity_predicate(
            &conn,
            "foundation:componentActionsSchema",
            "rdfs:label",
        )
        .unwrap()
        .triples
        .remove(0);

        let first_sync = sync(&mut conn).unwrap();
        assert!(!first_sync.changed);
        let after_first = crate::eavto::query::get_by_entity_predicate(
            &conn,
            "foundation:componentActionsSchema",
            "rdfs:label",
        )
        .unwrap()
        .triples;
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].object, before.object);
        assert_eq!(after_first[0].tx, before.tx);
        assert_eq!(
            crate::eavto::query::get_origin_name(&conn, after_first[0].origin_id).unwrap(),
            Some("ai".into())
        );

        let before_repeat = current_count(&conn);
        let second_sync = sync(&mut conn).unwrap();
        assert!(!second_sync.changed);
        assert_eq!(current_count(&conn), before_repeat);
    }

    #[test]
    fn omits_existing_versioned_contract_individual_for_app_side_validation() {
        let mut conn = setup_test_db();
        let subject = "foundation:SemanticComponentDefinition_Action_v1";
        let existing_facts = [
            Triple::new(
                subject,
                "rdf:type",
                Object::Iri("foundation:SemanticComponentDefinition".into()),
            ),
            Triple::new(
                subject,
                "foundation:componentId",
                Object::Literal {
                    value: "Action".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            ),
            Triple::new(
                subject,
                "foundation:componentContractVersion",
                Object::Literal {
                    value: "legacy-version".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            ),
            Triple::new(
                subject,
                "foundation:componentManifestVersion",
                Object::Literal {
                    value: "999".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            ),
            Triple::new(
                subject,
                "foundation:componentContractHash",
                Object::Literal {
                    value: "sha256:existing-app-managed-hash".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            ),
            Triple::new(
                subject,
                "foundation:componentPropsSchema",
                Object::Literal {
                    value: "{\"legacy\":true}".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            ),
        ];
        store::append_triples(&mut conn, &existing_facts, "test_fixture").unwrap();

        sync(&mut conn).unwrap();
        for fact in existing_facts {
            let result =
                crate::eavto::query::get_by_entity_predicate(&conn, &fact.subject, &fact.predicate)
                    .unwrap();
            assert!(result
                .triples
                .iter()
                .any(|triple| triple.object == fact.object));
        }
        let after_first_sync = current_count(&conn);
        sync(&mut conn).unwrap();
        assert_eq!(current_count(&conn), after_first_sync);
        assert_eq!(current_count(&conn), after_first_sync);
    }

    #[test]
    fn preserves_existing_details_v1_hash_and_seeds_v2_on_an_existing_install() {
        let mut conn = setup_test_db();
        let v1 = "foundation:SemanticComponentDefinition_Details_v1";
        let old_hash = "sha256:92e-existing-local-contract-hash";
        let original = Triple::new(
            v1,
            "foundation:componentContractHash",
            Object::Literal {
                value: old_hash.into(),
                datatype: Some("xsd:string".into()),
                language: None,
            },
        );
        // Simulate an existing app-synced contract whose immutable facts were
        // written without its rdf:type assertion. The IRI/hash already identify
        // a persisted version and the base seed must not merge its metadata.
        let first_write = store::append_triples(
            &mut conn,
            std::slice::from_ref(&original),
            "semantic_catalog_sync",
        )
        .unwrap();
        let existing_tx = first_write;

        sync(&mut conn).unwrap();

        let retained = crate::eavto::query::get_by_entity_predicate(
            &conn,
            v1,
            "foundation:componentContractHash",
        )
        .unwrap();
        assert_eq!(retained.triples.len(), 1);
        assert_eq!(retained.triples[0].object, original.object);
        assert_eq!(retained.triples[0].tx, existing_tx);
        let retained_type =
            crate::eavto::query::get_by_entity_predicate(&conn, v1, "rdf:type").unwrap();
        assert!(retained_type
            .triples
            .iter()
            .any(|triple| triple.object
                == Object::Iri("foundation:SemanticComponentDefinition".into())));
        assert_eq!(
            crate::eavto::query::get_origin_name(&conn, retained.triples[0].origin_id).unwrap(),
            Some("semantic_catalog_sync".into())
        );
        assert!(
            crate::eavto::query::get_by_entity_predicate(
                &conn,
                v1,
                "foundation:componentPropsSchema"
            )
            .unwrap()
            .triples
            .is_empty(),
            "existing immutable v1 must not be partially merged from the overlay"
        );

        let v2 = "foundation:SemanticComponentDefinition_Details_v2";
        let version = crate::eavto::query::get_by_entity_predicate(
            &conn,
            v2,
            "foundation:componentContractVersion",
        )
        .unwrap();
        assert_eq!(version.triples.len(), 1);
        assert_eq!(version.triples[0].object.as_literal().as_deref(), Some("2"));
        assert_eq!(
            current_value(&conn, v2, "foundation:componentContractHash"),
            Some("sha256:b2063f2ddda1eba7a7a7e03d3dc57650134be88b10f148f7f07faad42ae0c5df".into())
        );

        let after_first_sync = current_count(&conn);
        let repeated = sync(&mut conn).unwrap();
        assert!(!repeated.changed, "repeat sync must be idempotent");
        assert_eq!(current_count(&conn), after_first_sync);
        let retained_again = crate::eavto::query::get_by_entity_predicate(
            &conn,
            v1,
            "foundation:componentContractHash",
        )
        .unwrap();
        assert_eq!(retained_again.triples[0].object, original.object);
        assert_eq!(retained_again.triples[0].tx, existing_tx);
    }

    #[test]
    fn preflight_preserves_existing_multivalued_comments() {
        let mut conn = setup_test_db();
        store::append_triples(
            &mut conn,
            &[Triple::new(
                "foundation:SemanticComponentDefinition_Action_v1",
                "rdfs:comment",
                Object::Literal {
                    value: "existing annotation".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            )],
            "test_fixture",
        )
        .unwrap();
        let expected = Triple::new(
            "foundation:SemanticComponentDefinition_Action_v1",
            "rdfs:comment",
            Object::Literal {
                value: "catalog annotation".into(),
                datatype: Some("xsd:string".into()),
                language: None,
            },
        );

        validate_existing_facts(&conn, std::slice::from_ref(&expected)).unwrap();
        store::append_triples(&mut conn, &[expected], OVERLAY_ORIGIN).unwrap();

        let comments: Vec<String> = conn
            .prepare(
                "SELECT object_value FROM triples_current
                 WHERE subject = 'foundation:SemanticComponentDefinition_Action_v1'
                   AND predicate = 'rdfs:comment' ORDER BY object_value",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(comments, vec!["catalog annotation", "existing annotation"]);
    }

    #[test]
    fn reports_existing_and_expected_fact_details_on_conflict() {
        let mut conn = setup_test_db();
        store::append_triples(
            &mut conn,
            &[Triple::new(
                "foundation:componentContractHash",
                "rdfs:label",
                Object::Literal {
                    value: "Existing Contract Hash".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            )],
            "ai",
        )
        .unwrap();
        let expected = Triple::new(
            "foundation:componentContractHash",
            "rdfs:label",
            Object::Literal {
                value: "Component Contract Hash".into(),
                datatype: Some("http://www.w3.org/2001/XMLSchema#string".into()),
                language: None,
            },
        );

        let error = validate_existing_facts(&conn, &[expected]).unwrap_err();

        assert!(error.contains("subject=\"foundation:componentContractHash\""));
        assert!(error.contains("predicate=\"rdfs:label\""));
        assert!(error.contains("object=\"Existing Contract Hash\""));
        assert!(error.contains("datatype=Some(\"xsd:string\")"));
        assert!(error.contains("tx="));
        assert!(error.contains("origin=\"ai\""));
        assert!(error.contains("object=\"Component Contract Hash\""));
        assert!(error.contains("datatype=Some(\"http://www.w3.org/2001/XMLSchema#string\")"));
        assert!(error.contains("tx=<not persisted>"));
        assert!(error.contains(&format!("origin={OVERLAY_ORIGIN:?}")));
    }

    #[test]
    fn accepts_string_datatype_aliases_for_multiple_ai_catalog_facts_without_writes() {
        let mut conn = setup_test_db();
        sync(&mut conn).unwrap();
        let labels = [
            Triple::new(
                "foundation:componentContractHash",
                "rdfs:label",
                Object::Literal {
                    value: "Component Contract Hash".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            ),
            Triple::new(
                "foundation:componentPropsSchema",
                "rdfs:label",
                Object::Literal {
                    value: "Component Props Schema".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            ),
        ];
        store::assert_triples(&mut conn, &labels, "ai").unwrap();
        let before_count = current_count(&conn);
        let before_facts = labels
            .iter()
            .map(|label| {
                crate::eavto::query::get_by_entity_predicate(
                    &conn,
                    &label.subject,
                    &label.predicate,
                )
                .unwrap()
                .triples
                .remove(0)
            })
            .collect::<Vec<_>>();

        let summary = sync(&mut conn).unwrap();

        assert!(!summary.changed);
        assert_eq!(current_count(&conn), before_count);
        for ((expected, before), after) in
            labels
                .iter()
                .zip(before_facts.iter())
                .zip(labels.iter().map(|label| {
                    crate::eavto::query::get_by_entity_predicate(
                        &conn,
                        &label.subject,
                        &label.predicate,
                    )
                    .unwrap()
                    .triples
                    .remove(0)
                }))
        {
            assert_eq!(after.object, expected.object);
            assert_eq!(after.tx, before.tx);
            assert_eq!(
                crate::eavto::query::get_origin_name(&conn, after.origin_id).unwrap(),
                Some("ai".into())
            );
        }
    }

    #[test]
    fn rejects_ai_fact_when_it_conflicts_with_an_expected_overlay_value() {
        let mut conn = setup_test_db();
        store::assert_triples(
            &mut conn,
            &[Triple::new(
                "foundation:componentContractHash",
                "rdfs:label",
                Object::Literal {
                    value: "Different Contract Hash".into(),
                    datatype: Some("xsd:string".into()),
                    language: None,
                },
            )],
            "ai",
        )
        .unwrap();

        let expected = Triple::new(
            "foundation:componentContractHash",
            "rdfs:label",
            Object::Literal {
                value: "Component Contract Hash".into(),
                datatype: Some("http://www.w3.org/2001/XMLSchema#string".into()),
                language: None,
            },
        );
        let error = validate_existing_facts(&conn, &[expected]).unwrap_err();

        assert!(error.contains("foundation:componentContractHash rdfs:label"));
        assert!(error.contains("Different Contract Hash"));
        assert!(error.contains("Component Contract Hash"));
        assert!(error.contains("origin=\"ai\""));
    }
}
