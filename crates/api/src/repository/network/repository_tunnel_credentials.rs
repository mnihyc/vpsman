use anyhow::{Context, Result};
use base64::Engine;
use rand::{rngs::OsRng, RngCore};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use vpsman_common::{
    RuntimeTunnelManager, TunnelBuiltinCredentials, TunnelEndpointSide, TunnelKind,
    TunnelOpenvpnIdentity, TunnelPlan, TunnelWireguardIdentity,
};
use x25519_dalek::{PublicKey, StaticSecret};

pub(crate) fn reconcile_tunnel_builtin_credentials(
    plan_id: Uuid,
    previous_plan: Option<&TunnelPlan>,
    previous: Option<&TunnelBuiltinCredentials>,
    next_plan: &TunnelPlan,
) -> Result<Option<TunnelBuiltinCredentials>> {
    let Some(kind) = credential_kind(next_plan) else {
        return Ok(None);
    };
    match (kind, previous_plan, previous) {
        (
            TunnelKind::Wireguard,
            Some(previous_plan),
            Some(previous @ TunnelBuiltinCredentials::Wireguard { left, right, .. }),
        ) if credential_kind(previous_plan) == Some(TunnelKind::Wireguard) => {
            let left_changed = previous_plan.left_client_id != next_plan.left_client_id;
            let right_changed = previous_plan.right_client_id != next_plan.right_client_id;
            let generation = if left_changed || right_changed {
                next_credential_generation(previous)?
            } else {
                previous.generation()
            };
            let left = if !left_changed {
                left.clone()
            } else {
                generate_wireguard_identity()
            };
            let right = if !right_changed {
                right.clone()
            } else {
                generate_wireguard_identity()
            };
            Ok(Some(TunnelBuiltinCredentials::Wireguard {
                generation,
                left,
                right,
            }))
        }
        (
            TunnelKind::Openvpn,
            Some(previous_plan),
            Some(previous @ TunnelBuiltinCredentials::Openvpn { left, right, .. }),
        ) if credential_kind(previous_plan) == Some(TunnelKind::Openvpn) => {
            let left_changed = previous_plan.left_client_id != next_plan.left_client_id;
            let right_changed = previous_plan.right_client_id != next_plan.right_client_id;
            let generation = if left_changed || right_changed {
                next_credential_generation(previous)?
            } else {
                previous.generation()
            };
            let left = if !left_changed {
                left.clone()
            } else {
                generate_openvpn_identity(plan_id, TunnelEndpointSide::Left, generation)?
            };
            let right = if !right_changed {
                right.clone()
            } else {
                generate_openvpn_identity(plan_id, TunnelEndpointSide::Right, generation)?
            };
            Ok(Some(TunnelBuiltinCredentials::Openvpn {
                generation,
                left,
                right,
            }))
        }
        _ => generate_tunnel_builtin_credentials(plan_id, next_plan, 1),
    }
}

pub(crate) fn next_credential_generation(previous: &TunnelBuiltinCredentials) -> Result<u64> {
    previous
        .generation()
        .checked_add(1)
        .context("tunnel credential generation exhausted")
}

pub(crate) fn generate_tunnel_builtin_credentials(
    plan_id: Uuid,
    plan: &TunnelPlan,
    generation: u64,
) -> Result<Option<TunnelBuiltinCredentials>> {
    Ok(match credential_kind(plan) {
        Some(TunnelKind::Wireguard) => Some(TunnelBuiltinCredentials::Wireguard {
            generation,
            left: generate_wireguard_identity(),
            right: generate_wireguard_identity(),
        }),
        Some(TunnelKind::Openvpn) => Some(TunnelBuiltinCredentials::Openvpn {
            generation,
            left: generate_openvpn_identity(plan_id, TunnelEndpointSide::Left, generation)?,
            right: generate_openvpn_identity(plan_id, TunnelEndpointSide::Right, generation)?,
        }),
        _ => None,
    })
}

fn credential_kind(plan: &TunnelPlan) -> Option<TunnelKind> {
    if plan.runtime_control.manager != RuntimeTunnelManager::AgentBuiltin {
        return None;
    }
    match plan.kind {
        TunnelKind::Wireguard | TunnelKind::Openvpn => Some(plan.kind),
        _ => None,
    }
}

fn generate_wireguard_identity() -> TunnelWireguardIdentity {
    let mut private_bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut private_bytes);
    let private = StaticSecret::from(private_bytes);
    let public = PublicKey::from(&private);
    TunnelWireguardIdentity {
        private_key_base64: base64::engine::general_purpose::STANDARD.encode(private.to_bytes()),
        public_key_base64: base64::engine::general_purpose::STANDARD.encode(public.as_bytes()),
    }
}

fn generate_openvpn_identity(
    plan_id: Uuid,
    side: TunnelEndpointSide,
    generation: u64,
) -> Result<TunnelOpenvpnIdentity> {
    let issuer_key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("generate OpenVPN issuer P-256 private key")?;
    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("generate OpenVPN P-256 private key")?;
    let side_name = match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    };
    let now = OffsetDateTime::now_utc();
    let mut issuer_params =
        CertificateParams::new(Vec::<String>::new()).context("prepare OpenVPN issuer")?;
    issuer_params.not_before = now - Duration::days(1);
    issuer_params.not_after = now + Duration::days(365 * 20);
    issuer_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let mut issuer_distinguished_name = DistinguishedName::new();
    issuer_distinguished_name.push(
        DnType::CommonName,
        format!("vpsman-{plan_id}-{side_name}-{generation}-issuer"),
    );
    issuer_params.distinguished_name = issuer_distinguished_name;
    issuer_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let issuer_certificate = issuer_params
        .self_signed(&issuer_key_pair)
        .context("self-sign OpenVPN endpoint issuer")?;
    let issuer = Issuer::new(issuer_params, issuer_key_pair);
    let mut params =
        CertificateParams::new(Vec::<String>::new()).context("prepare OpenVPN certificate")?;
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(365 * 20);
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(
        DnType::CommonName,
        format!("vpsman-{plan_id}-{side_name}-{generation}"),
    );
    params.distinguished_name = distinguished_name;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ClientAuth,
        ExtendedKeyUsagePurpose::ServerAuth,
    ];
    let certificate = params
        .signed_by(&key_pair, &issuer)
        .context("sign OpenVPN endpoint certificate")?;
    let digest = Sha256::digest(certificate.der().as_ref());
    let fingerprint = digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    Ok(TunnelOpenvpnIdentity {
        private_key_pem: key_pair.serialize_pem(),
        certificate_pem: certificate.pem(),
        issuer_certificate_pem: issuer_certificate.pem(),
        certificate_sha256_fingerprint: fingerprint,
    })
}

#[cfg(test)]
#[path = "tests_repository_tunnel_credentials.rs"]
mod tests;
