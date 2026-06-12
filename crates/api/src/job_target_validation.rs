use vpsman_common::JobCommand;

use crate::error::ApiError;

pub(crate) fn validate_network_apply_target(
    command: &JobCommand,
    resolved_targets: &[String],
) -> Result<(), ApiError> {
    vpsman_server_core::validate_network_apply_target(command, resolved_targets)
        .map_err(|error| ApiError::bad_request(error.code()))
}
