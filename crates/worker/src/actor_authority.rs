use anyhow::Result;
use serde_json::Value;
use sqlx::{types::Json as SqlJson, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_server_core::operator_is_active_authorized;

pub(crate) async fn actor_authorized(
    pool: &PgPool,
    actor_id: Option<Uuid>,
    required_role: &str,
    required_scopes: &[&str],
) -> Result<bool> {
    let Some(actor_id) = actor_id.filter(|id| !id.is_nil()) else {
        return Ok(false);
    };
    let Some(row) = sqlx::query(
        r#"
        SELECT status, role, scopes
        FROM operators
        WHERE id = $1
        "#,
    )
    .bind(actor_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(false);
    };
    Ok(row_authorized(&row, required_role, required_scopes)?)
}

pub(crate) async fn actor_authorized_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor_id: Option<Uuid>,
    required_role: &str,
    required_scopes: &[&str],
) -> Result<bool> {
    let Some(actor_id) = actor_id.filter(|id| !id.is_nil()) else {
        return Ok(false);
    };
    let Some(row) = sqlx::query(
        r#"
        SELECT status, role, scopes
        FROM operators
        WHERE id = $1
        "#,
    )
    .bind(actor_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(false);
    };
    Ok(row_authorized(&row, required_role, required_scopes)?)
}

fn row_authorized(
    row: &sqlx::postgres::PgRow,
    required_role: &str,
    required_scopes: &[&str],
) -> Result<bool> {
    let status: String = row.try_get("status")?;
    let role: String = row.try_get("role")?;
    let scopes = parse_scopes(row.try_get::<SqlJson<Value>, _>("scopes")?.0);
    Ok(operator_is_active_authorized(
        &status,
        &role,
        &scopes,
        required_role,
        required_scopes,
    ))
}

fn parse_scopes(value: Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|scope| scope.as_str())
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
