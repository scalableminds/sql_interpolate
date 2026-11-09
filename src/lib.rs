//! SQL queries for Postgres with inline value interpolation, on top of [sqlx].
//!
//! ```no_run
//! use sql_interpolate::{q, SqlToken};
//!
//! # async fn example(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
//! let user_id: i64 = 42;
//! let states = vec!["active".to_string(), "pending".to_string()];
//! let filter = q!("AND created > now() - ${std::time::Duration::from_secs(3600)}");
//!
//! let names: Vec<(String,)> = q!(
//!     "SELECT name FROM users
//!      WHERE _owner = ${user_id} AND state = ANY(${&states}) ${filter}"
//! )
//! .fetch_all(pool)
//! .await?;
//! # Ok(())
//! # }
//! ```
//!
//! Interpolated values are never put into the SQL string. Each `${value}` becomes a bind parameter
//! (`$1`, `$2`, …). Interpolated [`SqlToken`]s are spliced in, so queries compose from fragments.
//! The parameters are numbered only when the query is built, so fragments can be freely reused.
//!
//! Run queries with [`SqlToken::fetch_all`], [`fetch_one`](SqlToken::fetch_one),
//! [`fetch_optional`](SqlToken::fetch_optional) and [`execute`](SqlToken::execute), or get a regular sqlx query
//! with [`query`](SqlToken::query), [`query_as`](SqlToken::query_as) and [`query_scalar`](SqlToken::query_scalar),
//! e.g. for streaming.
//!
//! What can be interpolated:
//! - values implementing [`SqlValue`]: numbers, `bool`, strings, byte arrays, arrays, `Option`,
//!   [`std::time::Duration`], [`SqlEnum`], [`Bind`] (for any sqlx-encodable type) and, with features,
//!   `serde_json::Value`, `chrono` and `uuid` types. References to these work, too.
//! - other [`SqlToken`]s, by value or by reference.
//!
//! Use `$${` for a literal `${` in the SQL. Any other `$` is passed through unchanged.

extern crate self as sql_interpolate;

mod escape;
mod token;
mod value;

pub use sql_interpolate_macros::q;
pub use token::{IntoSqlToken, SqlToken};
pub use value::{Bind, SqlEnum, SqlEnumArray, SqlValue};
