use crate::pattern_engine::PatternTriple;

/// Determine if a pattern can use the fast-path (Case 1)
///
/// Fast-path conditions:
/// - edge_type = Some("X")
/// - NO start_label
/// - NO end_label  
/// - NO property filters
pub fn can_use_fast_path(pattern: &PatternTriple) -> bool {
    pattern.start_label.is_none()
        && pattern.end_label.is_none()
        && pattern.start_props.is_empty()
        && pattern.end_props.is_empty()
}

/// Determine if pattern can use partial fast-path (Case 2)
///
/// Partial fast-path conditions:
/// - Has label filters OR property filters
/// - Can use cache to narrow candidates
pub fn can_use_partial_fast_path(pattern: &PatternTriple) -> bool {
    !pattern.start_props.is_empty()
        || !pattern.end_props.is_empty()
        || pattern.start_label.is_some()
        || pattern.end_label.is_some()
}
