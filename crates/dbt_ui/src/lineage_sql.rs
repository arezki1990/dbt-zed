//! AST-based column lineage.
//!
//! Parses a model's compiled SQL with `sqlparser` (Snowflake dialect first,
//! generic as fallback) and resolves every output column to the exact
//! `(upstream node, upstream column)` pairs it derives from, walking CTE
//! scopes, FROM/JOIN aliases, subqueries, unions, and `select *` expansion
//! (using the upstream column lists the lineage graph already knows).
//!
//! Returns `None` when the SQL doesn't parse or yields no query — callers
//! fall back to the text heuristic in `lineage.rs`.

use std::collections::HashMap;

use sqlparser::ast::{
    Expr, Query, Select, SelectItem, SetExpr, Statement, TableFactor,
};
use sqlparser::dialect::{GenericDialect, SnowflakeDialect};
use sqlparser::parser::Parser;

/// An upstream relation the SQL may reference: keyed by the (lowercased)
/// table identifier it appears as in FROM clauses.
#[derive(Clone, Debug)]
pub struct UpstreamRelation {
    /// The lineage graph node name (model/source name).
    pub node: String,
    /// Lowercased column names of that relation.
    pub columns: Vec<String>,
}

/// (upstream node name, upstream column) — one exact lineage leaf.
pub type Leaf = (String, String);

/// One resolved scope: output columns in order with their lineage leaves.
#[derive(Clone, Debug, Default)]
struct Scope {
    columns: Vec<(String, Vec<Leaf>)>,
}

impl Scope {
    fn lookup(&self, column: &str) -> Option<&Vec<Leaf>> {
        self.columns
            .iter()
            .find(|(name, _)| name == column)
            .map(|(_, leaves)| leaves)
    }
}

fn norm_ident(ident: &str) -> String {
    ident.trim_matches('"').to_lowercase()
}

/// Resolves the lineage of every output column of `sql`.
pub fn column_lineage(
    sql: &str,
    upstream: &HashMap<String, UpstreamRelation>,
) -> Option<HashMap<String, Vec<Leaf>>> {
    let statements = Parser::parse_sql(&SnowflakeDialect {}, sql)
        .or_else(|_| Parser::parse_sql(&GenericDialect {}, sql))
        .ok()?;
    let query = statements.iter().rev().find_map(|statement| match statement {
        Statement::Query(query) => Some(query),
        _ => None,
    })?;
    let scope = resolve_query(query, upstream, &HashMap::new(), 0)?;
    let mut out: HashMap<String, Vec<Leaf>> = HashMap::new();
    for (name, leaves) in scope.columns {
        out.entry(name).or_insert(leaves);
    }
    (!out.is_empty()).then_some(out)
}

fn resolve_query(
    query: &Query,
    upstream: &HashMap<String, UpstreamRelation>,
    outer_ctes: &HashMap<String, Scope>,
    depth: usize,
) -> Option<Scope> {
    if depth > 24 {
        return None;
    }
    let mut ctes = outer_ctes.clone();
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            let name = norm_ident(&cte.alias.name.value);
            if let Some(mut scope) = resolve_query(&cte.query, upstream, &ctes, depth + 1) {
                // Explicit CTE column aliases rename positionally.
                if !cte.alias.columns.is_empty() {
                    for (ix, alias) in cte.alias.columns.iter().enumerate() {
                        if let Some(entry) = scope.columns.get_mut(ix) {
                            entry.0 = norm_ident(&alias.name.value);
                        }
                    }
                }
                ctes.insert(name, scope);
            }
        }
    }
    resolve_set_expr(&query.body, upstream, &ctes, depth)
}

fn resolve_set_expr(
    body: &SetExpr,
    upstream: &HashMap<String, UpstreamRelation>,
    ctes: &HashMap<String, Scope>,
    depth: usize,
) -> Option<Scope> {
    match body {
        SetExpr::Select(select) => resolve_select(select, upstream, ctes, depth),
        SetExpr::Query(query) => resolve_query(query, upstream, ctes, depth + 1),
        SetExpr::SetOperation { left, right, .. } => {
            // Union branches contribute lineage positionally; names from left.
            let left = resolve_set_expr(left, upstream, ctes, depth)?;
            let right = resolve_set_expr(right, upstream, ctes, depth);
            let mut columns = left.columns;
            if let Some(right) = right {
                for (ix, (_, leaves)) in right.columns.into_iter().enumerate() {
                    if let Some(entry) = columns.get_mut(ix) {
                        for leaf in leaves {
                            if !entry.1.contains(&leaf) {
                                entry.1.push(leaf);
                            }
                        }
                    }
                }
            }
            Some(Scope { columns })
        }
        _ => None,
    }
}

fn resolve_select(
    select: &Select,
    upstream: &HashMap<String, UpstreamRelation>,
    ctes: &HashMap<String, Scope>,
    depth: usize,
) -> Option<Scope> {
    // FROM/JOIN sources: ordered (alias, scope) pairs.
    let mut sources: Vec<(String, Scope)> = Vec::new();
    for table in &select.from {
        collect_table_factor(&table.relation, upstream, ctes, depth, &mut sources);
        for join in &table.joins {
            collect_table_factor(&join.relation, upstream, ctes, depth, &mut sources);
        }
    }

    let mut columns: Vec<(String, Vec<Leaf>)> = Vec::new();
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(_) => {
                for (_, scope) in &sources {
                    columns.extend(scope.columns.iter().cloned());
                }
            }
            SelectItem::QualifiedWildcard(kind, _) => {
                let qualifier = norm_ident(&kind.to_string());
                let qualifier = qualifier.rsplit('.').next().unwrap_or(&qualifier).to_owned();
                if let Some((_, scope)) =
                    sources.iter().find(|(alias, _)| *alias == qualifier)
                {
                    columns.extend(scope.columns.iter().cloned());
                }
            }
            SelectItem::UnnamedExpr(expr) => {
                let name = match expr {
                    Expr::Identifier(ident) => norm_ident(&ident.value),
                    Expr::CompoundIdentifier(parts) => parts
                        .last()
                        .map(|ident| norm_ident(&ident.value))
                        .unwrap_or_default(),
                    _ => continue,
                };
                let mut leaves = Vec::new();
                collect_expr_leaves(expr, &sources, upstream, ctes, depth, &mut leaves);
                columns.push((name, leaves));
            }
            SelectItem::ExprWithAlias { expr, alias } => {
                let mut leaves = Vec::new();
                collect_expr_leaves(expr, &sources, upstream, ctes, depth, &mut leaves);
                columns.push((norm_ident(&alias.value), leaves));
            }
            _ => {}
        }
    }
    Some(Scope { columns })
}

fn collect_table_factor(
    factor: &TableFactor,
    upstream: &HashMap<String, UpstreamRelation>,
    ctes: &HashMap<String, Scope>,
    depth: usize,
    sources: &mut Vec<(String, Scope)>,
) {
    match factor {
        TableFactor::Table { name, alias, .. } => {
            let table = name
                .0
                .last()
                .map(|part| norm_ident(&part.to_string()))
                .unwrap_or_default();
            let scope = if let Some(cte) = ctes.get(&table) {
                cte.clone()
            } else if let Some(relation) = upstream.get(&table) {
                Scope {
                    columns: relation
                        .columns
                        .iter()
                        .map(|column| {
                            (
                                column.clone(),
                                vec![(relation.node.clone(), column.clone())],
                            )
                        })
                        .collect(),
                }
            } else {
                Scope::default()
            };
            let label = alias
                .as_ref()
                .map(|alias| norm_ident(&alias.name.value))
                .unwrap_or(table);
            sources.push((label, scope));
        }
        TableFactor::Derived {
            subquery, alias, ..
        } => {
            if let Some(scope) = resolve_query(subquery, upstream, ctes, depth + 1) {
                let label = alias
                    .as_ref()
                    .map(|alias| norm_ident(&alias.name.value))
                    .unwrap_or_default();
                sources.push((label, scope));
            }
        }
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => {
            collect_table_factor(&table_with_joins.relation, upstream, ctes, depth, sources);
            for join in &table_with_joins.joins {
                collect_table_factor(&join.relation, upstream, ctes, depth, sources);
            }
        }
        _ => {}
    }
}

/// Walks an expression, resolving identifier leaves through the FROM sources.
fn collect_expr_leaves(
    expr: &Expr,
    sources: &[(String, Scope)],
    upstream: &HashMap<String, UpstreamRelation>,
    ctes: &HashMap<String, Scope>,
    depth: usize,
    out: &mut Vec<Leaf>,
) {
    let mut push_all = |leaves: &Vec<Leaf>, out: &mut Vec<Leaf>| {
        for leaf in leaves {
            if !out.contains(leaf) {
                out.push(leaf.clone());
            }
        }
    };
    match expr {
        Expr::Identifier(ident) => {
            let column = norm_ident(&ident.value);
            for (_, scope) in sources {
                if let Some(leaves) = scope.lookup(&column) {
                    push_all(leaves, out);
                }
            }
        }
        Expr::CompoundIdentifier(parts) => {
            if parts.len() >= 2 {
                let qualifier = norm_ident(&parts[parts.len() - 2].value);
                let column = norm_ident(&parts[parts.len() - 1].value);
                let mut matched = false;
                for (alias, scope) in sources {
                    if *alias == qualifier {
                        if let Some(leaves) = scope.lookup(&column) {
                            push_all(leaves, out);
                        }
                        matched = true;
                    }
                }
                if !matched {
                    // Unknown qualifier: fall back to unqualified lookup.
                    for (_, scope) in sources {
                        if let Some(leaves) = scope.lookup(&column) {
                            push_all(leaves, out);
                        }
                    }
                }
            }
        }
        Expr::Subquery(query) => {
            if let Some(scope) = resolve_query(query, upstream, ctes, depth + 1) {
                for (_, leaves) in &scope.columns {
                    push_all(leaves, out);
                }
            }
        }
        other => {
            // Generic traversal: visit child expressions through the AST's
            // Display-independent structure via `sqlparser`'s visitor-free
            // API — enumerate the common containers explicitly.
            visit_children(other, &mut |child| {
                collect_expr_leaves(child, sources, upstream, ctes, depth, out);
            });
        }
    }
}

/// Calls `visit` on every direct child expression of `expr`. Cases not listed
/// contribute no identifier leaves (literals, intervals, etc.).
fn visit_children(expr: &Expr, visit: &mut dyn FnMut(&Expr)) {
    use Expr::*;
    match expr {
        BinaryOp { left, right, .. } => {
            visit(left);
            visit(right);
        }
        UnaryOp { expr, .. }
        | Cast { expr, .. }
        | Nested(expr)
        | IsNull(expr)
        | IsNotNull(expr)
        | IsTrue(expr)
        | IsNotTrue(expr)
        | IsFalse(expr)
        | IsNotFalse(expr)
        | IsUnknown(expr)
        | IsNotUnknown(expr) => visit(expr),
        InList { expr, list, .. } => {
            visit(expr);
            for item in list {
                visit(item);
            }
        }
        Between {
            expr, low, high, ..
        } => {
            visit(expr);
            visit(low);
            visit(high);
        }
        Like { expr, pattern, .. }
        | ILike { expr, pattern, .. }
        | SimilarTo { expr, pattern, .. } => {
            visit(expr);
            visit(pattern);
        }
        Case {
            operand,
            conditions,
            else_result,
            ..
        } => {
            if let Some(operand) = operand {
                visit(operand);
            }
            for condition in conditions {
                visit(&condition.condition);
                visit(&condition.result);
            }
            if let Some(else_result) = else_result {
                visit(else_result);
            }
        }
        Function(function) => {
            if let sqlparser::ast::FunctionArguments::List(list) = &function.args {
                for arg in &list.args {
                    if let sqlparser::ast::FunctionArg::Unnamed(
                        sqlparser::ast::FunctionArgExpr::Expr(expr),
                    )
                    | sqlparser::ast::FunctionArg::Named {
                        arg: sqlparser::ast::FunctionArgExpr::Expr(expr),
                        ..
                    } = arg
                    {
                        visit(expr);
                    }
                }
            }
            if let Some(sqlparser::ast::WindowType::WindowSpec(spec)) = &function.over {
                for expr in &spec.partition_by {
                    visit(expr);
                }
                for order in &spec.order_by {
                    visit(&order.expr);
                }
            }
        }
        Tuple(items) => {
            for item in items {
                visit(item);
            }
        }
        Collate { expr, .. } => visit(expr),
        Substring {
            expr,
            substring_from,
            substring_for,
            ..
        } => {
            visit(expr);
            if let Some(from) = substring_from {
                visit(from);
            }
            if let Some(for_) = substring_for {
                visit(for_);
            }
        }
        Trim { expr, .. } => visit(expr),
        Extract { expr, .. } => visit(expr),
        Floor { expr, .. } | Ceil { expr, .. } => visit(expr),
        Position { expr, r#in } => {
            visit(expr);
            visit(r#in);
        }
        InSubquery { expr, .. } => visit(expr),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corpus benchmark against a real project; run explicitly with
    /// `cargo test -p dbt_ui -- --ignored corpus`.
    #[test]
    #[ignore]
    fn corpus_dbt_employees() {
        let target = std::path::Path::new(
            "/Users/arezkipro/projects/dbt-employees/employees/target",
        );
        let manifest: serde_json::Value = serde_json::from_reader(
            std::fs::File::open(target.join("manifest.json")).unwrap(),
        )
        .unwrap();
        let catalog: serde_json::Value = std::fs::File::open(target.join("catalog.json"))
            .ok()
            .and_then(|file| serde_json::from_reader(file).ok())
            .unwrap_or(serde_json::json!({"nodes": {}, "sources": {}}));

        let catalog_cols = |uid: &str| -> Vec<String> {
            ["nodes", "sources"]
                .iter()
                .find_map(|section| {
                    catalog
                        .get(section)?
                        .get(uid)?
                        .get("columns")?
                        .as_object()
                        .map(|cols| cols.keys().map(|k| k.to_lowercase()).collect())
                })
                .unwrap_or_default()
        };
        let all = |section: &str| {
            manifest[section]
                .as_object()
                .cloned()
                .unwrap_or_default()
        };
        let nodes = all("nodes");
        let sources = all("sources");
        let get = |uid: &str| nodes.get(uid).or_else(|| sources.get(uid));

        let table_ident = |uid: &str| -> Option<String> {
            let node = get(uid)?;
            node.get("relation_name")
                .and_then(|value| value.as_str())
                .and_then(|relation| relation.rsplit('.').next().map(str::to_owned))
                .map(|last| norm_ident(&last))
                .or_else(|| {
                    node.get("alias")
                        .or_else(|| node.get("name"))
                        .and_then(|value| value.as_str())
                        .map(norm_ident)
                })
        };

        let (mut parsed, mut failed, mut total_cols, mut linked) = (0, 0, 0, 0);
        let mut fail_names = Vec::new();
        for (uid, node) in &nodes {
            if node["resource_type"] != "model" {
                continue;
            }
            let Some(sql) = node.get("compiled_code").and_then(|value| value.as_str())
            else {
                continue;
            };
            let mut upstream_map = HashMap::new();
            for parent in node["depends_on"]["nodes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                let parent_uid = parent.as_str().unwrap_or_default();
                let Some(parent_node) = get(parent_uid) else { continue };
                let Some(ident) = table_ident(parent_uid) else { continue };
                let mut columns = catalog_cols(parent_uid);
                if columns.is_empty() {
                    columns = parent_node["columns"]
                        .as_object()
                        .map(|cols| cols.keys().map(|k| k.to_lowercase()).collect())
                        .unwrap_or_default();
                }
                upstream_map.insert(
                    ident,
                    UpstreamRelation {
                        node: parent_node["name"].as_str().unwrap_or("").to_owned(),
                        columns,
                    },
                );
            }
            if upstream_map.is_empty() {
                continue;
            }
            match column_lineage(sql, &upstream_map) {
                Some(lineage) => {
                    parsed += 1;
                    for column in catalog_cols(uid) {
                        total_cols += 1;
                        if lineage.get(&column).is_some_and(|leaves| !leaves.is_empty())
                        {
                            linked += 1;
                        } else if fail_names.len() < 25 {
                            fail_names.push(format!(
                                "{}.{column}{}",
                                node["name"].as_str().unwrap_or(""),
                                if lineage.contains_key(&column) { " (empty)" } else { " (absent)" },
                            ));
                        }
                    }
                }
                None => {
                    failed += 1;
                    if fail_names.len() < 10 {
                        fail_names.push(node["name"].as_str().unwrap_or(uid).to_owned());
                    }
                }
            }
        }
        println!(
            "AST lineage: parsed {parsed} models, failed {failed} | columns linked {linked}/{total_cols} ({:.1}%)",
            100.0 * linked as f64 / total_cols.max(1) as f64
        );
        println!("parse failures: {fail_names:?}");
    }

    fn upstream(entries: &[(&str, &str, &[&str])]) -> HashMap<String, UpstreamRelation> {
        entries
            .iter()
            .map(|(table, node, columns)| {
                (
                    table.to_string(),
                    UpstreamRelation {
                        node: node.to_string(),
                        columns: columns.iter().map(|s| s.to_string()).collect(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn resolves_cte_rename_through_function() {
        let sql = "WITH a AS (SELECT date_absence AS absence_date FROM db.sch.tbl) \
                   SELECT MAX(absence_date) AS date_fin FROM a";
        let up = upstream(&[("tbl", "src_model", &["date_absence"])]);
        let lineage = column_lineage(sql, &up).unwrap();
        assert_eq!(
            lineage.get("date_fin").unwrap(),
            &vec![("src_model".to_string(), "date_absence".to_string())]
        );
    }

    #[test]
    fn alias_qualification_disambiguates() {
        let sql = "SELECT a.x AS ax, b.x AS bx FROM db.s.t1 a JOIN db.s.t2 b ON a.id = b.id";
        let up = upstream(&[
            ("t1", "model_one", &["id", "x"]),
            ("t2", "model_two", &["id", "x"]),
        ]);
        let lineage = column_lineage(sql, &up).unwrap();
        assert_eq!(
            lineage.get("ax").unwrap(),
            &vec![("model_one".into(), "x".into())]
        );
        assert_eq!(
            lineage.get("bx").unwrap(),
            &vec![("model_two".into(), "x".into())]
        );
    }

    #[test]
    fn star_expands_through_cte() {
        let sql = "WITH base AS (SELECT * FROM db.s.orders) SELECT * FROM base";
        let up = upstream(&[("orders", "stg_orders", &["order_id", "amount"])]);
        let lineage = column_lineage(sql, &up).unwrap();
        assert_eq!(
            lineage.get("amount").unwrap(),
            &vec![("stg_orders".into(), "amount".into())]
        );
        assert!(lineage.contains_key("order_id"));
    }

    #[test]
    fn union_merges_positionally() {
        let sql = "SELECT id FROM db.s.a UNION ALL SELECT id2 AS id FROM db.s.b";
        let up = upstream(&[("a", "ma", &["id"]), ("b", "mb", &["id2"])]);
        let lineage = column_lineage(sql, &up).unwrap();
        let leaves = lineage.get("id").unwrap();
        assert!(leaves.contains(&("ma".into(), "id".into())));
        assert!(leaves.contains(&("mb".into(), "id2".into())));
    }
}
