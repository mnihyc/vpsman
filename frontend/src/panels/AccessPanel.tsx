import { useEffect, useMemo, useState, type FormEvent } from "react";
import {
  Ban,
  Fingerprint,
  KeyRound,
  LockKeyhole,
  RefreshCw,
  Save,
  ShieldCheck,
  Trash2,
  UserPlus,
  UserX,
  Wifi,
} from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { CrudPager } from "../components/CrudPager";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { clearPrivilegeVault, hasPrivilegeVault } from "../vault";
import { usePanelDisplaySettings } from "../panelDisplay";
import type {
  GatewaySessionRecord,
  OperatorSessionRecord,
  OperatorView,
  TotpSetupResponse,
} from "../types";
import type {
  AgentIdentityView,
  ClientKeyRevocationView,
  KeyLifecycleReportView,
  UpsertAgentIdentityRequest,
} from "../typesAccess";
import type { PrivilegeMaterial } from "../privilege";
import {
  clientDisplayNameFromMap,
  clientLifecycleNameMap,
  formatTime,
  formatVpsName,
  shortHash,
  statusClass,
} from "../utils";

const accessSubpages = [
  "Overview",
  "Operators",
  "Privilege unlock",
  "VPS keys",
  "Gateway",
] as const;

type AccessSubpage = (typeof accessSubpages)[number];
type AccessConfirmationAction = "agent-identity" | "key-revoke";

type AccessPanelProps = {
  activeSubpage: string;
  apiToken: string;
  error: string | null;
  gatewaySessions: GatewaySessionRecord[];
  lastLiveEvent: string;
  loading: boolean;
  onClearSession: () => void;
  onCreateOperator: (
    username: string,
    role: string,
    password: string,
    scopes: string[],
  ) => Promise<void>;
  onConfirmTotp: (password: string, code: string) => Promise<void>;
  onDisableTotp: (password: string, code: string) => Promise<void>;
  onRefresh: () => Promise<void>;
  onRevokeClientKey: (
    clientId: string,
    reason: string | null,
    confirmed: boolean,
  ) => Promise<void>;
  onRevokeOperatorSession: (sessionId: string) => Promise<void>;
  onSetupTotp: (password: string) => Promise<TotpSetupResponse | null>;
  onUpsertAgentIdentity: (
    request: UpsertAgentIdentityRequest,
  ) => Promise<AgentIdentityView>;
  operator: OperatorView | null;
  clientKeyRevocations: ClientKeyRevocationView[];
  keyLifecycleReport: KeyLifecycleReportView | null;
  operatorSessions: OperatorSessionRecord[];
  operators: OperatorView[];
  privilegeMaterial: PrivilegeMaterial | null;
  sessionVaultAvailable: boolean;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  wsState: string;
};

function accessSubpageFromRoute(subpage: string): AccessSubpage {
  switch (subpage) {
    case "operators":
      return "Operators";
    case "privilege":
      return "Privilege unlock";
    case "clients":
      return "VPS keys";
    case "gateway":
      return "Gateway";
    default:
      return "Overview";
  }
}

export function AccessPanel({
  activeSubpage: routeSubpage,
  apiToken,
  error,
  gatewaySessions,
  lastLiveEvent,
  loading,
  onClearSession,
  onConfirmTotp,
  onCreateOperator,
  onDisableTotp,
  onRefresh,
  onRevokeClientKey,
  onRevokeOperatorSession,
  onSetupTotp,
  onUpsertAgentIdentity,
  operator,
  clientKeyRevocations,
  keyLifecycleReport,
  operatorSessions,
  operators,
  privilegeMaterial,
  sessionVaultAvailable,
  setPrivilegeMaterial,
  wsState,
}: AccessPanelProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [activeSubpage, setActiveSubpage] = useState<AccessSubpage>(
    accessSubpageFromRoute(routeSubpage),
  );
  const [vaultAvailable, setVaultAvailable] = useState(() =>
    hasPrivilegeVault(),
  );
  const [newOperatorUsername, setNewOperatorUsername] = useState("");
  const [newOperatorPassword, setNewOperatorPassword] = useState("");
  const [newOperatorRole, setNewOperatorRole] = useState("operator");
  const [newOperatorScopes, setNewOperatorScopes] = useState("");
  const [operatorActionError, setOperatorActionError] = useState<string | null>(
    null,
  );
  const [operatorActionPending, setOperatorActionPending] = useState(false);
  const [totpPassword, setTotpPassword] = useState("");
  const [totpCode, setTotpCode] = useState("");
  const [totpSetup, setTotpSetup] = useState<TotpSetupResponse | null>(null);
  const [totpPending, setTotpPending] = useState(false);
  const [totpError, setTotpError] = useState<string | null>(null);
  const [identityClientId, setIdentityClientId] = useState("");
  const [identityPublicKeyHex, setIdentityPublicKeyHex] = useState("");
  const [identityDisplayName, setIdentityDisplayName] = useState("");
  const [identityTags, setIdentityTags] = useState("");
  const [identityReplaceExistingKey, setIdentityReplaceExistingKey] =
    useState(false);
  const [identityPending, setIdentityPending] = useState(false);
  const [identityError, setIdentityError] = useState<string | null>(null);
  const [createdIdentity, setCreatedIdentity] =
    useState<AgentIdentityView | null>(null);
  const [revokeClientId, setRevokeClientId] = useState("");
  const [revokeReason, setRevokeReason] = useState("");
  const [revokePending, setRevokePending] = useState(false);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [pendingConfirmation, setPendingConfirmation] =
    useState<AccessConfirmationAction | null>(null);

  const canManageOperators = operator?.role === "admin";
  const sessionState = apiToken ? "Bearer session active" : "No bearer session";
  const vaultState = privilegeMaterial
    ? "Privilege unlocked"
    : vaultAvailable
      ? "Encrypted privilege vault present"
      : "No privilege vault";
  const tokenStorageState = apiToken
    ? sessionVaultAvailable
      ? "encrypted vault"
      : "memory only"
    : "none";
  const lifecycleClients = keyLifecycleReport?.clients ?? [];
  const lifecycleNameById = useMemo(
    () => clientLifecycleNameMap(lifecycleClients, vpsNameDisplayMode),
    [lifecycleClients, vpsNameDisplayMode],
  );
  const lifecycleClientLabel = (clientId: string | null | undefined) =>
    clientDisplayNameFromMap(clientId, lifecycleNameById);
  const activeGatewaySessions = gatewaySessions.filter(
    (session) => !session.ended_at,
  ).length;
  const revokedClientCount = lifecycleClients.filter(
    (client) => client.status === "revoked" || client.current_key_revoked,
  ).length;
  const canCreateOperator =
    canManageOperators &&
    newOperatorUsername.trim().length > 0 &&
    newOperatorPassword.length >= 12 &&
    !operatorActionPending;
  const canUpsertIdentity =
    canManageOperators &&
    !identityPending &&
    identityClientId.trim().length > 0 &&
    isFixedHex32(identityPublicKeyHex);
  const canRevokeClientKey =
    canManageOperators && revokeClientId.trim().length > 0 && !revokePending;

  useEffect(() => {
    setActiveSubpage(accessSubpageFromRoute(routeSubpage));
  }, [routeSubpage]);

  function clearVault() {
    clearPrivilegeVault();
    setVaultAvailable(false);
    setPrivilegeMaterial(null);
  }

  async function createOperator(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!canCreateOperator) {
      return;
    }
    setOperatorActionPending(true);
    setOperatorActionError(null);
    try {
      await onCreateOperator(
        newOperatorUsername.trim(),
        newOperatorRole,
        newOperatorPassword,
        parseListInput(newOperatorScopes),
      );
      setNewOperatorUsername("");
      setNewOperatorPassword("");
      setNewOperatorRole("operator");
      setNewOperatorScopes("");
    } catch (actionError) {
      setOperatorActionError(
        actionError instanceof Error
          ? actionError.message
          : "Operator creation failed",
      );
    } finally {
      setOperatorActionPending(false);
    }
  }

  async function setupTotp() {
    if (!totpPassword) {
      return;
    }
    setTotpPending(true);
    setTotpError(null);
    try {
      setTotpSetup(await onSetupTotp(totpPassword));
    } catch (actionError) {
      setTotpError(
        actionError instanceof Error
          ? actionError.message
          : "TOTP setup failed",
      );
    } finally {
      setTotpPending(false);
    }
  }

  async function confirmTotp() {
    if (!totpPassword || !totpCode) {
      return;
    }
    setTotpPending(true);
    setTotpError(null);
    try {
      await onConfirmTotp(totpPassword, totpCode);
      setTotpPassword("");
      setTotpCode("");
      setTotpSetup(null);
    } catch (actionError) {
      setTotpError(
        actionError instanceof Error
          ? actionError.message
          : "TOTP confirmation failed",
      );
    } finally {
      setTotpPending(false);
    }
  }

  async function disableTotp() {
    if (!totpPassword || !totpCode) {
      return;
    }
    setTotpPending(true);
    setTotpError(null);
    try {
      await onDisableTotp(totpPassword, totpCode);
      setTotpPassword("");
      setTotpCode("");
      setTotpSetup(null);
    } catch (actionError) {
      setTotpError(
        actionError instanceof Error
          ? actionError.message
          : "TOTP disable failed",
      );
    } finally {
      setTotpPending(false);
    }
  }

  function requestIdentityImport(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!canUpsertIdentity) {
      setIdentityError(
        identityClientId.trim().length === 0
          ? "Client ID is required"
          : "Client public key must be exactly 64 hex characters",
      );
      return;
    }
    setIdentityError(null);
    setPendingConfirmation("agent-identity");
  }

  async function confirmIdentityImport() {
    if (!canUpsertIdentity) {
      return;
    }
    setIdentityPending(true);
    setIdentityError(null);
    try {
      const response = await onUpsertAgentIdentity({
        client_id: identityClientId.trim(),
        client_public_key_hex: identityPublicKeyHex.trim().toLowerCase(),
        display_name: identityDisplayName.trim() || null,
        tags: parseListInput(identityTags),
        replace_existing_key: identityReplaceExistingKey,
        confirmed: true,
      });
      setCreatedIdentity(response);
      setIdentityClientId("");
      setIdentityPublicKeyHex("");
      setIdentityDisplayName("");
      setIdentityTags("");
      setIdentityReplaceExistingKey(false);
      setPendingConfirmation(null);
    } catch (actionError) {
      setIdentityError(
        actionError instanceof Error
          ? actionError.message
          : "Agent identity import failed",
      );
    } finally {
      setIdentityPending(false);
    }
  }

  function requestClientKeyRevoke(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!canRevokeClientKey) {
      return;
    }
    setRevokeError(null);
    setPendingConfirmation("key-revoke");
  }

  async function confirmClientKeyRevoke() {
    if (!canRevokeClientKey) {
      return;
    }
    setRevokePending(true);
    setRevokeError(null);
    try {
      await onRevokeClientKey(
        revokeClientId.trim(),
        revokeReason.trim() || null,
        true,
      );
      setRevokeClientId("");
      setRevokeReason("");
      setPendingConfirmation(null);
    } catch (actionError) {
      setRevokeError(
        actionError instanceof Error
          ? actionError.message
          : "Client key revoke failed",
      );
    } finally {
      setRevokePending(false);
    }
  }

  return (
    <div className="workspace accessWorkspace">
      <section className="fleetPanel accessMain">
        <div className="sectionHeader heroHeader compactHeroHeader">
          <div>
            <h2>Access control</h2>
            <span>
              {error ??
                (loading
                  ? "Refreshing access records"
                  : "Operators, direct gateway agent identities, and active sessions")}
            </span>
          </div>
          <button
            className="secondaryAction"
            disabled={loading}
            onClick={() => void onRefresh()}
            type="button"
          >
            <RefreshCw size={17} />
            Refresh
          </button>
        </div>

        <div className="accessSummaryCards denseSummaryCards">
          <SummaryCard
            label="Operator"
            value={operator?.username ?? "anonymous"}
            detail={operator?.role ?? "not signed in"}
          />
          <SummaryCard
            label="Session"
            value={sessionState}
            detail={`token in ${tokenStorageState}`}
          />
          <SummaryCard
            label="Privilege"
            value={vaultState}
            detail="request-bound assertions only"
          />
          <SummaryCard
            label="Gateway"
            value={`${activeGatewaySessions} active`}
            detail={`${gatewaySessions.length} recent sessions`}
          />
          <SummaryCard
            label="Agent identities"
            value={`${keyLifecycleReport?.direct_identity_client_count ?? lifecycleClients.length}`}
            detail={`${revokedClientCount} revoked/current blocked`}
          />
        </div>

        <nav className="subpanelTabs accessTabs" aria-label="Access subpanels">
          {accessSubpages.map((subpage) => (
            <button
              className={activeSubpage === subpage ? "active" : ""}
              key={subpage}
              onClick={() => setActiveSubpage(subpage)}
              type="button"
            >
              {subpage}
            </button>
          ))}
        </nav>

        {activeSubpage === "Overview" && (
          <div className="workspaceSection accessOverviewGrid">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Security posture</h2>
                <span>Direct gateway identity, no panel claim workflow</span>
              </div>
              <div className="metricRows">
                <MetricRow
                  label="Server signing public key"
                  value={
                    keyLifecycleReport?.server_ed25519_public_key_configured
                      ? "configured"
                      : "not configured"
                  }
                />
                <MetricRow
                  label="Direct identity clients"
                  value={String(
                    keyLifecycleReport?.direct_identity_client_count ??
                      lifecycleClients.length,
                  )}
                />
                <MetricRow
                  label="Revoked key records"
                  value={String(
                    keyLifecycleReport?.revocation_count ??
                      clientKeyRevocations.length,
                  )}
                />
                <MetricRow
                  label="Current keys blocked"
                  value={String(
                    keyLifecycleReport?.current_key_revoked_count ?? 0,
                  )}
                />
                <MetricRow label="WebSocket" value={wsState} />
                <MetricRow label="Last event" value={lastLiveEvent || "none"} />
              </div>
            </section>
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Operational model</h2>
                <span>
                  Gateway endpoints are selected from provisioned endpoint
                  priorities
                </span>
              </div>
              <p className="formNoteText">
                Agents are installed with their Noise private key, client ID,
                trusted gateway server key, and prioritized gateway endpoints.
                The browser panel can register the matching public identity for
                inventory visibility, but it never issues claim tokens and
                agents never call the panel for runtime configuration.
              </p>
            </section>
          </div>
        )}

        {activeSubpage === "Operators" && (
          <div className="workspaceSection">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Operators</h2>
                <span>{operators.length} configured</span>
              </div>
              <CrudPager
                fields={[
                  { label: "Username", value: (item) => item.username },
                  { label: "Role", value: (item) => item.role },
                  { label: "Scopes", value: (item) => item.scopes.join(" ") },
                ]}
                itemLabel="operators"
                items={operators}
                pageSize={8}
                storageKey="vpsman.access.operators"
                title="Operators"
              >
                {(pagedOperators) => (
                  <table className="dataTable compactTable">
                    <thead>
                      <tr>
                        <th>Username</th>
                        <th>Role</th>
                        <th>Scopes</th>
                        <th>TOTP</th>
                      </tr>
                    </thead>
                    <tbody>
                      {pagedOperators.map((item) => (
                        <tr key={item.id}>
                          <td>{item.username}</td>
                          <td>
                            <span
                              className={`statusPill ${statusClass(item.role)}`}
                            >
                              {item.role}
                            </span>
                          </td>
                          <td>
                            {item.scopes.length > 0
                              ? item.scopes.join(", ")
                              : "all role defaults"}
                          </td>
                          <td>{item.totp_enabled ? "enabled" : "off"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </CrudPager>
            </section>
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Operator sessions</h2>
                <span>{operatorSessions.length} recent sessions</span>
              </div>
              <CrudPager
                fields={[
                  {
                    label: "Operator",
                    value: (item) => item.operator_username,
                  },
                  { label: "Role", value: (item) => item.operator_role },
                  { label: "Revoked", value: (item) => item.revoked },
                ]}
                itemLabel="sessions"
                items={operatorSessions}
                pageSize={8}
                storageKey="vpsman.access.operatorSessions"
                title="Operator sessions"
              >
                {(pagedSessions) => (
                  <table className="dataTable compactTable">
                    <thead>
                      <tr>
                        <th>Operator</th>
                        <th>Created</th>
                        <th>Expires</th>
                        <th>Status</th>
                        <th>Action</th>
                      </tr>
                    </thead>
                    <tbody>
                      {pagedSessions.map((session) => (
                        <tr key={session.id}>
                          <td>{session.operator_username}</td>
                          <td>{formatTime(session.created_at)}</td>
                          <td>{formatTime(session.expires_at)}</td>
                          <td>
                            {session.current
                              ? "current"
                              : session.revoked
                                ? "revoked"
                                : "active"}
                          </td>
                          <td>
                            <button
                              className="secondaryAction compactAction dangerAction"
                              disabled={
                                !canManageOperators ||
                                session.current ||
                                session.revoked
                              }
                              onClick={() =>
                                void onRevokeOperatorSession(session.id)
                              }
                              type="button"
                            >
                              <UserX size={14} />
                              Revoke
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </CrudPager>
            </section>
          </div>
        )}

        {activeSubpage === "Privilege unlock" && (
          <div className="workspaceSection accessOverviewGrid">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Privilege unlock</h2>
                <span>
                  Kept in the browser; API receives only request-bound
                  assertions
                </span>
              </div>
              <PrivilegeVaultBox
                lastPayloadHash={privilegeMaterial ? "unlocked" : null}
                onPrivilegeMaterialChange={setPrivilegeMaterial}
                onVaultAvailabilityChange={setVaultAvailable}
                privilegeMaterial={privilegeMaterial}
              />
            </section>
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>TOTP</h2>
                <span>
                  {totpError ??
                    (operator?.totp_enabled ? "enabled" : "optional hardening")}
                </span>
              </div>
              <div className="sideForm standaloneForm">
                <label>
                  <span>Current password</span>
                  <input
                    aria-label="TOTP password"
                    onChange={(event) => setTotpPassword(event.target.value)}
                    type="password"
                    value={totpPassword}
                  />
                </label>
                <label>
                  <span>Authenticator code</span>
                  <input
                    aria-label="TOTP code"
                    onChange={(event) => setTotpCode(event.target.value)}
                    value={totpCode}
                  />
                </label>
                {totpSetup && (
                  <div className="inlineSecret">
                    <strong>{totpSetup.secret_base32}</strong>
                    <small>{totpSetup.otpauth_uri}</small>
                  </div>
                )}
                <div className="actionRow">
                  <button
                    className="secondaryAction"
                    disabled={totpPending || !totpPassword}
                    onClick={() => void setupTotp()}
                    type="button"
                  >
                    <ShieldCheck size={17} />
                    Setup TOTP
                  </button>
                  <button
                    className="secondaryAction"
                    disabled={totpPending || !totpPassword || !totpCode}
                    onClick={() => void confirmTotp()}
                    type="button"
                  >
                    <Save size={17} />
                    Confirm
                  </button>
                  <button
                    className="secondaryAction dangerAction"
                    disabled={totpPending || !totpPassword || !totpCode}
                    onClick={() => void disableTotp()}
                    type="button"
                  >
                    <Trash2 size={17} />
                    Disable
                  </button>
                </div>
              </div>
            </section>
          </div>
        )}

        {activeSubpage === "VPS keys" && (
          <div className="workspaceSection">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Gateway agent identities</h2>
                <span>Registered public keys and revocation state</span>
              </div>
              <CrudPager
                fields={[
                  {
                    label: "Client",
                    value: (item) => `${item.display_name} ${item.client_id}`,
                  },
                  { label: "Status", value: (item) => item.status },
                  {
                    label: "Key",
                    value: (item) => item.current_public_key_sha256_hex ?? "",
                  },
                  {
                    label: "Revoked",
                    value: (item) => item.current_key_revoked,
                  },
                ]}
                itemLabel="identities"
                items={lifecycleClients}
                pageSize={10}
                storageKey="vpsman.access.agentIdentities"
                title="Gateway agent identities"
              >
                {(pagedClients) => (
                  <table className="dataTable compactTable">
                    <thead>
                      <tr>
                        <th>VPS</th>
                        <th>Status</th>
                        <th>Current key</th>
                        <th>Latest revocation</th>
                      </tr>
                    </thead>
                    <tbody>
                      {pagedClients.map((client) => (
                        <tr key={client.client_id}>
                          <td title={client.client_id}>
                            {formatVpsName(
                              {
                                client_id: client.client_id,
                                display_name: client.display_name,
                              },
                              vpsNameDisplayMode,
                            )}
                          </td>
                          <td>
                            <span
                              className={`statusPill ${statusClass(client.status)}`}
                            >
                              {client.current_key_revoked
                                ? "blocked"
                                : client.status}
                            </span>
                          </td>
                          <td>
                            {client.current_public_key_sha256_hex
                              ? shortHash(client.current_public_key_sha256_hex)
                              : "no key"}
                          </td>
                          <td>
                            {client.latest_revoked_at
                              ? `${formatTime(client.latest_revoked_at)} ${client.latest_revocation_reason ?? ""}`
                              : "none"}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </CrudPager>
            </section>

            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Client key revocations</h2>
                <span>{clientKeyRevocations.length} retained records</span>
              </div>
              <CrudPager
                fields={[
                  { label: "Client", value: (item) => item.client_id },
                  { label: "Key", value: (item) => item.public_key_sha256_hex },
                  { label: "Reason", value: (item) => item.reason ?? "" },
                ]}
                itemLabel="revocations"
                items={clientKeyRevocations}
                pageSize={8}
                storageKey="vpsman.access.revocations"
                title="Client key revocations"
              >
                {(pagedRevocations) => (
                  <table className="dataTable compactTable">
                    <thead>
                      <tr>
                        <th>VPS</th>
                        <th>Key hash</th>
                        <th>Reason</th>
                        <th>Created</th>
                      </tr>
                    </thead>
                    <tbody>
                      {pagedRevocations.map((revocation) => (
                        <tr key={revocation.id}>
                          <td title={revocation.client_id}>
                            {lifecycleClientLabel(revocation.client_id)}
                          </td>
                          <td>{shortHash(revocation.public_key_sha256_hex)}</td>
                          <td>{revocation.reason ?? "operator request"}</td>
                          <td>{formatTime(revocation.created_at)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </CrudPager>
            </section>
          </div>
        )}

        {activeSubpage === "Gateway" && (
          <div className="workspaceSection">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Gateway sessions</h2>
                <span>
                  {activeGatewaySessions} active / {gatewaySessions.length}{" "}
                  recent
                </span>
              </div>
              <CrudPager
                fields={[
                  { label: "Client", value: (item) => item.client_id },
                  { label: "Gateway", value: (item) => item.gateway_id },
                  { label: "Status", value: (item) => item.status },
                ]}
                itemLabel="gateway sessions"
                items={gatewaySessions}
                pageSize={12}
                storageKey="vpsman.access.gatewaySessions"
                title="Gateway sessions"
              >
                {(pagedSessions) => (
                  <table className="dataTable compactTable">
                    <thead>
                      <tr>
                        <th>VPS</th>
                        <th>Gateway</th>
                        <th>Status</th>
                        <th>Last seen</th>
                        <th>Noise key</th>
                        <th>End reason</th>
                      </tr>
                    </thead>
                    <tbody>
                      {pagedSessions.map((session) => (
                        <tr key={session.id}>
                          <td title={session.client_id}>
                            {lifecycleClientLabel(session.client_id)}
                          </td>
                          <td>{session.gateway_id}</td>
                          <td>
                            <span
                              className={`statusPill ${statusClass(session.status)}`}
                            >
                              {session.status}
                            </span>
                          </td>
                          <td>{formatTime(session.last_seen_at)}</td>
                          <td>
                            {session.noise_public_key_hex
                              ? shortHash(session.noise_public_key_hex)
                              : "n/a"}
                          </td>
                          <td>
                            {session.end_reason ??
                              (session.ended_at ? "ended" : "active")}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </CrudPager>
            </section>
          </div>
        )}
      </section>

      <aside className="fleetPanel accessInspector">
        <div className="accessConfigHeading">
          <strong>
            {activeSubpage === "VPS keys"
              ? "Direct identity actions"
              : "Access actions"}
          </strong>
          <span>
            {canManageOperators ? "Admin controls" : "Admin role required"}
          </span>
        </div>

        <div
          className="sectionHeader compact"
          hidden={activeSubpage !== "Operators"}
        >
          <h2>Create operator</h2>
          <span>{operatorActionError ?? "Role and scope assignment"}</span>
        </div>
        <form
          className="sideForm"
          hidden={activeSubpage !== "Operators"}
          onSubmit={(event) => void createOperator(event)}
        >
          <label>
            <span>Username</span>
            <input
              aria-label="New operator username"
              disabled={!canManageOperators || operatorActionPending}
              onChange={(event) => setNewOperatorUsername(event.target.value)}
              value={newOperatorUsername}
            />
          </label>
          <label>
            <span>Password</span>
            <input
              aria-label="New operator password"
              disabled={!canManageOperators || operatorActionPending}
              minLength={12}
              onChange={(event) => setNewOperatorPassword(event.target.value)}
              type="password"
              value={newOperatorPassword}
            />
          </label>
          <label>
            <span>Role</span>
            <select
              aria-label="New operator role"
              disabled={!canManageOperators || operatorActionPending}
              onChange={(event) => setNewOperatorRole(event.target.value)}
              value={newOperatorRole}
            >
              <option value="operator">Operator</option>
              <option value="admin">Admin</option>
            </select>
          </label>
          <label>
            <span>Scopes</span>
            <input
              aria-label="New operator scopes"
              disabled={!canManageOperators || operatorActionPending}
              onChange={(event) => setNewOperatorScopes(event.target.value)}
              placeholder="inventory:write, jobs:run"
              value={newOperatorScopes}
            />
          </label>
          <button
            className="secondaryAction"
            disabled={!canCreateOperator}
            type="submit"
          >
            <UserPlus size={17} />
            Create operator
          </button>
        </form>

        <div
          className="sectionHeader compact"
          hidden={activeSubpage !== "VPS keys"}
        >
          <h2>Import identity</h2>
          <span>
            {identityError ?? "Gateway-issued client ID and public key"}
          </span>
        </div>
        <form
          className="sideForm"
          hidden={activeSubpage !== "VPS keys"}
          onSubmit={requestIdentityImport}
        >
          <label>
            <span>Client ID</span>
            <input
              aria-label="Agent identity client ID"
              disabled={!canManageOperators || identityPending}
              onChange={(event) => setIdentityClientId(event.target.value)}
              placeholder="vps-edge-nrt-04"
              value={identityClientId}
            />
          </label>
          <label className="wideField">
            <span>Noise public key hex</span>
            <textarea
              aria-label="Agent identity public key hex"
              disabled={!canManageOperators || identityPending}
              onChange={(event) => setIdentityPublicKeyHex(event.target.value)}
              placeholder="64 hex characters"
              rows={3}
              value={identityPublicKeyHex}
            />
          </label>
          <label>
            <span>Display name</span>
            <input
              aria-label="Agent identity display name"
              disabled={!canManageOperators || identityPending}
              onChange={(event) => setIdentityDisplayName(event.target.value)}
              placeholder="edge-nrt-04"
              value={identityDisplayName}
            />
          </label>
          <label>
            <span>Tags</span>
            <input
              aria-label="Agent identity tags"
              disabled={!canManageOperators || identityPending}
              onChange={(event) => setIdentityTags(event.target.value)}
              placeholder="country:JP, role:edge"
              value={identityTags}
            />
          </label>
          <label className="inlineCheck">
            <input
              checked={identityReplaceExistingKey}
              disabled={!canManageOperators || identityPending}
              onChange={(event) =>
                setIdentityReplaceExistingKey(event.target.checked)
              }
              type="checkbox"
            />
            <span>Replace existing current key</span>
          </label>
          <button
            className="secondaryAction"
            disabled={!canUpsertIdentity}
            type="submit"
          >
            <Fingerprint size={17} />
            Import gateway identity
          </button>
          {createdIdentity && (
            <div className="formNote">
              <strong>{createdIdentity.display_name}</strong>
              <span>
                {createdIdentity.client_id} /{" "}
                {shortHash(createdIdentity.current_public_key_sha256_hex)}
              </span>
            </div>
          )}
        </form>

        <div
          className="sectionHeader compact"
          hidden={activeSubpage !== "VPS keys"}
        >
          <h2>Revoke key</h2>
          <span>{revokeError ?? "Block the current gateway key"}</span>
        </div>
        <form
          className="sideForm"
          hidden={activeSubpage !== "VPS keys"}
          onSubmit={requestClientKeyRevoke}
        >
          <label>
            <span>VPS ID</span>
            <input
              aria-label="VPS key revoke VPS ID"
              disabled={!canManageOperators || revokePending}
              onChange={(event) => setRevokeClientId(event.target.value)}
              placeholder="VPS ID from details"
              value={revokeClientId}
            />
          </label>
          <label>
            <span>Reason</span>
            <input
              aria-label="VPS key revoke reason"
              disabled={!canManageOperators || revokePending}
              onChange={(event) => setRevokeReason(event.target.value)}
              placeholder="lost host, rebuild, or operator request"
              value={revokeReason}
            />
          </label>
          <button
            className="secondaryAction dangerAction"
            disabled={!canRevokeClientKey}
            type="submit"
          >
            <Ban size={17} />
            Revoke current key
          </button>
        </form>

        <div
          className="accessConfigHeading"
          hidden={activeSubpage !== "Gateway"}
        >
          <strong>Gateway model</strong>
          <span>Agents connect only to gateways</span>
        </div>
        <div className="timeline" hidden={activeSubpage !== "Gateway"}>
          <Wifi size={18} />
          <div>
            <strong>Endpoint priority controls routing</strong>
            <span>
              Use DNS and endpoint priorities in agent config; no panel-side
              endpoint lookup is used by agents.
            </span>
          </div>
        </div>

        <div
          className="accessConfigHeading"
          hidden={activeSubpage !== "Operators"}
        >
          <strong>Local panel state</strong>
          <span>Browser session and privilege vault controls</span>
        </div>
        <div className="sideForm" hidden={activeSubpage !== "Operators"}>
          <button
            className="secondaryAction"
            onClick={onClearSession}
            type="button"
          >
            <KeyRound size={17} />
            Clear bearer session
          </button>
          <button
            className="secondaryAction dangerAction"
            disabled={!vaultAvailable}
            onClick={clearVault}
            type="button"
          >
            <Trash2 size={17} />
            Clear privilege vault
          </button>
        </div>

        <div
          className="timeline"
          hidden={
            activeSubpage === "Operators" ||
            activeSubpage === "VPS keys" ||
            activeSubpage === "Gateway"
          }
        >
          <LockKeyhole size={18} />
          <div>
            <strong>Deny by default</strong>
            <span>
              Mutating work still requires explicit confirmation and privilege
              material when the server marks it privileged.
            </span>
          </div>
        </div>
      </aside>

      <ConfirmationPrompt
        confirmLabel="Import identity"
        detail="This registers a gateway-issued client public key for inventory and key lifecycle management. It does not create a token and does not give the agent a panel endpoint."
        items={[
          { label: "Client", value: identityClientId.trim() },
          {
            label: "Public key",
            value: shortHash(identityPublicKeyHex.trim()),
          },
          {
            label: "Replace key",
            value: identityReplaceExistingKey ? "yes" : "no",
          },
        ]}
        onCancel={() => setPendingConfirmation(null)}
        onConfirm={() => void confirmIdentityImport()}
        open={pendingConfirmation === "agent-identity"}
        pending={identityPending}
        title="Confirm direct gateway identity import"
      />
      <ConfirmationPrompt
        confirmLabel="Revoke key"
        detail="The current stored public key is revoked, the VPS is hidden as revoked, and active gateway sessions are ended. Revoked or deleted identities cannot be reused through direct import."
        items={[
          { label: "VPS", value: revokeClientId.trim() },
          { label: "Reason", value: revokeReason.trim() || "operator request" },
        ]}
        onCancel={() => setPendingConfirmation(null)}
        onConfirm={() => void confirmClientKeyRevoke()}
        open={pendingConfirmation === "key-revoke"}
        pending={revokePending}
        title="Confirm current key revocation"
        tone="danger"
      />
    </div>
  );
}

function SummaryCard({
  detail,
  label,
  value,
}: {
  detail: string;
  label: string;
  value: string;
}) {
  return (
    <div className="summaryCard">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function MetricRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="metricRow">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function parseListInput(value: string): string[] {
  return Array.from(
    new Set(
      value
        .split(/[\n,]+/)
        .map((item) => item.trim())
        .filter(Boolean),
    ),
  );
}

function isFixedHex32(value: string): boolean {
  return /^[0-9a-fA-F]{64}$/.test(value.trim());
}
