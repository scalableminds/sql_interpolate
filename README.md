# sql_interpolate

Postgres queries with inline value interpolation for [sqlx](https://github.com/launchbadge/sqlx), modeled on the `q"…"` interpolator in webknossos (`app/utils/sql/SqlInterpolation.scala`).

```rust
use sql_interpolate::{q, SqlEnum, SqlToken};

let filter = if only_active { q!("AND state = ${SqlEnum("active")}") } else { SqlToken::new() };
let rows: Vec<(String,)> = q!("SELECT name FROM users WHERE _organization = ${&org_id} ${filter}")
    .fetch_all(&pool)
    .await?;
```

- `${value}` becomes a bind parameter (`$1`, `$2`, …), never part of the SQL string.
- `${token}` splices in another `SqlToken`, so queries compose from fragments. Parameters are numbered only when the query is built.
- `token.debug_info()` (or `Display`) renders the query with inlined literals, for logs and pasting into an SQL client.
- Use `$${` for a literal `${`. Any other `$` (`$1`, `$$`, `$tag$`) passes through unchanged.

| Scala | Rust |
|---|---|
| `q"… $x"` | `q!("… ${x}")` (moves `x`; use `${&x}` to borrow) |
| `run(q"…".as[R])` | `q!("…").fetch_all::<R, _>(&pool)` / `fetch_one` / `fetch_optional` |
| `run(q"…".asUpdate)` | `q!("…").execute(&pool)` |
| `SqlToken.raw`, `identifier`, `joinBySeparator`, `joinByComma`, `tupleFromList` | `SqlToken::raw`, `identifier`, `join`, `join_by_comma`, `tuple` |
| `SqlValue` subclass with `placeholder = "?::JSONB"` | `impl SqlValue` with `cast() = Some("JSONB")` |
| `Enumeration#Value` | `SqlEnum("label")` (untyped, Postgres infers the enum type) |
| `EnumerationArrayValue(vs, "webknossos.STATE")` | `SqlEnumArray::new("webknossos.STATE", vs)` |
| `None` | `None` (untyped `NULL`) |

Supported values: `bool`, integers, floats, strings, `Vec<u8>`/`[u8]` (BYTEA), arrays of those, `Option<T>`, `std::time::Duration` (INTERVAL), `Bind(x)` for anything sqlx can encode, and with features `json` (`serde_json::Value` as JSONB), `chrono`, `uuid`. Implement `SqlValue` for your own types.

## Tests

```sh
cargo test --all-features                                                     # Postgres tests are skipped
DATABASE_URL=postgres://postgres@localhost/postgres cargo test --all-features
```
