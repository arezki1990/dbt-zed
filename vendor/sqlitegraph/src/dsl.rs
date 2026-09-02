use crate::{
    backend::BackendDirection,
    pattern::{PatternLeg, PatternQuery},
};

#[derive(Debug, Clone)]
pub enum DslResult {
    Pattern(PatternQuery),
    Error(String),
}

pub fn parse_dsl(input: &str) -> DslResult {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return DslResult::Error("empty DSL string".into());
    }
    if trimmed.contains("->") || trimmed.contains('*') {
        return DslResult::Pattern(parse_repetitive_pattern(trimmed));
    }
    DslResult::Error(format!("unsupported DSL form: {trimmed}"))
}

fn parse_repetitive_pattern(segment: &str) -> PatternQuery {
    let mut legs = Vec::new();
    for token in segment.split("->") {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (ty, count) = if let Some((name, times)) = trimmed.split_once('*') {
            let repeats = times.trim().parse::<usize>().unwrap_or(1);
            (name.trim(), repeats)
        } else {
            (trimmed, 1)
        };
        for _ in 0..count {
            legs.push(PatternLeg {
                direction: BackendDirection::Outgoing,
                edge_type: Some(ty.to_string()),
                constraint: None,
            });
        }
    }
    PatternQuery { root: None, legs }
}
