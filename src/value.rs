use std::borrow::Cow;
use std::fmt::Debug;
use std::sync::Arc;

use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::types::Oid;
use sqlx::postgres::{PgArgumentBuffer, PgArguments, PgTypeInfo, Postgres};
use sqlx::{Arguments, Encode, Type};

use crate::escape::escape_literal;

/// A value that can be interpolated into a query as a bind parameter.
///
/// Implement this for your own types, or wrap them in [`Bind`] (any sqlx-encodable type)
/// or [`SqlEnum`] (Postgres enums).
pub trait SqlValue: Send + Sync {
    /// Adds the value to the query arguments.
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError>;

    /// An optional cast for the placeholder: `Some("JSONB")` yields `$1::JSONB`.
    fn cast(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// The value as an SQL literal, for [`SqlToken::debug_info`](crate::SqlToken::debug_info).
    fn debug_info(&self) -> String;
}

impl<T: SqlValue + ?Sized> SqlValue for &T {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        (**self).bind(arguments)
    }
    fn cast(&self) -> Option<Cow<'_, str>> {
        (**self).cast()
    }
    fn debug_info(&self) -> String {
        (**self).debug_info()
    }
}

impl<T: SqlValue + ?Sized> SqlValue for Box<T> {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        (**self).bind(arguments)
    }
    fn cast(&self) -> Option<Cow<'_, str>> {
        (**self).cast()
    }
    fn debug_info(&self) -> String {
        (**self).debug_info()
    }
}

impl<T: SqlValue + ?Sized> SqlValue for Arc<T> {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        (**self).bind(arguments)
    }
    fn cast(&self) -> Option<Cow<'_, str>> {
        (**self).cast()
    }
    fn debug_info(&self) -> String {
        (**self).debug_info()
    }
}

/// `None` is bound as an untyped `NULL`, so Postgres infers its type from the context.
impl<T: SqlValue> SqlValue for Option<T> {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        match self {
            Some(value) => value.bind(arguments),
            None => arguments.add(Untyped(None)),
        }
    }
    fn cast(&self) -> Option<Cow<'_, str>> {
        self.as_ref().and_then(|value| value.cast())
    }
    fn debug_info(&self) -> String {
        match self {
            Some(value) => value.debug_info(),
            None => "NULL".to_owned(),
        }
    }
}

macro_rules! impl_sql_value_for_integers {
    ($($t:ty),*) => {$(
        impl SqlValue for $t {
            fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
                arguments.add(*self)
            }
            fn debug_info(&self) -> String {
                self.to_string()
            }
        }
    )*};
}
impl_sql_value_for_integers!(bool, i8, i16, i32, i64);

macro_rules! impl_sql_value_for_floats {
    ($($t:ty),*) => {$(
        impl SqlValue for $t {
            fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
                arguments.add(*self)
            }
            fn debug_info(&self) -> String {
                // NaN and infinities are only valid SQL as quoted literals
                if self.is_finite() { self.to_string() } else { format!("'{self}'") }
            }
        }
    )*};
}
impl_sql_value_for_floats!(f32, f64);

impl SqlValue for str {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(self)
    }
    fn debug_info(&self) -> String {
        escape_literal(self)
    }
}

impl SqlValue for String {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(self.as_str())
    }
    fn debug_info(&self) -> String {
        escape_literal(self)
    }
}

impl SqlValue for Cow<'_, str> {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(self.as_ref())
    }
    fn debug_info(&self) -> String {
        escape_literal(self)
    }
}

impl SqlValue for [u8] {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(self)
    }
    fn debug_info(&self) -> String {
        let hex: String = self.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("'\\x{hex}'::BYTEA")
    }
}

impl SqlValue for Vec<u8> {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        self.as_slice().bind(arguments)
    }
    fn debug_info(&self) -> String {
        self.as_slice().debug_info()
    }
}

/// Bound as `INTERVAL`.
impl SqlValue for std::time::Duration {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(*self)
    }
    fn debug_info(&self) -> String {
        format!("'{} microseconds'::INTERVAL", self.as_micros())
    }
}

/// Postgres arrays, e.g. for `WHERE _id = ANY(${ids})`.
macro_rules! impl_sql_value_for_arrays {
    ($($t:ty),*) => {$(
        impl SqlValue for [$t] {
            fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
                arguments.add(self)
            }
            fn debug_info(&self) -> String {
                array_debug_info(self)
            }
        }

        impl SqlValue for Vec<$t> {
            fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
                self.as_slice().bind(arguments)
            }
            fn debug_info(&self) -> String {
                self.as_slice().debug_info()
            }
        }
    )*};
}
impl_sql_value_for_arrays!(String, &str, bool, i16, i32, i64, f32, f64);

fn array_debug_info<T: SqlValue>(values: &[T]) -> String {
    if values.is_empty() {
        return "'{}'".to_owned();
    }
    let elements: Vec<String> = values.iter().map(SqlValue::debug_info).collect();
    format!("ARRAY[{}]", elements.join(", "))
}

/// A value of a Postgres enum type, given as its label.
///
/// It is bound as untyped text, so Postgres infers the enum type from the context, as in
/// `WHERE state = ${SqlEnum("active")}`. Where there is no context, add a cast to the SQL.
///
/// To interpolate your own enum types directly, implement [`SqlValue`] by delegating to `SqlEnum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlEnum<S>(pub S);

impl<S: AsRef<str> + Send + Sync> SqlValue for SqlEnum<S> {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(Untyped(Some(self.0.as_ref())))
    }
    fn debug_info(&self) -> String {
        escape_literal(self.0.as_ref())
    }
}

/// An array of Postgres enum labels, bound as `TEXT[]` and cast to `type_name[]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlEnumArray {
    type_name: String,
    labels: Vec<String>,
}

impl SqlEnumArray {
    /// `type_name` is included verbatim in the SQL. Never pass user input here.
    pub fn new<S: AsRef<str>>(
        type_name: impl Into<String>,
        labels: impl IntoIterator<Item = S>,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            labels: labels
                .into_iter()
                .map(|label| label.as_ref().to_owned())
                .collect(),
        }
    }
}

impl SqlValue for SqlEnumArray {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(self.labels.as_slice())
    }
    fn cast(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Owned(format!("{}[]", self.type_name)))
    }
    fn debug_info(&self) -> String {
        array_debug_info(&self.labels)
    }
}

/// Binds any value that sqlx can encode, e.g. types with `#[derive(sqlx::Type)]`.
///
/// Its debug info is based on [`Debug`], so it is not necessarily valid SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bind<T>(pub T);

impl<T> SqlValue for Bind<T>
where
    T: for<'q> Encode<'q, Postgres> + Type<Postgres> + Debug + Send + Sync,
{
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(&self.0)
    }
    fn debug_info(&self) -> String {
        format!("{:?}", self.0)
    }
}

/// Bound as text and cast to `JSONB`.
#[cfg(feature = "json")]
impl SqlValue for serde_json::Value {
    fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
        arguments.add(self.to_string())
    }
    fn cast(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed("JSONB"))
    }
    fn debug_info(&self) -> String {
        escape_literal(&self.to_string())
    }
}

#[cfg(feature = "chrono")]
mod chrono_impls {
    use super::*;
    use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

    impl SqlValue for DateTime<Utc> {
        fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
            arguments.add(*self)
        }
        fn debug_info(&self) -> String {
            format!("'{}'::TIMESTAMPTZ", self.to_rfc3339())
        }
    }

    impl SqlValue for NaiveDateTime {
        fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
            arguments.add(*self)
        }
        fn debug_info(&self) -> String {
            format!("'{self}'::TIMESTAMP")
        }
    }

    impl SqlValue for NaiveDate {
        fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
            arguments.add(*self)
        }
        fn debug_info(&self) -> String {
            format!("'{self}'::DATE")
        }
    }
}

#[cfg(feature = "uuid")]
mod uuid_impls {
    use super::*;
    use uuid::Uuid;

    impl SqlValue for Uuid {
        fn bind(&self, arguments: &mut PgArguments) -> Result<(), BoxDynError> {
            arguments.add(*self)
        }
        fn debug_info(&self) -> String {
            format!("'{self}'::UUID")
        }
    }

    impl_sql_value_for_arrays!(Uuid);
}

/// Text (or `NULL`) with an unspecified type, which Postgres infers from the context.
struct Untyped<'a>(Option<&'a str>);

impl Type<Postgres> for Untyped<'_> {
    fn type_info() -> PgTypeInfo {
        // OID 0 leaves the parameter type unspecified
        PgTypeInfo::with_oid(Oid(0))
    }
}

impl Encode<'_, Postgres> for Untyped<'_> {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        match self.0 {
            // Postgres expects the binary format, which is the plain label for enums
            Some(text) => {
                buf.extend_from_slice(text.as_bytes());
                Ok(IsNull::No)
            }
            None => Ok(IsNull::Yes),
        }
    }
}
