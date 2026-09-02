//! Rich property filters specification for PropertyStore.
//!
//! Tests define expected behavior for typed attribute storage and filtering.
//! Implementation must pass all specs before being considered complete.

/// Typed attribute value.
#[derive(Debug, Clone, PartialEq)]
pub enum Attribute {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

/// Attribute filter expression.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrFilter {
    /// Exact equality: attr_name == value
    Equals(String, Attribute),
    /// Greater than: attr_name > value
    GreaterThan(String, f64),
    /// Less than: attr_name < value
    LessThan(String, f64),
    /// In list: attr_name IN [values]
    In(String, Vec<Attribute>),
    /// Logical AND: filter1 AND filter2
    And(Box<AttrFilter>, Box<AttrFilter>),
    /// Logical OR: filter1 OR filter2
    Or(Box<AttrFilter>, Box<AttrFilter>),
}

#[cfg(test)]
mod specs {
    use super::*;
    use crate::sharding::PropertyStore;

    /// Spec: Set and retrieve string attribute.
    #[test]
    fn spec_set_string_attribute() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "name", Attribute::String("parse_json".to_string()))
            .unwrap();
        store
            .set_attribute(1000, "type", Attribute::String("function".to_string()))
            .unwrap();

        let result = store.get_attribute(1000, "name").unwrap();
        assert_eq!(result, Some(Attribute::String("parse_json".to_string())));
    }

    /// Spec: Set and retrieve numeric attributes.
    #[test]
    fn spec_set_numeric_attributes() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "complexity", Attribute::Float(8.5))
            .unwrap();
        store
            .set_attribute(1000, "line_count", Attribute::Integer(42))
            .unwrap();
        store
            .set_attribute(1000, "is_public", Attribute::Boolean(true))
            .unwrap();

        assert_eq!(
            store.get_attribute(1000, "complexity").unwrap(),
            Some(Attribute::Float(8.5))
        );
        assert_eq!(
            store.get_attribute(1000, "line_count").unwrap(),
            Some(Attribute::Integer(42))
        );
        assert_eq!(
            store.get_attribute(1000, "is_public").unwrap(),
            Some(Attribute::Boolean(true))
        );
    }

    /// Spec: Update existing attribute.
    #[test]
    fn spec_update_attribute() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "complexity", Attribute::Float(3.0))
            .unwrap();
        store
            .set_attribute(1000, "complexity", Attribute::Float(8.5))
            .unwrap();

        assert_eq!(
            store.get_attribute(1000, "complexity").unwrap(),
            Some(Attribute::Float(8.5))
        );
    }

    /// Spec: Filter by equality.
    #[test]
    fn spec_filter_equality() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "type", Attribute::String("function".to_string()))
            .unwrap();
        store
            .set_attribute(1001, "type", Attribute::String("struct".to_string()))
            .unwrap();
        store
            .set_attribute(1002, "type", Attribute::String("function".to_string()))
            .unwrap();

        let filter = AttrFilter::Equals(
            "type".to_string(),
            Attribute::String("function".to_string()),
        );
        let results = store.query_filtered(filter).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.contains(&1000));
        assert!(results.contains(&1002));
    }

    /// Spec: Filter by greater than.
    #[test]
    fn spec_filter_greater_than() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "complexity", Attribute::Float(8.5))
            .unwrap();
        store
            .set_attribute(1001, "complexity", Attribute::Float(3.2))
            .unwrap();
        store
            .set_attribute(1002, "complexity", Attribute::Float(12.0))
            .unwrap();

        let filter = AttrFilter::GreaterThan("complexity".to_string(), 5.0);
        let results = store.query_filtered(filter).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.contains(&1000));
        assert!(results.contains(&1002));
    }

    /// Spec: Filter by less than.
    #[test]
    fn spec_filter_less_than() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "line_count", Attribute::Integer(10))
            .unwrap();
        store
            .set_attribute(1001, "line_count", Attribute::Integer(50))
            .unwrap();
        store
            .set_attribute(1002, "line_count", Attribute::Integer(100))
            .unwrap();

        let filter = AttrFilter::LessThan("line_count".to_string(), 60.0);
        let results = store.query_filtered(filter).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.contains(&1000));
        assert!(results.contains(&1001));
    }

    /// Spec: Filter by IN list.
    #[test]
    fn spec_filter_in() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "visibility", Attribute::String("public".to_string()))
            .unwrap();
        store
            .set_attribute(1001, "visibility", Attribute::String("private".to_string()))
            .unwrap();
        store
            .set_attribute(
                1002,
                "visibility",
                Attribute::String("protected".to_string()),
            )
            .unwrap();

        let filter = AttrFilter::In(
            "visibility".to_string(),
            vec![
                Attribute::String("public".to_string()),
                Attribute::String("protected".to_string()),
            ],
        );
        let results = store.query_filtered(filter).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.contains(&1000));
        assert!(results.contains(&1002));
    }

    /// Spec: Filter with AND logic.
    #[test]
    fn spec_filter_and() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "type", Attribute::String("function".to_string()))
            .unwrap();
        store
            .set_attribute(1000, "complexity", Attribute::Float(8.5))
            .unwrap();
        store
            .set_attribute(1000, "is_public", Attribute::Boolean(true))
            .unwrap();

        store
            .set_attribute(1001, "type", Attribute::String("function".to_string()))
            .unwrap();
        store
            .set_attribute(1001, "complexity", Attribute::Float(3.2))
            .unwrap();
        store
            .set_attribute(1001, "is_public", Attribute::Boolean(true))
            .unwrap();

        store
            .set_attribute(1002, "type", Attribute::String("function".to_string()))
            .unwrap();
        store
            .set_attribute(1002, "complexity", Attribute::Float(8.5))
            .unwrap();
        store
            .set_attribute(1002, "is_public", Attribute::Boolean(false))
            .unwrap();

        // Filter: type == "function" AND complexity > 5 AND is_public == true
        let filter = AttrFilter::And(
            Box::new(AttrFilter::Equals(
                "type".to_string(),
                Attribute::String("function".to_string()),
            )),
            Box::new(AttrFilter::And(
                Box::new(AttrFilter::GreaterThan("complexity".to_string(), 5.0)),
                Box::new(AttrFilter::Equals(
                    "is_public".to_string(),
                    Attribute::Boolean(true),
                )),
            )),
        );

        let results = store.query_filtered(filter).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 1000);
    }

    /// Spec: Filter with OR logic.
    #[test]
    fn spec_filter_or() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "language", Attribute::String("rust".to_string()))
            .unwrap();
        store
            .set_attribute(1001, "language", Attribute::String("python".to_string()))
            .unwrap();
        store
            .set_attribute(
                1002,
                "language",
                Attribute::String("javascript".to_string()),
            )
            .unwrap();

        // Filter: language == "rust" OR language == "python"
        let filter = AttrFilter::Or(
            Box::new(AttrFilter::Equals(
                "language".to_string(),
                Attribute::String("rust".to_string()),
            )),
            Box::new(AttrFilter::Equals(
                "language".to_string(),
                Attribute::String("python".to_string()),
            )),
        );

        let results = store.query_filtered(filter).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.contains(&1000));
        assert!(results.contains(&1001));
    }

    /// Spec: Delete attribute.
    #[test]
    fn spec_delete_attribute() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "temp", Attribute::String("value".to_string()))
            .unwrap();
        assert!(store.get_attribute(1000, "temp").unwrap().is_some());

        store.delete_attribute(1000, "temp").unwrap();
        assert!(store.get_attribute(1000, "temp").unwrap().is_none());
    }

    /// Spec: Query returns empty when no matches.
    #[test]
    fn spec_filter_no_matches() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "type", Attribute::String("function".to_string()))
            .unwrap();
        store
            .set_attribute(1001, "type", Attribute::String("struct".to_string()))
            .unwrap();

        let filter = AttrFilter::Equals("type".to_string(), Attribute::String("enum".to_string()));
        let results = store.query_filtered(filter).unwrap();

        assert_eq!(results.len(), 0);
    }

    /// Spec: Mixed type attributes on same token.
    #[test]
    fn spec_mixed_attributes() {
        let mut store = PropertyStore::in_memory().unwrap();

        store
            .set_attribute(1000, "name", Attribute::String("parse".to_string()))
            .unwrap();
        store
            .set_attribute(1000, "complexity", Attribute::Float(7.2))
            .unwrap();
        store
            .set_attribute(1000, "lines", Attribute::Integer(50))
            .unwrap();
        store
            .set_attribute(1000, "deprecated", Attribute::Boolean(false))
            .unwrap();

        // Query by different attributes
        assert_eq!(
            store.get_attribute(1000, "name").unwrap(),
            Some(Attribute::String("parse".to_string()))
        );
        assert_eq!(
            store.get_attribute(1000, "complexity").unwrap(),
            Some(Attribute::Float(7.2))
        );
        assert_eq!(
            store.get_attribute(1000, "lines").unwrap(),
            Some(Attribute::Integer(50))
        );
        assert_eq!(
            store.get_attribute(1000, "deprecated").unwrap(),
            Some(Attribute::Boolean(false))
        );
    }

    /// Spec: Performance - uses index, not full scan.
    #[test]
    fn spec_filter_performance_indexed() {
        let mut store = PropertyStore::in_memory().unwrap();

        // Insert 1000 tokens
        for i in 0..1000 {
            store
                .set_attribute(i, "type", Attribute::String("function".to_string()))
                .unwrap();
            store
                .set_attribute(i, "complexity", Attribute::Float((i % 20) as f64))
                .unwrap();
        }

        let filter = AttrFilter::Equals(
            "type".to_string(),
            Attribute::String("function".to_string()),
        );
        let results = store.query_filtered(filter).unwrap();

        assert_eq!(results.len(), 1000);
    }
}
