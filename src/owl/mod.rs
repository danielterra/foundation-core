pub mod aggregation;
pub mod cardinality;
mod class;
pub mod expr;
pub mod formula;
mod graph_config;
mod icons;
mod individual;
mod property;
pub mod query_property;
pub mod search;
mod thing;
pub mod vocabulary;

pub use graph_config::{get_graph_node_type_config, load_graph_node_groups, GraphNodeTypeConfig};

pub use icons::{
    icon_iri_to_display, icon_literal_to_display, icon_name_to_iri, icon_store_value,
    seed_icon_library, validate_icon,
};

pub use crate::eavto::get_stats;
pub use crate::eavto::initialize_db;
pub use crate::eavto::Connection;
pub use crate::eavto::DbExecutor;
pub use crate::eavto::Object;
pub use crate::eavto::Triple;
pub use class::{Class, ClassType};
pub use individual::Individual;
pub use property::{DomainLabel, Property, PropertyClassification, PropertyType};
pub use thing::Thing;

/// Append triples without retracting existing (subject, predicate) pairs.
/// Wraps `eavto::store::append_triples` so that `commands/` does not import `eavto/` directly.
pub fn batch_insert_triples(
    conn: &mut Connection,
    triples: &[Triple],
    origin: &str,
) -> std::result::Result<(), String> {
    crate::eavto::store::append_triples(conn, triples, origin)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Assert triples (retracts existing same-predicate values first).
/// Wraps `eavto::store::assert_triples` so that `commands/` does not import `eavto/` directly.
pub fn assert_raw_triples(
    conn: &mut Connection,
    triples: &[Triple],
    origin: &str,
) -> std::result::Result<i64, String> {
    crate::eavto::store::assert_triples(conn, triples, origin).map_err(|e| e.to_string())
}

pub use individual::{
    check_system_locked, find_entities_with_predicate, find_entities_with_property,
    find_entities_with_property_bounded, find_entities_with_property_keyset,
    get_all_current_triples, get_all_iri_properties, get_all_literal_properties,
    get_all_property_values, get_entity_status_info, get_iri_property, get_literal_property,
    has_property_iri, has_property_literal, is_instance_of, is_subclass_of, is_system_locked,
    list_ai_models_as_of, list_records_page, materialize_individual_shallow,
    replace_all_property_iris, replace_all_property_literals, set_system_locked, touch,
    validate_allowed_status, AiModelRow, RecordListItem, RecordListPage, RecordListQuery,
    RecordStateGroup, ShallowValue, LAST_UPDATED_AT,
};

#[derive(Debug)]
pub enum OwlError {
    DatabaseError(String),
    ValidationError(String),
    NotFound(String),
    InvalidOperation(String),
    CardinalityViolation(String),
}

impl std::fmt::Display for OwlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OwlError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            OwlError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            OwlError::NotFound(msg) => write!(f, "Not found: {}", msg),
            OwlError::InvalidOperation(msg) => write!(f, "Invalid operation: {}", msg),
            OwlError::CardinalityViolation(msg) => write!(f, "Cardinality violation: {}", msg),
        }
    }
}

impl std::error::Error for OwlError {}

impl From<crate::eavto::connection::DbError> for OwlError {
    fn from(err: crate::eavto::connection::DbError) -> Self {
        OwlError::DatabaseError(err.to_string())
    }
}

impl From<Box<dyn std::error::Error>> for OwlError {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        OwlError::DatabaseError(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, OwlError>;

#[cfg(test)]
#[path = "owl_tests.rs"]
mod tests;
