export type AgentIdentityView = {
  client_id: string;
  display_name: string;
  status: string;
  current_public_key_sha256_hex: string;
  tags: string[];
};

export type UpsertAgentIdentityRequest = {
  client_id?: string | null;
  client_public_key_hex: string;
  display_name?: string | null;
  tags: string[];
  replace_existing_key: boolean;
  confirmed: boolean;
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
  direct_identity_client_count: number;
  current_key_revoked_count: number;
  revocation_count: number;
  clients: KeyLifecycleClientView[];
};
