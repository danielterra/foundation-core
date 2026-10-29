use rusqlite::{named_params, Connection};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordListItem {
    pub iri: String,
    pub creation_tx: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordStateGroup {
    pub state_iri: Option<String>,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordListPage {
    pub items: Vec<RecordListItem>,
    pub state_groups: Vec<RecordStateGroup>,
    pub snapshot_tx: i64,
    pub next_after_tx: Option<i64>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RecordListQuery<'a> {
    pub type_predicate: &'a str,
    pub class_iri: &'a str,
    pub parent_predicate: &'a str,
    pub parent_iri: Option<&'a str>,
    pub state_predicate: &'a str,
    pub state_iri: Option<&'a str>,
    pub snapshot_tx: Option<i64>,
    pub after_tx: Option<i64>,
    pub limit: usize,
}

const RECORD_CTES: &str = "
WITH
latest_type AS (
    SELECT subject, MAX(tx) AS tx
    FROM triples
    WHERE predicate = :type_predicate AND tx <= :snapshot_tx
    GROUP BY subject
),
creation AS (
    SELECT subject, MIN(tx) AS creation_tx
    FROM triples
    WHERE predicate = :type_predicate
      AND object = :class_iri
      AND object_type = 'iri'
      AND retracted = 0
      AND tx <= :snapshot_tx
    GROUP BY subject
),
members AS (
    SELECT DISTINCT t.subject, c.creation_tx
    FROM latest_type lt
    JOIN triples t
      ON t.subject = lt.subject
     AND t.predicate = :type_predicate
     AND t.tx = lt.tx
     AND t.object = :class_iri
     AND t.object_type = 'iri'
     AND t.retracted = 0
    JOIN creation c ON c.subject = t.subject
),
latest_parent AS (
    SELECT subject, MAX(tx) AS tx
    FROM triples
    WHERE predicate = :parent_predicate AND tx <= :snapshot_tx
    GROUP BY subject
),
parents AS (
    SELECT DISTINCT p.subject
    FROM latest_parent lp
    JOIN triples p
      ON p.subject = lp.subject
     AND p.predicate = :parent_predicate
     AND p.tx = lp.tx
     AND p.object = :parent_iri
     AND p.object_type = 'iri'
     AND p.retracted = 0
),
latest_state AS (
    SELECT subject, MAX(tx) AS tx
    FROM triples
    WHERE predicate = :state_predicate AND tx <= :snapshot_tx
    GROUP BY subject
),
states AS (
    SELECT DISTINCT s.subject, s.object AS state_iri
    FROM latest_state ls
    JOIN triples s
      ON s.subject = ls.subject
     AND s.predicate = :state_predicate
     AND s.tx = ls.tx
     AND s.object_type = 'iri'
     AND s.retracted = 0
),
eligible AS (
    SELECT m.subject, m.creation_tx
    FROM members m
    WHERE :parent_iri IS NULL OR EXISTS (
        SELECT 1
        FROM parents p
        WHERE p.subject = m.subject
    )
)
";

pub fn list_records_page(conn: &Connection, query: RecordListQuery<'_>) -> Result<RecordListPage> {
    validate_query(&query)?;

    let max_tx = conn.query_row("SELECT COALESCE(MAX(tx), 0) FROM triples", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let snapshot_tx = query.snapshot_tx.unwrap_or(max_tx);
    if snapshot_tx < 0 {
        return Err(invalid_input("snapshot_tx must be non-negative"));
    }
    if let Some(after_tx) = query.after_tx {
        if after_tx < 0 || after_tx > snapshot_tx {
            return Err(invalid_input(
                "after_tx must be non-negative and no greater than snapshot_tx",
            ));
        }
    }

    let fetch_limit = query
        .limit
        .checked_add(1)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| invalid_input("limit is too large"))?;

    let page_sql = format!(
        "{RECORD_CTES}
         SELECT e.subject, e.creation_tx
         FROM eligible e
         WHERE (:state_iri IS NULL OR EXISTS (
             SELECT 1 FROM states s
             WHERE s.subject = e.subject AND s.state_iri = :state_iri
         ))
           AND (:after_tx IS NULL OR e.creation_tx < :after_tx)
         ORDER BY e.creation_tx DESC, e.subject ASC
         LIMIT :limit"
    );
    let mut page_stmt = conn.prepare(&page_sql)?;
    let mut items = page_stmt
        .query_map(
            named_params! {
                ":type_predicate": query.type_predicate,
                ":class_iri": query.class_iri,
                ":parent_predicate": query.parent_predicate,
                ":parent_iri": query.parent_iri,
                ":state_predicate": query.state_predicate,
                ":state_iri": query.state_iri,
                ":snapshot_tx": snapshot_tx,
                ":after_tx": query.after_tx,
                ":limit": fetch_limit,
            },
            |row| {
                Ok(RecordListItem {
                    iri: row.get(0)?,
                    creation_tx: row.get(1)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let has_more = items.len() > query.limit;
    if has_more {
        items.pop();
    }
    let next_after_tx = has_more && !items.is_empty();
    let next_after_tx = next_after_tx.then(|| items.last().expect("non-empty page").creation_tx);

    let groups_sql = format!(
        "{RECORD_CTES}
         SELECT s.state_iri, COUNT(DISTINCT e.subject) AS total
         FROM eligible e
         LEFT JOIN states s ON s.subject = e.subject
         GROUP BY s.state_iri
         ORDER BY s.state_iri ASC"
    );
    let mut groups_stmt = conn.prepare(&groups_sql)?;
    let state_groups = groups_stmt
        .query_map(
            named_params! {
                ":type_predicate": query.type_predicate,
                ":class_iri": query.class_iri,
                ":parent_predicate": query.parent_predicate,
                ":parent_iri": query.parent_iri,
                ":state_predicate": query.state_predicate,
                ":snapshot_tx": snapshot_tx,
            },
            |row| {
                let count = row.get::<_, i64>(1)?;
                Ok(RecordStateGroup {
                    state_iri: row.get(0)?,
                    count: count as usize,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(RecordListPage {
        items,
        state_groups,
        snapshot_tx,
        next_after_tx,
        has_more,
    })
}

fn validate_query(query: &RecordListQuery<'_>) -> Result<()> {
    for (name, value) in [
        ("type_predicate", query.type_predicate),
        ("class_iri", query.class_iri),
        ("parent_predicate", query.parent_predicate),
        ("state_predicate", query.state_predicate),
    ] {
        if value.trim().is_empty() {
            return Err(invalid_input(&format!("{name} must not be empty")));
        }
    }
    if query.state_iri.is_some_and(|value| value.trim().is_empty()) {
        return Err(invalid_input("state_iri must not be empty when provided"));
    }
    if query
        .parent_iri
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(invalid_input("parent_iri must not be empty when provided"));
    }
    if query.limit == 0 {
        return Err(invalid_input("limit must be greater than zero"));
    }
    Ok(())
}

fn invalid_input(message: &str) -> Box<dyn std::error::Error> {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eavto::{store, test_helpers::setup_test_db, Object, Triple};

    const TYPE: &str = "test:type";
    const CLASS: &str = "test:Record";
    const PARENT: &str = "test:belongsTo";
    const ROOT: &str = "test:Root";
    const STATE: &str = "test:state";

    fn add_record(conn: &mut Connection, iri: &str, state: &str) -> i64 {
        store::assert_triples(
            conn,
            &[
                Triple::new(iri, TYPE, Object::Iri(CLASS.to_string())),
                Triple::new(iri, PARENT, Object::Iri(ROOT.to_string())),
                Triple::new(iri, STATE, Object::Iri(state.to_string())),
            ],
            "test",
        )
        .unwrap()
    }

    fn query<'a>(
        snapshot_tx: Option<i64>,
        after_tx: Option<i64>,
        limit: usize,
    ) -> RecordListQuery<'a> {
        RecordListQuery {
            type_predicate: TYPE,
            class_iri: CLASS,
            parent_predicate: PARENT,
            parent_iri: Some(ROOT),
            state_predicate: STATE,
            state_iri: None,
            snapshot_tx,
            after_tx,
            limit,
        }
    }

    #[test]
    fn counts_groups_and_filters_state() {
        let mut conn = setup_test_db();
        add_record(&mut conn, "test:A", "test:Open");
        add_record(&mut conn, "test:B", "test:Open");
        add_record(&mut conn, "test:C", "test:Closed");

        let page = list_records_page(&conn, query(None, None, 10)).unwrap();
        assert_eq!(page.items.len(), 3);
        assert_eq!(
            page.state_groups,
            vec![
                RecordStateGroup {
                    state_iri: Some("test:Closed".to_string()),
                    count: 1,
                },
                RecordStateGroup {
                    state_iri: Some("test:Open".to_string()),
                    count: 2,
                },
            ]
        );

        let mut filtered = query(Some(page.snapshot_tx), None, 10);
        filtered.state_iri = Some("test:Closed");
        let page = list_records_page(&conn, filtered).unwrap();
        assert_eq!(
            page.items,
            vec![RecordListItem {
                iri: "test:C".to_string(),
                creation_tx: 3
            }]
        );
    }

    #[test]
    fn orders_paginates_and_applies_limit_in_sql() {
        let mut conn = setup_test_db();
        let tx_a = add_record(&mut conn, "test:A", "test:Open");
        let tx_b = add_record(&mut conn, "test:B", "test:Open");
        let tx_c = add_record(&mut conn, "test:C", "test:Open");

        let first = list_records_page(&conn, query(None, None, 2)).unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.creation_tx)
                .collect::<Vec<_>>(),
            vec![tx_c, tx_b]
        );
        assert!(first.has_more);
        assert_eq!(first.next_after_tx, Some(tx_b));

        let second = list_records_page(
            &conn,
            query(Some(first.snapshot_tx), first.next_after_tx, 2),
        )
        .unwrap();
        assert_eq!(
            second.items,
            vec![RecordListItem {
                iri: "test:A".to_string(),
                creation_tx: tx_a
            }]
        );
        assert!(!second.has_more);
    }

    #[test]
    fn omitting_parent_includes_records_from_every_parent() {
        let mut conn = setup_test_db();
        add_record(&mut conn, "test:A", "test:Open");
        store::assert_triples(
            &mut conn,
            &[
                Triple::new("test:B", TYPE, Object::Iri(CLASS.to_string())),
                Triple::new("test:B", PARENT, Object::Iri("test:OtherRoot".to_string())),
                Triple::new("test:B", STATE, Object::Iri("test:Open".to_string())),
            ],
            "test",
        )
        .unwrap();

        let mut all_parents = query(None, None, 10);
        all_parents.parent_iri = None;
        let page = list_records_page(&conn, all_parents).unwrap();

        assert_eq!(page.items.len(), 2);
        assert_eq!(page.snapshot_tx, 2);
    }

    #[test]
    fn snapshot_excludes_later_records_and_latest_tx_keeps_multivalues() {
        let mut conn = setup_test_db();
        let snapshot_tx = add_record(&mut conn, "test:A", "test:Open");
        add_record(&mut conn, "test:B", "test:Open");
        store::assert_triples(
            &mut conn,
            &[
                Triple::new("test:A", STATE, Object::Iri("test:Open".to_string())),
                Triple::new("test:A", STATE, Object::Iri("test:Review".to_string())),
            ],
            "test",
        )
        .unwrap();

        let current = list_records_page(&conn, query(None, None, 10)).unwrap();
        assert!(current.state_groups.contains(&RecordStateGroup {
            state_iri: Some("test:Open".to_string()),
            count: 2,
        }));
        assert!(current.state_groups.contains(&RecordStateGroup {
            state_iri: Some("test:Review".to_string()),
            count: 1,
        }));

        let frozen = list_records_page(&conn, query(Some(snapshot_tx), None, 10)).unwrap();
        assert_eq!(
            frozen.items,
            vec![RecordListItem {
                iri: "test:A".to_string(),
                creation_tx: snapshot_tx
            }]
        );
        assert_eq!(frozen.state_groups.len(), 1);
    }

    #[test]
    fn validates_limit_identifiers_and_cursor() {
        let conn = setup_test_db();
        assert!(list_records_page(&conn, query(None, None, 0)).is_err());

        let mut empty = query(None, None, 1);
        empty.class_iri = "";
        assert!(list_records_page(&conn, empty).is_err());

        let invalid_cursor = query(Some(1), Some(2), 1);
        assert!(list_records_page(&conn, invalid_cursor).is_err());
    }
}
