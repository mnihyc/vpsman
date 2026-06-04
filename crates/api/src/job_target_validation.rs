use vpsman_common::{JobCommand, TunnelEndpointSide};

use crate::error::ApiError;

pub(crate) fn validate_network_apply_target(
    command: &JobCommand,
    resolved_targets: &[String],
) -> Result<(), ApiError> {
    let expected = match command {
        JobCommand::NetworkApply { plan, side, .. }
        | JobCommand::NetworkOspfCostUpdate { plan, side, .. }
        | JobCommand::NetworkRollback { plan, side }
        | JobCommand::NetworkStatus { plan, side }
        | JobCommand::NetworkProbe { plan, side, .. } => match side {
            TunnelEndpointSide::Left => &plan.left_client_id,
            TunnelEndpointSide::Right => &plan.right_client_id,
        },
        JobCommand::NetworkSpeedTest { plan, .. } => {
            let mut expected = vec![plan.left_client_id.clone(), plan.right_client_id.clone()];
            expected.sort();
            let mut actual = resolved_targets.to_vec();
            actual.sort();
            if actual != expected {
                return Err(ApiError::bad_request("network_speed_test_target_mismatch"));
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    if resolved_targets.len() != 1 || resolved_targets.first() != Some(expected) {
        return Err(ApiError::bad_request("network_apply_target_mismatch"));
    }
    Ok(())
}
