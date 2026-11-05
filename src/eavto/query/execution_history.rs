use rusqlite::Connection;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone)]
pub struct DateTimePageItem {
    pub subject: String,
    pub key_value: String,
    pub key_julian_day: f64,
    pub status_iri: Option<String>,
    pub end_value: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DateTimePageStats {
    pub total_count: i64,
    pub terminal_count: i64,
    pub failed_count: i64,
    pub min_duration_ms: Option<f64>,
    pub average_duration_ms: Option<f64>,
    pub max_duration_ms: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct DateTimePage {
    pub items: Vec<DateTimePageItem>,
    pub stats: DateTimePageStats,
}

const SUBJECT_MEMBERSHIP_SQL: &str = "
    SELECT DISTINCT t.subject AS subject
    FROM triples t
    WHERE t.predicate = ?1 AND t.object = ?2 AND t.object_type = 'iri' AND t.tx <= ?3
      AND t.tx = (SELECT MAX(t2.tx) FROM triples t2
                  WHERE t2.subject=t.subject AND t2.predicate=?1 AND t2.tx<=?3)
      AND t.retracted=0";

const OBJECT_MEMBERSHIP_SQL: &str = "
    SELECT DISTINCT t.object AS subject
    FROM triples t
    WHERE t.subject = ?1 AND t.predicate = ?2 AND t.object_type = 'iri' AND t.tx <= ?3
      AND t.tx = (SELECT MAX(t2.tx) FROM triples t2
                  WHERE t2.subject=?1 AND t2.predicate=?2 AND t2.tx<=?3)
      AND t.retracted=0";

/// Reads a stable page of individuals linked through a relation and keyed by an
/// xsd:dateTime literal, while freezing membership, filter, order and status as-of
/// `snapshot_tx`. The keyset cursor is `(julian_day, subject)`.
#[allow(clippy::too_many_arguments)]
pub fn page_relation_by_datetime_as_of(
    conn: &Connection,
    membership_predicate: &str,
    membership_object: &str,
    snapshot_tx: i64,
    datetime_predicate: &str,
    range_start: Option<&str>,
    range_end: Option<&str>,
    ascending: bool,
    after: Option<(f64, &str)>,
    limit: i64,
    status_predicate: &str,
    end_predicate: &str,
    completed_status: &str,
    failed_status: &str,
) -> Result<DateTimePage> {
    page_datetime_by_membership(
        conn,
        SUBJECT_MEMBERSHIP_SQL,
        membership_predicate,
        membership_object,
        snapshot_tx,
        datetime_predicate,
        range_start,
        range_end,
        ascending,
        after,
        limit,
        status_predicate,
        end_predicate,
        completed_status,
        failed_status,
    )
}

/// Reads a stable page of objects referenced by the owner through a relation,
/// keyed by an xsd:dateTime literal, with all values frozen as-of `snapshot_tx`.
/// The keyset cursor is `(julian_day, object_iri)`.
#[allow(clippy::too_many_arguments)]
pub fn page_related_objects_by_datetime_as_of(
    conn: &Connection,
    owner_subject: &str,
    relation_predicate: &str,
    snapshot_tx: i64,
    datetime_predicate: &str,
    range_start: Option<&str>,
    range_end: Option<&str>,
    ascending: bool,
    after: Option<(f64, &str)>,
    limit: i64,
    status_predicate: &str,
    end_predicate: &str,
    completed_status: &str,
    failed_status: &str,
) -> Result<DateTimePage> {
    page_datetime_by_membership(
        conn,
        OBJECT_MEMBERSHIP_SQL,
        owner_subject,
        relation_predicate,
        snapshot_tx,
        datetime_predicate,
        range_start,
        range_end,
        ascending,
        after,
        limit,
        status_predicate,
        end_predicate,
        completed_status,
        failed_status,
    )
}

#[allow(clippy::too_many_arguments)]
fn page_datetime_by_membership(
    conn: &Connection,
    members_sql: &str,
    membership_first: &str,
    membership_second: &str,
    snapshot_tx: i64,
    datetime_predicate: &str,
    range_start: Option<&str>,
    range_end: Option<&str>,
    ascending: bool,
    after: Option<(f64, &str)>,
    limit: i64,
    status_predicate: &str,
    end_predicate: &str,
    completed_status: &str,
    failed_status: &str,
) -> Result<DateTimePage> {
    let direction = if ascending { "ASC" } else { "DESC" };
    let cursor_clause = match after {
        Some(_) if ascending => "AND (r.jd > ?8 OR (r.jd = ?8 AND r.subject > ?9)",
        Some(_) => "AND (r.jd < ?8 OR (r.jd = ?8 AND r.subject > ?9)",
        None => "",
    };
    let cursor_close = if after.is_some() { ")" } else { "" };
    let sql = format!(
        "WITH members AS ({members_sql}), keys AS (
            SELECT t.subject, t.object_value AS key_value,
                   julianday(t.object_value) AS jd
            FROM triples t
            WHERE t.predicate=?4 AND t.object_datatype='xsd:dateTime'
              AND t.tx<=?3
              AND t.tx=(SELECT MAX(t2.tx) FROM triples t2
                        WHERE t2.subject=t.subject AND t2.predicate=?4 AND t2.tx<=?3)
              AND t.retracted=0
         ), statuses AS (
            SELECT t.subject, t.object AS status_iri
            FROM triples t WHERE t.predicate=?5 AND t.tx<=?3
              AND t.tx=(SELECT MAX(t2.tx) FROM triples t2
                        WHERE t2.subject=t.subject AND t2.predicate=?5 AND t2.tx<=?3)
              AND t.retracted=0
         ), ends AS (
            SELECT t.subject, t.object_value AS end_value
            FROM triples t WHERE t.predicate=?6 AND t.object_datatype='xsd:dateTime' AND t.tx<=?3
              AND t.tx=(SELECT MAX(t2.tx) FROM triples t2
                        WHERE t2.subject=t.subject AND t2.predicate=?6 AND t2.tx<=?3)
              AND t.retracted=0
         ), records AS (
            SELECT m.subject,k.key_value,k.jd,s.status_iri,e.end_value
            FROM members m JOIN keys k ON k.subject=m.subject
            LEFT JOIN statuses s ON s.subject=m.subject
            LEFT JOIN ends e ON e.subject=m.subject
            WHERE (?7 IS NULL OR k.jd >= julianday(?7))
              AND (?10 IS NULL OR k.jd < julianday(?10))
         ), aggregate AS (
            SELECT COUNT(*) AS total_count,
                   SUM(CASE WHEN status_iri IN (?11,?12) THEN 1 ELSE 0 END) AS terminal_count,
                   SUM(CASE WHEN status_iri=?12 THEN 1 ELSE 0 END) AS failed_count,
                   MIN(CASE WHEN status_iri IN (?11,?12) AND end_value IS NOT NULL
                            THEN (julianday(end_value)-jd)*86400000.0 END) AS min_duration_ms,
                   AVG(CASE WHEN status_iri IN (?11,?12) AND end_value IS NOT NULL
                            THEN (julianday(end_value)-jd)*86400000.0 END) AS average_duration_ms,
                   MAX(CASE WHEN status_iri IN (?11,?12) AND end_value IS NOT NULL
                            THEN (julianday(end_value)-jd)*86400000.0 END) AS max_duration_ms
            FROM records
         )
         SELECT r.subject,r.key_value,r.jd,r.status_iri,r.end_value,
                a.total_count,a.terminal_count,a.failed_count,
                a.min_duration_ms,a.average_duration_ms,a.max_duration_ms
         FROM records r CROSS JOIN aggregate a
         WHERE 1=1 {cursor_clause}{cursor_close}
         ORDER BY r.jd {direction}, r.subject ASC
         LIMIT ?13"
    );

    let (after_jd, after_subject) = after
        .map(|(jd, subject)| (Some(jd), Some(subject)))
        .unwrap_or((None, None));
    // Bind the cursor placeholders only when a cursor is present; parameters 8/9
    // are still reserved by the query and are NULL on the first page.
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params![
        membership_first,
        membership_second,
        snapshot_tx,
        datetime_predicate,
        status_predicate,
        end_predicate,
        range_start,
        after_jd,
        after_subject,
        range_end,
        completed_status,
        failed_status,
        limit
    ])?;
    let mut page = DateTimePage {
        items: Vec::new(),
        stats: DateTimePageStats::default(),
    };
    while let Some(row) = rows.next()? {
        if page.items.is_empty() {
            page.stats.total_count = row.get::<_, Option<i64>>(5)?.unwrap_or(0);
            page.stats.terminal_count = row.get::<_, Option<i64>>(6)?.unwrap_or(0);
            page.stats.failed_count = row.get::<_, Option<i64>>(7)?.unwrap_or(0);
            page.stats.min_duration_ms = row.get(8)?;
            page.stats.average_duration_ms = row.get(9)?;
            page.stats.max_duration_ms = row.get(10)?;
        }
        page.items.push(DateTimePageItem {
            subject: row.get(0)?,
            key_value: row.get(1)?,
            key_julian_day: row.get(2)?,
            status_iri: row.get(3)?,
            end_value: row.get(4)?,
        });
    }
    // Empty pages still need aggregate metadata.
    if page.items.is_empty() {
        let aggregate_sql = format!(
            "WITH members AS ({members_sql}), keys AS (
                SELECT t.subject,t.object_value AS key_value,julianday(t.object_value) AS jd FROM triples t
                WHERE t.predicate=?4 AND t.object_datatype='xsd:dateTime' AND t.tx<=?3
                  AND t.tx=(SELECT MAX(t2.tx) FROM triples t2 WHERE t2.subject=t.subject AND t2.predicate=?4 AND t2.tx<=?3)
                  AND t.retracted=0
             ), statuses AS (
                SELECT t.subject,t.object AS status_iri FROM triples t WHERE t.predicate=?5 AND t.tx<=?3
                  AND t.tx=(SELECT MAX(t2.tx) FROM triples t2 WHERE t2.subject=t.subject AND t2.predicate=?5 AND t2.tx<=?3)
                  AND t.retracted=0
             ), ends AS (
                SELECT t.subject,t.object_value AS end_value FROM triples t WHERE t.predicate=?6 AND t.tx<=?3
                  AND t.tx=(SELECT MAX(t2.tx) FROM triples t2 WHERE t2.subject=t.subject AND t2.predicate=?6 AND t2.tx<=?3)
                  AND t.retracted=0
             ), records AS (
                SELECT m.subject,k.jd,s.status_iri,e.end_value FROM members m JOIN keys k ON k.subject=m.subject
                LEFT JOIN statuses s ON s.subject=m.subject LEFT JOIN ends e ON e.subject=m.subject
                WHERE (?7 IS NULL OR k.jd>=julianday(?7)) AND (?8 IS NULL OR k.jd<julianday(?8))
             )
             SELECT COUNT(*),SUM(CASE WHEN status_iri IN (?9,?10) THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status_iri=?10 THEN 1 ELSE 0 END),
                    MIN(CASE WHEN status_iri IN (?9,?10) AND end_value IS NOT NULL THEN (julianday(end_value)-jd)*86400000.0 END),
                    AVG(CASE WHEN status_iri IN (?9,?10) AND end_value IS NOT NULL THEN (julianday(end_value)-jd)*86400000.0 END),
                    MAX(CASE WHEN status_iri IN (?9,?10) AND end_value IS NOT NULL THEN (julianday(end_value)-jd)*86400000.0 END)
             FROM records"
        );
        page.stats = conn.query_row(
            &aggregate_sql,
            rusqlite::params![
                membership_first,
                membership_second,
                snapshot_tx,
                datetime_predicate,
                status_predicate,
                end_predicate,
                range_start,
                range_end,
                completed_status,
                failed_status
            ],
            |row| {
                Ok(DateTimePageStats {
                    total_count: row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    terminal_count: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    failed_count: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    min_duration_ms: row.get(3)?,
                    average_duration_ms: row.get(4)?,
                    max_duration_ms: row.get(5)?,
                })
            },
        )?;
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eavto::test_helpers::setup_test_db;

    fn tx(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO transactions (origin, created_at) VALUES ('test', 0)",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn iri(conn: &Connection, subject: &str, predicate: &str, value: &str, tx: i64) {
        conn.execute(
            "INSERT INTO triples (subject,predicate,object,object_type,tx,origin_id,retracted,created_at,is_current)
             VALUES (?1,?2,?3,'iri',?4,1,0,0,1)",
            rusqlite::params![subject,predicate,value,tx],
        ).unwrap();
    }

    fn date(conn: &Connection, subject: &str, predicate: &str, value: &str, tx: i64) {
        conn.execute(
            "INSERT INTO triples (subject,predicate,object_value,object_datatype,object_type,tx,origin_id,retracted,created_at,is_current)
             VALUES (?1,?2,?3,'xsd:dateTime','literal',?4,1,0,0,1)",
            rusqlite::params![subject,predicate,value,tx],
        ).unwrap();
    }

    #[test]
    fn date_keyset_is_stable_and_reports_terminal_kpis() {
        let conn = setup_test_db();
        let t1 = tx(&conn);
        iri(&conn, "ex:a", "ex:for", "ex:process", t1);
        iri(&conn, "ex:b", "ex:for", "ex:process", t1);
        date(&conn, "ex:a", "ex:start", "2026-05-01T10:00:00+00:00", t1);
        date(&conn, "ex:b", "ex:start", "2026-05-02T10:00:00+00:00", t1);
        date(&conn, "ex:a", "ex:end", "2026-05-01T10:00:02+00:00", t1);
        date(&conn, "ex:b", "ex:end", "2026-05-02T10:00:04+00:00", t1);
        iri(&conn, "ex:a", "ex:status", "ex:complete", t1);
        iri(&conn, "ex:b", "ex:status", "ex:failed", t1);

        let first = page_relation_by_datetime_as_of(
            &conn,
            "ex:for",
            "ex:process",
            t1,
            "ex:start",
            None,
            None,
            false,
            None,
            1,
            "ex:status",
            "ex:end",
            "ex:complete",
            "ex:failed",
        )
        .unwrap();
        assert_eq!(first.items[0].subject, "ex:b");
        assert_eq!(first.stats.total_count, 2);
        assert_eq!(first.stats.terminal_count, 2);
        assert_eq!(first.stats.failed_count, 1);
        assert!((first.stats.average_duration_ms.unwrap() - 3000.0).abs() < 1.0);

        let t2 = tx(&conn);
        iri(&conn, "ex:c", "ex:for", "ex:process", t2);
        date(&conn, "ex:a", "ex:start", "2026-05-03T10:00:00+00:00", t2);
        let next = page_relation_by_datetime_as_of(
            &conn,
            "ex:for",
            "ex:process",
            t1,
            "ex:start",
            None,
            None,
            false,
            Some((first.items[0].key_julian_day, "ex:b")),
            1,
            "ex:status",
            "ex:end",
            "ex:complete",
            "ex:failed",
        )
        .unwrap();
        assert_eq!(next.items[0].subject, "ex:a");
        assert_eq!(next.stats.total_count, 2);
    }

    #[test]
    fn owner_relation_pages_use_latest_set_and_freeze_snapshot() {
        let conn = setup_test_db();
        let t1 = tx(&conn);
        iri(&conn, "ex:owner", "ex:children", "ex:a", t1);
        iri(&conn, "ex:owner", "ex:children", "ex:b", t1);
        date(&conn, "ex:a", "ex:started", "2026-05-01T10:00:00+00:00", t1);
        date(&conn, "ex:b", "ex:started", "2026-05-01T10:00:00+00:00", t1);

        let first = page_related_objects_by_datetime_as_of(
            &conn,
            "ex:owner",
            "ex:children",
            t1,
            "ex:started",
            None,
            None,
            true,
            None,
            1,
            "ex:status",
            "ex:ended",
            "ex:complete",
            "ex:failed",
        )
        .unwrap();
        assert_eq!(first.stats.total_count, 2);
        assert_eq!(first.items[0].subject, "ex:a");

        let t2 = tx(&conn);
        iri(&conn, "ex:owner", "ex:children", "ex:c", t2);
        date(&conn, "ex:c", "ex:started", "2026-05-02T10:00:00+00:00", t2);

        let continuation = page_related_objects_by_datetime_as_of(
            &conn,
            "ex:owner",
            "ex:children",
            t1,
            "ex:started",
            None,
            None,
            true,
            Some((first.items[0].key_julian_day, "ex:a")),
            1,
            "ex:status",
            "ex:ended",
            "ex:complete",
            "ex:failed",
        )
        .unwrap();
        assert_eq!(continuation.stats.total_count, 2);
        assert_eq!(continuation.items[0].subject, "ex:b");

        let replaced = page_related_objects_by_datetime_as_of(
            &conn,
            "ex:owner",
            "ex:children",
            t2,
            "ex:started",
            None,
            None,
            true,
            None,
            10,
            "ex:status",
            "ex:ended",
            "ex:complete",
            "ex:failed",
        )
        .unwrap();
        assert_eq!(replaced.stats.total_count, 1);
        assert_eq!(replaced.items[0].subject, "ex:c");

        let empty = page_related_objects_by_datetime_as_of(
            &conn,
            "ex:owner",
            "ex:children",
            t2,
            "ex:started",
            None,
            None,
            true,
            Some((replaced.items[0].key_julian_day, "ex:c")),
            10,
            "ex:status",
            "ex:ended",
            "ex:complete",
            "ex:failed",
        )
        .unwrap();
        assert!(empty.items.is_empty());
        assert_eq!(empty.stats.total_count, 1);
    }

    #[test]
    fn workflow_execution_child_relation_returns_all_three_steps() {
        let conn = setup_test_db();
        let snapshot_tx = tx(&conn);
        let owner = "foundation:WorkflowExecution_1793662549698";
        let steps = [
            (
                "foundation:StepExecution_1793662549807",
                "2026-11-02T23:35:49.807+00:00",
            ),
            (
                "foundation:StepExecution_1793662551635",
                "2026-11-02T23:35:51.635+00:00",
            ),
            (
                "foundation:StepExecution_1793662563916",
                "2026-11-02T23:36:03.916+00:00",
            ),
        ];
        for (step_iri, started_at) in steps {
            iri(
                &conn,
                step_iri,
                "foundation:belongsToExecution",
                owner,
                snapshot_tx,
            );
            date(
                &conn,
                step_iri,
                "foundation:stepStartedAt",
                started_at,
                snapshot_tx,
            );
        }

        let page = page_relation_by_datetime_as_of(
            &conn,
            "foundation:belongsToExecution",
            owner,
            snapshot_tx,
            "foundation:stepStartedAt",
            None,
            None,
            true,
            None,
            25,
            "foundation:hasStatus",
            "foundation:stepFinishedAt",
            "foundation:Completed",
            "foundation:Status_1772993026091",
        )
        .unwrap();
        assert_eq!(page.stats.total_count, 3);
        assert_eq!(page.items.len(), 3);
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.subject.as_str())
                .collect::<Vec<_>>(),
            steps.map(|(step_iri, _)| step_iri)
        );
    }
}
