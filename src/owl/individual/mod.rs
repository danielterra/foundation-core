use crate::eavto::Connection;
use crate::eavto::{query, store, Object, Triple};
use crate::owl::{
    vocabulary::{rdf, rdfs},
    Class, OwlError, Result, Thing,
};

mod core;
mod find;
mod lock;
mod materialize;
mod models;
mod properties;
mod records;
pub mod status;
mod timestamps;
mod validation;
mod write;

pub use materialize::{materialize_individual_shallow, ShallowValue};
pub use timestamps::{touch, LAST_UPDATED_AT};

pub use lock::{check_system_locked, is_system_locked, set_system_locked};
pub use models::{list_ai_models_as_of, AiModelRow};
pub use properties::{
    find_entities_with_predicate, find_entities_with_property, find_entities_with_property_bounded,
    find_entities_with_property_keyset, get_all_current_triples, get_all_iri_properties,
    get_all_literal_properties, get_all_property_values, get_iri_property, get_literal_property,
    has_property_iri, has_property_literal, is_instance_of, is_subclass_of,
    replace_all_property_iris, replace_all_property_literals,
};
pub use records::{
    list_records_page, RecordListItem, RecordListPage, RecordListQuery, RecordStateGroup,
};
pub use status::{get_entity_status_info, validate_allowed_status};

/// Represents an OWL Individual (instance of a class)
///
/// An Individual is an instance of a Class, not a Class itself.
/// It uses rdf:type to declare its class membership.
///
/// Example:
/// ```text
/// foundation:John rdf:type foundation:Person .  // John is an instance
/// foundation:Person rdf:type owl:Class .         // Person is a class
/// ```
#[derive(Debug, Clone)]
pub struct Individual {
    pub iri: String,
    pub label: Option<String>,
    pub icon: Option<String>,
    pub comment: Option<String>,
    pub types: Vec<Thing>,
    pub properties: Vec<(String, Object)>, // (property_iri, value)
    pub property_tx: Vec<i64>,             // transaction IDs parallel to properties
    pub backlinks: Vec<crate::eavto::query::BacklinkRow>,
    /// Total value count per predicate when the predicate has more values than were loaded.
    /// Predicates with ≤ FORWARD_LIMIT_PER_GROUP values are absent (no truncation occurred).
    pub forward_group_totals: std::collections::HashMap<String, usize>,
    /// For each truncated predicate: `(value_tx, object_key)` of the last loaded row,
    /// where `object_key = COALESCE(object, object_value)`.
    /// The command layer uses this to build the `property_next_cursor` for the FE.
    pub forward_value_cutoffs: std::collections::HashMap<String, (i64, String)>,
}

impl Individual {
    /// Create a new empty Individual reference (only IRI)
    pub fn new(iri: impl Into<String>) -> Self {
        Self {
            iri: iri.into(),
            label: None,
            icon: None,
            comment: None,
            types: Vec::new(),
            properties: Vec::new(),
            property_tx: Vec::new(),
            backlinks: Vec::new(),
            forward_group_totals: std::collections::HashMap::new(),
            forward_value_cutoffs: std::collections::HashMap::new(),
        }
    }
}
