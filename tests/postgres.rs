//! Runs against a real database. Set `DATABASE_URL` to enable, e.g.
//! `DATABASE_URL=postgres://postgres@localhost/postgres cargo test --all-features`.
//! All changes happen in a transaction that is rolled back.

use std::time::Duration;

use sql_interpolate::{SqlEnum, SqlEnumArray, SqlToken, q};
use sqlx::{Connection, PgConnection};

async fn connect() -> Option<PgConnection> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set, skipping");
        return None;
    };
    Some(
        PgConnection::connect(&url)
            .await
            .expect("could not connect to DATABASE_URL"),
    )
}

#[tokio::test]
async fn round_trips_values() {
    let Some(mut conn) = connect().await else {
        return;
    };

    let name = "it's a \\ test";
    let row: (i32, i64, f64, bool, String, Vec<u8>, Vec<String>) = q!("SELECT ${7i32}, ${8i64}, ${1.5f64}, ${true}, ${name}, ${vec![0u8, 255]}, ${vec![\"a\".to_string(), \"b\".to_string()]}")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(
        row,
        (
            7,
            8,
            1.5,
            true,
            name.to_string(),
            vec![0, 255],
            vec!["a".to_string(), "b".to_string()]
        )
    );

    let seconds: f64 = q!("SELECT extract(epoch FROM ${Duration::from_millis(1500)})::float8")
        .query_scalar()
        .unwrap()
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(seconds, 1.5);
}

#[tokio::test]
async fn composes_fragments() {
    let Some(mut conn) = connect().await else {
        return;
    };

    let ids = vec![2i32, 3];
    let filter = q!("AND x = ANY(${&ids})");
    let rows: Vec<(i32,)> =
        q!("SELECT x FROM generate_series(1, ${5i32}) x WHERE x >= ${2i32} ${filter} ORDER BY x")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert_eq!(rows, vec![(2,), (3,)]);

    let rows: Vec<(i32,)> =
        q!("SELECT x FROM generate_series(1, 5) x WHERE x IN ${SqlToken::tuple([1i32, 4])}")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    assert_eq!(rows, vec![(1,), (4,)]);
}

#[tokio::test]
async fn binds_enums_nulls_and_json() {
    let Some(mut conn) = connect().await else {
        return;
    };
    let mut tx = conn.begin().await.unwrap();

    q!("CREATE TYPE pg_temp.sql_interpolate_state AS ENUM ('active', 'finished')")
        .execute(&mut *tx)
        .await
        .unwrap();
    q!("CREATE TEMPORARY TABLE items (id INT, state pg_temp.sql_interpolate_state, note TEXT, meta JSONB)")
        .execute(&mut *tx)
        .await
        .unwrap();

    let no_note: Option<String> = None;
    let meta = serde_json::json!({"a": [1, 2]});
    let inserted = q!("INSERT INTO items VALUES (1, ${SqlEnum(\"active\")}, ${&no_note}, ${&meta}), (2, ${SqlEnum(\"finished\")}, ${Some(\"x\")}, NULL)")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(inserted, 2);

    let active: Vec<(i32,)> = q!("SELECT id FROM items WHERE state = ${SqlEnum(\"active\")} AND note IS NOT DISTINCT FROM ${no_note}")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(active, vec![(1,)]);

    let states = SqlEnumArray::new("pg_temp.sql_interpolate_state", ["active", "finished"]);
    let count: i64 = q!("SELECT count(*) FROM items WHERE state = ANY(${states})")
        .query_scalar()
        .unwrap()
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 2);

    let first: Option<(i32,)> = q!(
        "SELECT (meta->'a'->>0)::INT FROM items WHERE meta @> ${serde_json::json!({\"a\": [2]})}"
    )
    .fetch_optional(&mut *tx)
    .await
    .unwrap();
    assert_eq!(first, Some((1,)));

    tx.rollback().await.unwrap();
}
