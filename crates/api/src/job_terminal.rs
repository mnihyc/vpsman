use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use vpsman_common::{
    MAX_TERMINAL_COLS, MAX_TERMINAL_FLOW_WINDOW_BYTES, MAX_TERMINAL_IDLE_TIMEOUT_SECS,
    MAX_TERMINAL_INPUT_BYTES, MAX_TERMINAL_REASON_BYTES, MAX_TERMINAL_ROWS, MIN_TERMINAL_COLS,
    MIN_TERMINAL_FLOW_WINDOW_BYTES, MIN_TERMINAL_IDLE_TIMEOUT_SECS, MIN_TERMINAL_ROWS,
};

use crate::ApiError;

pub(crate) fn validate_terminal_open(
    session_id: uuid::Uuid,
    argv: &[String],
    cwd: Option<&str>,
    cols: u16,
    rows: u16,
    idle_timeout_secs: u32,
    flow_window_bytes: u32,
) -> Result<(), ApiError> {
    validate_terminal_session_id(session_id)?;
    validate_terminal_argv(argv)?;
    if let Some(cwd) = cwd {
        if cwd.len() > 4096
            || !cwd.starts_with('/')
            || cwd.as_bytes().contains(&0)
            || path_contains_dot_segment(cwd)
        {
            return Err(ApiError::bad_request("terminal_cwd_invalid"));
        }
    }
    validate_terminal_dimensions(cols, rows)?;
    if !(MIN_TERMINAL_IDLE_TIMEOUT_SECS..=MAX_TERMINAL_IDLE_TIMEOUT_SECS)
        .contains(&idle_timeout_secs)
    {
        return Err(ApiError::bad_request(
            "terminal_idle_timeout_secs_out_of_range",
        ));
    }
    if !(MIN_TERMINAL_FLOW_WINDOW_BYTES..=MAX_TERMINAL_FLOW_WINDOW_BYTES)
        .contains(&flow_window_bytes)
    {
        return Err(ApiError::bad_request(
            "terminal_flow_window_bytes_out_of_range",
        ));
    }
    Ok(())
}

pub(crate) fn validate_terminal_input(
    session_id: uuid::Uuid,
    data_base64: &str,
) -> Result<(), ApiError> {
    validate_terminal_session_id(session_id)?;
    if data_base64.is_empty() || data_base64.len() > MAX_TERMINAL_INPUT_BYTES.div_ceil(3) * 4 + 16 {
        return Err(ApiError::bad_request("terminal_input_size_invalid"));
    }
    let data = BASE64_STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|_| ApiError::bad_request("terminal_input_base64_invalid"))?;
    if data.is_empty() || data.len() > MAX_TERMINAL_INPUT_BYTES {
        return Err(ApiError::bad_request("terminal_input_size_invalid"));
    }
    Ok(())
}

pub(crate) fn validate_terminal_poll(session_id: uuid::Uuid) -> Result<(), ApiError> {
    validate_terminal_session_id(session_id)
}

pub(crate) fn validate_terminal_resize(
    session_id: uuid::Uuid,
    cols: u16,
    rows: u16,
) -> Result<(), ApiError> {
    validate_terminal_session_id(session_id)?;
    validate_terminal_dimensions(cols, rows)
}

pub(crate) fn validate_terminal_close(
    session_id: uuid::Uuid,
    reason: Option<&str>,
) -> Result<(), ApiError> {
    validate_terminal_session_id(session_id)?;
    if let Some(reason) = reason {
        if reason.len() > MAX_TERMINAL_REASON_BYTES
            || reason
                .chars()
                .any(|value| value.is_control() && !matches!(value, '\n' | '\r' | '\t'))
        {
            return Err(ApiError::bad_request("terminal_close_reason_invalid"));
        }
    }
    Ok(())
}

fn validate_terminal_session_id(session_id: uuid::Uuid) -> Result<(), ApiError> {
    if session_id.is_nil() {
        return Err(ApiError::bad_request("terminal_session_id_invalid"));
    }
    Ok(())
}

fn validate_terminal_argv(argv: &[String]) -> Result<(), ApiError> {
    if argv.is_empty() {
        return Err(ApiError::bad_request("terminal_argv_required"));
    }
    if argv.len() > 64 {
        return Err(ApiError::bad_request("terminal_argv_too_large"));
    }
    if argv
        .iter()
        .any(|part| part.is_empty() || part.len() > 4096 || part.as_bytes().contains(&0))
    {
        return Err(ApiError::bad_request("terminal_argv_invalid"));
    }
    if !argv[0].starts_with('/') {
        return Err(ApiError::bad_request(
            "terminal_executable_must_be_absolute",
        ));
    }
    Ok(())
}

fn validate_terminal_dimensions(cols: u16, rows: u16) -> Result<(), ApiError> {
    if !(MIN_TERMINAL_COLS..=MAX_TERMINAL_COLS).contains(&cols) {
        return Err(ApiError::bad_request("terminal_cols_out_of_range"));
    }
    if !(MIN_TERMINAL_ROWS..=MAX_TERMINAL_ROWS).contains(&rows) {
        return Err(ApiError::bad_request("terminal_rows_out_of_range"));
    }
    Ok(())
}

fn path_contains_dot_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}
