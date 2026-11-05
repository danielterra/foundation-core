//! Trusted writes of values computed for calculated OWL properties.
//!
//! Ordinary `Individual::add_property` continues to reject calculated properties.
//! Formula/aggregation workers use this narrow API after computing a result.

use crate::eavto::{DbExecutor, Object, Triple};

/// The calculated-property metadata the caller resolved for this predicate.
/// This explicit classification keeps the materialization path unavailable to
/// ordinary property writes without introducing Foundation-specific IRIs here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputedPropertyKind {
    Formula,
    Aggregation,
}

/// Append one materialized value using the normal executor/store write path.
///
/// `kind` is the generic property-type metadata resolved by the caller. The
/// property definition itself remains owned by the ontology; this API is a
/// trusted worker boundary and deliberately does not repeat OWL validation.
/// `assert_triples` preserves the existing replacement semantics for a
/// single-valued calculated result, and its write notifications are drained
/// and delivered by `DbExecutor` as usual.
pub async fn write_computed_property(
    executor: &DbExecutor,
    subject: String,
    predicate: String,
    value: String,
    datatype: String,
    kind: ComputedPropertyKind,
    formula_metadata_predicate: String,
    aggregation_metadata_predicate: String,
) -> Result<i64, String> {
    let calculated_metadata_predicate = match kind {
        ComputedPropertyKind::Formula => formula_metadata_predicate,
        ComputedPropertyKind::Aggregation => aggregation_metadata_predicate,
    };
    let result = executor
        .write(move |conn| {
            let metadata = crate::eavto::query::get_by_entity_predicate(
                conn,
                &predicate,
                &calculated_metadata_predicate,
            )
            .map_err(|error| error.to_string())?;
            if metadata.triples.is_empty() {
                return Err(format!(
                    "Property '{}' is not marked as the requested calculated property type",
                    predicate
                ));
            }
            let fact = Triple::new(
                &subject,
                &predicate,
                Object::Literal {
                    value,
                    datatype: Some(datatype),
                    language: None,
                },
            );
            crate::owl::assert_raw_triples(conn, &[fact], "formula_worker").map(|tx| tx.to_string())
        })
        .await?;
    result.parse::<i64>().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eavto::{initialize_db, query};
    use std::{collections::HashMap, path::PathBuf};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn writes_computed_fact_and_notifies_through_executor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("materialization.db");
        let conn = initialize_db(&path).unwrap();
        let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<(
            HashMap<String, Vec<String>>,
            Vec<String>,
            Vec<crate::eavto::store::WrittenTriple>,
        )>();
        let executor = DbExecutor::new_with_notify(conn, PathBuf::from(&path), Some(notify_tx));
        executor
            .write(|conn| {
                let marker = Triple::new(
                    "test:computed",
                    "test:formula",
                    Object::Literal {
                        value: "1 + 1".into(),
                        datatype: Some("xsd:string".into()),
                        language: None,
                    },
                );
                crate::owl::assert_raw_triples(conn, &[marker], "test").map(|_| String::new())
            })
            .await
            .unwrap();
        let _ = notify_rx.try_recv().unwrap();

        let tx = write_computed_property(
            &executor,
            "test:instance".into(),
            "test:computed".into(),
            "42".into(),
            "xsd:integer".into(),
            ComputedPropertyKind::Formula,
            "test:formula".into(),
            "test:aggregation".into(),
        )
        .await
        .unwrap();

        let current = executor
            .read(move |conn| {
                query::get_by_entity_predicate(conn, "test:instance", "test:computed")
                    .map(|result| result.triples)
                    .map_err(|error| error.to_string())
            })
            .await
            .unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].tx, tx);
        assert_eq!(current[0].object.as_literal(), Some("42".into()));

        let (subjects, _, written) = notify_rx.try_recv().unwrap();
        assert_eq!(subjects.get("test:instance").unwrap(), &["test:computed"]);
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].tx, tx);
        assert_eq!(written[0].object_value.as_deref(), Some("42"));
    }

    #[test]
    fn ordinary_property_write_still_rejects_formula_properties() {
        use crate::owl::{Class, ClassType, Individual, Property, PropertyType};
        let mut conn = crate::eavto::test_helpers::setup_test_db();
        Class::new("test:Class")
            .assert(
                &mut conn,
                ClassType::OwlClass,
                "Class",
                "https://example.com/icon.svg",
                None,
                "test",
            )
            .unwrap();
        Property::new("test:computed")
            .assert(
                &mut conn,
                PropertyType::DatatypeProperty,
                "computed",
                None,
                &["test:Class"],
                Some("xsd:integer"),
                Some("unit:Count"),
                "test",
            )
            .unwrap();
        let subject = Individual::new("test:instance");
        subject
            .assert(
                &mut conn,
                "test:Class",
                "Instance",
                "https://example.com/icon.svg",
                "test",
            )
            .unwrap();
        conn.execute(
            "INSERT INTO transactions (origin, created_at) VALUES ('test', 0)",
            [],
        )
        .unwrap();
        let tx = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO triples (subject, predicate, object_value, object_type, object_datatype, origin_id, tx, created_at, retracted) VALUES ('test:computed', 'foundation:formula', '1 + 1', 'literal', 'xsd:string', 1, ?1, 0, 0)",
            [tx],
        )
        .unwrap();

        let result = subject.add_property(
            &mut conn,
            "test:computed",
            vec![Object::Literal {
                value: "2".into(),
                datatype: Some("xsd:integer".into()),
                language: None,
            }],
            "test",
        );
        assert!(matches!(
            result,
            Err(crate::owl::OwlError::ValidationError(_))
        ));
    }
}
