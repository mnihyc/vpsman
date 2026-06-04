use axum::{extract::State, Json};
use ed25519_dalek::SigningKey;
use vpsman_common::{sign_discovery_document, DiscoveryDocument};

use crate::{state::AppState, state::EnrollmentSettings, unix_now};

const DISCOVERY_VERSION: u32 = 1;
const DISCOVERY_TTL_SECS: u64 = 60;

pub(crate) async fn discovery_endpoints(State(state): State<AppState>) -> Json<DiscoveryDocument> {
    Json(build_discovery_document(
        &state.enrollment,
        unix_now(),
        state.server_signing_key.as_deref(),
    ))
}

pub(crate) fn build_discovery_document(
    settings: &EnrollmentSettings,
    now_unix: u64,
    signing_key: Option<&SigningKey>,
) -> DiscoveryDocument {
    let mut document = DiscoveryDocument {
        version: DISCOVERY_VERSION,
        issued_unix: now_unix,
        expires_unix: now_unix + DISCOVERY_TTL_SECS,
        endpoints: settings.tcp_endpoints.clone(),
        signature: Vec::new(),
    };
    if let Some(signing_key) = signing_key {
        document.signature = sign_discovery_document(signing_key, &document);
    }
    document
}
