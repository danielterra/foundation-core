use crate::eavto::{query, Connection};
use crate::owl::Result;

pub use query::{RecordListItem, RecordListPage, RecordListQuery, RecordStateGroup};

pub fn list_records_page(
    conn: &Connection,
    request: RecordListQuery<'_>,
) -> Result<RecordListPage> {
    query::list_records_page(conn, request).map_err(Into::into)
}
