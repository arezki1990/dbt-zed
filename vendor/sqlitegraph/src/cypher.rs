//! Cypher-inspired query language for SQLiteGraph.
//!
//! Supports:
//! - `MATCH (n:Label)` — node pattern
//! - `MATCH (a)-[:REL]->(b)` — outgoing edge pattern
//! - `MATCH (a)<-[:REL]-(b)` — incoming edge pattern
//! - `MATCH (a)-[:REL]-(b)` — undirected edge pattern
//! - `MATCH (a)-[:X]->(b)-[:Y]->(c)` — multi-hop chain
//! - `MATCH (a)-[:X*1..3]->(b)` — variable-depth traversal
//! - `WHERE n.field = "value" AND m.field = "x"` — multi-predicate
//! - `WHERE n.field =~ "pattern"` — regex match
//! - `WHERE n.field > 5` — numeric comparison
//! - `WHERE n.f = "x" OR n.f = "y"` — disjunction
//! - `RETURN a.name, b.name` — projection
//! - `LIMIT N` — result cap (applied after filtering)
//! - `CREATE (n:Label {prop: "val"})` — insert a node
//! - `CREATE (1)-[:REL]->(2)` — insert an edge between node IDs
//! - `MATCH (n) WHERE ... SET n.prop = "val"` — update a property
//! - `MATCH (n) WHERE ... DELETE n` — remove a node

use serde_json::Value;

use crate::PatternTriple;
use crate::backend::{BackendDirection, EdgeSpec, GraphBackend, NodeSpec};
use crate::graph::GraphEntity;
use crate::multi_hop::ChainStep;
use crate::snapshot::SnapshotId;

// ── Public types ─────────────────────────────────────────────

/// Direction of an edge in a MATCH pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeDirection {
    /// `-[:REL]->`
    #[default]
    Outgoing,
    /// `<-[:REL]-`
    Incoming,
    /// `-[:REL]-` (undirected)
    Both,
}

/// Comparison operator used inside a `WHERE` clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhereOp {
    /// `=`
    #[default]
    Eq,
    /// `<>` or `!=`
    NotEq,
    /// `>`
    GreaterThan,
    /// `<`
    LessThan,
    /// `>=`
    GreaterEq,
    /// `<=`
    LessEq,
    /// `=~` (regex)
    Regex,
}

/// Top-level statement kind. `Match` is the default for read queries.
#[derive(Debug, Default)]
pub enum Statement {
    /// `MATCH ... RETURN ...` (default)
    #[default]
    Match,
    /// `CREATE (n:Label {prop: "val"})`
    CreateNode {
        var: String,
        label: Option<String>,
        props: Vec<(String, String)>,
    },
    /// `CREATE (from_id)-[:REL]->(to_id)`
    CreateEdge {
        from_id: i64,
        to_id: i64,
        rel_type: String,
    },
    /// `MATCH (n) ... SET n.field = "value"`
    Set {
        var: String,
        field: String,
        value: String,
    },
    /// `MATCH (n) ... DELETE n`
    Delete { var: String },
    /// `CALL db.index.vector.queryNodes('idx', k, [v1, v2, ...])` — HNSW
    /// k-nearest-neighbour search over an already-loaded vector index.
    CallVectorQuery {
        index_name: String,
        k: usize,
        vector: Vec<f32>,
    },
}

/// One step in a multi-hop pattern: `(from)-[:rel]->(to)`.
#[derive(Debug)]
pub struct EdgeLeg {
    pub rel_type: String,
    pub direction: EdgeDirection,
    pub from: NodePattern,
    pub to: NodePattern,
}

/// A graph pattern — node scan, edge traversal, multi-hop chain, or
/// variable-depth traversal.
#[derive(Debug)]
pub enum Pattern {
    /// No pattern (used by `CREATE (n:...)` statements).
    None,
    /// `MATCH (n:Label)` — scan nodes.
    Node(NodePattern),
    /// `MATCH (a)-[:REL]->(b)` — single-hop traversal.
    Edge(NodePattern, String, NodePattern),
    /// `MATCH (a)-[:X]->(b)-[:Y]->(c)` — chain of two or more hops.
    MultiHop(Vec<EdgeLeg>),
    /// `MATCH (a)-[:REL*min..max]->(b)` — variable-depth traversal.
    VariableDepth {
        rel_type: String,
        min_hops: usize,
        max_hops: usize,
    },
    /// `MATCH (a)-[:X]->(b), (a)-[:Y]->(c)` — star pattern where multiple
    /// legs all start from the same root variable. Each leg is an
    /// independent edge pattern; the result is the cartesian product of
    /// per-leg matches, joined on the shared root binding.
    Star { legs: Vec<EdgeLeg> },
}

/// A node pattern within a MATCH clause.
#[derive(Debug, Clone)]
pub struct NodePattern {
    pub var: String,
    pub label: Option<String>,
    pub props: Vec<(String, String)>,
}

/// A single WHERE clause predicate.
#[derive(Debug, Clone)]
pub struct WhereClause {
    pub var: String,
    pub field: String,
    pub operator: WhereOp,
    pub value: String,
}

/// A parsed Cypher-inspired query.
#[derive(Debug)]
pub struct CypherQuery {
    pub statement: Statement,
    pub pattern: Pattern,
    pub direction: EdgeDirection,
    pub returns: Vec<String>,
    /// WHERE predicates in disjunctive normal form: outer `Vec` is OR-joined,
    /// inner `Vec` is AND-joined. `WHERE a AND b OR c` parses to
    /// `vec![vec![a, b], vec![c]]`. Empty groups mean "no filter".
    pub where_groups: Vec<Vec<WhereClause>>,
    pub limit: Option<usize>,
    /// For variable-depth and multi-hop queries: the start-node constraint.
    pub start_node: Option<NodePattern>,
    /// For variable-depth and multi-hop queries: the end-node constraint.
    pub end_node: Option<NodePattern>,
}

impl Default for CypherQuery {
    fn default() -> Self {
        Self {
            statement: Statement::Match,
            pattern: Pattern::None,
            direction: EdgeDirection::Outgoing,
            returns: vec!["*".to_string()],
            where_groups: Vec::new(),
            limit: None,
            start_node: None,
            end_node: None,
        }
    }
}

// ── Parser ───────────────────────────────────────────────────

/// Parse a Cypher-inspired query string.
///
/// # Errors
///
/// Returns an error for unsupported syntax or malformed input.
pub fn parse(query: &str) -> Result<CypherQuery, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err("empty query".into());
    }
    let upper = trimmed.to_uppercase();
    if upper.starts_with("MATCH ") {
        parse_match(trimmed)
    } else if upper.starts_with("CREATE ") {
        parse_create(trimmed)
    } else if upper.starts_with("CALL ") {
        parse_call(trimmed)
    } else {
        Err("only MATCH, CREATE, and CALL queries are supported".into())
    }
}

fn parse_match(query: &str) -> Result<CypherQuery, String> {
    let rest = query[6..].trim();

    // Pull off DELETE / SET first (they come at the end of the statement).
    let (rest, statement_override) = extract_match_statement(rest);

    // Then LIMIT (after RETURN), RETURN, WHERE.
    let (rest, limit) = extract_limit(rest);
    let (pattern_str, returns) = extract_return(rest);
    let (pattern_str, where_groups) = extract_where(pattern_str)?;

    let (pattern, direction, start_node, end_node) = parse_pattern(pattern_str.trim())?;

    let statement = statement_override.unwrap_or_default();

    Ok(CypherQuery {
        statement,
        pattern,
        direction,
        returns,
        where_groups,
        limit,
        start_node,
        end_node,
    })
}

fn parse_create(query: &str) -> Result<CypherQuery, String> {
    let rest = query[7..].trim();

    // `CREATE (id1)-[:REL]->(id2)` — numeric IDs separated by an edge pattern.
    if rest.contains("-[:") && rest.contains("]->") {
        return parse_create_edge(rest);
    }

    // `CREATE (n:Label {...})` — node creation.
    parse_create_node(rest)
}

fn parse_create_node(s: &str) -> Result<CypherQuery, String> {
    let inner = s
        .trim()
        .strip_prefix('(')
        .and_then(|x| x.strip_suffix(')'))
        .ok_or_else(|| format!("CREATE node must be parenthesised: {s}"))?;
    let node = parse_node(inner)?;
    Ok(CypherQuery {
        statement: Statement::CreateNode {
            var: node.var.clone(),
            label: node.label.clone(),
            props: node.props.clone(),
        },
        pattern: Pattern::None,
        ..Default::default()
    })
}

fn parse_create_edge(s: &str) -> Result<CypherQuery, String> {
    let arrow_pos = s.find("->").expect("validated by caller");
    let left_end = s
        .find("-[")
        .ok_or_else(|| "expected -[ in CREATE edge".to_string())?;
    let rel_open = s
        .find("[:")
        .ok_or_else(|| "expected [: in CREATE edge".to_string())?;
    let rel_close = s
        .find(']')
        .ok_or_else(|| "expected ] in CREATE edge".to_string())?;

    let from_id = parse_node_id(s[..left_end].trim())?;
    let to_id = parse_node_id(s[arrow_pos + 2..].trim())?;
    let rel_type = s[rel_open + 2..rel_close].trim().to_string();
    if rel_type.is_empty() {
        return Err("relationship type cannot be empty".into());
    }

    Ok(CypherQuery {
        statement: Statement::CreateEdge {
            from_id,
            to_id,
            rel_type,
        },
        pattern: Pattern::None,
        ..Default::default()
    })
}

/// Parse `CALL db.index.vector.queryNodes('name', k, [v1, v2, ...])`.
///
/// Only one procedure is currently supported. Argument types are positional:
/// 1. string literal (single or double quotes) — index name
/// 2. integer — `k` (number of neighbours to return)
/// 3. bracketed list of f32 literals — query vector
fn parse_call(query: &str) -> Result<CypherQuery, String> {
    const PROC: &str = "db.index.vector.queryNodes";
    let rest = query[5..].trim(); // strip "CALL "

    let open = rest
        .find('(')
        .ok_or_else(|| "CALL: missing '(' after procedure name".to_string())?;
    let proc_name = rest[..open].trim();
    if proc_name != PROC {
        return Err(format!(
            "CALL: unknown procedure `{proc_name}` (only `{PROC}` is supported)"
        ));
    }
    if !rest.ends_with(')') {
        return Err("CALL: missing closing ')'".into());
    }
    let args_str = &rest[open + 1..rest.len() - 1];

    let args = split_call_args(args_str);
    if args.len() != 3 {
        return Err(format!(
            "CALL {PROC}: expected 3 arguments (index_name, k, vector), got {}",
            args.len()
        ));
    }

    let index_name = parse_string_literal(args[0].trim())
        .ok_or_else(|| format!("CALL: invalid index name `{}`", args[0].trim()))?;

    let k = args[1]
        .trim()
        .parse::<usize>()
        .map_err(|e| format!("CALL: `k` must be a non-negative integer ({e})"))?;

    let vector = parse_vector_literal(args[2].trim())?;

    Ok(CypherQuery {
        statement: Statement::CallVectorQuery {
            index_name,
            k,
            vector,
        },
        pattern: Pattern::None,
        ..Default::default()
    })
}

/// Split a call-argument list on top-level commas, the same way as
/// [`split_top_level_commas`]. Defined separately so future divergence (e.g.
/// commas inside `{...}` maps that aren't yet legal here) doesn't churn the
/// other splitter.
fn split_call_args(s: &str) -> Vec<String> {
    split_top_level_commas(s)
}

/// Parse a single-quoted or double-quoted string literal. Returns `None`
/// when the input is malformed.
fn parse_string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() < 2 {
        return None;
    }
    let first = s.chars().next()?;
    let last = s.chars().last()?;
    if (first == '"' || first == '\'') && first == last {
        Some(s[1..s.len() - 1].to_string())
    } else {
        None
    }
}

/// Parse `[v1, v2, ...]` into a `Vec<f32>`. Supports negative, decimal, and
/// scientific notation.
fn parse_vector_literal(s: &str) -> Result<Vec<f32>, String> {
    let s = s.trim();
    let inner = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .ok_or_else(|| format!("CALL: vector must be `[...]`, got `{s}`"))?;
    let mut out = Vec::new();
    for part in inner.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let v: f32 = p
            .parse()
            .map_err(|e| format!("CALL: invalid float in vector `{p}` ({e})"))?;
        out.push(v);
    }
    Ok(out)
}

fn parse_node_id(s: &str) -> Result<i64, String> {
    let inner = s
        .strip_prefix('(')
        .and_then(|x| x.strip_suffix(')'))
        .ok_or_else(|| format!("expected (id): {s}"))?;
    inner
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("expected numeric node id, got {inner:?}"))
}

/// Detect a trailing `SET ...` or `DELETE ...` clause and return the remaining
/// MATCH body along with the resulting Statement, if any.
fn extract_match_statement(input: &str) -> (&str, Option<Statement>) {
    let upper = input.to_uppercase();
    if let Some(pos) = upper.rfind(" DELETE ") {
        let var = input[pos + 8..].trim().to_string();
        return (&input[..pos], Some(Statement::Delete { var }));
    }
    if let Some(pos) = upper.rfind(" SET ") {
        let set_part = input[pos + 5..].trim();
        if let Some((var, field, value)) = parse_set_assignment(set_part) {
            return (&input[..pos], Some(Statement::Set { var, field, value }));
        }
    }
    (input, None)
}

fn parse_set_assignment(s: &str) -> Option<(String, String, String)> {
    let eq_pos = s.find('=')?;
    let left = s[..eq_pos].trim();
    let right = s[eq_pos + 1..]
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    let dot_pos = left.find('.')?;
    let var = left[..dot_pos].trim().to_string();
    let field = left[dot_pos + 1..].trim().to_string();
    Some((var, field, right))
}

fn extract_limit(input: &str) -> (&str, Option<usize>) {
    let upper = input.to_uppercase();
    if let Some(pos) = upper.rfind(" LIMIT ") {
        let limit_str = input[pos + 7..].trim();
        let limit = limit_str.parse::<usize>().ok();
        (&input[..pos], limit)
    } else {
        (input, None)
    }
}

fn extract_return(input: &str) -> (&str, Vec<String>) {
    let upper = input.to_uppercase();
    if let Some(pos) = upper.find(" RETURN ") {
        let pattern_part = &input[..pos];
        let return_part = &input[pos + 8..];
        let returns: Vec<String> = return_part
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        (pattern_part.trim(), returns)
    } else {
        (input, vec!["*".to_string()])
    }
}

fn extract_where(input: &str) -> Result<(&str, Vec<Vec<WhereClause>>), String> {
    let upper = input.to_uppercase();
    if let Some(pos) = upper.find(" WHERE ") {
        let pattern_part = &input[..pos];
        let where_part = &input[pos + 7..];
        let groups = parse_where_clauses(where_part)?;
        Ok((pattern_part.trim(), groups))
    } else {
        Ok((input, Vec::new()))
    }
}

/// Internal tree representation of a WHERE expression, used during parsing
/// before flattening to the public `where_groups` DNF form.
#[derive(Debug)]
enum WhereExpr {
    Atom(WhereClause),
    And(Vec<WhereExpr>),
    Or(Vec<WhereExpr>),
}

/// Parse a WHERE body into disjunctive normal form: outer `Vec` is OR-joined,
/// inner `Vec` is AND-joined.
///
/// Supports parenthesised sub-expressions and the standard precedence (`OR`
/// binds looser than `AND`). The recursive-descent parser builds a
/// [`WhereExpr`] tree which is then flattened to DNF.
///
/// Examples:
/// - `a` parses to `[[a]]`
/// - `a AND b` → `[[a, b]]`
/// - `a OR b` → `[[a], [b]]`
/// - `a AND b OR c` → `[[a, b], [c]]`
/// - `(a OR b) AND c` → `[[a, c], [b, c]]`
/// - `a OR (b AND c)` → `[[a], [b, c]]`
fn parse_where_clauses(input: &str) -> Result<Vec<Vec<WhereClause>>, String> {
    let mut parser = WhereParser::new(input);
    let expr = parser.parse_or()?;
    parser.skip_ws();
    if parser.pos < parser.src.len() {
        return Err(format!(
            "trailing input in WHERE: `{}`",
            &input[parser.pos..]
        ));
    }
    Ok(where_expr_to_dnf(&expr))
}

/// Recursive-descent parser for the WHERE expression grammar:
/// ```text
/// or_expr  := and_expr ('OR' and_expr)*
/// and_expr := primary ('AND' primary)*
/// primary  := '(' or_expr ')' | atom
/// atom     := <predicate-text terminated by AND/OR/')'/EOF at depth 0>
/// ```
struct WhereParser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> WhereParser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            src: s.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    /// True if the upcoming bytes match `kw` (case-insensitive) and the
    /// following byte is a word boundary (not alphanumeric or `_`).
    fn peek_keyword(&self, kw: &str) -> bool {
        let bytes = kw.as_bytes();
        if self.pos + bytes.len() > self.src.len() {
            return false;
        }
        for (i, &b) in bytes.iter().enumerate() {
            if !self.src[self.pos + i].eq_ignore_ascii_case(&b) {
                return false;
            }
        }
        if self.pos + bytes.len() < self.src.len() {
            let nxt = self.src[self.pos + bytes.len()];
            if nxt.is_ascii_alphanumeric() || nxt == b'_' {
                return false;
            }
        }
        true
    }

    fn consume_keyword(&mut self, kw: &str) -> bool {
        self.skip_ws();
        if self.peek_keyword(kw) {
            self.pos += kw.len();
            true
        } else {
            false
        }
    }

    fn parse_or(&mut self) -> Result<WhereExpr, String> {
        let mut terms = vec![self.parse_and()?];
        loop {
            self.skip_ws();
            if !self.consume_keyword("OR") {
                break;
            }
            terms.push(self.parse_and()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().expect("non-empty")
        } else {
            WhereExpr::Or(terms)
        })
    }

    fn parse_and(&mut self) -> Result<WhereExpr, String> {
        let mut terms = vec![self.parse_primary()?];
        loop {
            self.skip_ws();
            if !self.consume_keyword("AND") {
                break;
            }
            terms.push(self.parse_primary()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().expect("non-empty")
        } else {
            WhereExpr::And(terms)
        })
    }

    fn parse_primary(&mut self) -> Result<WhereExpr, String> {
        self.skip_ws();
        if self.pos < self.src.len() && self.src[self.pos] == b'(' {
            self.pos += 1;
            let inner = self.parse_or()?;
            self.skip_ws();
            if self.pos >= self.src.len() || self.src[self.pos] != b')' {
                return Err("expected closing ')' in WHERE".into());
            }
            self.pos += 1;
            return Ok(inner);
        }

        // Atom: scan until next AND/OR keyword at depth 0, a closing ')',
        // or end-of-input. Skip over string literals so quoted AND/OR is
        // treated as data, not a keyword.
        let start = self.pos;
        let mut in_str = false;
        let mut str_quote: u8 = b'"';
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if in_str {
                if c == str_quote {
                    in_str = false;
                }
                self.pos += 1;
                continue;
            }
            if c == b'"' || c == b'\'' {
                in_str = true;
                str_quote = c;
                self.pos += 1;
                continue;
            }
            if c == b')' {
                break;
            }
            if c.is_ascii_whitespace() {
                let save = self.pos;
                self.skip_ws();
                if self.peek_keyword("AND") || self.peek_keyword("OR") {
                    self.pos = save;
                    break;
                }
                continue;
            }
            self.pos += 1;
        }
        let atom_bytes = &self.src[start..self.pos];
        let atom_str = std::str::from_utf8(atom_bytes)
            .map_err(|e| format!("WHERE atom not utf8: {e}"))?
            .trim();
        if atom_str.is_empty() {
            return Err("empty predicate in WHERE".into());
        }
        let clause = parse_single_predicate(atom_str)
            .ok_or_else(|| format!("invalid predicate in WHERE: `{atom_str}`"))?;
        Ok(WhereExpr::Atom(clause))
    }
}

/// Flatten a [`WhereExpr`] tree to disjunctive normal form.
///
/// Note: for an AND of N OR-groups each of size K, the resulting DNF has
/// K^N rows. For realistic Cypher WHERE clauses (≤ 5 predicates) this is
/// fine; pathological inputs are bounded by query parser limits upstream.
fn where_expr_to_dnf(expr: &WhereExpr) -> Vec<Vec<WhereClause>> {
    match expr {
        WhereExpr::Atom(c) => vec![vec![c.clone()]],
        WhereExpr::Or(terms) => {
            let mut groups = Vec::new();
            for t in terms {
                groups.extend(where_expr_to_dnf(t));
            }
            groups
        }
        WhereExpr::And(terms) => {
            let mut acc: Vec<Vec<WhereClause>> = vec![vec![]];
            for t in terms {
                let dnf = where_expr_to_dnf(t);
                let mut next: Vec<Vec<WhereClause>> = Vec::new();
                for existing in &acc {
                    for grp in &dnf {
                        let mut combined = existing.clone();
                        combined.extend(grp.clone());
                        next.push(combined);
                    }
                }
                acc = next;
            }
            acc
        }
    }
}

fn parse_single_predicate(part: &str) -> Option<WhereClause> {
    let (op, op_len) = detect_where_op(part)?;
    let op_pos = match op {
        WhereOp::Regex => part.find("=~")?,
        WhereOp::NotEq if part.contains("<>") => part.find("<>")?,
        WhereOp::NotEq => part.find("!=")?,
        WhereOp::GreaterEq => part.find(">=")?,
        WhereOp::LessEq => part.find("<=")?,
        WhereOp::GreaterThan => part.find('>')?,
        WhereOp::LessThan => part.find('<')?,
        WhereOp::Eq => part.find('=')?,
    };
    let left = part[..op_pos].trim();
    let right = part[op_pos + op_len..]
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    let dot_pos = left.find('.')?;
    let var = left[..dot_pos].trim().to_string();
    let field = left[dot_pos + 1..].trim().to_string();
    Some(WhereClause {
        var,
        field,
        operator: op,
        value: right,
    })
}

/// Detect the longest WHERE operator at any position. Returns (op, op_len).
fn detect_where_op(part: &str) -> Option<(WhereOp, usize)> {
    if part.contains("=~") {
        Some((WhereOp::Regex, 2))
    } else if part.contains("<>") || part.contains("!=") {
        Some((WhereOp::NotEq, 2))
    } else if part.contains(">=") {
        Some((WhereOp::GreaterEq, 2))
    } else if part.contains("<=") {
        Some((WhereOp::LessEq, 2))
    } else if part.contains('>') {
        Some((WhereOp::GreaterThan, 1))
    } else if part.contains('<') {
        Some((WhereOp::LessThan, 1))
    } else if part.contains('=') {
        Some((WhereOp::Eq, 1))
    } else {
        None
    }
}

/// Parsed pattern + the bookkeeping the executor needs to bind start/end vars.
type ParsedPattern = (
    Pattern,
    EdgeDirection,
    Option<NodePattern>,
    Option<NodePattern>,
);

/// Returns (pattern, direction, start_node, end_node).
fn parse_pattern(s: &str) -> Result<ParsedPattern, String> {
    let s = s.trim();

    // Star pattern: comma-separated edge patterns sharing a root var.
    // `(a)-[:X]->(b), (a)-[:Y]->(c)`. Star must be tried before edge/multi-hop
    // because a chain like `(a)-[:X]->(b)-[:Y]->(c)` has no top-level commas.
    if has_top_level_comma(s) {
        return parse_star_pattern(s);
    }

    // Variable-depth: (a)-[:REL*min..max]->(b)
    if s.contains("-[:")
        && (s.contains("*") || s.contains("]*"))
        && let Some(var_depth) = try_parse_variable_depth(s)?
    {
        return Ok(var_depth);
    }

    // Multi-hop: (a)-[:X]->(b)-[:Y]->(c)  (two or more arrows)
    let arrow_count = s.matches("->").count() + s.matches("-[").count() / 2;
    let arrow_segments = count_edge_segments(s);
    if arrow_segments >= 2 {
        return parse_multi_hop(s);
    }
    let _ = arrow_count; // suppress unused if unreachable above

    // Edge pattern (forward, backward, or undirected)
    if s.contains("-[:") {
        return parse_edge_pattern(s);
    }

    // Node pattern
    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        let node = parse_node(inner)?;
        return Ok((Pattern::Node(node), EdgeDirection::Outgoing, None, None));
    }

    Err(format!("invalid pattern syntax: {s}"))
}

/// Returns `true` if `s` contains a comma at depth zero — outside any
/// parenthesis or square bracket. Used to detect star patterns without
/// misreading commas inside `{key: "val", ...}` property maps or rel-type
/// lists.
fn has_top_level_comma(s: &str) -> bool {
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut in_string = false;
    for ch in s.chars() {
        if in_string {
            if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            ',' if depth_paren == 0 && depth_bracket == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Split on top-level commas (same rules as `has_top_level_comma`).
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut in_string = false;
    let mut last = 0usize;
    for (i, ch) in s.char_indices() {
        if in_string {
            if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            ',' if depth_paren == 0 && depth_bracket == 0 => {
                parts.push(s[last..i].to_string());
                last = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[last..].to_string());
    parts
}

fn parse_star_pattern(s: &str) -> Result<ParsedPattern, String> {
    let parts = split_top_level_commas(s);
    if parts.len() < 2 {
        return Err("star pattern needs at least two comma-separated legs".into());
    }
    let mut legs: Vec<EdgeLeg> = Vec::new();

    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return Err("empty leg in star pattern".into());
        }
        let parsed = parse_edge_pattern(trimmed)?;
        let (pattern, direction, _, _) = parsed;
        let (from_pat, rel_type, to_pat) = match pattern {
            Pattern::Edge(from, rel, to) => (from, rel, to),
            _ => return Err(format!("star leg must be an edge pattern: {trimmed}")),
        };
        // Legs may share any variable (or none, producing a cross product).
        // The executor performs a hash-join on every shared variable name.
        legs.push(EdgeLeg {
            rel_type,
            direction,
            from: from_pat,
            to: to_pat,
        });
    }

    Ok((Pattern::Star { legs }, EdgeDirection::Outgoing, None, None))
}

fn count_edge_segments(s: &str) -> usize {
    // Each segment is "-[:REL]->" or "<-[:REL]-" or "-[:REL]-".
    s.matches("-[:").count()
}

fn try_parse_variable_depth(s: &str) -> Result<Option<ParsedPattern>, String> {
    // Pattern shape: (a)-[:REL*min..max]->(b)
    let star_pos = match s.find("*") {
        Some(p) => p,
        None => return Ok(None),
    };
    // Confirm the * is inside [:..]
    let rel_open = s
        .find("[:")
        .ok_or_else(|| "expected [: in variable-depth pattern".to_string())?;
    let rel_close = s
        .find(']')
        .ok_or_else(|| "expected ] in variable-depth pattern".to_string())?;
    if star_pos <= rel_open || star_pos >= rel_close {
        return Ok(None);
    }

    let rel_with_depth = &s[rel_open + 2..rel_close];
    let star_inner = rel_with_depth
        .find('*')
        .ok_or_else(|| "expected * in variable-depth".to_string())?;
    let rel_type = rel_with_depth[..star_inner].trim().to_string();
    let depth_str = rel_with_depth[star_inner + 1..].trim();
    let (min_hops, max_hops) = parse_depth_range(depth_str)?;

    let left_end = s
        .find("-[")
        .or_else(|| s.find("<-["))
        .ok_or_else(|| "expected -[ in variable-depth pattern".to_string())?;
    let arrow_pos = s
        .rfind("->")
        .or_else(|| s.rfind("]-"))
        .ok_or_else(|| "expected -> in variable-depth pattern".to_string())?;

    let from = parse_node_pattern_str(s[..left_end].trim())?;
    let to_start = arrow_pos + 2;
    let to = parse_node_pattern_str(s[to_start..].trim())?;

    Ok(Some((
        Pattern::VariableDepth {
            rel_type,
            min_hops,
            max_hops,
        },
        EdgeDirection::Outgoing,
        Some(from),
        Some(to),
    )))
}

fn parse_depth_range(s: &str) -> Result<(usize, usize), String> {
    if let Some(dot_pos) = s.find("..") {
        let min = s[..dot_pos]
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("invalid min depth: {s}"))?;
        let max = s[dot_pos + 2..]
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("invalid max depth: {s}"))?;
        Ok((min, max))
    } else {
        let n = s
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("invalid depth: {s}"))?;
        Ok((n, n))
    }
}

fn parse_multi_hop(s: &str) -> Result<ParsedPattern, String> {
    // Split on the boundaries between -[:REL]-> segments.
    // Format: (a)-[:X]->(b)-[:Y]->(c)
    let mut legs = Vec::new();
    let mut cursor = 0;
    let bytes = s.as_bytes();
    let mut nodes: Vec<NodePattern> = Vec::new();

    while cursor < bytes.len() {
        let open = match s[cursor..].find('(') {
            Some(o) => cursor + o,
            None => break,
        };
        let close = match s[open..].find(')') {
            Some(c) => open + c,
            None => return Err(format!("unbalanced parens at {open} in {s}")),
        };
        let node = parse_node(&s[open + 1..close])?;
        nodes.push(node);
        cursor = close + 1;
    }

    if nodes.len() < 3 {
        return Err(format!("multi-hop needs 3+ nodes: {s}"));
    }

    let segments: Vec<&str> = collect_edge_segments(s);
    if segments.len() != nodes.len() - 1 {
        return Err(format!(
            "edge segments ({}) != hops ({}) in {s}",
            segments.len(),
            nodes.len() - 1
        ));
    }

    for (i, seg) in segments.iter().enumerate() {
        let (rel_type, direction) = parse_edge_segment(seg)?;
        legs.push(EdgeLeg {
            rel_type,
            direction,
            from: nodes[i].clone(),
            to: nodes[i + 1].clone(),
        });
    }

    let start = nodes.first().cloned();
    let end = nodes.last().cloned();
    Ok((Pattern::MultiHop(legs), EdgeDirection::Outgoing, start, end))
}

fn collect_edge_segments(s: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    while let Some(open) = s[cursor..].find("-[") {
        let abs_open = cursor + open;
        // Allow the leading `<` for incoming edges.
        let segment_start = if abs_open > 0 && &s[abs_open - 1..abs_open] == "<" {
            abs_open - 1
        } else {
            abs_open
        };
        // Find the end of this segment: the next `(` after `]`.
        let close = match s[abs_open..].find(']') {
            Some(c) => abs_open + c,
            None => break,
        };
        let after = close + 1;
        // Edge end is the character (or two) right after `]`: `-` then optional `>`.
        let end = if after < s.len() && &s[after..after + 1] == "-" {
            if after + 1 < s.len() && &s[after + 1..after + 2] == ">" {
                after + 2
            } else {
                after + 1
            }
        } else {
            after
        };
        segments.push(&s[segment_start..end]);
        cursor = end;
    }
    segments
}

fn parse_edge_segment(seg: &str) -> Result<(String, EdgeDirection), String> {
    let direction = if seg.starts_with("<-") {
        EdgeDirection::Incoming
    } else if seg.ends_with("->") {
        EdgeDirection::Outgoing
    } else {
        EdgeDirection::Both
    };
    let rel_open = seg
        .find("[:")
        .ok_or_else(|| format!("expected [: in edge segment: {seg}"))?;
    let rel_close = seg
        .find(']')
        .ok_or_else(|| format!("expected ] in edge segment: {seg}"))?;
    let rel_type = seg[rel_open + 2..rel_close].trim().to_string();
    if rel_type.is_empty() {
        return Err(format!("relationship type cannot be empty in {seg}"));
    }
    Ok((rel_type, direction))
}

fn parse_edge_pattern(s: &str) -> Result<ParsedPattern, String> {
    let direction = detect_direction(s);

    let left_end = s
        .find("-[")
        .or_else(|| s.find("<-["))
        .ok_or_else(|| "expected -[ or <-[ in edge pattern".to_string())?;
    let actual_left_end = if s[..left_end].ends_with(')') {
        left_end
    } else if let Some(rp) = s[..left_end].rfind(')') {
        rp + 1
    } else {
        left_end
    };
    let left_str = s[..actual_left_end].trim();

    let rel_open = s
        .find("[:")
        .ok_or_else(|| "expected [: in edge pattern".to_string())?;
    let rel_close = s
        .find(']')
        .ok_or_else(|| "expected ] in edge pattern".to_string())?;
    let rel_type = s[rel_open + 2..rel_close].trim().to_string();
    if rel_type.is_empty() {
        return Err("relationship type cannot be empty".into());
    }

    let right_start = if let Some(arrow) = s.rfind("->") {
        arrow + 2
    } else if let Some(dash) = s.rfind("]-") {
        dash + 2
    } else {
        rel_close + 1
    };
    let right_str = s[right_start..].trim();

    let left_node = parse_node_pattern_str(left_str)?;
    let right_node = parse_node_pattern_str(right_str)?;

    Ok((
        Pattern::Edge(left_node.clone(), rel_type, right_node.clone()),
        direction,
        Some(left_node),
        Some(right_node),
    ))
}

fn detect_direction(s: &str) -> EdgeDirection {
    if s.contains("<-[") {
        EdgeDirection::Incoming
    } else if s.contains("]->") {
        EdgeDirection::Outgoing
    } else {
        EdgeDirection::Both
    }
}

fn parse_node_pattern_str(s: &str) -> Result<NodePattern, String> {
    let s = s.trim();
    if !s.starts_with('(') || !s.ends_with(')') {
        return Err(format!("node pattern must be enclosed in parentheses: {s}"));
    }
    let inner = &s[1..s.len() - 1];
    parse_node(inner)
}

fn parse_node(s: &str) -> Result<NodePattern, String> {
    let s = s.trim();
    let var_end = s.find(|c: char| c == ':' || c == '{' || c.is_whitespace());
    let var = if let Some(end) = var_end {
        s[..end].trim().to_string()
    } else {
        s.to_string()
    };
    if var.is_empty() {
        return Err("node variable name cannot be empty".into());
    }

    let label = if let Some(colon_pos) = s.find(':') {
        let after_colon = &s[colon_pos + 1..];
        let label_end = after_colon.find(|c: char| c == '{' || c.is_whitespace());
        if let Some(end) = label_end {
            Some(after_colon[..end].trim().to_string())
        } else {
            Some(after_colon.trim().to_string())
        }
    } else {
        None
    };

    let mut props = Vec::new();
    if let Some(open_brace) = s.find('{')
        && let Some(close_brace) = s.rfind('}')
    {
        let props_str = &s[open_brace + 1..close_brace];
        for part in props_str.split(',') {
            let part = part.trim();
            if let Some(colon_pos) = part.find(':') {
                let key = part[..colon_pos].trim().to_string();
                let value = part[colon_pos + 1..]
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                props.push((key, value));
            }
        }
    }

    Ok(NodePattern { var, label, props })
}

// ── Executor ─────────────────────────────────────────────────

/// Execute a parsed [`CypherQuery`] against a SQLite-backed graph.
///
/// Returns a JSON object whose shape depends on the statement kind:
/// - `Match`: `{"results": [...], "count": N}`
/// - `CreateNode` / `CreateEdge`: `{"id": <new_id>}`
/// - `Set`: `{"updated": N}`
/// - `Delete`: `{"deleted": N}`
///
/// # Errors
///
/// Returns an error if execution fails (parse-time errors are surfaced via
/// [`parse`]).
pub fn execute<B: GraphBackend>(backend: &B, query: &CypherQuery) -> Result<Value, String> {
    match &query.statement {
        Statement::Match => execute_match(backend, query),
        Statement::CreateNode { var, label, props } => {
            execute_create_node(backend, var, label.as_deref(), props)
        }
        Statement::CreateEdge {
            from_id,
            to_id,
            rel_type,
        } => execute_create_edge(backend, *from_id, *to_id, rel_type),
        Statement::Set { var, field, value } => execute_set(backend, query, var, field, value),
        Statement::Delete { var } => execute_delete(backend, query, var),
        Statement::CallVectorQuery {
            index_name,
            k,
            vector,
        } => execute_call_vector_query(backend, index_name, *k, vector),
    }
}

fn execute_match<B: GraphBackend>(backend: &B, query: &CypherQuery) -> Result<Value, String> {
    match &query.pattern {
        Pattern::Node(node_pat) => execute_node_match(backend, node_pat, query),
        Pattern::Edge(from_pat, rel_type, to_pat) => {
            execute_edge_match(backend, from_pat, rel_type, to_pat, query)
        }
        Pattern::MultiHop(legs) => execute_multi_hop(backend, legs, query),
        Pattern::VariableDepth {
            rel_type,
            min_hops,
            max_hops,
        } => execute_variable_depth(backend, rel_type, *min_hops, *max_hops, query),
        Pattern::Star { legs } => execute_star(backend, legs, query),
        Pattern::None => Err("MATCH requires a pattern".into()),
    }
}

fn execute_node_match<B: GraphBackend>(
    backend: &B,
    node_pat: &NodePattern,
    query: &CypherQuery,
) -> Result<Value, String> {
    let snapshot = SnapshotId::current();

    // Prefer query_nodes_by_kind when a label filter is present — avoids
    // brute-forcing all node IDs.
    let candidate_ids: Vec<i64> = if let Some(ref label) = node_pat.label {
        backend
            .query_nodes_by_kind(snapshot, label)
            .map_err(|e| e.to_string())?
    } else {
        backend.entity_ids().map_err(|e| e.to_string())?
    };

    let mut filtered = Vec::new();
    for id in candidate_ids {
        if let Ok(node) = backend.get_node(snapshot, id) {
            if !node_pattern_matches(node_pat, &node) {
                continue;
            }
            if !where_clauses_match(query, node_pat, &node, None) {
                continue;
            }
            filtered.push(node);
        }
    }

    let limit = query.limit.unwrap_or(usize::MAX);
    let results: Vec<Value> = filtered
        .into_iter()
        .take(limit)
        .map(|node| {
            let obj = project_node(node_pat, &node, &query.returns);
            Value::Object(obj)
        })
        .filter(|v| !v.as_object().map(|o| o.is_empty()).unwrap_or(true))
        .collect();

    Ok(serde_json::json!({
        "results": results.clone(),
        "count": results.len(),
    }))
}

fn execute_edge_match<B: GraphBackend>(
    backend: &B,
    from_pat: &NodePattern,
    rel_type: &str,
    to_pat: &NodePattern,
    query: &CypherQuery,
) -> Result<Value, String> {
    // For incoming direction we swap the pattern's start/end roles when matching.
    let (start_pat, end_pat) = match query.direction {
        EdgeDirection::Incoming => (to_pat, from_pat),
        _ => (from_pat, to_pat),
    };

    let mut pattern = PatternTriple::new(rel_type);
    if let Some(ref label) = start_pat.label {
        pattern = pattern.start_label(label);
    }
    if let Some(ref label) = end_pat.label {
        pattern = pattern.end_label(label);
    }
    for (key, value) in &start_pat.props {
        pattern = pattern.start_property(key, value);
    }
    for (key, value) in &end_pat.props {
        pattern = pattern.end_property(key, value);
    }

    let triples = backend.match_triples(&pattern).map_err(|e| e.to_string())?;
    let snapshot = SnapshotId::current();

    let mut filtered = Vec::new();
    for triple in triples.iter() {
        let from_node = backend.get_node(snapshot, triple.start_id).ok();
        let to_node = backend.get_node(snapshot, triple.end_id).ok();
        if let (Some(ref from), Some(ref to)) = (from_node, to_node) {
            // Use the pattern's named vars for WHERE binding regardless of direction.
            let (var_from, var_to) = match query.direction {
                EdgeDirection::Incoming => (to, from),
                _ => (from, to),
            };
            if !where_clauses_match(query, from_pat, var_from, Some((to_pat, var_to))) {
                continue;
            }
            filtered.push((triple.edge_id, var_from.clone(), var_to.clone()));
        }
    }

    let limit = query.limit.unwrap_or(usize::MAX);
    let results: Vec<Value> = filtered
        .into_iter()
        .take(limit)
        .map(|(edge_id, from, to)| {
            let obj = project_edge(from_pat, to_pat, &from, &to, edge_id, &query.returns);
            Value::Object(obj)
        })
        .filter(|v| !v.as_object().map(|o| o.is_empty()).unwrap_or(true))
        .collect();

    Ok(serde_json::json!({
        "results": results.clone(),
        "count": results.len(),
    }))
}

fn execute_multi_hop<B: GraphBackend>(
    backend: &B,
    legs: &[EdgeLeg],
    query: &CypherQuery,
) -> Result<Value, String> {
    let snapshot = SnapshotId::current();
    let start_pat = legs[0].from.clone();
    let end_pat = legs.last().expect("multi-hop has >=1 leg").to.clone();

    // Build ChainSteps from legs.
    let chain: Vec<ChainStep> = legs
        .iter()
        .map(|leg| ChainStep {
            edge_type: Some(leg.rel_type.clone()),
            direction: edge_direction_to_backend(leg.direction),
        })
        .collect();

    // Candidate start nodes match the first leg's `from` pattern.
    let start_candidates: Vec<i64> = if let Some(ref label) = start_pat.label {
        backend
            .query_nodes_by_kind(snapshot, label)
            .map_err(|e| e.to_string())?
    } else {
        backend.entity_ids().map_err(|e| e.to_string())?
    };

    let mut results = Vec::new();
    for start_id in start_candidates {
        let start_node = match backend.get_node(snapshot, start_id) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if !node_pattern_matches(&start_pat, &start_node) {
            continue;
        }
        // Dispatch through the GraphBackend trait so both SqliteGraphBackend
        // and V3Backend execute their own backend-native chain traversal.
        let end_ids = backend
            .chain_query(snapshot, start_id, &chain)
            .map_err(|e| e.to_string())?;
        for end_id in end_ids {
            let end_node = match backend.get_node(snapshot, end_id) {
                Ok(n) => n,
                Err(_) => continue,
            };
            if !node_pattern_matches(&end_pat, &end_node) {
                continue;
            }
            // Apply WHERE on start/end.
            if !where_clauses_match(query, &start_pat, &start_node, Some((&end_pat, &end_node))) {
                continue;
            }
            let mut obj = serde_json::Map::new();
            extend_with_node(&mut obj, &start_pat, &start_node, &query.returns);
            extend_with_node(&mut obj, &end_pat, &end_node, &query.returns);
            if !obj.is_empty() {
                results.push(Value::Object(obj));
            }
        }
    }

    let limit = query.limit.unwrap_or(usize::MAX);
    let truncated: Vec<Value> = results.into_iter().take(limit).collect();

    Ok(serde_json::json!({
        "results": truncated.clone(),
        "count": truncated.len(),
    }))
}

fn execute_star<B: GraphBackend>(
    backend: &B,
    legs: &[EdgeLeg],
    query: &CypherQuery,
) -> Result<Value, String> {
    use std::collections::HashMap;

    if legs.is_empty() {
        return Err("star pattern must have at least one leg".into());
    }
    let snapshot = SnapshotId::current();

    /// A partial result row: variable name → bound node id, plus the per-var
    /// pattern (so we can re-check node-pattern constraints after the join).
    type Binding = HashMap<String, i64>;

    // For each leg, build the list of bindings produced by match_triples.
    // The binding maps the leg's `from` var to start_id and `to` var to end_id
    // (regardless of direction — match_triples already orders by edge
    // direction).
    let mut leg_bindings_per_leg: Vec<Vec<Binding>> = Vec::with_capacity(legs.len());
    for leg in legs {
        let (start_pat, end_pat) = match leg.direction {
            EdgeDirection::Incoming => (&leg.to, &leg.from),
            _ => (&leg.from, &leg.to),
        };
        let mut pattern = PatternTriple::new(&leg.rel_type);
        if let Some(ref label) = start_pat.label {
            pattern = pattern.start_label(label);
        }
        if let Some(ref label) = end_pat.label {
            pattern = pattern.end_label(label);
        }
        for (key, value) in &start_pat.props {
            pattern = pattern.start_property(key, value);
        }
        for (key, value) in &end_pat.props {
            pattern = pattern.end_property(key, value);
        }
        let triples = backend.match_triples(&pattern).map_err(|e| e.to_string())?;

        let mut bindings: Vec<Binding> = Vec::with_capacity(triples.len());
        for triple in triples {
            let (from_id, to_id) = match leg.direction {
                EdgeDirection::Incoming => (triple.end_id, triple.start_id),
                _ => (triple.start_id, triple.end_id),
            };
            let mut b = Binding::new();
            b.insert(leg.from.var.clone(), from_id);
            b.insert(leg.to.var.clone(), to_id);
            bindings.push(b);
        }
        leg_bindings_per_leg.push(bindings);
    }

    // Hash-join legs sequentially. After processing leg `i`, `joined` holds
    // all bindings that satisfy legs `0..=i`, with all bound variables merged.
    let mut joined: Vec<Binding> = leg_bindings_per_leg.remove(0);
    for next_bindings in leg_bindings_per_leg {
        let mut merged: Vec<Binding> = Vec::new();
        for left in &joined {
            for right in &next_bindings {
                // Compatible if shared keys agree.
                let mut ok = true;
                for (k, rv) in right {
                    if let Some(lv) = left.get(k)
                        && lv != rv
                    {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    let mut combo = left.clone();
                    for (k, v) in right {
                        combo.insert(k.clone(), *v);
                    }
                    merged.push(combo);
                }
            }
        }
        joined = merged;
        if joined.is_empty() {
            break;
        }
    }

    // Build a var → NodePattern map so we can re-check pattern constraints
    // (labels, props) once per binding. The same var should have the same
    // pattern across legs, but if a label appears on one leg and not another
    // we still want it enforced.
    let mut var_pat: HashMap<String, NodePattern> = HashMap::new();
    for leg in legs {
        var_pat
            .entry(leg.from.var.clone())
            .or_insert_with(|| leg.from.clone());
        var_pat
            .entry(leg.to.var.clone())
            .or_insert_with(|| leg.to.clone());
    }

    let limit = query.limit.unwrap_or(usize::MAX);
    let mut results: Vec<Value> = Vec::new();
    for binding in joined {
        // Fetch nodes for every bound variable; bail on any missing.
        let mut nodes: HashMap<String, GraphEntity> = HashMap::new();
        let mut ok = true;
        for (var, id) in &binding {
            match backend.get_node(snapshot, *id) {
                Ok(n) => {
                    if let Some(pat) = var_pat.get(var)
                        && !node_pattern_matches(pat, &n)
                    {
                        ok = false;
                        break;
                    }
                    nodes.insert(var.clone(), n);
                }
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }

        let node_refs: HashMap<String, &GraphEntity> =
            nodes.iter().map(|(k, v)| (k.clone(), v)).collect();
        if !where_clauses_match_multi(query, &node_refs) {
            continue;
        }

        let mut obj = serde_json::Map::new();
        for (var, node) in &nodes {
            let pat = var_pat.get(var).cloned().unwrap_or_else(|| NodePattern {
                var: var.clone(),
                label: None,
                props: Vec::new(),
            });
            extend_with_node(&mut obj, &pat, node, &query.returns);
        }
        if !obj.is_empty() {
            results.push(Value::Object(obj));
            if results.len() >= limit {
                break;
            }
        }
    }

    Ok(serde_json::json!({
        "results": results.clone(),
        "count": results.len(),
    }))
}

/// Multi-binding WHERE evaluator: same DNF semantics as
/// [`where_clauses_match`] but uses an arbitrary `var → node` map rather
/// than the primary/secondary special-case. Predicates referencing unknown
/// vars pass through, matching the single-/edge-pattern behaviour.
fn where_clauses_match_multi(
    query: &CypherQuery,
    bindings: &std::collections::HashMap<String, &GraphEntity>,
) -> bool {
    if query.where_groups.is_empty() {
        return true;
    }
    query.where_groups.iter().any(|and_group| {
        and_group
            .iter()
            .all(|clause| match bindings.get(&clause.var) {
                Some(n) => evaluate_predicate(clause, n),
                None => true,
            })
    })
}

fn execute_variable_depth<B: GraphBackend>(
    backend: &B,
    rel_type: &str,
    min_hops: usize,
    max_hops: usize,
    query: &CypherQuery,
) -> Result<Value, String> {
    let snapshot = SnapshotId::current();
    let start_pat = query
        .start_node
        .clone()
        .ok_or_else(|| "variable-depth requires a start node".to_string())?;
    let end_pat = query
        .end_node
        .clone()
        .ok_or_else(|| "variable-depth requires an end node".to_string())?;

    let start_candidates: Vec<i64> = if let Some(ref label) = start_pat.label {
        backend
            .query_nodes_by_kind(snapshot, label)
            .map_err(|e| e.to_string())?
    } else {
        backend.entity_ids().map_err(|e| e.to_string())?
    };

    let edge_types: Vec<&str> = vec![rel_type];
    let mut results = Vec::new();
    for start_id in start_candidates {
        let start_node = match backend.get_node(snapshot, start_id) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if !node_pattern_matches(&start_pat, &start_node) {
            continue;
        }
        // Union reachable sets at every depth in [min, max], excluding start.
        let mut reached: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for depth in min_hops..=max_hops {
            let hop_ids = backend
                .k_hop_filtered(
                    snapshot,
                    start_id,
                    depth as u32,
                    BackendDirection::Outgoing,
                    &edge_types,
                )
                .map_err(|e| e.to_string())?;
            for id in hop_ids {
                if id != start_id {
                    reached.insert(id);
                }
            }
        }
        for end_id in reached {
            let end_node = match backend.get_node(snapshot, end_id) {
                Ok(n) => n,
                Err(_) => continue,
            };
            if !node_pattern_matches(&end_pat, &end_node) {
                continue;
            }
            if !where_clauses_match(query, &start_pat, &start_node, Some((&end_pat, &end_node))) {
                continue;
            }
            let mut obj = serde_json::Map::new();
            extend_with_node(&mut obj, &start_pat, &start_node, &query.returns);
            extend_with_node(&mut obj, &end_pat, &end_node, &query.returns);
            if !obj.is_empty() {
                results.push(Value::Object(obj));
            }
        }
    }

    let limit = query.limit.unwrap_or(usize::MAX);
    let truncated: Vec<Value> = results.into_iter().take(limit).collect();

    Ok(serde_json::json!({
        "results": truncated.clone(),
        "count": truncated.len(),
    }))
}

fn execute_create_node<B: GraphBackend>(
    backend: &B,
    var: &str,
    label: Option<&str>,
    props: &[(String, String)],
) -> Result<Value, String> {
    let mut data = serde_json::Map::new();
    for (k, v) in props {
        if k == "name" {
            continue;
        }
        data.insert(k.clone(), Value::String(v.clone()));
    }
    // Use the explicit `name` property when present; fall back to the
    // pattern variable so that backends requiring a non-empty name
    // (sqlitegraph enforces this) still succeed.
    let name = props
        .iter()
        .find(|(k, _)| k == "name")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| var.to_string());
    let spec = NodeSpec {
        kind: label.unwrap_or("Node").to_string(),
        name,
        file_path: None,
        data: Value::Object(data),
    };
    let id = backend.insert_node(spec).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"id": id}))
}

fn execute_create_edge<B: GraphBackend>(
    backend: &B,
    from_id: i64,
    to_id: i64,
    rel_type: &str,
) -> Result<Value, String> {
    let id = backend
        .insert_edge(EdgeSpec {
            from: from_id,
            to: to_id,
            edge_type: rel_type.to_string(),
            data: Value::Object(serde_json::Map::new()),
        })
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"id": id}))
}

fn execute_set<B: GraphBackend>(
    backend: &B,
    query: &CypherQuery,
    _var: &str,
    field: &str,
    value: &str,
) -> Result<Value, String> {
    let matched = collect_match_targets(backend, query)?;
    let snapshot = SnapshotId::current();
    let mut updated = 0u64;
    for id in matched {
        let node = match backend.get_node(snapshot, id) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let mut data = match node.data {
            Value::Object(ref m) => m.clone(),
            _ => serde_json::Map::new(),
        };
        match field {
            "name" => {
                let spec = NodeSpec {
                    kind: node.kind.clone(),
                    name: value.to_string(),
                    file_path: None,
                    data: Value::Object(data),
                };
                backend.update_node(id, spec).map_err(|e| e.to_string())?;
            }
            "kind" => {
                let spec = NodeSpec {
                    kind: value.to_string(),
                    name: node.name.clone(),
                    file_path: None,
                    data: Value::Object(data),
                };
                backend.update_node(id, spec).map_err(|e| e.to_string())?;
            }
            other => {
                data.insert(other.to_string(), Value::String(value.to_string()));
                let spec = NodeSpec {
                    kind: node.kind.clone(),
                    name: node.name.clone(),
                    file_path: None,
                    data: Value::Object(data),
                };
                backend.update_node(id, spec).map_err(|e| e.to_string())?;
            }
        }
        updated += 1;
    }
    Ok(serde_json::json!({"updated": updated}))
}

fn execute_delete<B: GraphBackend>(
    backend: &B,
    query: &CypherQuery,
    _var: &str,
) -> Result<Value, String> {
    let matched = collect_match_targets(backend, query)?;
    let mut deleted = 0u64;
    for id in matched {
        backend.delete_entity(id).map_err(|e| e.to_string())?;
        deleted += 1;
    }
    Ok(serde_json::json!({"deleted": deleted}))
}

fn execute_call_vector_query<B: GraphBackend>(
    backend: &B,
    index_name: &str,
    k: usize,
    vector: &[f32],
) -> Result<Value, String> {
    let snapshot = SnapshotId::current();
    let raw_results = backend
        .vector_search(snapshot, index_name, vector, k)
        .map_err(|e| format!("CALL: vector search failed: {e}"))?;

    let rows: Vec<Value> = raw_results
        .into_iter()
        .map(|(id, score)| serde_json::json!({"id": id, "score": score}))
        .collect();

    Ok(serde_json::json!({
        "results": rows.clone(),
        "count": rows.len(),
    }))
}

/// For SET/DELETE: collect node IDs that match the MATCH clause.
fn collect_match_targets<B: GraphBackend>(
    backend: &B,
    query: &CypherQuery,
) -> Result<Vec<i64>, String> {
    let snapshot = SnapshotId::current();
    let node_pat = match &query.pattern {
        Pattern::Node(n) => n.clone(),
        Pattern::Edge(from, _, _) => from.clone(),
        _ => {
            return Err("SET/DELETE expects a node-shaped MATCH pattern".into());
        }
    };
    let candidates: Vec<i64> = if let Some(ref label) = node_pat.label {
        backend
            .query_nodes_by_kind(snapshot, label)
            .map_err(|e| e.to_string())?
    } else {
        backend.entity_ids().map_err(|e| e.to_string())?
    };
    let mut hits = Vec::new();
    for id in candidates {
        if let Ok(node) = backend.get_node(snapshot, id) {
            if !node_pattern_matches(&node_pat, &node) {
                continue;
            }
            if !where_clauses_match(query, &node_pat, &node, None) {
                continue;
            }
            hits.push(id);
        }
    }
    Ok(hits)
}

// ── Helpers ──────────────────────────────────────────────────

fn node_pattern_matches(pat: &NodePattern, node: &GraphEntity) -> bool {
    if let Some(ref label) = pat.label
        && node.kind != *label
    {
        return false;
    }
    for (key, value) in &pat.props {
        if key == "name" {
            if &node.name != value {
                return false;
            }
            continue;
        }
        if key == "kind" {
            if &node.kind != value {
                return false;
            }
            continue;
        }
        match node.data.get(key) {
            Some(v) if v.as_str() == Some(value) => continue,
            _ => return false,
        }
    }
    true
}

fn where_clauses_match(
    query: &CypherQuery,
    primary_pat: &NodePattern,
    primary_node: &GraphEntity,
    secondary: Option<(&NodePattern, &GraphEntity)>,
) -> bool {
    if query.where_groups.is_empty() {
        return true;
    }
    // DNF evaluation: any group whose every clause matches wins.
    query.where_groups.iter().any(|and_group| {
        and_group.iter().all(|clause| {
            let node = if clause.var == primary_pat.var {
                Some(primary_node)
            } else if let Some((pat, n)) = secondary {
                if clause.var == pat.var { Some(n) } else { None }
            } else {
                None
            };
            match node {
                Some(n) => evaluate_predicate(clause, n),
                None => true, // unknown var — don't filter on this clause
            }
        })
    })
}

fn evaluate_predicate(clause: &WhereClause, node: &GraphEntity) -> bool {
    let actual_str: String = match clause.field.as_str() {
        "kind" => node.kind.clone(),
        "name" => node.name.clone(),
        "id" => node.id.to_string(),
        other => match node.data.get(other) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            _ => return false,
        },
    };
    match clause.operator {
        WhereOp::Eq => actual_str == clause.value,
        WhereOp::NotEq => actual_str != clause.value,
        WhereOp::GreaterThan => compare_numeric(&actual_str, &clause.value)
            .map(|o| o == std::cmp::Ordering::Greater)
            .unwrap_or(false),
        WhereOp::LessThan => compare_numeric(&actual_str, &clause.value)
            .map(|o| o == std::cmp::Ordering::Less)
            .unwrap_or(false),
        WhereOp::GreaterEq => compare_numeric(&actual_str, &clause.value)
            .map(|o| matches!(o, std::cmp::Ordering::Greater | std::cmp::Ordering::Equal))
            .unwrap_or(false),
        WhereOp::LessEq => compare_numeric(&actual_str, &clause.value)
            .map(|o| matches!(o, std::cmp::Ordering::Less | std::cmp::Ordering::Equal))
            .unwrap_or(false),
        WhereOp::Regex => regex_match(&actual_str, &clause.value),
    }
}

fn compare_numeric(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let l: f64 = left.parse().ok()?;
    let r: f64 = right.parse().ok()?;
    l.partial_cmp(&r)
}

/// Lightweight glob-to-regex: tests use `ma.*` which is already valid regex.
/// We treat the pattern as a Rust-friendly regex and fall back to substring
/// match if regex compilation fails.
fn regex_match(actual: &str, pattern: &str) -> bool {
    match regex::Regex::new(pattern) {
        Ok(re) => re.is_match(actual),
        Err(_) => actual.contains(pattern),
    }
}

fn extend_with_node(
    obj: &mut serde_json::Map<String, Value>,
    pat: &NodePattern,
    node: &GraphEntity,
    returns: &[String],
) {
    for ret in returns {
        if ret == "*" || ret == &pat.var {
            obj.insert(
                pat.var.clone(),
                serde_json::json!({
                    "id": node.id,
                    "kind": node.kind,
                    "name": node.name,
                    "data": node.data,
                }),
            );
        } else if ret.starts_with(&format!("{}.", pat.var)) {
            let field = &ret[pat.var.len() + 1..];
            let value = match field {
                "id" => serde_json::json!(node.id),
                "kind" => serde_json::json!(node.kind),
                "name" => serde_json::json!(node.name),
                _ => node.data.get(field).cloned().unwrap_or(Value::Null),
            };
            obj.insert(ret.clone(), value);
        }
    }
}

fn project_node(
    pat: &NodePattern,
    node: &GraphEntity,
    returns: &[String],
) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    extend_with_node(&mut obj, pat, node, returns);
    obj
}

fn project_edge(
    from_pat: &NodePattern,
    to_pat: &NodePattern,
    from: &GraphEntity,
    to: &GraphEntity,
    edge_id: i64,
    returns: &[String],
) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    for ret in returns {
        if ret == "*" {
            extend_with_node(&mut obj, from_pat, from, &["*".to_string()]);
            extend_with_node(&mut obj, to_pat, to, &["*".to_string()]);
            obj.insert("edge_id".to_string(), serde_json::json!(edge_id));
        } else if ret == &from_pat.var {
            extend_with_node(
                &mut obj,
                from_pat,
                from,
                std::slice::from_ref(&from_pat.var),
            );
        } else if ret == &to_pat.var {
            extend_with_node(&mut obj, to_pat, to, std::slice::from_ref(&to_pat.var));
        } else if ret.starts_with(&format!("{}.", from_pat.var)) {
            extend_with_node(&mut obj, from_pat, from, std::slice::from_ref(ret));
        } else if ret.starts_with(&format!("{}.", to_pat.var)) {
            extend_with_node(&mut obj, to_pat, to, std::slice::from_ref(ret));
        }
    }
    obj
}

fn edge_direction_to_backend(dir: EdgeDirection) -> BackendDirection {
    match dir {
        EdgeDirection::Outgoing => BackendDirection::Outgoing,
        EdgeDirection::Incoming => BackendDirection::Incoming,
        // BackendDirection has no `Both`; undirected queries are handled in the
        // executor by running both directions and merging — this mapping is
        // only used by chain_query (multi-hop), which defaults to Outgoing.
        EdgeDirection::Both => BackendDirection::Outgoing,
    }
}
