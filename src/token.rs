use std::borrow::Cow;
use std::fmt::{self, Write};
use std::sync::Arc;

use sqlx::error::BoxDynError;
use sqlx::postgres::PgRow;
use sqlx::postgres::{PgArguments, Postgres};
use sqlx::query::{Query, QueryAs, QueryScalar};
use sqlx::{AssertSqlSafe, Executor, FromRow};

use crate::escape::quote_identifier;
use crate::value::{Scalar, SqlValue};

/// A piece of SQL together with the values bound in it. Usually built with [`q!`](crate::q).
///
/// Cloning is cheap: values are reference-counted, not copied.
#[derive(Clone, Default)]
pub struct SqlToken<'a> {
    parts: Vec<Part<'a>>,
}

#[doc(hidden)]
#[derive(Clone)]
pub enum Part<'a> {
    Sql(Cow<'a, str>),
    /// An owned value, reference-counted so that tokens stay cheap to clone.
    Owned(Arc<dyn SqlValue + 'a>),
    /// A borrowed value: interpolating references allocates nothing.
    Borrowed(&'a (dyn SqlValue + 'a)),
    Str(&'a str),
    /// A small `Copy` value or `NULL`, stored inline.
    Scalar(Scalar),
}

impl<'a> Part<'a> {
    pub(crate) fn owned(value: impl SqlValue + 'a) -> Self {
        Part::Owned(Arc::new(value))
    }

    fn value(&self) -> Option<&dyn SqlValue> {
        match self {
            Part::Sql(_) => None,
            Part::Owned(value) => Some(value.as_ref()),
            Part::Borrowed(value) => Some(*value),
            Part::Str(value) => Some(value),
            Part::Scalar(value) => Some(value),
        }
    }
}

impl<'a> SqlToken<'a> {
    pub fn new() -> Self {
        Self { parts: Vec::new() }
    }

    /// Used by [`q!`](crate::q), which knows how many parts it pushes.
    #[doc(hidden)]
    pub fn with_capacity(parts: usize) -> Self {
        Self {
            parts: Vec::with_capacity(parts),
        }
    }

    /// SQL that is included verbatim. Never pass user input here.
    pub fn raw(sql: impl Into<Cow<'a, str>>) -> Self {
        let mut token = Self::new();
        token.push_sql(sql);
        token
    }

    /// A single bind parameter.
    pub fn value(value: impl SqlValue + 'a) -> Self {
        let mut token = Self::new();
        token.parts.push(value.__into_part());
        token
    }

    /// A quoted identifier, e.g. `webknossos.annotations` becomes `"webknossos"."annotations"`.
    pub fn identifier(id: &str) -> Self {
        Self::raw(quote_identifier(id))
    }

    /// Joins tokens (or values) with a separator that is included verbatim.
    pub fn join<T: IntoSqlToken<'a>>(
        items: impl IntoIterator<Item = T>,
        separator: &'a str,
    ) -> Self {
        let items = items.into_iter();
        let mut token = Self::with_capacity(2 * items.size_hint().0);
        for (i, item) in items.enumerate() {
            if i > 0 {
                token.push_sql(separator);
            }
            item.append_to(&mut token);
        }
        token
    }

    pub fn join_by_comma<T: IntoSqlToken<'a>>(items: impl IntoIterator<Item = T>) -> Self {
        Self::join(items, ", ")
    }

    /// A parenthesized, comma-separated list, e.g. for `WHERE _id IN ${SqlToken::tuple(&ids)}`.
    ///
    /// Note that `IN ()` is invalid SQL, so handle empty lists separately (or use `= ANY(${array})`).
    pub fn tuple<T: IntoSqlToken<'a>>(items: impl IntoIterator<Item = T>) -> Self {
        let list = Self::join_by_comma(items);
        let mut token = Self::with_capacity(list.parts.len() + 2);
        token.push_sql("(");
        list.append_to(&mut token);
        token.push_sql(")");
        token
    }

    pub fn push_sql(&mut self, sql: impl Into<Cow<'a, str>>) {
        let sql = sql.into();
        if !sql.is_empty() {
            self.parts.push(Part::Sql(sql));
        }
    }

    /// Appends another token or a value.
    pub fn push(&mut self, item: impl IntoSqlToken<'a>) {
        item.append_to(self);
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The SQL with numbered placeholders (`$1`, `$2`, …) as it is sent to the database.
    pub fn sql(&self) -> String {
        let mut sql = String::new();
        let mut index = 0;
        for part in &self.parts {
            match part.value() {
                Some(value) => {
                    index += 1;
                    push_placeholder(&mut sql, index, value);
                }
                None => push_sql_part(&mut sql, part),
            }
        }
        sql
    }

    /// The SQL and its bind arguments, for use with [`sqlx::query_with`] and friends.
    pub fn build(&self) -> Result<(String, PgArguments), BoxDynError> {
        let (sql_len, value_count) =
            self.parts
                .iter()
                .fold((0, 0), |(len, count), part| match part {
                    Part::Sql(s) => (len + s.len(), count),
                    _ => (len + 4, count + 1),
                });
        let mut sql = String::with_capacity(sql_len);
        let mut arguments = PgArguments::default();
        // the size is only a hint for the argument buffer
        sqlx::Arguments::reserve(&mut arguments, value_count, value_count * 32);
        let mut index = 0;
        for part in &self.parts {
            match part.value() {
                Some(value) => {
                    index += 1;
                    push_placeholder(&mut sql, index, value);
                    value.bind(&mut arguments)?;
                }
                None => push_sql_part(&mut sql, part),
            }
        }
        Ok((sql, arguments))
    }

    /// The SQL with all values inlined as literals. Meant for logging and for pasting into an SQL client,
    /// never for execution.
    pub fn debug_info(&self) -> String {
        let mut sql = String::new();
        for part in &self.parts {
            match part.value() {
                None => push_sql_part(&mut sql, part),
                Some(value) => {
                    sql.push_str(&value.debug_info());
                    if let Some(cast) = value.cast() {
                        sql.push_str("::");
                        sql.push_str(&cast);
                    }
                }
            }
        }
        sql
    }

    /// A query whose result rows are not decoded, e.g. for `.execute(…)`.
    pub fn query(&self) -> Result<Query<'static, Postgres, PgArguments>, sqlx::Error> {
        let (sql, arguments) = self.build().map_err(sqlx::Error::Encode)?;
        Ok(sqlx::query_with(AssertSqlSafe(sql), arguments))
    }

    /// A query whose result rows are decoded into `R` (a [`FromRow`] struct or a tuple).
    pub fn query_as<R>(&self) -> Result<QueryAs<'static, Postgres, R, PgArguments>, sqlx::Error>
    where
        R: for<'r> FromRow<'r, PgRow>,
    {
        let (sql, arguments) = self.build().map_err(sqlx::Error::Encode)?;
        Ok(sqlx::query_as_with(AssertSqlSafe(sql), arguments))
    }

    /// A query whose result rows have a single column, decoded into `R`.
    pub fn query_scalar<R>(
        &self,
    ) -> Result<QueryScalar<'static, Postgres, R, PgArguments>, sqlx::Error>
    where
        (R,): for<'r> FromRow<'r, PgRow>,
    {
        let (sql, arguments) = self.build().map_err(sqlx::Error::Encode)?;
        Ok(sqlx::query_scalar_with(AssertSqlSafe(sql), arguments))
    }

    /// Runs the query and decodes all rows into `R` (a [`FromRow`] struct or a tuple).
    pub async fn fetch_all<'c, R, E>(&self, executor: E) -> Result<Vec<R>, sqlx::Error>
    where
        R: for<'r> FromRow<'r, PgRow> + Send + Unpin,
        E: Executor<'c, Database = Postgres>,
    {
        self.query_as()?.fetch_all(executor).await
    }

    /// Runs the query and decodes the first row, failing with [`sqlx::Error::RowNotFound`] if there is none.
    pub async fn fetch_one<'c, R, E>(&self, executor: E) -> Result<R, sqlx::Error>
    where
        R: for<'r> FromRow<'r, PgRow> + Send + Unpin,
        E: Executor<'c, Database = Postgres>,
    {
        self.query_as()?.fetch_one(executor).await
    }

    /// Runs the query and decodes the first row, if any.
    pub async fn fetch_optional<'c, R, E>(&self, executor: E) -> Result<Option<R>, sqlx::Error>
    where
        R: for<'r> FromRow<'r, PgRow> + Send + Unpin,
        E: Executor<'c, Database = Postgres>,
    {
        self.query_as()?.fetch_optional(executor).await
    }

    /// Runs the query and returns the number of affected rows.
    pub async fn execute<'c, E>(&self, executor: E) -> Result<u64, sqlx::Error>
    where
        E: Executor<'c, Database = Postgres>,
    {
        Ok(self.query()?.execute(executor).await?.rows_affected())
    }
}

fn push_sql_part(sql: &mut String, part: &Part<'_>) {
    if let Part::Sql(s) = part {
        sql.push_str(s);
    }
}

fn push_placeholder(sql: &mut String, index: usize, value: &dyn SqlValue) {
    // writing to a String cannot fail
    let _ = write!(sql, "${index}");
    if let Some(cast) = value.cast() {
        sql.push_str("::");
        sql.push_str(&cast);
    }
}

impl fmt::Debug for SqlToken<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SqlToken").field(&self.debug_info()).finish()
    }
}

impl fmt::Display for SqlToken<'_> {
    /// Same as [`SqlToken::debug_info`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.debug_info())
    }
}

/// Anything that can be interpolated with `${…}` in [`q!`](crate::q): [`SqlToken`]s are spliced in,
/// [`SqlValue`]s become bind parameters.
pub trait IntoSqlToken<'a> {
    fn append_to(self, token: &mut SqlToken<'a>);

    /// How many parts [`append_to`](Self::append_to) adds, so tokens can be allocated with their final size.
    fn part_count(&self) -> usize;

    /// Used by [`q!`](crate::q): appends, first reserving room for `remaining` more parts after this one.
    #[doc(hidden)]
    fn __append(self, token: &mut SqlToken<'a>, remaining: usize)
    where
        Self: Sized,
    {
        let needed = self.part_count() + remaining;
        let spare = token.parts.capacity() - token.parts.len();
        if needed > spare {
            token.parts.reserve_exact(needed);
        }
        self.append_to(token);
    }
}

impl<'a> IntoSqlToken<'a> for SqlToken<'a> {
    fn append_to(self, token: &mut SqlToken<'a>) {
        token.parts.extend(self.parts);
    }
    fn part_count(&self) -> usize {
        self.parts.len()
    }
}

impl<'a> IntoSqlToken<'a> for &SqlToken<'a> {
    fn append_to(self, token: &mut SqlToken<'a>) {
        token.parts.extend(self.parts.iter().cloned());
    }
    fn part_count(&self) -> usize {
        self.parts.len()
    }
}

impl<'a, T: SqlValue + 'a> IntoSqlToken<'a> for T {
    fn append_to(self, token: &mut SqlToken<'a>) {
        token.parts.push(self.__into_part());
    }
    fn part_count(&self) -> usize {
        1
    }
}
