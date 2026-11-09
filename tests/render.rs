use std::collections::HashMap;
use std::time::Duration;

use sql_interpolate::{Bind, SqlEnum, SqlEnumArray, SqlToken, q};

#[test]
fn values_become_numbered_placeholders() {
    let id = 42i64;
    let name = "it's".to_string();
    let token = q!("SELECT * FROM users WHERE _id = ${id} AND name = ${&name}");
    assert_eq!(
        token.sql(),
        "SELECT * FROM users WHERE _id = $1 AND name = $2"
    );
    assert_eq!(
        token.debug_info(),
        "SELECT * FROM users WHERE _id = 42 AND name = 'it''s'"
    );
    // name was only borrowed
    assert_eq!(name, "it's");
}

#[test]
fn tokens_are_spliced_and_renumbered() {
    let filter = q!("AND a = ${1i32} AND b = ${2i32}");
    let query =
        q!("SELECT ${0i32} WHERE true ${&filter} UNION SELECT ${3i32} WHERE true ${filter}");
    assert_eq!(
        query.sql(),
        "SELECT $1 WHERE true AND a = $2 AND b = $3 UNION SELECT $4 WHERE true AND a = $5 AND b = $6"
    );
}

#[test]
fn literal_dollars_pass_through() {
    let token = q!("SELECT $1, $body$ x $body$, $$, '$${not interpolated}', ${5i32}");
    assert_eq!(
        token.sql(),
        "SELECT $1, $body$ x $body$, $$, '${not interpolated}', $1"
    );
}

#[test]
fn interpolations_can_be_arbitrary_expressions() {
    let flag = true;
    let map = HashMap::from([("}", 7i32)]);
    let token = q!(
        r#"SELECT ${if flag { 1i32 } else { 2i32 }}, ${map.get("}").copied()}, ${'}'.to_string()}"#
    );
    assert_eq!(token.debug_info(), "SELECT 1, 7, '}'");
}

#[test]
fn casts_are_appended_to_placeholders() {
    let token = q!(
        "SELECT ${SqlEnumArray::new(\"webknossos.state\", [\"a\", \"b\"])}, ${Duration::from_millis(1500)}"
    );
    assert_eq!(token.sql(), "SELECT $1::webknossos.state[], $2");
    assert_eq!(
        token.debug_info(),
        "SELECT ARRAY['a', 'b']::webknossos.state[], '1500000 microseconds'::INTERVAL"
    );
}

#[test]
fn debug_info_covers_value_types() {
    let none: Option<i32> = None;
    let token = q!(
        "${none} ${Some(3i16)} ${f64::NAN} ${vec![1u8, 255]} ${Vec::<String>::new()} ${SqlEnum(\"active\")} ${Bind(1u32 as i64)}"
    );
    assert_eq!(
        token.debug_info(),
        r"NULL 3 'NaN' '\x01ff'::BYTEA '{}' 'active' 1"
    );
}

#[test]
fn helpers_build_fragments() {
    let ids = [1i64, 2, 3];
    let token = q!(
        "SELECT * FROM ${SqlToken::identifier(\"webknossos.users\")} WHERE _id IN ${SqlToken::tuple(ids)}"
    );
    assert_eq!(
        token.sql(),
        r#"SELECT * FROM "webknossos"."users" WHERE _id IN ($1, $2, $3)"#
    );

    let assignments = SqlToken::join_by_comma([q!("a = ${1i32}"), q!("b = ${2i32}")]);
    assert_eq!(
        q!("UPDATE t SET ${assignments}").sql(),
        "UPDATE t SET a = $1, b = $2"
    );

    let mut token = SqlToken::raw("SELECT 1");
    token.push(q!(" WHERE x = ${1i32}"));
    assert!(SqlToken::new().is_empty());
    assert_eq!(token.sql(), "SELECT 1 WHERE x = $1");
}

fn fragment_from_function(name: &str) -> SqlToken<'_> {
    q!("name = ${name}")
}

#[test]
fn tokens_can_borrow_from_their_context() {
    let name = String::from("bob");
    assert_eq!(
        q!("WHERE ${fragment_from_function(&name)}").debug_info(),
        "WHERE name = 'bob'"
    );
}
