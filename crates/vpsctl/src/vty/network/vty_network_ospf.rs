use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::TunnelPlan;

use crate::{
    commands_network::{
        tunnel_ospf_cost_action, tunnel_ospf_cost_payload_hash, tunnel_plan_client_ids,
        tunnel_plan_privilege_target,
    },
    http::{http_get, http_post_json},
    privilege::{
        build_privilege_for_db, load_super_password, load_super_salt_hex, DbPrivilegeRequest,
    },
};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelOspfCostUpdateRequest {
    pub(crate) plan_id: Uuid,
    pub(crate) plan_revision: i64,
    pub(crate) recommendation_id: String,
    pub(crate) left_current_ospf_cost: Option<u16>,
    pub(crate) right_current_ospf_cost: Option<u16>,
    pub(crate) desired_ospf_cost: u16,
    pub(crate) left_adapter_definition_hash: String,
    pub(crate) right_adapter_definition_hash: String,
    pub(crate) confirmed: bool,
}

pub(crate) fn parse_vty_tunnel_ospf_cost_update(
    tokens: &[&str],
) -> Result<VtyTunnelOspfCostUpdateRequest> {
    let mut plan_id = None::<Uuid>;
    let mut plan_revision = None::<i64>;
    let mut recommendation_id = None::<String>;
    let mut left_current_ospf_cost = None::<u16>;
    let mut right_current_ospf_cost = None::<u16>;
    let mut desired_ospf_cost = None::<u16>;
    let mut left_adapter_definition_hash = None::<String>;
    let mut right_adapter_definition_hash = None::<String>;
    let mut confirmed = false;

    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            "--plan-id" => {
                plan_id = Some(parse_uuid(
                    next_value(tokens, index, "--plan-id")?,
                    "--plan-id",
                )?);
                index += 2;
            }
            value if value.starts_with("--plan-id=") => {
                plan_id = Some(parse_uuid(flag_value(value, "--plan-id="), "--plan-id")?);
                index += 1;
            }
            "--plan-revision" => {
                plan_revision = Some(parse_revision(
                    next_value(tokens, index, "--plan-revision")?,
                    "--plan-revision",
                )?);
                index += 2;
            }
            value if value.starts_with("--plan-revision=") => {
                plan_revision = Some(parse_revision(
                    flag_value(value, "--plan-revision="),
                    "--plan-revision",
                )?);
                index += 1;
            }
            "--recommendation-id" => {
                recommendation_id = Some(parse_non_empty_string(
                    next_value(tokens, index, "--recommendation-id")?,
                    "--recommendation-id",
                )?);
                index += 2;
            }
            value if value.starts_with("--recommendation-id=") => {
                recommendation_id = Some(parse_non_empty_string(
                    flag_value(value, "--recommendation-id="),
                    "--recommendation-id",
                )?);
                index += 1;
            }
            "--left-current-ospf-cost" => {
                left_current_ospf_cost = Some(parse_u16(
                    next_value(tokens, index, "--left-current-ospf-cost")?,
                    "--left-current-ospf-cost",
                )?);
                index += 2;
            }
            value if value.starts_with("--left-current-ospf-cost=") => {
                left_current_ospf_cost = Some(parse_u16(
                    flag_value(value, "--left-current-ospf-cost="),
                    "--left-current-ospf-cost",
                )?);
                index += 1;
            }
            "--right-current-ospf-cost" => {
                right_current_ospf_cost = Some(parse_u16(
                    next_value(tokens, index, "--right-current-ospf-cost")?,
                    "--right-current-ospf-cost",
                )?);
                index += 2;
            }
            value if value.starts_with("--right-current-ospf-cost=") => {
                right_current_ospf_cost = Some(parse_u16(
                    flag_value(value, "--right-current-ospf-cost="),
                    "--right-current-ospf-cost",
                )?);
                index += 1;
            }
            "--desired-ospf-cost" => {
                desired_ospf_cost = Some(parse_u16(
                    next_value(tokens, index, "--desired-ospf-cost")?,
                    "--desired-ospf-cost",
                )?);
                index += 2;
            }
            value if value.starts_with("--desired-ospf-cost=") => {
                desired_ospf_cost = Some(parse_u16(
                    flag_value(value, "--desired-ospf-cost="),
                    "--desired-ospf-cost",
                )?);
                index += 1;
            }
            "--left-adapter-definition-hash" => {
                left_adapter_definition_hash = Some(parse_hash(
                    next_value(tokens, index, "--left-adapter-definition-hash")?,
                    "--left-adapter-definition-hash",
                )?);
                index += 2;
            }
            value if value.starts_with("--left-adapter-definition-hash=") => {
                left_adapter_definition_hash = Some(parse_hash(
                    flag_value(value, "--left-adapter-definition-hash="),
                    "--left-adapter-definition-hash",
                )?);
                index += 1;
            }
            "--right-adapter-definition-hash" => {
                right_adapter_definition_hash = Some(parse_hash(
                    next_value(tokens, index, "--right-adapter-definition-hash")?,
                    "--right-adapter-definition-hash",
                )?);
                index += 2;
            }
            value if value.starts_with("--right-adapter-definition-hash=") => {
                right_adapter_definition_hash = Some(parse_hash(
                    flag_value(value, "--right-adapter-definition-hash="),
                    "--right-adapter-definition-hash",
                )?);
                index += 1;
            }
            other => anyhow::bail!("unknown tunnel-ospf-cost-update flag {other}"),
        }
    }

    anyhow::ensure!(confirmed, "tunnel-ospf-cost-update requires --confirmed");
    let desired_ospf_cost = required(desired_ospf_cost, "--desired-ospf-cost")?;
    anyhow::ensure!(
        left_current_ospf_cost != Some(desired_ospf_cost)
            || right_current_ospf_cost != Some(desired_ospf_cost),
        "tunnel-ospf-cost-update requires at least one endpoint cost change"
    );

    Ok(VtyTunnelOspfCostUpdateRequest {
        plan_id: required(plan_id, "--plan-id")?,
        plan_revision: required(plan_revision, "--plan-revision")?,
        recommendation_id: required(recommendation_id, "--recommendation-id")?,
        left_current_ospf_cost,
        right_current_ospf_cost,
        desired_ospf_cost,
        left_adapter_definition_hash: required(
            left_adapter_definition_hash,
            "--left-adapter-definition-hash",
        )?,
        right_adapter_definition_hash: required(
            right_adapter_definition_hash,
            "--right-adapter-definition-hash",
        )?,
        confirmed,
    })
}

pub(crate) fn submit_vty_tunnel_ospf_cost_update(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelOspfCostUpdateRequest,
) -> Result<String> {
    let plan_raw = http_get(
        api_url,
        &format!("/api/v1/tunnel-plans/{}/plan", request.plan_id),
        token,
    )?;
    let plan: TunnelPlan =
        serde_json::from_str(&plan_raw).context("failed to parse tunnel plan export")?;
    let target_client_ids = tunnel_plan_client_ids(&plan)?;
    let payload_hash = tunnel_ospf_cost_payload_hash(
        request.plan_id,
        request.plan_revision,
        &request.recommendation_id,
        request.left_current_ospf_cost,
        request.right_current_ospf_cost,
        request.desired_ospf_cost,
        &request.left_adapter_definition_hash,
        &request.right_adapter_definition_hash,
    );
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let target = tunnel_plan_privilege_target(request.plan_id);
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: tunnel_ospf_cost_action(),
            target: &target,
            selector_expression: None,
            resolved_targets: &target_client_ids,
            confirmed: true,
            payload_hash: Some(&payload_hash),
        },
        &password,
        &salt_hex,
        300,
    )?;
    http_post_json(
        api_url,
        &format!("/api/v1/tunnel-plans/{}/ospf-cost", request.plan_id),
        token,
        &serde_json::json!({
            "plan_revision": request.plan_revision,
            "recommendation_id": request.recommendation_id,
            "left_current_ospf_cost": request.left_current_ospf_cost,
            "right_current_ospf_cost": request.right_current_ospf_cost,
            "desired_ospf_cost": request.desired_ospf_cost,
            "left_adapter_definition_hash": request.left_adapter_definition_hash,
            "right_adapter_definition_hash": request.right_adapter_definition_hash,
            "confirmed": request.confirmed,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

fn next_value<'a>(tokens: &'a [&str], index: usize, flag: &str) -> Result<&'a str> {
    tokens
        .get(index + 1)
        .copied()
        .with_context(|| format!("{flag} requires a value"))
}

fn flag_value<'a>(value: &'a str, prefix: &str) -> &'a str {
    value.trim_start_matches(prefix)
}

fn required<T>(value: Option<T>, flag: &str) -> Result<T> {
    value.with_context(|| format!("missing required {flag}"))
}

fn parse_u16(value: &str, flag: &str) -> Result<u16> {
    let parsed = value
        .parse::<u16>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(parsed > 0, "{flag} must be between 1 and 65535");
    Ok(parsed)
}

fn parse_revision(value: &str, flag: &str) -> Result<i64> {
    let parsed = value
        .parse::<i64>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(parsed > 0, "{flag} must be positive");
    Ok(parsed)
}

fn parse_non_empty_string(value: &str, flag: &str) -> Result<String> {
    let trimmed = value.trim();
    anyhow::ensure!(!trimmed.is_empty(), "{flag} must not be empty");
    Ok(trimmed.to_string())
}

fn parse_hash(value: &str, flag: &str) -> Result<String> {
    anyhow::ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{flag} must be a 64-character hexadecimal SHA-256 hash"
    );
    Ok(value.to_string())
}

fn parse_uuid(value: &str, flag: &str) -> Result<Uuid> {
    value
        .parse::<Uuid>()
        .with_context(|| format!("{flag} must be a UUID"))
}

#[cfg(test)]
#[path = "tests_vty_network_ospf.rs"]
mod tests;
