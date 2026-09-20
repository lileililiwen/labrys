use serde::{de::DeserializeOwned, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::error::Result;

/// Serializes a `labrys-core` contract into a JSONB value for storage.
pub fn to_value<T: Serialize>(value: &T) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(value)?)
}

/// Serializes an optional contract into a JSONB value, mapping `None` to a SQL
/// NULL (rather than a JSON `null`) so a nullable column round-trips cleanly.
pub fn to_value_opt<T: Serialize>(value: &Option<T>) -> Result<Option<serde_json::Value>> {
    match value {
        Some(v) => Ok(Some(serde_json::to_value(v)?)),
        None => Ok(None),
    }
}

/// Deserializes a stored JSONB value back into a `labrys-core` contract.
pub fn from_value<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    Ok(serde_json::from_value(value)?)
}

/// Reads a JSONB column and maps it into a core contract type.
pub fn json_get<T: DeserializeOwned>(row: &PgRow, column: &str) -> Result<T> {
    let value: serde_json::Value = row.try_get(column)?;
    from_value(value)
}

/// Reads a nullable JSONB column and maps it into an optional core contract type.
pub fn json_get_opt<T: DeserializeOwned>(row: &PgRow, column: &str) -> Result<Option<T>> {
    let value: Option<serde_json::Value> = row.try_get(column)?;
    value.map(from_value).transpose()
}

/// Reads a nullable text column.
pub fn text_opt(row: &PgRow, column: &str) -> Result<Option<String>> {
    let value: Option<String> = row.try_get(column)?;
    Ok(value)
}
