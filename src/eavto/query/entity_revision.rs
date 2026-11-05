use rusqlite::{Connection, Row};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn entity_last_tx(conn: &Connection, subject: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT MAX(tx) FROM triples WHERE subject = ?1",
        [subject],
        read_entity_last_tx,
    )
    .map_err(Into::into)
}

fn read_entity_last_tx(row: &Row<'_>) -> rusqlite::Result<Option<i64>> {
    row.get(0)
}

#[cfg(test)]
mod tests {
    use super::entity_last_tx;
    use crate::eavto::store::assert_triples;
    use crate::eavto::test_helpers::setup_test_db;
    use crate::eavto::{enter_batch_transaction, Object, Triple};

    #[test]
    fn entity_revision_tracks_latest_write_inside_active_transaction() {
        let mut conn = setup_test_db();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let guard = enter_batch_transaction();

        let first_tx = assert_triples(
            &mut conn,
            &[Triple::new(
                "test:EntityA",
                "test:first",
                Object::Iri("test:ValueA".to_string()),
            )],
            "test",
        )
        .unwrap();
        let latest_tx = assert_triples(
            &mut conn,
            &[Triple::new(
                "test:EntityA",
                "test:second",
                Object::Iri("test:ValueB".to_string()),
            )],
            "test",
        )
        .unwrap();
        let other_entity_tx = assert_triples(
            &mut conn,
            &[Triple::new(
                "test:EntityB",
                "test:first",
                Object::Iri("test:ValueC".to_string()),
            )],
            "test",
        )
        .unwrap();

        assert!(first_tx < latest_tx);
        assert!(latest_tx < other_entity_tx);
        assert_eq!(
            entity_last_tx(&conn, "test:EntityA").unwrap(),
            Some(latest_tx)
        );
        assert_eq!(
            entity_last_tx(&conn, "test:EntityB").unwrap(),
            Some(other_entity_tx)
        );

        drop(guard);
        conn.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn entity_without_revision_returns_none() {
        let conn = setup_test_db();

        assert_eq!(entity_last_tx(&conn, "test:MissingEntity").unwrap(), None);
    }
}
