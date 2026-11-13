# sql_interpolate

Postgres queries with inline value interpolation for [sqlx](https://github.com/launchbadge/sqlx).

```rust
use sql_interpolate::q;

let rows: Vec<(String,)> = q!("SELECT name FROM users WHERE _organization = ${&org_id} AND age >= ${min_age}")
    .fetch_all(&pool)
    .await?;
```

Values are never put into the SQL string. Each `${value}` becomes a bind parameter (`$1`, `$2`, …), so interpolation is safe from SQL injection. Queries are built at runtime, so they can be composed from fragments.

## Installation

```toml
[dependencies]
sql_interpolate = { version = "0.1", features = ["json", "chrono", "uuid"] }
sqlx = { version = "0.9", features = ["postgres", "runtime-tokio"] }
```

| Feature | Adds support for |
|---|---|
| `json` | `serde_json::Value` (bound as `JSONB`) |
| `chrono` | `DateTime<Utc>`, `NaiveDateTime`, `NaiveDate` |
| `uuid` | `Uuid` and `Vec<Uuid>` |

## Usage

### Writing queries

`q!` takes a string literal and returns a `SqlToken`. Anything between `${` and `}` is a Rust expression:

```rust
let token = q!("SELECT * FROM users WHERE _id = ${user.id} AND created > ${since}");
```

Interpolation moves the value into the token, like a function argument would. Use `${&x}` to borrow instead; the token then can't outlive `x`.

To write a literal `${` in the SQL, use `$${`. Any other `$` (`$1`, `$$`, `$tag$`) passes through unchanged, so dollar-quoted strings work as usual.

### Running queries

Run a `SqlToken` against anything that implements sqlx's `Executor` (a pool, a connection or a transaction):

```rust
#[derive(sqlx::FromRow)]
struct User {
    id: i64,
    name: String,
}

// all rows, decoded into a FromRow struct or a tuple
let users: Vec<User> = q!("SELECT id, name FROM users").fetch_all(&pool).await?;

// exactly one row (errors with RowNotFound otherwise)
let (count,): (i64,) = q!("SELECT COUNT(*) FROM users").fetch_one(&pool).await?;

// zero or one row
let user: Option<User> = q!("SELECT id, name FROM users WHERE id = ${id}").fetch_optional(&pool).await?;

// number of affected rows
let deleted: u64 = q!("DELETE FROM sessions WHERE expires < now()").execute(&pool).await?;
```

For anything else, `query()`, `query_as::<R>()` and `query_scalar::<R>()` return regular sqlx queries, e.g. for streaming:

```rust
let mut rows = q!("SELECT id FROM users").query_scalar::<i64>()?.fetch(&pool);
```

Transactions work like in sqlx:

```rust
let mut tx = pool.begin().await?;
q!("UPDATE accounts SET balance = balance - ${amount} WHERE id = ${from}").execute(&mut *tx).await?;
q!("UPDATE accounts SET balance = balance + ${amount} WHERE id = ${to}").execute(&mut *tx).await?;
tx.commit().await?;
```

### Composing queries

Interpolating a `SqlToken` splices it into the query instead of binding it. Parameters are numbered only when the query is run, so fragments can be built separately and reused:

```rust
use sql_interpolate::{q, SqlToken};

let state_filter = match state {
    Some(state) => q!("AND state = ${state}"),
    None => SqlToken::new(),
};
let order = if newest_first { q!("ORDER BY created DESC") } else { q!("ORDER BY created") };

let users: Vec<User> = q!("SELECT id, name FROM users WHERE _organization = ${&org_id} ${state_filter} ${order}")
    .fetch_all(&pool)
    .await?;
```

Reusable fragments are just functions returning a `SqlToken`:

```rust
fn readable_by(user_id: i64) -> SqlToken<'static> {
    q!("(_owner = ${user_id} OR is_public)")
}

q!("SELECT id FROM datasets WHERE ${readable_by(user_id)} AND name = ${name}")
```

`SqlToken` also has helpers for building SQL dynamically:

| Helper | Result |
|---|---|
| `SqlToken::raw("NOW()")` | SQL included verbatim. Never pass user input. |
| `SqlToken::identifier("webknossos.users")` | `"webknossos"."users"` (quoted identifier) |
| `SqlToken::value(x)` | a single bind parameter |
| `SqlToken::join(items, " AND ")` | tokens or values joined by a separator |
| `SqlToken::join_by_comma(items)` | tokens or values joined by `, ` |
| `SqlToken::tuple(&ids)` | `($1, $2, $3)`, e.g. for `IN` (`IN ()` is invalid, so handle empty lists or use `= ANY(${ids})`) |
| `token.push(x)` / `token.push_sql("…")` | append a token or value / raw SQL |

For example, a dynamic `WHERE` clause:

```rust
let mut conditions = vec![q!("_organization = ${&org_id}")];
if let Some(name) = &name {
    conditions.push(q!("name ILIKE ${format!(\"%{name}%\")}"));
}
if let Some(ids) = &ids {
    conditions.push(q!("id = ANY(${ids})"));
}
let query = q!("SELECT id, name FROM users WHERE ${SqlToken::join(conditions, \" AND \")}");
```

And a multi-row insert:

```rust
let rows = users.iter().map(|u| q!("(${u.id}, ${&u.name})"));
q!("INSERT INTO users (id, name) VALUES ${SqlToken::join_by_comma(rows)}").execute(&pool).await?;
```

### Debugging

`token.debug_info()` (or `Display`/`Debug`) renders the query with all values inlined as SQL literals, for logs and for pasting into an SQL client. `token.sql()` shows the SQL as it is sent to the database:

```rust
let token = q!("SELECT * FROM users WHERE name = ${\"O'Brien\"} AND id = ANY(${vec![1i64, 2]})");
token.sql();        // SELECT * FROM users WHERE name = $1 AND id = ANY($2)
token.debug_info(); // SELECT * FROM users WHERE name = 'O''Brien' AND id = ANY(ARRAY[1, 2])
```

The debug output is not meant to be executed.

## Supported values

| Rust type | Postgres |
|---|---|
| `bool` | `BOOLEAN` |
| `i8`, `i16`, `i32`, `i64` | `"char"`, `SMALLINT`, `INTEGER`, `BIGINT` |
| `f32`, `f64` | `REAL`, `DOUBLE PRECISION` |
| `String`, `&str`, `Cow<str>` | `TEXT` |
| `Vec<u8>`, `&[u8]` | `BYTEA` |
| `Vec<T>`, `&[T]` for `T` in `String`, `&str`, `bool`, `i16`, `i32`, `i64`, `f32`, `f64`, `Uuid` | arrays, e.g. for `= ANY(${ids})` |
| `Option<T>` | `T`, or an untyped `NULL` whose type Postgres infers |
| `std::time::Duration` | `INTERVAL` |
| `SqlEnum("label")` | a Postgres enum value; its type is inferred from the context |
| `SqlEnumArray::new("schema.enum_type", labels)` | `schema.enum_type[]` |
| `Bind(x)` | anything sqlx can encode for Postgres |
| `serde_json::Value` (feature `json`) | `JSONB` |
| `DateTime<Utc>`, `NaiveDateTime`, `NaiveDate` (feature `chrono`) | `TIMESTAMPTZ`, `TIMESTAMP`, `DATE` |
| `Uuid` (feature `uuid`) | `UUID` |

References, `Box<T>` and `Arc<T>` of supported values work, too.

## Custom types

There are two ways to interpolate your own types.

### `Bind`: anything sqlx can encode

Types that implement sqlx's `Encode` and `Type` (e.g. with `#[derive(sqlx::Type)]`) and `Debug` can be wrapped in `Bind`:

```rust
use sql_interpolate::{q, Bind};

#[derive(Debug, sqlx::Type)]
#[sqlx(transparent)]
struct UserId(i64);

q!("SELECT * FROM users WHERE id = ${Bind(user_id)}")
```

This is quick, but you have to write `Bind(…)` everywhere, and `debug_info` uses the `Debug` output, which is not necessarily valid SQL.

### Implementing `SqlValue`

To interpolate a type directly and get proper debug output, implement `SqlValue`. It has three methods:

- `bind` adds the value to the query arguments,
- `debug_info` renders the value as an SQL literal for `token.debug_info()`,
- `cast` (optional) adds a cast to the placeholder, e.g. `Some("JSONB")` renders `$1::JSONB`.

Usually, `bind` and `debug_info` delegate to a type that is already supported. A newtype ID:

```rust
use sql_interpolate::SqlValue;
use sqlx::error::BoxDynError;
use sqlx::postgres::PgArguments;

struct ObjectId(String);

impl SqlValue for ObjectId {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        self.0.bind(arguments)
    }
    fn debug_info(&self) -> String {
        self.0.debug_info()
    }
}

q!("SELECT * FROM datasets WHERE _id = ${&dataset_id}")
```

A Rust enum mapped to a Postgres enum, delegating to `SqlEnum`:

```rust
use sql_interpolate::{SqlEnum, SqlValue};

enum State {
    Active,
    Finished,
}

impl State {
    fn label(&self) -> &'static str {
        match self {
            State::Active => "active",
            State::Finished => "finished",
        }
    }
}

impl SqlValue for State {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        SqlEnum(self.label()).bind(arguments)
    }
    fn debug_info(&self) -> String {
        SqlEnum(self.label()).debug_info()
    }
}

q!("UPDATE tasks SET state = ${State::Finished} WHERE id = ${id}")
```

A type stored as JSON, bound as text and cast in SQL with `cast`:

```rust
use std::borrow::Cow;

#[derive(serde::Serialize)]
struct Settings {
    theme: String,
    notifications: bool,
}

impl SqlValue for Settings {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        serde_json::to_string(self)?.bind(arguments)
    }
    fn cast(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed("JSONB"))
    }
    fn debug_info(&self) -> String {
        serde_json::to_string(self).unwrap_or_default().debug_info()
    }
}

// UPDATE users SET settings = $1::JSONB WHERE id = $2
q!("UPDATE users SET settings = ${&settings} WHERE id = ${id}")
```

`bind` can also call sqlx directly with `sqlx::Arguments::add(arguments, value)` for any type sqlx can encode. Once a type implements `SqlValue`, `Option<T>`, `&T`, `Box<T>` and `Arc<T>` of it work automatically.

## Tests

```sh
cargo test --all-features                                                     # Postgres tests are skipped
DATABASE_URL=postgres://postgres@localhost/postgres cargo test --all-features
```
