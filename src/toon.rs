//! Minimal TOON (`toonformat.dev`) renderer, matching the AXI family's compact
//! labeled-list output. Default output is TOON; `--json` is the opt-in
//! machine-readable path handled by the caller (ops build serde_json Values).

/// Quote a CSV cell when it contains a comma, quote, or newline.
fn cell(v: &str) -> String {
    if v.contains(',') || v.contains('"') || v.contains('\n') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

/// Render the `bin:` + `description:` report header.
pub fn header(bin: &str, description: &str) -> String {
    format!("bin: {}\ndescription: {}", bin, description)
}

/// Render a labeled record list: `label[count]{schema}:` + one CSV row per record.
pub fn list(label: &str, schema: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = format!("{}[{}]{{{}}}:", label, rows.len(), schema.join(","));
    for row in rows {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&row.iter().map(|c| cell(c)).collect::<Vec<_>>().join(","));
    }
    out
}

/// Render a labeled scalar as `label:` + indented value(s).
pub fn kv(label: &str, value: &str) -> String {
    format!("{}: {}", label, value)
}

/// Render the `help[N]:` next-step suggestion block.
pub fn help(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!("help[{}]:", lines.len());
    for l in lines {
        out.push('\n');
        out.push_str(&format!("  - {}", l));
    }
    out
}

/// Render a structured error as TOON (`error:` + `code:` + optional `help:`).
pub fn error(message: &str, code: &str, suggestions: &[String]) -> String {
    let mut out = format!("error: {}\ncode: {}", message, code);
    if !suggestions.is_empty() {
        out.push('\n');
        out.push_str(&help(suggestions));
    }
    out
}

/// Join already-rendered TOON blocks with a blank line between non-empty ones.
pub fn join(blocks: &[String]) -> String {
    blocks
        .iter()
        .filter(|b| !b.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}
