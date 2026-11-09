/// Quotes a string as a Postgres literal, for debug output only. Ported from libpq's `PQescapeLiteral`.
pub(crate) fn escape_literal(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() + 2);
    let mut has_backslash = false;
    escaped.push('\'');
    for c in s.chars() {
        match c {
            '\'' => escaped.push_str("''"),
            '\\' => {
                escaped.push_str("\\\\");
                has_backslash = true;
            }
            c => escaped.push(c),
        }
    }
    escaped.push('\'');
    if has_backslash {
        format!("E{escaped}")
    } else {
        escaped
    }
}

/// Quotes an identifier, e.g. `webknossos.annotations` becomes `"webknossos"."annotations"`.
pub(crate) fn quote_identifier(id: &str) -> String {
    id.split('.')
        .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_literals() {
        assert_eq!(escape_literal("abc"), "'abc'");
        assert_eq!(escape_literal("it's"), "'it''s'");
        assert_eq!(escape_literal(r"a\b"), r"E'a\\b'");
    }

    #[test]
    fn quotes_identifiers() {
        assert_eq!(
            quote_identifier("webknossos.annotations"),
            r#""webknossos"."annotations""#
        );
        assert_eq!(quote_identifier(r#"we"ird"#), r#""we""ird""#);
    }
}
