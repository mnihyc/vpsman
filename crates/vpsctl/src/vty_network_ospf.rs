use anyhow::{Context, Result};
use uuid::Uuid;

use crate::http::http_post_json;

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelOspfCostUpdateRequest {
    pub(crate) plan_id: Uuid,
    pub(crate) recommendation_id: String,
    pub(crate) current_ospf_cost: u16,
    pub(crate) recommended_ospf_cost: u16,
    pub(crate) mutation_intent: String,
    pub(crate) confirmed: bool,
}

pub(crate) fn parse_vty_tunnel_ospf_cost_update(
    tokens: &[&str],
) -> Result<VtyTunnelOspfCostUpdateRequest> {
    let mut plan_id = None::<Uuid>;
    let mut recommendation_id = None::<String>;
    let mut current_ospf_cost = None::<u16>;
    let mut recommended_ospf_cost = None::<u16>;
    let mut mutation_intent = "apply".to_string();
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
            "--current-ospf-cost" => {
                current_ospf_cost = Some(parse_u16(
                    next_value(tokens, index, tokens[index])?,
                    "--current-ospf-cost",
                )?);
                index += 2;
            }
            value if value.starts_with("--current-ospf-cost=") => {
                current_ospf_cost = Some(parse_u16(
                    flag_value(value, "--current-ospf-cost="),
                    "--current-ospf-cost",
                )?);
                index += 1;
            }
            "--recommended-ospf-cost" => {
                recommended_ospf_cost = Some(parse_u16(
                    next_value(tokens, index, tokens[index])?,
                    "--recommended-ospf-cost",
                )?);
                index += 2;
            }
            value if value.starts_with("--recommended-ospf-cost=") => {
                recommended_ospf_cost = Some(parse_u16(
                    flag_value(value, "--recommended-ospf-cost="),
                    "--recommended-ospf-cost",
                )?);
                index += 1;
            }
            "--mutation-intent" => {
                mutation_intent =
                    parse_mutation_intent(next_value(tokens, index, "--mutation-intent")?)?;
                index += 2;
            }
            value if value.starts_with("--mutation-intent=") => {
                mutation_intent = parse_mutation_intent(flag_value(value, "--mutation-intent="))?;
                index += 1;
            }
            other => anyhow::bail!("unknown tunnel-ospf-cost-update flag {other}"),
        }
    }

    anyhow::ensure!(confirmed, "tunnel-ospf-cost-update requires --confirmed");
    let current_ospf_cost = required(current_ospf_cost, "--current-ospf-cost")?;
    let recommended_ospf_cost = required(recommended_ospf_cost, "--recommended-ospf-cost")?;
    anyhow::ensure!(
        current_ospf_cost != recommended_ospf_cost,
        "tunnel-ospf-cost-update requires a changed OSPF cost"
    );

    Ok(VtyTunnelOspfCostUpdateRequest {
        plan_id: required(plan_id, "--plan-id")?,
        recommendation_id: required(recommendation_id, "--recommendation-id")?,
        current_ospf_cost,
        recommended_ospf_cost,
        mutation_intent,
        confirmed,
    })
}

pub(crate) fn submit_vty_tunnel_ospf_cost_update(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelOspfCostUpdateRequest,
) -> Result<String> {
    http_post_json(
        api_url,
        &format!("/api/v1/tunnel-plans/{}/ospf-cost", request.plan_id),
        token,
        &serde_json::json!({
            "recommendation_id": request.recommendation_id,
            "current_ospf_cost": request.current_ospf_cost,
            "recommended_ospf_cost": request.recommended_ospf_cost,
            "mutation_intent": request.mutation_intent,
            "confirmed": request.confirmed,
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

fn parse_non_empty_string(value: &str, flag: &str) -> Result<String> {
    let trimmed = value.trim();
    anyhow::ensure!(!trimmed.is_empty(), "{flag} must not be empty");
    Ok(trimmed.to_string())
}

fn parse_mutation_intent(value: &str) -> Result<String> {
    match value {
        "apply" | "rollback" => Ok(value.to_string()),
        _ => anyhow::bail!("--mutation-intent must be apply or rollback"),
    }
}

fn parse_uuid(value: &str, flag: &str) -> Result<Uuid> {
    value
        .parse::<Uuid>()
        .with_context(|| format!("{flag} must be a UUID"))
}

#[cfg(test)]
mod tests {
    use super::parse_vty_tunnel_ospf_cost_update;

    #[test]
    fn parses_vty_tunnel_ospf_cost_update() {
        let request = parse_vty_tunnel_ospf_cost_update(&[
            "--plan-id",
            "00000000-0000-0000-0000-000000000001",
            "--recommendation-id=ospf-1234abcd5678ef90",
            "--current-ospf-cost",
            "100",
            "--recommended-ospf-cost=50",
            "--mutation-intent",
            "rollback",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(
            request.plan_id.to_string(),
            "00000000-0000-0000-0000-000000000001"
        );
        assert_eq!(request.recommendation_id, "ospf-1234abcd5678ef90");
        assert_eq!(request.current_ospf_cost, 100);
        assert_eq!(request.recommended_ospf_cost, 50);
        assert_eq!(request.mutation_intent, "rollback");
        assert!(request.confirmed);
        assert!(parse_vty_tunnel_ospf_cost_update(&[
            "--plan-id",
            "00000000-0000-0000-0000-000000000001",
            "--recommendation-id",
            "ospf-1234abcd5678ef90",
            "--current-ospf-cost",
            "100",
            "--recommended-ospf-cost",
            "100",
            "--confirmed",
        ])
        .is_err());
        assert!(parse_vty_tunnel_ospf_cost_update(&[
            "--plan-id",
            "00000000-0000-0000-0000-000000000001",
            "--current-ospf-cost",
            "100",
            "--recommended-ospf-cost",
            "50",
            "--confirmed",
        ])
        .is_err());
        assert!(parse_vty_tunnel_ospf_cost_update(&[
            "--plan-id",
            "00000000-0000-0000-0000-000000000001",
            "--recommendation-id",
            "ospf-1234abcd5678ef90",
            "--current-ospf-cost",
            "100",
            "--recommended-ospf-cost",
            "50",
            "--mutation-intent",
            "preview",
            "--confirmed",
        ])
        .is_err());
    }
}
