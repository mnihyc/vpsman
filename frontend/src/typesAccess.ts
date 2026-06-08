export type EnrollmentTokenPurpose = "provision" | "rebuild_reenrollment";

export type EnrollmentServerEndpoint = {
  label: string;
  tcp_addr: string;
  priority: number;
};

export type EnrollmentAgentUpdateDefaults = {
  trusted_artifact_signing_key_hex?: string | null;
  unmanaged_enabled: boolean;
  unmanaged_version_url: string;
  unmanaged_interval_secs: number;
  unmanaged_jitter_secs: number;
  unmanaged_activate: boolean;
  unmanaged_restart_agent: boolean;
};

export type EnrollmentRuntimeSettingsView = {
  tcp_endpoints: EnrollmentServerEndpoint[];
  discovery_url: string | null;
  gateway_server_public_key_hex: string | null;
  server_ed25519_public_key_hex: string | null;
  discovery_trusted_server_ed25519_public_keys_hex: string[];
  gateway_retry_secs: number;
  gateway_connect_timeout_secs: number;
  telemetry_light_secs: number;
  telemetry_full_secs: number;
  update: EnrollmentAgentUpdateDefaults;
};

export type UpdateEnrollmentRuntimeSettingsRequest = Omit<
  EnrollmentRuntimeSettingsView,
  "server_ed25519_public_key_hex"
>;

export type EnrollmentTokenView = {
  id: string;
  token_prefix: string;
  purpose: EnrollmentTokenPurpose;
  assigned_client_id: string | null;
  allowed_client_id: string | null;
  requires_existing_client: boolean;
  preserve_existing_assignments: boolean;
  expected_old_public_key_sha256_hex: string | null;
  created_by: string | null;
  created_at: string;
  expires_at: string;
  used_at: string | null;
  used_by_client_id: string | null;
  default_tags: string[];
  default_display_name: string | null;
  unmanaged_update_enabled: boolean;
  unmanaged_update_version_url: string;
  unmanaged_update_interval_secs: number;
  unmanaged_update_jitter_secs: number;
  unmanaged_update_activate: boolean;
  unmanaged_update_restart_agent: boolean;
};

export type CreateEnrollmentTokenRequest = {
  ttl_secs: number;
  purpose: EnrollmentTokenPurpose;
  allowed_client_id?: string | null;
  confirmed_reenrollment: boolean;
  preserve_existing_assignments: boolean;
  default_tags: string[];
  default_display_name?: string | null;
  unmanaged_update_enabled?: boolean;
  unmanaged_update_version_url?: string | null;
  unmanaged_update_interval_secs?: number;
  unmanaged_update_jitter_secs?: number;
  unmanaged_update_activate?: boolean;
  unmanaged_update_restart_agent?: boolean;
};

export type CreateEnrollmentTokenResponse = {
  id: string;
  token: string;
  token_prefix: string;
  purpose: EnrollmentTokenPurpose;
  assigned_client_id: string | null;
  allowed_client_id: string | null;
  requires_existing_client: boolean;
  preserve_existing_assignments: boolean;
  expected_old_public_key_sha256_hex: string | null;
  expires_at: string;
  default_tags: string[];
  default_display_name: string | null;
  unmanaged_update_enabled: boolean;
  unmanaged_update_version_url: string;
  unmanaged_update_interval_secs: number;
  unmanaged_update_jitter_secs: number;
  unmanaged_update_activate: boolean;
  unmanaged_update_restart_agent: boolean;
};

export type ClientKeyRevocationView = {
  id: string;
  client_id: string;
  public_key_sha256_hex: string;
  reason: string | null;
  revoked_by: string | null;
  created_at: string;
};

export type KeyLifecycleClientView = {
  client_id: string;
  display_name: string;
  status: string;
  current_public_key_sha256_hex: string | null;
  current_key_revoked: boolean;
  latest_revoked_at: string | null;
  latest_revocation_reason: string | null;
};

export type KeyLifecycleReportView = {
  server_ed25519_public_key_configured: boolean;
  discovery_trusted_server_key_count: number;
  gateway_server_public_key_configured: boolean;
  enrolled_client_count: number;
  current_key_revoked_count: number;
  revocation_count: number;
  rebuild_reenrollment_token_count: number;
  active_rebuild_reenrollment_token_count: number;
  clients: KeyLifecycleClientView[];
};
