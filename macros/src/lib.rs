//! Proc macro for the `sql_interpolate` crate. Use `sql_interpolate::q!` instead of depending on this crate directly.

use proc_macro::TokenStream;
use proc_macro2::{Group, Span, TokenStream as TokenStream2, TokenTree};
use quote::quote;
use syn::{Expr, LitStr, parse_macro_input};

/// Builds a `SqlToken` from a string literal with `${expr}` interpolations.
///
/// See the `sql_interpolate` crate docs for details.
#[proc_macro]
pub fn q(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    match expand(&lit) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

enum Segment {
    Sql(String),
    Expr(Expr),
}

fn expand(lit: &LitStr) -> syn::Result<TokenStream2> {
    let segments = parse_segments(&lit.value(), lit.span())?;

    // mixed_site hygiene: the user's expressions cannot accidentally refer to this variable
    let token = proc_macro2::Ident::new("__sql_token", Span::mixed_site());
    let statements = segments.into_iter().map(|segment| match segment {
        Segment::Sql(sql) => quote! { #token.push_sql(#sql); },
        Segment::Expr(expr) => {
            quote! { ::sql_interpolate::IntoSqlToken::append_to(#expr, &mut #token); }
        }
    });

    Ok(quote! {
        {
            let mut #token = ::sql_interpolate::SqlToken::new();
            #(#statements)*
            #token
        }
    })
}

fn parse_segments(source: &str, span: Span) -> syn::Result<Vec<Segment>> {
    let mut segments = Vec::new();
    let mut sql = String::new();
    let mut rest = source;

    while let Some(dollar) = rest.find('$') {
        sql.push_str(&rest[..dollar]);
        rest = &rest[dollar..];
        if rest.starts_with("$${") {
            // Escaped interpolation: `$${` yields a literal `${`
            sql.push_str("${");
            rest = &rest[3..];
        } else if rest.starts_with("${") {
            let body = &rest[2..];
            let end = find_closing_brace(body)
                .ok_or_else(|| syn::Error::new(span, "unclosed `${` in SQL string"))?;
            let expr_source = body[..end].trim();
            if expr_source.is_empty() {
                return Err(syn::Error::new(span, "empty `${}` in SQL string"));
            }
            if !sql.is_empty() {
                segments.push(Segment::Sql(std::mem::take(&mut sql)));
            }
            segments.push(Segment::Expr(parse_expr(expr_source, span)?));
            rest = &body[end + 1..];
        } else {
            // Any other `$` (positional parameters, dollar quoting) is plain SQL
            sql.push('$');
            rest = &rest[1..];
        }
    }
    sql.push_str(rest);
    if !sql.is_empty() {
        segments.push(Segment::Sql(sql));
    }
    Ok(segments)
}

/// Finds the `}` that closes an interpolation, skipping nested braces and Rust string/char literals.
fn find_closing_brace(body: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            '"' => {
                // skip string literal
                while let Some((_, c)) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '\'' => {
                // skip char literal like '}' or '\'' (but not lifetimes like 'a)
                let lookahead: String = body[i + 1..].chars().take(4).collect();
                if let Some(len) = char_literal_len(&lookahead) {
                    for _ in 0..len {
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Number of chars after the opening quote that make up a char literal, if one starts here.
fn char_literal_len(after_quote: &str) -> Option<usize> {
    let chars: Vec<char> = after_quote.chars().collect();
    match chars.as_slice() {
        ['\\', _, '\'', ..] => Some(3),
        [c, '\'', ..] if *c != '\\' => Some(2),
        _ => None,
    }
}

fn parse_expr(source: &str, span: Span) -> syn::Result<Expr> {
    let tokens: TokenStream2 = syn::parse_str(source).map_err(|err| {
        syn::Error::new(
            span,
            format!("invalid expression `{source}` in `${{…}}`: {err}"),
        )
    })?;
    // Give all tokens the span of the string literal: they resolve at the macro call site, and type errors
    // point at the query instead of at the macro invocation as a whole.
    syn::parse2(respan(tokens, span)).map_err(|err| {
        syn::Error::new(
            span,
            format!("invalid expression `{source}` in `${{…}}`: {err}"),
        )
    })
}

fn respan(tokens: TokenStream2, span: Span) -> TokenStream2 {
    tokens
        .into_iter()
        .map(|mut tree| {
            if let TokenTree::Group(group) = &tree {
                let mut new_group = Group::new(group.delimiter(), respan(group.stream(), span));
                new_group.set_span(span);
                tree = TokenTree::Group(new_group);
            } else {
                tree.set_span(span);
            }
            tree
        })
        .collect()
}
