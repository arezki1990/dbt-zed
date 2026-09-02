pub fn leading_keyword(sql: &str) -> Option<&str> {
    let trimmed = sql.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let end = trimmed
        .find(|c: char| c.is_ascii_whitespace() || c == ';')
        .unwrap_or(trimmed.len());
    Some(&trimmed[..end])
}
