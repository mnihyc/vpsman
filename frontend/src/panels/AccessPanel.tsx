import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Ban, Copy, Fingerprint, KeyRound, LockKeyhole, RefreshCw, RotateCcw, ShieldCheck, Trash2, UserPlus, UserX, Wifi } from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { clearPrivilegeVault, hasPrivilegeVault } from "../vault";
import { CrudPager } from "../components/CrudPager";
import { renderEnrollmentInstallCommand } from "../enrollmentInstallCommand";
import { usePanelDisplaySettings } from "../panelDisplay";
import type {
  GatewaySessionRecord,
  OperatorSessionRecord,
  OperatorView,
  TotpSetupResponse,
} from "../types";
import type {
  ClientKeyRevocationView,
  CreateEnrollmentTokenRequest,
  CreateEnrollmentTokenResponse,
  EnrollmentTokenPurpose,
  EnrollmentTokenView,
  KeyLifecycleReportView,
} from "../typesAccess";
import type { PrivilegeMaterial } from "../privilege";
import {
  clientDisplayNameFromMap,
  clientLifecycleNameMap,
  formatVpsName,
  formatTime,
  shortHash,
  shortId,
  statusClass,
} from "../utils";

const DEFAULT_UNMANAGED_UPDATE_VERSION_URL = "https://github.com/mnihyc/vpsman/releases/latest/download/version.json";
const accessSubpages = ["Overview", "Operators", "Privilege unlock", "VPS clients", "Gateway"] as const;

type AccessSubpage = (typeof accessSubpages)[number];
type AccessConfirmationAction = "rebuild-token" | "key-revoke";

type AccessPanelProps = {
  activeSubpage: string;
  apiToken: string;
  error: string | null;
  gatewaySessions: GatewaySessionRecord[];
  lastLiveEvent: string;
  loading: boolean;
  onClearSession: () => void;
  onCreateEnrollmentToken: (request: CreateEnrollmentTokenRequest) => Promise<CreateEnrollmentTokenResponse>;
  onCreateOperator: (username: string, role: string, password: string, scopes: string[]) => Promise<void>;
  onConfirmTotp: (password: string, code: string) => Promise<void>;
  onDisableTotp: (password: string, code: string) => Promise<void>;
  onRefresh: () => Promise<void>;
  onRevokeClientKey: (clientId: string, reason: string | null, confirmed: boolean) => Promise<void>;
  onRevokeOperatorSession: (sessionId: string) => Promise<void>;
  onSetupTotp: (password: string) => Promise<TotpSetupResponse | null>;
  operator: OperatorView | null;
  clientKeyRevocations: ClientKeyRevocationView[];
  enrollmentTokens: EnrollmentTokenView[];
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
      return "VPS clients";
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
  onCreateEnrollmentToken,
  onCreateOperator,
  onDisableTotp,
  onRefresh,
  onRevokeClientKey,
  onRevokeOperatorSession,
  onSetupTotp,
  operator,
  clientKeyRevocations,
  enrollmentTokens,
  keyLifecycleReport,
  operatorSessions,
  operators,
  privilegeMaterial,
  sessionVaultAvailable,
  setPrivilegeMaterial,
  wsState,
}: AccessPanelProps) {
  const { preferences, vpsNameDisplayMode } = usePanelDisplaySettings();
  const [activeSubpage, setActiveSubpage] = useState<AccessSubpage>(accessSubpageFromRoute(routeSubpage));
  const [vaultAvailable, setVaultAvailable] = useState(() => hasPrivilegeVault());
  const [newOperatorUsername, setNewOperatorUsername] = useState("");
  const [newOperatorPassword, setNewOperatorPassword] = useState("");
  const [newOperatorRole, setNewOperatorRole] = useState("operator");
  const [newOperatorScopes, setNewOperatorScopes] = useState("");
  const [operatorActionError, setOperatorActionError] = useState<string | null>(null);
  const [operatorActionPending, setOperatorActionPending] = useState(false);
  const [totpPassword, setTotpPassword] = useState("");
  const [totpCode, setTotpCode] = useState("");
  const [totpSetup, setTotpSetup] = useState<TotpSetupResponse | null>(null);
  const [totpPending, setTotpPending] = useState(false);
  const [totpError, setTotpError] = useState<string | null>(null);
  const [tokenPurpose, setTokenPurpose] = useState<EnrollmentTokenPurpose>("provision");
  const [tokenClientId, setTokenClientId] = useState("");
  const [tokenTtlSecs, setTokenTtlSecs] = useState("1800");
  const [tokenTags, setTokenTags] = useState("");
  const [tokenDisplayName, setTokenDisplayName] = useState("");
  const [tokenUnmanagedUpdateEnabled, setTokenUnmanagedUpdateEnabled] = useState(true);
  const [tokenUnmanagedUpdateVersionUrl, setTokenUnmanagedUpdateVersionUrl] = useState(DEFAULT_UNMANAGED_UPDATE_VERSION_URL);
  const [tokenUnmanagedUpdateIntervalSecs, setTokenUnmanagedUpdateIntervalSecs] = useState("86400");
  const [tokenUnmanagedUpdateJitterSecs, setTokenUnmanagedUpdateJitterSecs] = useState("86400");
  const [tokenUnmanagedUpdateActivate, setTokenUnmanagedUpdateActivate] = useState(true);
  const [tokenUnmanagedUpdateRestartAgent, setTokenUnmanagedUpdateRestartAgent] = useState(true);
  const [tokenPending, setTokenPending] = useState(false);
  const [tokenError, setTokenError] = useState<string | null>(null);
  const [createdToken, setCreatedToken] = useState<CreateEnrollmentTokenResponse | null>(null);
  const [installCommandCopied, setInstallCommandCopied] = useState(false);
  const [revokeClientId, setRevokeClientId] = useState("");
  const [revokeReason, setRevokeReason] = useState("");
  const [revokePending, setRevokePending] = useState(false);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [pendingConfirmation, setPendingConfirmation] = useState<AccessConfirmationAction | null>(null);
  const sessionState = apiToken ? "Bearer session active" : "No bearer session";
  const vaultState = privilegeMaterial ? "Privilege unlocked" : vaultAvailable ? "Encrypted privilege vault present" : "No privilege vault";
  const tokenStorageState = apiToken ? (sessionVaultAvailable ? "encrypted vault" : "memory only") : "none";
  const canManageOperators = operator?.role === "admin";
  const canCreateOperator =
    canManageOperators && newOperatorUsername.trim().length > 0 && newOperatorPassword.length >= 12 && !operatorActionPending;
  const canCreateEnrollmentToken =
    canManageOperators &&
    !tokenPending &&
    Number.parseInt(tokenTtlSecs, 10) >= 60 &&
    Number.parseInt(tokenUnmanagedUpdateIntervalSecs, 10) >= 300 &&
    Number.parseInt(tokenUnmanagedUpdateJitterSecs, 10) >= 0 &&
    (!tokenUnmanagedUpdateEnabled || tokenUnmanagedUpdateVersionUrl.trim().length > 0) &&
    (tokenPurpose === "provision" || tokenClientId.trim().length > 0);
  const canRevokeClientKey = canManageOperators && revokeClientId.trim().length > 0 && !revokePending;
  const lifecycleClients = keyLifecycleReport?.clients ?? [];
  const lifecycleNameById = useMemo(
    () => clientLifecycleNameMap(lifecycleClients, vpsNameDisplayMode),
    [lifecycleClients, vpsNameDisplayMode],
  );
  const lifecycleClientLabel = (clientId: string | null | undefined) => clientDisplayNameFromMap(clientId, lifecycleNameById);
  const enrollmentInstallRender = useMemo(() => {
    const origin = typeof window === "undefined" ? "https://panel.example.com" : window.location.origin;
    if (createdToken) {
      return renderEnrollmentInstallCommand(preferences.enrollment_install_command_template, {
        apiUrl: origin,
        installMode: "root",
        token: createdToken.token,
      });
    }
    return renderEnrollmentInstallCommand(preferences.enrollment_install_command_template, {
      apiUrl: origin,
      installMode: "root",
      token: null,
    });
  }, [createdToken, preferences.enrollment_install_command_template]);
  const enrollmentInstallCommand = enrollmentInstallRender.command ?? "";

  useEffect(() => {
    setActiveSubpage(accessSubpageFromRoute(routeSubpage));
  }, [routeSubpage]);

  useEffect(() => {
    setInstallCommandCopied(false);
  }, [enrollmentInstallCommand]);

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
        parseScopeInput(newOperatorScopes),
      );
      setNewOperatorUsername("");
      setNewOperatorPassword("");
      setNewOperatorRole("operator");
      setNewOperatorScopes("");
    } catch (error) {
      setOperatorActionError(error instanceof Error ? error.message : "Operator creation failed");
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
    } catch (error) {
      setTotpError(error instanceof Error ? error.message : "TOTP setup failed");
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
    } catch (error) {
      setTotpError(error instanceof Error ? error.message : "TOTP confirmation failed");
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
    } catch (error) {
      setTotpError(error instanceof Error ? error.message : "TOTP disable failed");
    } finally {
      setTotpPending(false);
    }
  }

  async function createEnrollmentToken(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!canCreateEnrollmentToken) {
      return;
    }
    if (tokenPurpose === "rebuild_reenrollment") {
      setPendingConfirmation("rebuild-token");
      return;
    }
    await executeCreateEnrollmentToken(false);
  }

  async function executeCreateEnrollmentToken(confirmedReenrollment: boolean) {
    setTokenPending(true);
    setTokenError(null);
    setCreatedToken(null);
    setInstallCommandCopied(false);
    try {
      const response = await onCreateEnrollmentToken({
        ttl_secs: Number.parseInt(tokenTtlSecs, 10),
        purpose: tokenPurpose,
        allowed_client_id: tokenPurpose === "rebuild_reenrollment" ? tokenClientId.trim() : null,
        confirmed_reenrollment: tokenPurpose === "rebuild_reenrollment" ? confirmedReenrollment : false,
        preserve_existing_assignments: true,
        default_tags: parseScopeInput(tokenTags),
        default_display_name: tokenDisplayName.trim() || null,
        unmanaged_update_enabled: tokenUnmanagedUpdateEnabled,
        unmanaged_update_version_url: tokenUnmanagedUpdateVersionUrl.trim() || null,
        unmanaged_update_interval_secs: Number.parseInt(tokenUnmanagedUpdateIntervalSecs, 10),
        unmanaged_update_jitter_secs: Number.parseInt(tokenUnmanagedUpdateJitterSecs, 10),
        unmanaged_update_activate: tokenUnmanagedUpdateActivate,
        unmanaged_update_restart_agent: tokenUnmanagedUpdateRestartAgent,
      });
      setCreatedToken(response);
      setTokenTags("");
      setTokenDisplayName("");
      if (tokenPurpose === "provision") {
        setTokenClientId("");
      }
    } catch (error) {
      setTokenError(error instanceof Error ? error.message : "Enrollment token creation failed");
    } finally {
      setTokenPending(false);
    }
  }

  async function revokeClientKey(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!canRevokeClientKey) {
      return;
    }
    setPendingConfirmation("key-revoke");
  }

  async function copyEnrollmentInstallCommand() {
    if (!enrollmentInstallCommand) {
      return;
    }
    await navigator.clipboard?.writeText(enrollmentInstallCommand);
    setInstallCommandCopied(true);
    window.setTimeout(() => setInstallCommandCopied(false), 1600);
  }

  async function executeRevokeClientKey() {
    setRevokePending(true);
    setRevokeError(null);
    try {
      await onRevokeClientKey(revokeClientId.trim(), revokeReason.trim() || null, true);
      setRevokeClientId("");
      setRevokeReason("");
    } catch (error) {
      setRevokeError(error instanceof Error ? error.message : "VPS key revoke failed");
    } finally {
      setRevokePending(false);
    }
  }

  async function confirmAccessAction() {
    const action = pendingConfirmation;
    if (!action) {
      return;
    }
    setPendingConfirmation(null);
    if (action === "rebuild-token") {
      await executeCreateEnrollmentToken(true);
    } else {
      await executeRevokeClientKey();
    }
  }

  return (
    <section className="workspace accessWorkspace">
      <div aria-label="Access sections" className="accessSubnav" role="tablist">
        {accessSubpages.map((subpage) => (
          <button
            aria-selected={activeSubpage === subpage}
            className={activeSubpage === subpage ? "selected" : ""}
            key={subpage}
            onClick={() => setActiveSubpage(subpage)}
            role="tab"
            type="button"
          >
            {subpage}
          </button>
        ))}
      </div>

      <div className="fleetPanel accessOverviewPanel" hidden={activeSubpage !== "Overview"}>
        <div className="sectionHeader">
          <div>
            <h2>Operator session</h2>
            <span>{error ?? (loading ? "Loading current operator" : sessionState)}</span>
          </div>
          <button className="secondaryAction" onClick={() => void onRefresh()} type="button">
            <RefreshCw size={17} />
            Refresh
          </button>
        </div>
        <div className="accessGrid">
          <div className="accessTile">
            <ShieldCheck size={20} />
            <span>Operator</span>
            <strong>{operator?.username ?? "memory-dev or unauthenticated"}</strong>
            <small>
              {operator ? `${operator.role} / ${operator.totp_enabled ? "TOTP on" : "TOTP off"} / ${shortId(operator.id)}` : "No operator record loaded"}
            </small>
          </div>
          <div className="accessTile">
            <KeyRound size={20} />
            <span>API session</span>
            <strong>{sessionState}</strong>
            <small>token {tokenStorageState}</small>
          </div>
          <div className="accessTile">
            <LockKeyhole size={20} />
            <span>Privilege unlock</span>
            <strong>{vaultState}</strong>
            <small>super password never leaves browser memory or encrypted local storage</small>
          </div>
          <div className="accessTile">
            <Wifi size={20} />
            <span>Live stream</span>
            <strong>{wsState}</strong>
            <small>last event {lastLiveEvent}</small>
          </div>
          <div className="accessTile">
            <Fingerprint size={20} />
            <span>VPS keys</span>
            <strong>{keyLifecycleReport ? `${keyLifecycleReport.enrolled_client_count} enrolled` : "not loaded"}</strong>
            <small>
              {keyLifecycleReport
                ? `${keyLifecycleReport.current_key_revoked_count} current revoked / ${keyLifecycleReport.revocation_count} records`
                : "admin report unavailable"}
            </small>
          </div>
          <div className="accessTile">
            <RotateCcw size={20} />
            <span>Rebuild tokens</span>
            <strong>{keyLifecycleReport ? `${keyLifecycleReport.active_rebuild_reenrollment_token_count} active` : "not loaded"}</strong>
            <small>
              {keyLifecycleReport
                ? `${keyLifecycleReport.rebuild_reenrollment_token_count} total / ${keyLifecycleReport.discovery_trusted_server_key_count} trusted discovery keys`
                : "re-enrollment state unavailable"}
            </small>
          </div>
        </div>
      </div>

      <div className="fleetPanel" hidden={activeSubpage !== "Privilege unlock"}>
        <div className="sectionHeader">
          <div>
            <h2>Privilege unlock</h2>
            <span>{privilegeMaterial ? "Unlocked in browser memory" : vaultAvailable ? "Encrypted vault locked" : "Manual privilege entry"}</span>
          </div>
        </div>
        <PrivilegeVaultBox
          lastPayloadHash={null}
          onPrivilegeMaterialChange={setPrivilegeMaterial}
          onVaultAvailabilityChange={setVaultAvailable}
          privilegeMaterial={privilegeMaterial}
        />
      </div>

      <div className="fleetPanel" hidden={activeSubpage !== "Operators"}>
        <div className="sectionHeader">
          <div>
            <h2>Operators</h2>
            <span>{operator?.role === "admin" ? `${operators.length} role records` : "Admin role required"}</span>
          </div>
        </div>
        <CrudPager
          fields={[
            { label: "Username", value: (record) => record.username },
            { label: "Role", value: (record) => record.role },
            { label: "Scopes", value: (record) => record.scopes.join(", ") },
            { label: "TOTP", value: (record) => record.totp_enabled },
            { label: "ID", value: (record) => record.id },
          ]}
          itemLabel="operators"
          items={operators}
          pageSize={8}
          title="Operator records"
          empty={
            <div className="emptyState">
              <ShieldCheck size={22} />
              <strong>No operator list available</strong>
              <span>Admin sessions can review role records here.</span>
            </div>
          }
        >
          {(operatorRows) => (
            <div className="table historyTable">
              <div className="historyRow operatorGrid heading">
                <span>Username</span>
                <span>Role</span>
                <span>Scopes</span>
                <span>ID</span>
              </div>
              {operatorRows.map((record) => (
                <div className="historyRow operatorGrid" key={record.id}>
                  <span className="historyPrimary">
                    <strong>{record.username}</strong>
                    <small>{record.id === operator?.id ? "current session" : record.totp_enabled ? "TOTP enabled" : "operator account"}</small>
                  </span>
                  <span className={`status ${record.role === "admin" ? "ok" : "warn"}`}>{record.role}</span>
                  <span className="monoValue">{record.scopes.join(", ") || "default"}</span>
                  <span className="monoValue">{shortId(record.id)}</span>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
      </div>

      <div className="fleetPanel" hidden={activeSubpage !== "Operators"}>
        <div className="sectionHeader">
          <div>
            <h2>Operator sessions</h2>
            <span>{operator?.role === "admin" ? `${operatorSessions.length} retained sessions` : "Admin role required"}</span>
          </div>
        </div>
        <CrudPager
          fields={[
            { label: "Operator", value: (session) => `${session.operator_username} ${session.operator_role}` },
            { label: "Status", value: (session) => (session.current ? "current" : session.revoked ? "revoked" : "active") },
            { label: "Created", value: (session) => session.created_at },
            { label: "ID", value: (session) => session.id },
          ]}
          itemLabel="sessions"
          items={operatorSessions}
          pageSize={8}
          title="Operator session records"
          empty={
            <div className="emptyState">
              <KeyRound size={22} />
              <strong>No operator sessions</strong>
              <span>Admin sessions can revoke retained bearer/refresh sessions here.</span>
            </div>
          }
        >
          {(sessionRows) => (
            <div className="table historyTable">
              <div className="historyRow operatorSessionGrid heading">
                <span>Operator</span>
                <span>Status</span>
                <span>Created</span>
                <span>Action</span>
              </div>
              {sessionRows.map((session) => (
                <div className="historyRow operatorSessionGrid" key={session.id}>
                  <span className="historyPrimary">
                    <strong>{session.operator_username}</strong>
                    <small>{session.operator_role} / {shortId(session.id)}</small>
                  </span>
                  <span className={`status ${session.revoked ? "warn" : "ok"}`}>
                    {session.current ? "current" : session.revoked ? "revoked" : "active"}
                  </span>
                  <span>{formatTime(session.created_at)}</span>
                  <button
                    className="secondaryAction compactAction dangerAction"
                    disabled={session.current || session.revoked || !canManageOperators}
                    onClick={() => void onRevokeOperatorSession(session.id)}
                    type="button"
                  >
                    <UserX size={15} />
                    Revoke
                  </button>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
      </div>

      <div className="fleetPanel" hidden={activeSubpage !== "VPS clients"}>
        <div className="sectionHeader">
          <div>
            <h2>Enrollment tokens</h2>
            <span>{canManageOperators ? `${enrollmentTokens.length} retained token policies` : "Admin role required"}</span>
          </div>
        </div>
        <CrudPager
          fields={[
            { label: "Token", value: (record) => record.token_prefix },
            { label: "Policy", value: (record) => `${record.purpose} ${enrollmentTokenDisplayName(record)}` },
            { label: "Status", value: (record) => (record.used_at ? `used ${lifecycleClientLabel(record.used_by_client_id)}` : "available") },
            { label: "Defaults", value: enrollmentTokenDefaultsLabel },
            { label: "Expires", value: (record) => record.expires_at },
          ]}
          itemLabel="tokens"
          items={enrollmentTokens}
          pageSize={8}
          title="Enrollment token records"
          empty={
            <div className="emptyState">
              <KeyRound size={22} />
              <strong>No enrollment tokens</strong>
              <span>Admin sessions can create provisioning and rebuild tokens.</span>
            </div>
          }
        >
          {(tokenRows) => (
            <div className="table historyTable">
              <div className="historyRow enrollmentTokenGrid heading">
                <span>Token</span>
                <span>Policy</span>
                <span>Status</span>
                <span>Defaults</span>
              </div>
              {tokenRows.map((record) => (
                <div className="historyRow enrollmentTokenGrid" key={record.id}>
                  <span className="historyPrimary">
                    <strong>{record.token_prefix}</strong>
                    <small>expires {formatTime(record.expires_at)}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{enrollmentPurposeLabel(record.purpose)}</strong>
                    <small>{enrollmentTokenDisplayName(record)}</small>
                  </span>
                  <span className={`status ${record.used_at ? "warn" : "ok"}`}>
                    {record.used_at ? `used by ${lifecycleClientLabel(record.used_by_client_id)}` : "available"}
                  </span>
                  <span className="monoValue">{enrollmentTokenDefaultsLabel(record) || "none"}</span>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
      </div>

      <div className="fleetPanel" hidden={activeSubpage !== "VPS clients"}>
        <div className="sectionHeader">
          <div>
            <h2>Key lifecycle</h2>
            <span>
              {keyLifecycleReport
                ? `${keyLifecycleReport.current_key_revoked_count} current revocations / ${clientKeyRevocations.length} recent records`
                : "Admin role required"}
            </span>
          </div>
        </div>
        <CrudPager
          fields={[
            { label: "VPS", value: (client) => formatVpsName(client, vpsNameDisplayMode) },
            { label: "Key", value: (client) => client.current_public_key_sha256_hex },
            { label: "Status", value: (client) => `${client.status} ${client.current_key_revoked ? "revoked" : ""}` },
            { label: "Revoked", value: (client) => `${client.latest_revoked_at ?? ""} ${client.latest_revocation_reason ?? ""}` },
          ]}
          itemLabel="VPSs"
          items={lifecycleClients}
          pageSize={10}
          title="Key lifecycle records"
          empty={
            <div className="emptyState">
              <Fingerprint size={22} />
              <strong>No lifecycle report</strong>
              <span>Admin sessions can review enrolled keys and revocation records here.</span>
            </div>
          }
        >
          {(clientRows) => (
            <div className="table historyTable">
              <div className="historyRow keyLifecycleGrid heading">
                <span>VPS</span>
                <span>Current key</span>
                <span>Status</span>
                <span>Latest revoke</span>
              </div>
              {clientRows.map((client) => (
                <div className="historyRow keyLifecycleGrid" key={client.client_id}>
                  <span className="historyPrimary">
                    <strong>{formatVpsName(client, vpsNameDisplayMode)}</strong>
                    <small>{client.status}</small>
                  </span>
                  <span className="monoValue">{client.current_public_key_sha256_hex ? shortHash(client.current_public_key_sha256_hex) : "no key"}</span>
                  <span className={`status ${client.current_key_revoked ? "warn" : statusClass(client.status)}`}>
                    {client.current_key_revoked ? "revoked" : client.status}
                  </span>
                  <span className="historyPrimary">
                    <strong>{client.latest_revoked_at ? formatTime(client.latest_revoked_at) : "none"}</strong>
                    <small>{client.latest_revocation_reason ?? "no current-key revocation"}</small>
                  </span>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
      </div>

      <div className="fleetPanel" hidden={activeSubpage !== "Gateway"}>
        <div className="sectionHeader">
          <div>
            <h2>Gateway sessions</h2>
            <span>{gatewaySessions.length} retained TCP lifecycle records</span>
          </div>
          <button className="secondaryAction" disabled={loading} onClick={() => void onRefresh()} type="button">
            <RefreshCw size={17} />
            Refresh
          </button>
        </div>
        <CrudPager
          fields={[
            { label: "VPS", value: (session) => lifecycleClientLabel(session.client_id) },
            { label: "Status", value: (session) => `${session.status} ${session.end_reason ?? ""}` },
            { label: "Gateway", value: (session) => session.gateway_id },
            { label: "Last seen", value: (session) => session.last_seen_at },
          ]}
          itemLabel="sessions"
          items={gatewaySessions}
          pageSize={10}
          title="Gateway session records"
          empty={
            <div className="emptyState">
              <Wifi size={22} />
              <strong>No gateway sessions</strong>
              <span>Noise-over-TCP agent session starts and ends will appear here.</span>
            </div>
          }
        >
          {(gatewayRows) => (
            <div className="table historyTable">
              <div className="historyRow heading gatewaySessionGrid">
                <span>VPS</span>
                <span>Status</span>
                <span>Gateway</span>
                <span>Last seen</span>
              </div>
              {gatewayRows.map((session) => (
                <div className="historyRow gatewaySessionGrid" key={session.id}>
                  <span className="historyPrimary">
                    <strong>{lifecycleClientLabel(session.client_id)}</strong>
                    <small>{shortId(session.id)}</small>
                  </span>
                  <span className={`status ${statusClass(session.status)}`}>{session.status}</span>
                  <span className="historyPrimary">
                    <strong>{session.gateway_id}</strong>
                    <small>{session.end_reason ?? "active route"}</small>
                  </span>
                  <span>{formatTime(session.last_seen_at)}</span>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
      </div>

      <aside className="inspector accessInspector" hidden={activeSubpage === "Overview" || activeSubpage === "Privilege unlock"}>
        <div className="accessConfigHeading" hidden={activeSubpage !== "Operators"}>
          <strong>Operator configuration</strong>
          <span>Operators, TOTP, and retained sessions</span>
        </div>
        <div className="sectionHeader compact" hidden={activeSubpage !== "Operators"}>
          <h2>Create operator</h2>
          <span>{operatorActionError ?? (canManageOperators ? "Admin role" : "Admin role required")}</span>
        </div>
        <form className="sideForm" hidden={activeSubpage !== "Operators"} onSubmit={(event) => void createOperator(event)}>
          <input
            aria-label="Operator username"
            disabled={!canManageOperators || operatorActionPending}
            onChange={(event) => setNewOperatorUsername(event.target.value)}
            placeholder="operator username"
            value={newOperatorUsername}
          />
          <select
            aria-label="Operator role"
            disabled={!canManageOperators || operatorActionPending}
            onChange={(event) => setNewOperatorRole(event.target.value)}
            value={newOperatorRole}
          >
            <option value="operator">operator</option>
            <option value="viewer">viewer</option>
            <option value="admin">admin</option>
          </select>
          <input
            aria-label="Operator password"
            autoComplete="new-password"
            disabled={!canManageOperators || operatorActionPending}
            onChange={(event) => setNewOperatorPassword(event.target.value)}
            placeholder="temporary password, min 12 chars"
            type="password"
            value={newOperatorPassword}
          />
          <input
            aria-label="Operator scopes"
            disabled={!canManageOperators || operatorActionPending}
            onChange={(event) => setNewOperatorScopes(event.target.value)}
            placeholder="scopes, blank for role defaults"
            value={newOperatorScopes}
          />
          <button className="secondaryAction" disabled={!canCreateOperator} type="submit">
            <UserPlus size={17} />
            Create operator
          </button>
        </form>

        <div className="sectionHeader compact" hidden={activeSubpage !== "Operators"}>
          <h2>TOTP</h2>
          <span>{totpError ?? (operator?.totp_enabled ? "Enabled" : "Optional")}</span>
        </div>
        <div className="sideForm" hidden={activeSubpage !== "Operators"}>
          <input
            aria-label="TOTP password"
            autoComplete="current-password"
            onChange={(event) => setTotpPassword(event.target.value)}
            placeholder="current password"
            type="password"
            value={totpPassword}
          />
          <input
            aria-label="TOTP code"
            autoComplete="one-time-code"
            inputMode="numeric"
            maxLength={6}
            onChange={(event) => setTotpCode(event.target.value)}
            placeholder="6-digit code"
            value={totpCode}
          />
          {totpSetup && (
            <div className="inlineSecret">
              <strong>{totpSetup.secret_base32}</strong>
              <small>{totpSetup.otpauth_uri}</small>
            </div>
          )}
          <button
            className="secondaryAction"
            disabled={totpPending || !totpPassword || operator?.totp_enabled}
            onClick={() => void setupTotp()}
            type="button"
          >
            <ShieldCheck size={17} />
            Setup TOTP
          </button>
          <button
            className="secondaryAction"
            disabled={totpPending || !totpPassword || !totpCode || operator?.totp_enabled}
            onClick={() => void confirmTotp()}
            type="button"
          >
            <LockKeyhole size={17} />
            Confirm TOTP
          </button>
          <button
            className="secondaryAction dangerAction"
            disabled={totpPending || !totpPassword || !totpCode || !operator?.totp_enabled}
            onClick={() => void disableTotp()}
            type="button"
          >
            <Trash2 size={17} />
            Disable TOTP
          </button>
        </div>

        <div className="accessConfigHeading" hidden={activeSubpage !== "VPS clients"}>
          <strong>VPS client configuration</strong>
          <span>Enrollment, rebuild tokens, default tags, and key revocation</span>
        </div>
        <ConfirmationPrompt
          confirmLabel={pendingConfirmation === "key-revoke" ? "Revoke key" : "Create rebuild token"}
          detail={
            pendingConfirmation === "key-revoke"
              ? "Confirm revoking the current VPS key for the selected client."
              : "Confirm issuing a rebuild re-enrollment token for the existing VPS identity."
          }
          items={
            pendingConfirmation === "key-revoke"
              ? [
                  { label: "VPS", value: lifecycleClientLabel(revokeClientId) },
                  { label: "Reason", value: revokeReason.trim() || "none" },
                ]
              : [
                  { label: "VPS", value: lifecycleClientLabel(tokenClientId) },
                  { label: "TTL", value: `${Number.parseInt(tokenTtlSecs, 10)}s` },
                ]
          }
          onCancel={() => setPendingConfirmation(null)}
          onConfirm={() => void confirmAccessAction()}
          open={activeSubpage === "VPS clients" && pendingConfirmation !== null}
          pending={tokenPending || revokePending}
          title={pendingConfirmation === "key-revoke" ? "Revoke VPS key" : "Create rebuild token"}
          tone="danger"
        />
        <div className="sectionHeader compact" hidden={activeSubpage !== "VPS clients"}>
          <h2>Create token</h2>
          <span>{tokenError ?? (canManageOperators ? "Provision or rebuild" : "Admin role required")}</span>
        </div>
        <form className="sideForm" hidden={activeSubpage !== "VPS clients"} onSubmit={(event) => void createEnrollmentToken(event)}>
          <select
            aria-label="Enrollment token purpose"
            disabled={!canManageOperators || tokenPending}
            onChange={(event) => {
              const nextPurpose = event.target.value as EnrollmentTokenPurpose;
              setTokenPurpose(nextPurpose);
              if (nextPurpose === "provision") {
                setTokenClientId("");
              }
            }}
            value={tokenPurpose}
          >
            <option value="provision">Provision token</option>
            <option value="rebuild_reenrollment">Rebuild token</option>
          </select>
          {tokenPurpose === "rebuild_reenrollment" ? (
            <input
              aria-label="Enrollment token existing VPS ID"
              disabled={!canManageOperators || tokenPending}
              onChange={(event) => setTokenClientId(event.target.value)}
              placeholder="VPS ID from details"
              value={tokenClientId}
            />
          ) : (
            <div className="formNote">
              <strong>VPS identity</strong>
              <span>System ID is assigned server-side; set the display name below.</span>
            </div>
          )}
          <input
            aria-label="Enrollment token ttl"
            disabled={!canManageOperators || tokenPending}
            inputMode="numeric"
            onChange={(event) => setTokenTtlSecs(event.target.value)}
            placeholder="ttl seconds"
            value={tokenTtlSecs}
          />
          <input
            aria-label="Enrollment default tags"
            disabled={!canManageOperators || tokenPending}
            onChange={(event) => setTokenTags(event.target.value)}
            placeholder="default tags"
            value={tokenTags}
          />
          <input
            aria-label="Enrollment default display name"
            disabled={!canManageOperators || tokenPending}
            onChange={(event) => setTokenDisplayName(event.target.value)}
            placeholder="initial display name"
            value={tokenDisplayName}
          />
          <label className="inlineCheck">
            <input
              checked={tokenUnmanagedUpdateEnabled}
              disabled={!canManageOperators || tokenPending}
              onChange={(event) => setTokenUnmanagedUpdateEnabled(event.target.checked)}
              type="checkbox"
            />
            <span>Auto-check updates</span>
          </label>
          <input
            aria-label="Enrollment unmanaged update version URL"
            disabled={!canManageOperators || tokenPending || !tokenUnmanagedUpdateEnabled}
            onChange={(event) => setTokenUnmanagedUpdateVersionUrl(event.target.value)}
            placeholder="version.json URL"
            value={tokenUnmanagedUpdateVersionUrl}
          />
          <input
            aria-label="Enrollment unmanaged update interval"
            disabled={!canManageOperators || tokenPending || !tokenUnmanagedUpdateEnabled}
            inputMode="numeric"
            onChange={(event) => setTokenUnmanagedUpdateIntervalSecs(event.target.value)}
            placeholder="interval seconds"
            value={tokenUnmanagedUpdateIntervalSecs}
          />
          <input
            aria-label="Enrollment unmanaged update jitter"
            disabled={!canManageOperators || tokenPending || !tokenUnmanagedUpdateEnabled}
            inputMode="numeric"
            onChange={(event) => setTokenUnmanagedUpdateJitterSecs(event.target.value)}
            placeholder="jitter seconds"
            value={tokenUnmanagedUpdateJitterSecs}
          />
          <label className="inlineCheck">
            <input
              checked={tokenUnmanagedUpdateActivate}
              disabled={!canManageOperators || tokenPending || !tokenUnmanagedUpdateEnabled}
              onChange={(event) => setTokenUnmanagedUpdateActivate(event.target.checked)}
              type="checkbox"
            />
            <span>Activate update</span>
          </label>
          <label className="inlineCheck">
            <input
              checked={tokenUnmanagedUpdateRestartAgent}
              disabled={!canManageOperators || tokenPending || !tokenUnmanagedUpdateEnabled || !tokenUnmanagedUpdateActivate}
              onChange={(event) => setTokenUnmanagedUpdateRestartAgent(event.target.checked)}
              type="checkbox"
            />
            <span>Restart agent</span>
          </label>
          <div className="enrollmentInstallCommand">
            <div className="enrollmentInstallHeading">
              <strong>Root install command</strong>
              {enrollmentInstallRender.command && (
                <button aria-label="Copy enrollment install command" onClick={() => void copyEnrollmentInstallCommand()} type="button">
                  <Copy size={15} />
                  {installCommandCopied ? "Copied" : "Copy"}
                </button>
              )}
            </div>
            {enrollmentInstallRender.command ? (
              <code>{enrollmentInstallRender.command}</code>
            ) : (
              <small>{enrollmentInstallRender.error}</small>
            )}
            {createdToken && (
              <>
                <small>{enrollmentPurposeLabel(createdToken.purpose)} / {createdToken.token_prefix}</small>
                <small>Name {enrollmentTokenDisplayName(createdToken)}</small>
              </>
            )}
          </div>
          {pendingConfirmation !== "rebuild-token" && (
            <button className="secondaryAction" disabled={!canCreateEnrollmentToken} type="submit">
              {tokenPurpose === "rebuild_reenrollment" ? <RotateCcw size={17} /> : <UserPlus size={17} />}
              {tokenPurpose === "rebuild_reenrollment" ? "Rebuild token" : "Create token"}
            </button>
          )}
          {createdToken && (
            <div className="inlineSecret enrollmentSecret">
              <strong>{createdToken.token}</strong>
            </div>
          )}
        </form>

        <div className="sectionHeader compact" hidden={activeSubpage !== "VPS clients"}>
          <h2>Revoke key</h2>
          <span>{revokeError ?? (canManageOperators ? "Current VPS key" : "Admin role required")}</span>
        </div>
        <form className="sideForm" hidden={activeSubpage !== "VPS clients"} onSubmit={(event) => void revokeClientKey(event)}>
          <input
            aria-label="VPS key revoke VPS ID"
            disabled={!canManageOperators || revokePending}
            onChange={(event) => setRevokeClientId(event.target.value)}
            placeholder="VPS ID from details"
            value={revokeClientId}
          />
          <input
            aria-label="VPS key revoke reason"
            disabled={!canManageOperators || revokePending}
            onChange={(event) => setRevokeReason(event.target.value)}
            placeholder="reason"
            value={revokeReason}
          />
          {pendingConfirmation !== "key-revoke" && (
            <button className="secondaryAction dangerAction" disabled={!canRevokeClientKey} type="submit">
              <Ban size={17} />
              Revoke current key
            </button>
          )}
        </form>

        <div className="accessConfigHeading" hidden={activeSubpage !== "Operators"}>
          <strong>Local panel state</strong>
          <span>Browser session and privilege vault controls</span>
        </div>
        <div className="sectionHeader compact" hidden={activeSubpage !== "Operators"}>
          <h2>Session controls</h2>
          <span>Local browser state only</span>
        </div>
        <div className="sideForm" hidden={activeSubpage !== "Operators"}>
          <button className="secondaryAction" onClick={onClearSession} type="button">
            <KeyRound size={17} />
            Clear bearer session
          </button>
          <button className="secondaryAction dangerAction" disabled={!vaultAvailable} onClick={clearVault} type="button">
            <Trash2 size={17} />
            Clear privilege vault
          </button>
        </div>
        <div className="timeline" hidden={activeSubpage !== "Operators"}>
          <ShieldCheck size={18} />
          <div>
            <strong>Deny by default</strong>
            <span>Non-telemetry actions still require local privilege unlock assertions.</span>
          </div>
        </div>
        <div className="timeline" hidden={activeSubpage !== "Operators"}>
          <LockKeyhole size={18} />
          <div>
            <strong>Secret handling</strong>
            <span>Plaintext super password is not sent to the control-plane API.</span>
          </div>
        </div>

        <div className="accessConfigHeading" hidden={activeSubpage !== "Gateway"}>
          <strong>Gateway lifecycle</strong>
          <span>Noise-over-TCP VPS session inventory</span>
        </div>
        <div className="sectionHeader compact" hidden={activeSubpage !== "Gateway"}>
          <h2>Gateway records</h2>
          <span>{gatewaySessions.length} retained sessions</span>
        </div>
        <div className="sideForm" hidden={activeSubpage !== "Gateway"}>
          <button className="secondaryAction" disabled={loading} onClick={() => void onRefresh()} type="button">
            <RefreshCw size={17} />
            Refresh sessions
          </button>
        </div>
        <div className="timeline" hidden={activeSubpage !== "Gateway"}>
          <Wifi size={18} />
          <div>
            <strong>Agent connectivity</strong>
            <span>Current and recently closed gateway routes are retained as lifecycle records.</span>
          </div>
        </div>
        <div className="timeline" hidden={activeSubpage !== "Gateway"}>
          <Fingerprint size={18} />
          <div>
            <strong>VPS identity</strong>
            <span>Gateway tables show display names; system IDs stay available from VPS details.</span>
          </div>
        </div>
      </aside>
    </section>
  );
}

function parseScopeInput(input: string): string[] {
  return input
    .split(",")
    .map((scope) => scope.trim())
    .filter(Boolean);
}

function enrollmentPurposeLabel(purpose: EnrollmentTokenPurpose): string {
  return purpose === "rebuild_reenrollment" ? "rebuild re-enrollment" : "provision";
}

function enrollmentTokenDefaultsLabel(record: EnrollmentTokenView): string {
  const updateLabel = record.unmanaged_update_enabled
    ? `updates ${Math.round(record.unmanaged_update_interval_secs / 3600)}h`
    : "updates off";
  return [record.default_display_name, ...record.default_tags, updateLabel]
    .filter((value): value is string => Boolean(value))
    .join(", ");
}

function enrollmentTokenDisplayName(record: EnrollmentTokenView | CreateEnrollmentTokenResponse): string {
  return record.default_display_name?.trim() || (record.purpose === "rebuild_reenrollment" ? "Existing VPS" : "Pending VPS");
}
