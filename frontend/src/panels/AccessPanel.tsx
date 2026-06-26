import { useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import {
  AlertTriangle,
  Ban,
  Clock,
  Copy,
  Fingerprint,
  KeyRound,
  LockKeyhole,
  RefreshCw,
  Save,
  ShieldCheck,
  UsersRound,
  Trash2,
  Wifi,
} from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ConsoleDataGrid, type ConsoleDataGridColumn } from "../components/ConsoleDataGrid";
import { useReviewGenerationGuard, waitForReviewRender } from "../hooks/useReviewGenerationGuard";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { VpsCombobox } from "../components/VpsCombobox";
import { clearPrivilegeVault, hasPrivilegeVault } from "../vault";
import { generateNoiseKeypair } from "../noiseKeygen";
import { scrollIntoViewWithMotion } from "../motion";
import { usePanelDisplaySettings } from "../panelDisplay";
import type {
  GatewaySessionRecord,
  OperatorView,
  OperatorSessionRecord,
  TotpSetupResponse,
} from "../types";
import type {
  AgentIdentityView,
  ClientKeyRevocationView,
  KeyLifecycleClientView,
  KeyLifecycleReportView,
  UpsertAgentIdentityRequest,
} from "../typesAccess";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  type PrivilegeAssertion,
  type PrivilegeMaterial,
} from "../privilege";
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
  "VPS identities",
  "Gateway sessions",
  "Privilege vault",
] as const;

type AccessSubpage = (typeof accessSubpages)[number];
type AccessReleaseSubpage =
  | "overview"
  | "vps_identities"
  | "gateway_sessions"
  | "privilege_vault";
type VpsNameDisplayMode = ReturnType<
  typeof usePanelDisplaySettings
>["vpsNameDisplayMode"];
type AccessConfirmationAction =
  | "agent-identity"
  | "key-revoke"
  | "session-clear"
  | "totp-disable"
  | "vault-clear";

type AgentIdentityConfirmationSnapshot = {
  clientId: string;
  publicKeyHex: string;
  displayName: string | null;
  tags: string[];
  replaceExistingKey: boolean;
  privilegeAssertion: PrivilegeAssertion;
};

type KeyRevokeConfirmationSnapshot = {
  clientId: string;
  reason: string | null;
  privilegeAssertion: PrivilegeAssertion;
};

type AccessPanelProps = {
  activeSubpage: string;
  apiToken: string;
  error: string | null;
  gatewaySessions: GatewaySessionRecord[];
  lastLiveEvent: string;
  loading: boolean;
  onClearSession: () => void;
  onConfirmTotp: (password: string, code: string) => Promise<void>;
  onDisableTotp: (password: string, code: string) => Promise<void>;
  onOpenSystemConfig: () => void;
  onOpenSystemPreferences: () => void;
  onOpenSystemSessions: () => void;
  onOpenSystemUsers: () => void;
  onRefresh: () => Promise<void>;
  onRevokeClientKey: (
    clientId: string,
    reason: string | null,
    confirmed: boolean,
    privilegeAssertion: PrivilegeAssertion | null,
  ) => Promise<void>;
  onSetupTotp: (password: string) => Promise<TotpSetupResponse | null>;
  onSelectSubpage: (subpage: AccessReleaseSubpage) => void;
  onUpsertAgentIdentity: (
    request: UpsertAgentIdentityRequest,
  ) => Promise<AgentIdentityView>;
  operator: OperatorView | null;
  operatorSessions: OperatorSessionRecord[];
  operators: OperatorView[];
  clientKeyRevocations: ClientKeyRevocationView[];
  keyLifecycleReport: KeyLifecycleReportView | null;
  privilegeMaterial: PrivilegeMaterial | null;
  sessionVaultAvailable: boolean;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  wsState: string;
};

function accessSubpageFromRoute(subpage: string): AccessSubpage {
  switch (subpage) {
    case "privilege":
    case "privilege_vault":
      return "Privilege vault";
    case "clients":
    case "vps_identities":
      return "VPS identities";
    case "gateway":
    case "gateway_sessions":
      return "Gateway sessions";
    default:
      return "Overview";
  }
}

function accessRouteForSubpage(subpage: AccessSubpage): AccessReleaseSubpage {
  switch (subpage) {
    case "VPS identities":
      return "vps_identities";
    case "Gateway sessions":
      return "gateway_sessions";
    case "Privilege vault":
      return "privilege_vault";
    default:
      return "overview";
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
  onDisableTotp,
  onOpenSystemConfig,
  onOpenSystemPreferences,
  onOpenSystemSessions,
  onOpenSystemUsers,
  onRefresh,
  onRevokeClientKey,
  onSetupTotp,
  onSelectSubpage,
  onUpsertAgentIdentity,
  operator,
  operatorSessions,
  operators,
  clientKeyRevocations,
  keyLifecycleReport,
  privilegeMaterial,
  sessionVaultAvailable,
  setPrivilegeMaterial,
  wsState,
}: AccessPanelProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const identityFormRef = useRef<HTMLFormElement | null>(null);
  const revokeFormRef = useRef<HTMLFormElement | null>(null);
  const [activeSubpage, setActiveSubpage] = useState<AccessSubpage>(
    accessSubpageFromRoute(routeSubpage),
  );
  const [vaultAvailable, setVaultAvailable] = useState(() =>
    hasPrivilegeVault(),
  );
  const [totpPassword, setTotpPassword] = useState("");
  const [totpCode, setTotpCode] = useState("");
  const [totpSetup, setTotpSetup] = useState<TotpSetupResponse | null>(null);
  const [totpPending, setTotpPending] = useState(false);
  const [totpError, setTotpError] = useState<string | null>(null);
  const [identityClientId, setIdentityClientId] = useState("");
  const [identityPublicKeyHex, setIdentityPublicKeyHex] = useState("");
  const [identityDisplayName, setIdentityDisplayName] = useState("");
  const [identityTags, setIdentityTags] = useState("");
  const [identityMode, setIdentityMode] = useState<"register" | "rotate">(
    "register",
  );
  const [identityPending, setIdentityPending] = useState(false);
  const [identityReviewPending, setIdentityReviewPending] = useState(false);
  const [identityError, setIdentityError] = useState<string | null>(null);
  const [privateKeyHex, setPrivateKeyHex] = useState<string | null>(null);
  const [createdIdentity, setCreatedIdentity] =
    useState<AgentIdentityView | null>(null);
  const [revokeClientId, setRevokeClientId] = useState("");
  const [revokeReason, setRevokeReason] = useState("");
  const [revokePending, setRevokePending] = useState(false);
  const [revokeReviewPending, setRevokeReviewPending] = useState(false);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [pendingConfirmation, setPendingConfirmation] =
    useState<AccessConfirmationAction | null>(null);
  const [identitySnapshot, setIdentitySnapshot] =
    useState<AgentIdentityConfirmationSnapshot | null>(null);
  const [revokeSnapshot, setRevokeSnapshot] =
    useState<KeyRevokeConfirmationSnapshot | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();

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
  const currentSession =
    operatorSessions.find((session) => session.current) ?? operatorSessions[0];
  const operatorRefreshTtlDays = operator
    ? Math.round(operator.session_refresh_ttl_secs / 86_400)
    : 0;
  const adminMfaRisk = operator?.role === "admin" && !operator.totp_enabled;
  const adminTtlRisk =
    operator?.role === "admin" && operator.session_refresh_ttl_secs > 30 * 86_400;
  const gatewayInstallDefaultsReady = Boolean(
    operator?.preferences.gateway_endpoints.trim() &&
      operator.preferences.gateway_server_public_key_hex?.trim(),
  );
  const visibleRoleModel = summarizeRoleModel(operators, operator);
  const operatorScopeSummary = summarizeOperatorScopes(operator);
  const lifecycleClients = keyLifecycleReport?.clients ?? [];
  const lifecycleVpsOptions = useMemo(
    () =>
      lifecycleClients.map((client) => ({
        display_name: client.display_name,
        id: client.client_id,
        status: client.status,
        tags: [],
      })),
    [lifecycleClients],
  );
  const lifecycleNameById = useMemo(
    () => clientLifecycleNameMap(lifecycleClients, vpsNameDisplayMode),
    [lifecycleClients, vpsNameDisplayMode],
  );
  const lifecycleClientLabel = (clientId: string | null | undefined) =>
    clientDisplayNameFromMap(clientId, lifecycleNameById);
  const activeGatewaySessions = gatewaySessions.filter(
    (session) => !session.ended_at,
  ).length;
  const activeOperatorSessions = operatorSessions.filter(
    (session) => !session.revoked,
  ).length;
  const revokedClientCount = lifecycleClients.filter(
    (client) => client.status === "revoked" || client.current_key_revoked,
  ).length;
  const blockedOrPendingClientCount = lifecycleClients.filter((client) =>
    ["blocked", "pending", "revoked"].includes(identityStatus(client)),
  ).length;
  const currentSessionExpiry = currentSession
    ? formatTime(currentSession.expires_at)
    : "Not reported";
  const currentRefreshExpiry = currentSession
    ? formatTime(currentSession.refresh_expires_at)
    : "Not reported";
  const authPostureValue = adminMfaRisk
    ? "Admin MFA required"
    : operator?.totp_enabled
      ? "TOTP enabled"
      : "TOTP not enrolled";
  const sessionPostureValue = adminTtlRisk
    ? `Admin refresh TTL ${operatorRefreshTtlDays}d`
    : currentSession
      ? `Access expires ${currentSessionExpiry}`
      : sessionState;
  const gatewayPostureValue = gatewayInstallDefaultsReady
    ? `${activeGatewaySessions} active sessions`
    : "Install defaults missing";
  const canUpsertIdentity =
    canManageOperators &&
    !identityPending &&
    !identityReviewPending &&
    Boolean(privilegeMaterial) &&
    identityClientId.trim().length > 0 &&
    isFixedHex32(identityPublicKeyHex);
  const canRevokeClientKey =
    canManageOperators &&
    Boolean(privilegeMaterial) &&
    revokeClientId.trim().length > 0 &&
    !revokePending &&
    !revokeReviewPending;
  const identityColumns = useMemo<ConsoleDataGridColumn<KeyLifecycleClientView>[]>(
    () => [
      {
        id: "vps",
        header: "VPS",
        cell: (client) => (
          <span title={client.client_id}>
            {formatVpsName(
              {
                client_id: client.client_id,
                display_name: client.display_name,
              },
              vpsNameDisplayMode,
            )}
          </span>
        ),
        searchValue: (client) => `${client.display_name} ${client.client_id}`,
        sortValue: (client) => client.display_name || client.client_id,
        size: 250,
      },
      {
        id: "status",
        header: "Status",
        cell: (client) => (
          <span className={`statusPill ${statusClass(identityStatus(client))}`}>
            {identityStatus(client)}
          </span>
        ),
        searchValue: (client) => identityStatus(client),
        sortValue: (client) => identityStatus(client),
        size: 130,
      },
      {
        id: "key",
        header: "Current key",
        cell: (client) =>
          client.current_public_key_sha256_hex ? (
            <span
              className="monoValue"
              title={client.current_public_key_sha256_hex}
            >
              {shortHash(client.current_public_key_sha256_hex)}
            </span>
          ) : (
            "no key"
          ),
        searchValue: (client) => client.current_public_key_sha256_hex ?? "",
        size: 180,
      },
      {
        id: "revocation",
        header: "Latest revocation",
        cell: (client) =>
          client.latest_revoked_at
            ? `${formatTime(client.latest_revoked_at)} ${client.latest_revocation_reason ?? ""}`
            : "none",
        searchValue: (client) =>
          `${client.latest_revoked_at ?? ""} ${client.latest_revocation_reason ?? ""}`,
        sortValue: (client) => client.latest_revoked_at ?? "",
        size: 260,
      },
    ],
    [vpsNameDisplayMode],
  );
  const revocationColumns = useMemo<ConsoleDataGridColumn<ClientKeyRevocationView>[]>(
    () => [
      {
        id: "vps",
        header: "VPS",
        cell: (revocation) => (
          <span title={revocation.client_id}>
            {clientDisplayNameFromMap(revocation.client_id, lifecycleNameById)}
          </span>
        ),
        searchValue: (revocation) =>
          `${clientDisplayNameFromMap(revocation.client_id, lifecycleNameById)} ${revocation.client_id}`,
        sortValue: (revocation) =>
          clientDisplayNameFromMap(revocation.client_id, lifecycleNameById),
        size: 250,
      },
      {
        id: "key",
        header: "Key hash",
        cell: (revocation) => (
          <span className="monoValue" title={revocation.public_key_sha256_hex}>
            {shortHash(revocation.public_key_sha256_hex)}
          </span>
        ),
        searchValue: (revocation) => revocation.public_key_sha256_hex,
        size: 180,
      },
      {
        id: "reason",
        header: "Reason",
        cell: (revocation) => revocation.reason ?? "operator request",
        searchValue: (revocation) => revocation.reason ?? "operator request",
        size: 240,
      },
      {
        id: "created",
        header: "Created",
        cell: (revocation) => formatTime(revocation.created_at),
        sortValue: (revocation) => revocation.created_at,
        size: 200,
      },
    ],
    [lifecycleNameById],
  );
  const gatewaySessionColumns = useMemo<ConsoleDataGridColumn<GatewaySessionRecord>[]>(
    () => [
      {
        id: "vps",
        header: "VPS",
        cell: (session) => (
          <span title={session.client_id}>
            {clientDisplayNameFromMap(session.client_id, lifecycleNameById)}
          </span>
        ),
        searchValue: (session) =>
          `${clientDisplayNameFromMap(session.client_id, lifecycleNameById)} ${session.client_id}`,
        sortValue: (session) =>
          clientDisplayNameFromMap(session.client_id, lifecycleNameById),
        size: 240,
      },
      {
        id: "gateway",
        header: "Gateway",
        cell: (session) => session.gateway_id,
        searchValue: (session) => session.gateway_id,
        sortValue: (session) => session.gateway_id,
        size: 160,
      },
      {
        id: "status",
        header: "Status",
        cell: (session) => (
          <span className={`statusPill ${statusClass(session.status)}`}>
            {session.status}
          </span>
        ),
        searchValue: (session) => session.status,
        sortValue: (session) => session.status,
        size: 120,
      },
      {
        id: "lastSeen",
        header: "Last seen",
        cell: (session) => formatTime(session.last_seen_at),
        sortValue: (session) => session.last_seen_at,
        size: 190,
      },
      {
        id: "noiseKey",
        header: "Noise key",
        cell: (session) =>
          session.noise_public_key_hex ? (
            <span className="monoValue" title={session.noise_public_key_hex}>
              {shortHash(session.noise_public_key_hex)}
            </span>
          ) : (
            "n/a"
          ),
        searchValue: (session) => session.noise_public_key_hex ?? "",
        size: 160,
      },
      {
        id: "endReason",
        header: "End reason",
        cell: (session) =>
          session.end_reason ?? (session.ended_at ? "ended" : "active"),
        searchValue: (session) =>
          session.end_reason ?? (session.ended_at ? "ended" : "active"),
        size: 190,
      },
    ],
    [lifecycleNameById],
  );

  useEffect(() => {
    setActiveSubpage(accessSubpageFromRoute(routeSubpage));
  }, [routeSubpage]);

  function openAccessSubpage(subpage: AccessSubpage) {
    setActiveSubpage(subpage);
    onSelectSubpage(accessRouteForSubpage(subpage));
  }

  function clearVault() {
    clearPrivilegeVault();
    setVaultAvailable(false);
    setPrivilegeMaterial(null);
  }

  function confirmLocalAccessAction() {
    if (pendingConfirmation === "session-clear") {
      onClearSession();
    } else if (pendingConfirmation === "vault-clear") {
      clearVault();
    }
    setPendingConfirmation(null);
  }

  function clearIdentityReview() {
    invalidateReviewGeneration();
    setIdentitySnapshot(null);
    setIdentityReviewPending(false);
    setPendingConfirmation((current) =>
      current === "agent-identity" ? null : current,
    );
  }

  function clearRevokeReview() {
    invalidateReviewGeneration();
    setRevokeSnapshot(null);
    setRevokeReviewPending(false);
    setPendingConfirmation((current) => (current === "key-revoke" ? null : current));
  }

  function prepareNewIdentity() {
    clearIdentityReview();
    setIdentityMode("register");
    setIdentityClientId("");
    setIdentityPublicKeyHex("");
    setIdentityDisplayName("");
    setIdentityTags("");
    setPrivateKeyHex(null);
    setCreatedIdentity(null);
    setIdentityError(null);
    scrollIntoViewSoon(identityFormRef.current);
  }

  function prepareIdentityRotation(client: KeyLifecycleClientView) {
    clearIdentityReview();
    setIdentityMode("rotate");
    setIdentityClientId(client.client_id);
    setIdentityPublicKeyHex("");
    setIdentityDisplayName("");
    setIdentityTags("");
    setPrivateKeyHex(null);
    setCreatedIdentity(null);
    setIdentityError(null);
    openAccessSubpage("VPS identities");
    scrollIntoViewSoon(identityFormRef.current);
  }

  function prepareClientKeyRevoke(clientId: string, reason = "") {
    clearRevokeReview();
    setRevokeClientId(clientId);
    setRevokeReason(reason);
    setRevokeError(null);
    openAccessSubpage("VPS identities");
    scrollIntoViewSoon(revokeFormRef.current);
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
      setPendingConfirmation(null);
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

  async function handleGenerateKeypair() {
    try {
      const keypair = await generateNoiseKeypair();
      setIdentityPublicKeyHex(keypair.publicKeyHex);
      setPrivateKeyHex(keypair.privateKeyHex);
      setIdentityError(null);
    } catch {
      setIdentityError("Key generation failed — browser may not support Web Crypto");
    }
  }

  function handleCopyPrivateKey() {
    if (privateKeyHex) {
      navigator.clipboard.writeText(privateKeyHex).catch(() => {});
    }
  }

  async function requestIdentityImport(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const clientId = identityClientId.trim();
    if (!canUpsertIdentity) {
      setIdentityError(
        privilegeMaterial
          ? "Client ID and 64-hex public key are required"
          : "Privilege vault unlock is required",
      );
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    const isRotate = identityMode === "rotate";
    const snapshotInput = {
      clientId,
      displayName: isRotate ? null : identityDisplayName.trim() || null,
      publicKeyHex: identityPublicKeyHex.trim().toLowerCase(),
      replaceExistingKey: isRotate,
      tags: isRotate ? [] : parseListInput(identityTags),
    };
    setIdentityError(null);
    setIdentityReviewPending(true);
    try {
      await waitForReviewRender();
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: isRotate ? "agent_identity.rotate" : "agent_identity.import",
          confirmed: true,
          resolvedTargets: [clientId],
          target: clientId,
        }),
        privilegeMaterial: privilegeMaterial!,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setIdentitySnapshot({
        ...snapshotInput,
        privilegeAssertion,
      });
      setPendingConfirmation("agent-identity");
    } catch (error) {
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setIdentityError(
        error instanceof Error ? error.message : "Privilege assertion failed",
      );
    } finally {
      setIdentityReviewPending(false);
    }
  }

  async function confirmIdentityImport() {
    const snapshot = identitySnapshot;
    if (!snapshot || identityPending) {
      return;
    }
    setIdentityPending(true);
    setIdentityError(null);
    try {
      const response = await onUpsertAgentIdentity({
        client_id: snapshot.clientId,
        client_public_key_hex: snapshot.publicKeyHex,
        display_name: snapshot.displayName,
        tags: snapshot.tags,
        replace_existing_key: snapshot.replaceExistingKey,
        confirmed: true,
        privilege_assertion: snapshot.privilegeAssertion,
      });
      setCreatedIdentity(response);
      setIdentityClientId("");
      setIdentityPublicKeyHex("");
      setIdentityDisplayName("");
      setIdentityTags("");
      setIdentityMode("register");
      setIdentitySnapshot(null);
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

  async function requestClientKeyRevoke(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const clientId = revokeClientId.trim();
    if (!canRevokeClientKey) {
      setRevokeError(privilegeMaterial ? "VPS ID is required" : "Privilege vault unlock is required");
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    const snapshotInput = {
      clientId,
      reason: revokeReason.trim() || null,
    };
    setRevokeError(null);
    setRevokeReviewPending(true);
    try {
      await waitForReviewRender();
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: "client_key.revoke",
          confirmed: true,
          resolvedTargets: [clientId],
          target: clientId,
        }),
        privilegeMaterial: privilegeMaterial!,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setRevokeSnapshot({
        ...snapshotInput,
        privilegeAssertion,
      });
      setPendingConfirmation("key-revoke");
    } catch (error) {
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setRevokeError(
        error instanceof Error ? error.message : "Privilege assertion failed",
      );
    } finally {
      setRevokeReviewPending(false);
    }
  }

  async function confirmClientKeyRevoke() {
    const snapshot = revokeSnapshot;
    if (!snapshot || revokePending) {
      return;
    }
    setRevokePending(true);
    setRevokeError(null);
    try {
      await onRevokeClientKey(
        snapshot.clientId,
        snapshot.reason,
        true,
        snapshot.privilegeAssertion,
      );
      setRevokeClientId("");
      setRevokeReason("");
      setRevokeSnapshot(null);
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
        <div className="sectionHeader compactSectionHeader">
          <div>
            <h2>Access overview</h2>
            <span>
              {error ??
                (loading
                  ? "Refreshing access records"
                  : "Direct gateway identities, browser session state, and live access streams")}
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

        <div className="accessPostureGrid" aria-label="Access posture overview">
          <AccessPostureCard
            action={operator?.totp_enabled ? "Review MFA" : "Set up MFA"}
            detail={`${operator?.username ?? "anonymous"} · ${operatorScopeSummary}`}
            icon={<ShieldCheck size={17} />}
            label="Operator authentication"
            onAction={() => openAccessSubpage("Privilege vault")}
            tone={adminMfaRisk ? "attention" : operator?.totp_enabled ? "ready" : "neutral"}
            value={authPostureValue}
          />
          <AccessPostureCard
            action="Open Operators"
            detail={visibleRoleModel.detail}
            icon={<UsersRound size={17} />}
            label="RBAC roles/scopes"
            onAction={onOpenSystemUsers}
            tone={operators.length > 0 ? "ready" : "attention"}
            value={visibleRoleModel.value}
          />
          <AccessPostureCard
            action="Manage sessions"
            detail={`Token in ${tokenStorageState}; refresh expires ${currentRefreshExpiry}.`}
            icon={<Clock size={17} />}
            label="Bearer session"
            onAction={onOpenSystemSessions}
            tone={adminTtlRisk || !currentSession ? "attention" : "ready"}
            value={sessionPostureValue}
          />
          <AccessPostureCard
            action={privilegeMaterial ? "Open vault" : "Unlock"}
            detail="Local-only secret material; API receives signed request-bound assertions."
            icon={<LockKeyhole size={17} />}
            label="Privilege vault"
            onAction={() => openAccessSubpage("Privilege vault")}
            tone={privilegeMaterial ? "ready" : vaultAvailable ? "neutral" : "attention"}
            value={privilegeMaterial ? "Unlocked for this browser" : vaultState}
          />
          <AccessPostureCard
            action="Open identities"
            detail={`Lifecycle: register -> pending install -> connected -> rotate -> revoke -> blocked.`}
            icon={<Fingerprint size={17} />}
            label="VPS identities"
            onAction={() => openAccessSubpage("VPS identities")}
            tone={blockedOrPendingClientCount > 0 ? "attention" : "ready"}
            value={`${keyLifecycleReport?.direct_identity_client_count ?? lifecycleClients.length} registered / ${blockedOrPendingClientCount} attention`}
          />
          <AccessPostureCard
            action="Open sessions"
            detail={gatewayInstallDefaultsReady ? `${gatewaySessions.length} recent sessions.` : "Configure endpoints and gateway server key before generating install commands."}
            icon={<Wifi size={17} />}
            label="Gateway sessions"
            onAction={() => openAccessSubpage("Gateway sessions")}
            tone={gatewayInstallDefaultsReady ? "ready" : "attention"}
            value={gatewayPostureValue}
          />
        </div>

        <nav className="subpanelTabs accessTabs" aria-label="Access subpanels">
          {accessSubpages.map((subpage) => (
            <button
              className={activeSubpage === subpage ? "active" : ""}
              key={subpage}
              onClick={() => openAccessSubpage(subpage)}
              type="button"
            >
              {subpage}
            </button>
          ))}
        </nav>

        {activeSubpage === "Overview" && (
          <div className="workspaceSection accessOverviewGrid">
            <section
              className="controlPanel accessWorkflowMap"
              aria-label="Access overview authority links"
            >
              <div className="sectionHeader compact">
                <h2>Authority workflow map</h2>
                <span>Jump to the canonical page for each access-control responsibility</span>
              </div>
              <div className="accessWorkflowLinks">
                <AccessWorkflowLink
                  detail={`${operators.length} operators; ${activeOperatorSessions} active sessions`}
                  icon={<UsersRound size={16} />}
                  label="Operators"
                  onClick={onOpenSystemUsers}
                />
                <AccessWorkflowLink
                  detail={`${keyLifecycleReport?.direct_identity_client_count ?? lifecycleClients.length} registered identities; ${blockedOrPendingClientCount} need attention`}
                  icon={<Fingerprint size={16} />}
                  label="VPS identities"
                  onClick={() => openAccessSubpage("VPS identities")}
                />
                <AccessWorkflowLink
                  detail={`${activeGatewaySessions} active gateway streams; ${gatewaySessions.length} recent sessions`}
                  icon={<Wifi size={16} />}
                  label="Gateway sessions"
                  onClick={() => openAccessSubpage("Gateway sessions")}
                />
                <AccessWorkflowLink
                  detail={privilegeMaterial ? "Unlocked in this browser" : vaultState}
                  icon={<LockKeyhole size={16} />}
                  label="Privilege vault"
                  onClick={() => openAccessSubpage("Privilege vault")}
                />
              </div>
            </section>
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Security posture</h2>
                <span>Authentication, RBAC, session, privilege, identity, and gateway readiness</span>
              </div>
              <div className="metricRows">
                <MetricRow
                  label="Operators"
                  value={`${operators.length} total / ${visibleRoleModel.value}`}
                />
                <MetricRow
                  label="Active operator sessions"
                  value={`${activeOperatorSessions} active / ${operatorSessions.length} listed`}
                />
                <MetricRow
                  label="Operator authentication"
                  value={authPostureValue}
                />
                <MetricRow
                  label="Admin session policy"
                  value={
                    operator
                      ? `refresh TTL ${operatorRefreshTtlDays}d${adminTtlRisk ? " (review)" : ""}`
                      : "operator not loaded"
                  }
                />
                <MetricRow
                  label="Current bearer session"
                  value={`${currentSessionExpiry}; refresh ${currentRefreshExpiry}`}
                />
                <MetricRow
                  label="Role model"
                  value={visibleRoleModel.detail}
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
                <MetricRow
                  label="Gateway install defaults"
                  value={gatewayInstallDefaultsReady ? "configured" : "missing endpoints or server key"}
                />
                <MetricRow
                  label="Gateway sessions"
                  value={`${activeGatewaySessions} active / ${gatewaySessions.length} recent`}
                />
                <MetricRow
                  label="Privilege vault"
                  value={privilegeMaterial ? "unlocked for this browser" : vaultState}
                />
                <MetricRow label="WebSocket" value={wsState} />
                <MetricRow label="Last event" value={lastLiveEvent || "none"} />
              </div>
            </section>
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Attention queues</h2>
                <span>Jump from access state to the working table</span>
              </div>
              <div className="accessQueueList">
                <AccessQueueRow
                  action="Open identities"
                  detail={`${keyLifecycleReport?.revocation_count ?? clientKeyRevocations.length} revocations retained`}
                  label="VPS identity lifecycle"
                  onClick={() => openAccessSubpage("VPS identities")}
                  value={`${revokedClientCount} current blocked`}
                />
                <AccessQueueRow
                  action="Open sessions"
                  detail={gatewayInstallDefaultsReady ? `${gatewaySessions.length} recent sessions` : "install defaults need endpoints and server key"}
                  label="Gateway sessions"
                  onClick={() => openAccessSubpage("Gateway sessions")}
                  value={`${activeGatewaySessions} active`}
                />
                <AccessQueueRow
                  action="Open vault"
                  detail={privilegeMaterial ? "ready for privileged review" : "required for key lifecycle actions"}
                  label="Privilege vault"
                  onClick={() => openAccessSubpage("Privilege vault")}
                  value={vaultState}
                />
                <AccessQueueRow
                  action="Manage sessions"
                  detail={`token in ${tokenStorageState}; refresh TTL ${operatorRefreshTtlDays || "-"}d`}
                  label="Bearer sessions"
                  onClick={onOpenSystemSessions}
                  value={currentSession ? "server listed" : sessionState}
                />
                <AccessQueueRow
                  action="Manage users"
                  detail={adminMfaRisk ? "admin TOTP is off" : visibleRoleModel.detail}
                  label="Operator roles and MFA"
                  onClick={onOpenSystemUsers}
                  value={adminMfaRisk ? "attention" : visibleRoleModel.value}
                />
              </div>
            </section>
          </div>
        )}

        {activeSubpage === "Privilege vault" && (
          <div className="workspaceSection accessOverviewGrid">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Privilege vault</h2>
                <span>
                  Kept in the browser; API receives only request-bound
                  assertions
                </span>
              </div>
              <PrivilegeVaultBox
                labelPrefix="Access"
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
                    (operator?.totp_enabled
                      ? "enabled"
                      : operator?.role === "admin"
                        ? "admin MFA required"
                        : "recommended account hardening")}
                </span>
              </div>
              <div className={`accessRiskNotice ${adminMfaRisk ? "attention" : "ready"}`}>
                {adminMfaRisk ? <AlertTriangle size={17} /> : <ShieldCheck size={17} />}
                <div>
                  <strong>{adminMfaRisk ? "Admin MFA is off" : "MFA posture recorded"}</strong>
                  <span>
                    {adminMfaRisk
                      ? "Production admin accounts should require TOTP before long-lived access or privileged workflows."
                      : operator?.totp_enabled
                        ? "This operator account has TOTP enabled."
                        : "TOTP is available for this operator account."}
                  </span>
                </div>
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
                    onClick={() => setPendingConfirmation("totp-disable")}
                    type="button"
                  >
                    <Trash2 size={17} />
                    Review disable
                  </button>
                </div>
              </div>
            </section>
          </div>
        )}

        {activeSubpage === "VPS identities" && (
          <div className="workspaceSection accessTableStack">
            <IdentityLifecycleGuide
              blockedCount={blockedOrPendingClientCount}
              onNewIdentity={prepareNewIdentity}
              registeredCount={
                keyLifecycleReport?.direct_identity_client_count ??
                lifecycleClients.length
              }
            />
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>VPS identities</h2>
                <span>Registered public keys and revocation state</span>
              </div>
              <ConsoleDataGrid
                actions={[
                  {
                    label: "Rotate selected",
                    description: (rows) =>
                      rows.length === 1
                        ? `Prefill key rotation for ${rows[0].display_name}.`
                        : "Select exactly one VPS identity to rotate.",
                    disabled: (rows) => rows.length !== 1,
                    icon: <KeyRound size={14} />,
                    onSelect: (rows) => prepareIdentityRotation(rows[0]),
                  },
                  {
                    label: "Revoke selected",
                    description: (rows) =>
                      rows.length === 1
                        ? `Prefill current key revocation for ${rows[0].display_name}.`
                        : "Select exactly one VPS identity to revoke.",
                    disabled: (rows) => rows.length !== 1,
                    icon: <Ban size={14} />,
                    onSelect: (rows) => prepareClientKeyRevoke(rows[0].client_id),
                    tone: "danger",
                  },
                ]}
                columns={identityColumns}
                defaultPageSize={10}
                empty="No VPS identities"
                expandOnRowClick
                getRowId={(client) => client.client_id}
                itemLabel="identities"
                renderExpandedRow={(client) => (
                  <IdentityDetailGrid
                    client={client}
                    vpsNameDisplayMode={vpsNameDisplayMode}
                  />
                )}
                renderSelectionPanel={(rows) => (
                  <AccessSelectionPanel
                    label="Selected identities"
                    value={rows
                      .map((client) => client.display_name || client.client_id)
                      .join(", ")}
                  />
                )}
                rowActions={[
                  {
                    label: "Prepare rotation",
                    description: (rows) =>
                      `Prefill key rotation for ${rows[0].display_name}.`,
                    icon: <KeyRound size={14} />,
                    onSelect: (rows) => prepareIdentityRotation(rows[0]),
                  },
                  {
                    label: "Prepare revoke",
                    description: (rows) =>
                      `Prefill current key revocation for ${rows[0].display_name}.`,
                    icon: <Ban size={14} />,
                    onSelect: (rows) => prepareClientKeyRevoke(rows[0].client_id),
                    tone: "danger",
                  },
                ]}
                rows={lifecycleClients}
                searchPlaceholder="Search VPS, status, key, or revocation"
                singleExpandedRow
                storageKey="vpsman.access.agentIdentities"
                title="VPS identities"
                toolbarActions={
                  <button
                    className="secondaryAction compactAction"
                    onClick={prepareNewIdentity}
                    type="button"
                  >
                    <Fingerprint size={15} />
                    <span>New</span>
                  </button>
                }
              />
            </section>

            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Client key revocations</h2>
                <span>{clientKeyRevocations.length} retained records</span>
              </div>
              <ConsoleDataGrid
                columns={revocationColumns}
                defaultPageSize={8}
                empty="No client key revocations"
                expandOnRowClick
                getRowId={(revocation) => revocation.id}
                itemLabel="revocations"
                renderExpandedRow={(revocation) => (
                  <RevocationDetailGrid
                    label={lifecycleClientLabel(revocation.client_id)}
                    revocation={revocation}
                  />
                )}
                rows={clientKeyRevocations}
                searchPlaceholder="Search VPS, key hash, reason, or operator"
                selectable={false}
                singleExpandedRow
                storageKey="vpsman.access.revocations"
                title="Client key revocations"
              />
            </section>
          </div>
        )}

        {activeSubpage === "Gateway sessions" && (
          <div className="workspaceSection">
            <GatewayReadinessPanel
              activeSessions={activeGatewaySessions}
              gatewaySessions={gatewaySessions.length}
              installDefaultsReady={gatewayInstallDefaultsReady}
              onOpenPreferences={onOpenSystemPreferences}
              onOpenSuiteConfig={onOpenSystemConfig}
            />
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Gateway sessions</h2>
                <span>
                  {activeGatewaySessions} active / {gatewaySessions.length}{" "}
                  recent
                </span>
              </div>
              <ConsoleDataGrid
                columns={gatewaySessionColumns}
                defaultPageSize={12}
                empty="No gateway sessions"
                expandOnRowClick
                getRowId={(session) => session.id}
                itemLabel="gateway sessions"
                renderExpandedRow={(session) => (
                  <GatewaySessionDetailGrid
                    label={lifecycleClientLabel(session.client_id)}
                    session={session}
                  />
                )}
                rows={gatewaySessions}
                searchPlaceholder="Search VPS, gateway, status, key, or reason"
                selectable={false}
                singleExpandedRow
                storageKey="vpsman.access.gatewaySessions"
                title="Gateway sessions"
              />
            </section>
          </div>
        )}
      </section>

      <aside className="fleetPanel accessInspector">
        <div className="accessConfigHeading">
          <strong>
            {activeSubpage === "VPS identities"
              ? "Direct identity actions"
              : "Access actions"}
          </strong>
          <span>
            {canManageOperators ? "Admin controls" : "Admin role required"}
          </span>
        </div>

        <div
          className="sectionHeader compact"
          hidden={activeSubpage !== "VPS identities"}
        >
          <h2>Import identity</h2>
          <span>
            {identityError ??
              (identityMode === "rotate"
                ? "Rotate an existing VPS gateway key"
                : "Register a VPS client ID and Noise public key")}
          </span>
        </div>
        <nav
          className="accessSubnav"
          hidden={activeSubpage !== "VPS identities"}
        >
          <button
            className={identityMode === "register" ? "selected" : ""}
            onClick={prepareNewIdentity}
            type="button"
          >
            New registration
          </button>
          <button
            className={identityMode === "rotate" ? "selected" : ""}
            onClick={() => {
              setIdentityMode("rotate");
              clearIdentityReview();
            }}
            type="button"
          >
            Key rotation
          </button>
        </nav>
        <form
          className="sideForm"
          hidden={activeSubpage !== "VPS identities"}
          onSubmit={requestIdentityImport}
          ref={identityFormRef}
        >
          <div className="formNote identityFormGuide">
            <strong>
              {identityMode === "rotate"
                ? "Rotation keeps the VPS identity and replaces only the key."
                : "Registration starts an install-ready VPS identity."}
            </strong>
            <span>
              Generate a keypair when creating a new identity, then copy the install command after gateway defaults are configured in Preferences.
            </span>
          </div>
          <label>
            <span>VPS client ID</span>
            <input
              aria-label="Agent identity client ID"
              disabled={!canManageOperators || identityPending}
              onChange={(event) => {
                setIdentityClientId(event.target.value);
                clearIdentityReview();
              }}
              placeholder={
                identityMode === "rotate"
                  ? "existing VPS ID"
                  : "new VPS ID"
              }
              value={identityClientId}
            />
          </label>
          <label className="wideField">
            <span>Noise public key</span>
            <textarea
              aria-label="Agent identity public key hex"
              disabled={!canManageOperators || identityPending}
              onChange={(event) => {
                setIdentityPublicKeyHex(event.target.value);
                clearIdentityReview();
              }}
              placeholder="64 hex characters"
              rows={3}
              value={identityPublicKeyHex}
            />
            <small className="fieldHelp">
              64 hex characters. Use Generate keypair for a new install, or paste the agent public key for a pre-generated identity.
            </small>
            <button
              className="secondaryAction compact"
              disabled={!canManageOperators || identityPending}
              onClick={() => {
                clearIdentityReview();
                void handleGenerateKeypair();
              }}
              type="button"
            >
              <KeyRound size={15} />
              Generate keypair
            </button>
          </label>
          {privateKeyHex && (
            <div className="inlineSecret">
              <strong>Private key</strong>
              <div className="secretRow">
                <input
                  aria-label="Agent identity private key"
                  className="monospace"
                  readOnly
                  value={privateKeyHex}
                />
                <button
                  className="secondaryAction compact"
                  onClick={handleCopyPrivateKey}
                  type="button"
                >
                  <Copy size={15} />
                  Copy
                </button>
              </div>
              <small>
                Store this key securely. It is not saved by the panel and cannot be
                recovered.
              </small>
            </div>
          )}
          <label>
            <span>Display name</span>
            <input
              aria-label="Agent identity display name"
              disabled={
                !canManageOperators || identityPending || identityMode === "rotate"
              }
              onChange={(event) => {
                setIdentityDisplayName(event.target.value);
                clearIdentityReview();
              }}
              placeholder={
                identityMode === "rotate" ? "unchanged" : "edge-nrt-04"
              }
              value={identityDisplayName}
            />
          </label>
          <label>
            <span>Tags</span>
            <input
              aria-label="Agent identity tags"
              disabled={
                !canManageOperators || identityPending || identityMode === "rotate"
              }
              onChange={(event) => {
                setIdentityTags(event.target.value);
                clearIdentityReview();
              }}
              placeholder={
                identityMode === "rotate" ? "unchanged" : "country:JP, role:edge"
              }
              value={identityTags}
            />
          </label>
          <button
            className="secondaryAction"
            disabled={!canUpsertIdentity}
            type="submit"
          >
            <Fingerprint size={17} />
            {identityReviewPending
              ? "Preparing review"
              : identityMode === "rotate"
              ? "Rotate key"
              : "Import gateway identity"}
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
          {createdIdentity && privateKeyHex && (
            <InstallCommand
              clientId={createdIdentity.client_id}
              privateKeyHex={privateKeyHex}
              preferences={operator?.preferences ?? null}
            />
          )}
        </form>

        <div
          className="sectionHeader compact"
          hidden={activeSubpage !== "VPS identities"}
        >
          <h2>Revoke key</h2>
          <span>{revokeError ?? "Block the current gateway key"}</span>
        </div>
        <form
          className="sideForm"
          hidden={activeSubpage !== "VPS identities"}
          onSubmit={requestClientKeyRevoke}
          ref={revokeFormRef}
        >
          <label>
            <span>VPS ID</span>
            <VpsCombobox
              agents={lifecycleVpsOptions}
              ariaLabel="VPS identity revoke VPS ID"
              disabled={!canManageOperators || revokePending}
              onChange={(value) => {
                setRevokeClientId(value);
                clearRevokeReview();
              }}
              placeholder="Search VPS identity"
              value={revokeClientId}
            />
          </label>
          <label>
            <span>Reason</span>
            <input
              aria-label="VPS identity revoke reason"
              disabled={!canManageOperators || revokePending}
              onChange={(event) => {
                setRevokeReason(event.target.value);
                clearRevokeReview();
              }}
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
            {revokeReviewPending ? "Preparing review" : "Revoke current key"}
          </button>
        </form>

        <div
          className="accessConfigHeading"
          hidden={activeSubpage !== "Gateway sessions"}
        >
          <strong>Gateway model</strong>
          <span>Agents connect to configured gateways; the panel does not perform endpoint lookup.</span>
        </div>
        <div className="timeline" hidden={activeSubpage !== "Gateway sessions"}>
          <Wifi size={18} />
          <div>
            <strong>Endpoint priority controls agent routing</strong>
            <span>
              Use Preferences for install defaults and Suite config for gateway
              bind/control settings.
            </span>
            <div className="inlineActions">
              <button className="secondaryAction compact" onClick={onOpenSystemPreferences} type="button">
                Preferences
              </button>
              <button className="secondaryAction compact" onClick={onOpenSystemConfig} type="button">
                Suite config
              </button>
            </div>
          </div>
        </div>

        <div
          className="timeline"
          hidden={
            activeSubpage === "VPS identities" ||
            activeSubpage === "Gateway sessions"
          }
        >
          <LockKeyhole size={18} />
          <div>
            <strong>Deny by default</strong>
            <span>
              Mutating work still requires explicit confirmation and privilege
              material when the server marks it privileged.
            </span>
            <div className="inlineActions">
              <button
                className="secondaryAction compact"
                disabled={!apiToken}
                onClick={() => setPendingConfirmation("session-clear")}
                type="button"
              >
                Clear local session
              </button>
              <button
                className="secondaryAction compact dangerAction"
                disabled={!vaultAvailable && !privilegeMaterial}
                onClick={() => setPendingConfirmation("vault-clear")}
                type="button"
              >
                Clear privilege vault
              </button>
            </div>
          </div>
        </div>
      </aside>

      <ConfirmationPrompt
        confirmLabel={
          identitySnapshot?.replaceExistingKey ? "Rotate key" : "Import identity"
        }
        detail={
          identitySnapshot?.replaceExistingKey
            ? "This replaces the stored client public key, disconnects the old gateway session, and marks old active work lost."
            : "This registers a gateway-issued client public key for inventory and key lifecycle management. It does not create a token and does not give the agent a panel endpoint."
        }
        items={[
          { label: "Client", value: identitySnapshot?.clientId ?? "" },
          {
            label: "Public key",
            title: identitySnapshot?.publicKeyHex,
            value: identitySnapshot ? shortHash(identitySnapshot.publicKeyHex) : "",
          },
          {
            label: "Mode",
            value: identitySnapshot?.replaceExistingKey
              ? "key rotation"
              : "new registration",
          },
        ]}
        onCancel={() => {
          setIdentitySnapshot(null);
          setPendingConfirmation(null);
        }}
        onConfirm={() => void confirmIdentityImport()}
        open={pendingConfirmation === "agent-identity" && Boolean(identitySnapshot)}
        pending={identityPending}
        title={
          identitySnapshot?.replaceExistingKey
            ? "Confirm client key rotation"
            : "Confirm direct gateway identity import"
        }
      />
      <ConfirmationPrompt
        confirmLabel="Revoke key"
        detail="The current stored public key is revoked, the VPS is hidden as revoked, the live gateway session is disconnected, and old active work is marked lost. Revoked or deleted identities cannot be reused through direct import."
        items={[
          { label: "VPS", value: revokeSnapshot?.clientId ?? "" },
          { label: "Reason", value: revokeSnapshot?.reason ?? "operator request" },
        ]}
        onCancel={() => {
          setRevokeSnapshot(null);
          setPendingConfirmation(null);
        }}
        onConfirm={() => void confirmClientKeyRevoke()}
        open={pendingConfirmation === "key-revoke" && Boolean(revokeSnapshot)}
        pending={revokePending}
        title="Confirm current key revocation"
        tone="danger"
      />
      <ConfirmationPrompt
        confirmLabel="Disable TOTP"
        detail="This disables TOTP for the current operator after validating the supplied password and authenticator code."
        onCancel={() => setPendingConfirmation(null)}
        onConfirm={() => void disableTotp()}
        open={pendingConfirmation === "totp-disable"}
        pending={totpPending}
        title="Confirm TOTP disable"
        tone="danger"
      />
      <ConfirmationPrompt
        confirmLabel={
          pendingConfirmation === "session-clear"
            ? "Clear bearer session"
            : "Clear privilege vault"
        }
        detail={
          pendingConfirmation === "session-clear"
            ? "This clears the current bearer token from the local panel session."
            : "This removes the encrypted local privilege vault and locks local privilege material."
        }
        onCancel={() => setPendingConfirmation(null)}
        onConfirm={confirmLocalAccessAction}
        open={
          pendingConfirmation === "session-clear" ||
          pendingConfirmation === "vault-clear"
        }
        title={
          pendingConfirmation === "session-clear"
            ? "Confirm bearer session clear"
            : "Confirm privilege vault clear"
        }
        tone="danger"
      />
    </div>
  );
}

function AccessPostureCard({
  action,
  detail,
  icon,
  label,
  onAction,
  tone = "neutral",
  value,
}: {
  action: string;
  detail: string;
  icon: ReactNode;
  label: string;
  onAction: () => void;
  tone?: "attention" | "neutral" | "ready";
  value: string;
}) {
  return (
    <div className={`accessPostureCard ${tone}`}>
      <div className="accessPostureIcon">{icon}</div>
      <div className="accessPostureBody">
        <span>{label}</span>
        <strong>{value}</strong>
        <p>{detail}</p>
      </div>
      <button className="secondaryAction compact" onClick={onAction} type="button">
        {action}
      </button>
    </div>
  );
}

function IdentityLifecycleGuide({
  blockedCount,
  onNewIdentity,
  registeredCount,
}: {
  blockedCount: number;
  onNewIdentity: () => void;
  registeredCount: number;
}) {
  const steps = [
    "Register",
    "Pending install",
    "Connected",
    "Rotate",
    "Revoke",
    "Blocked",
  ];
  return (
    <section className="accessLifecycleGuide" aria-label="Agent identity lifecycle">
      <div>
        <strong>Agent identity lifecycle</strong>
        <span>
          {registeredCount} registered identities; {blockedCount} pending, revoked, or blocked.
        </span>
      </div>
      <ol>
        {steps.map((step) => (
          <li key={step}>{step}</li>
        ))}
      </ol>
      <button className="secondaryAction compact" onClick={onNewIdentity} type="button">
        <Fingerprint size={15} />
        Register VPS
      </button>
    </section>
  );
}

function GatewayReadinessPanel({
  activeSessions,
  gatewaySessions,
  installDefaultsReady,
  onOpenPreferences,
  onOpenSuiteConfig,
}: {
  activeSessions: number;
  gatewaySessions: number;
  installDefaultsReady: boolean;
  onOpenPreferences: () => void;
  onOpenSuiteConfig: () => void;
}) {
  return (
    <section className="accessReadinessPanel" aria-label="Gateway readiness">
      <div className={installDefaultsReady ? "ready" : "attention"}>
        <span>Install defaults</span>
        <strong>{installDefaultsReady ? "Configured" : "Missing endpoints or server key"}</strong>
        <p>Preferences provide generated agent install commands with gateway endpoint priority and server key.</p>
      </div>
      <div className={activeSessions > 0 ? "ready" : "attention"}>
        <span>Live sessions</span>
        <strong>{activeSessions} active / {gatewaySessions} recent</strong>
        <p>Gateway sessions show agent connectivity evidence; absence is actionable, not just empty.</p>
      </div>
      <div>
        <span>Routing model</span>
        <strong>No panel-side endpoint lookup</strong>
        <p>Agents use their configured gateway endpoint list; suite config controls gateway bind and control listener.</p>
      </div>
      <div className="accessReadinessActions">
        <button className="secondaryAction compact" onClick={onOpenPreferences} type="button">
          Preferences
        </button>
        <button className="secondaryAction compact" onClick={onOpenSuiteConfig} type="button">
          Suite config
        </button>
      </div>
    </section>
  );
}

function AccessQueueRow({
  action,
  detail,
  label,
  onClick,
  value,
}: {
  action: string;
  detail: string;
  label: string;
  onClick: () => void;
  value: string;
}) {
  return (
    <div className="accessQueueRow">
      <div>
        <strong>{label}</strong>
        <span>{detail}</span>
      </div>
      <span className="accessQueueValue">{value}</span>
      <button className="secondaryAction compact" onClick={onClick} type="button">
        {action}
      </button>
    </div>
  );
}

function AccessWorkflowLink({
  detail,
  icon,
  label,
  onClick,
}: {
  detail: string;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button className="accessWorkflowLink" onClick={onClick} type="button">
      <span className="accessWorkflowIcon">{icon}</span>
      <span>
        <strong>{label}</strong>
        <small>{detail}</small>
      </span>
    </button>
  );
}

function AccessSelectionPanel({
  label,
  value,
}: {
  label: string;
  value: string;
}) {
  return (
    <div className="accessSelectionPanel">
      <strong>{label}</strong>
      <span>{value}</span>
    </div>
  );
}

function IdentityDetailGrid({
  client,
  vpsNameDisplayMode,
}: {
  client: KeyLifecycleClientView;
  vpsNameDisplayMode: VpsNameDisplayMode;
}) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>VPS</strong>
        <span>
          {formatVpsName(
            {
              client_id: client.client_id,
              display_name: client.display_name,
            },
            vpsNameDisplayMode,
          )}
        </span>
      </span>
      <span>
        <strong>Client ID</strong>
        <span className="monoValue">{client.client_id}</span>
      </span>
      <span>
        <strong>Status</strong>
        <span>{identityStatus(client)}</span>
      </span>
      <span>
        <strong>Current key</strong>
        <span className="monoValue">
          {client.current_public_key_sha256_hex ?? "none"}
        </span>
      </span>
      <span>
        <strong>Latest revoke</strong>
        <span>
          {client.latest_revoked_at
            ? formatTime(client.latest_revoked_at)
            : "none"}
        </span>
      </span>
      <span>
        <strong>Reason</strong>
        <span>{client.latest_revocation_reason ?? "none"}</span>
      </span>
    </div>
  );
}

function RevocationDetailGrid({
  label,
  revocation,
}: {
  label: string;
  revocation: ClientKeyRevocationView;
}) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>VPS</strong>
        <span>{label}</span>
      </span>
      <span>
        <strong>Client ID</strong>
        <span className="monoValue">{revocation.client_id}</span>
      </span>
      <span>
        <strong>Key hash</strong>
        <span className="monoValue">{revocation.public_key_sha256_hex}</span>
      </span>
      <span>
        <strong>Reason</strong>
        <span>{revocation.reason ?? "operator request"}</span>
      </span>
      <span>
        <strong>Revoked by</strong>
        <span>{revocation.revoked_by ?? "unknown"}</span>
      </span>
      <span>
        <strong>Created</strong>
        <span>{formatTime(revocation.created_at)}</span>
      </span>
    </div>
  );
}

function GatewaySessionDetailGrid({
  label,
  session,
}: {
  label: string;
  session: GatewaySessionRecord;
}) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>VPS</strong>
        <span>{label}</span>
      </span>
      <span>
        <strong>Session ID</strong>
        <span className="monoValue">{session.id}</span>
      </span>
      <span>
        <strong>Gateway</strong>
        <span className="monoValue">{session.gateway_id}</span>
      </span>
      <span>
        <strong>Client ID</strong>
        <span className="monoValue">{session.client_id}</span>
      </span>
      <span>
        <strong>Status</strong>
        <span>{session.status}</span>
      </span>
      <span>
        <strong>Started</strong>
        <span>{formatTime(session.started_at)}</span>
      </span>
      <span>
        <strong>Last seen</strong>
        <span>{formatTime(session.last_seen_at)}</span>
      </span>
      <span>
        <strong>Ended</strong>
        <span>{session.ended_at ? formatTime(session.ended_at) : "active"}</span>
      </span>
      <span>
        <strong>End reason</strong>
        <span>{session.end_reason ?? (session.ended_at ? "ended" : "active")}</span>
      </span>
      <span>
        <strong>Noise key</strong>
        <span className="monoValue">{session.noise_public_key_hex ?? "n/a"}</span>
      </span>
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

function identityStatus(client: KeyLifecycleClientView): string {
  return client.current_key_revoked ? "blocked" : client.status;
}

function summarizeRoleModel(
  operators: OperatorView[],
  operator: OperatorView | null,
): { detail: string; value: string } {
  const source = operators.length > 0 ? operators : operator ? [operator] : [];
  if (source.length === 0) {
    return { detail: "Role records not loaded", value: "API gap" };
  }
  const counts = new Map<string, number>();
  source.forEach((entry) => {
    counts.set(entry.role, (counts.get(entry.role) ?? 0) + 1);
  });
  const roles = Array.from(counts.entries())
    .map(([role, count]) => `${role} ${count}`)
    .join(", ");
  const current = operator ? `${operator.role}: ${operator.scopes.join(", ") || "no scopes"}` : "current role unknown";
  return {
    detail: `${roles}; current ${current}`,
    value: `${counts.size} role${counts.size === 1 ? "" : "s"} visible`,
  };
}

function summarizeOperatorScopes(operator: OperatorView | null): string {
  if (!operator) {
    return "operator record unavailable";
  }
  if (operator.scopes.includes("*")) {
    return `${operator.role} with all scopes`;
  }
  return `${operator.role}; ${operator.scopes.length} scope${operator.scopes.length === 1 ? "" : "s"}`;
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

function scrollIntoViewSoon(element: HTMLElement | null) {
  if (!element) {
    return;
  }
  window.requestAnimationFrame(() => {
    scrollIntoViewWithMotion(element, { block: "start" });
  });
}

function InstallCommand({
  clientId,
  privateKeyHex,
  preferences,
}: {
  clientId: string;
  privateKeyHex: string;
  preferences: {
    gateway_server_public_key_hex: string | null;
    gateway_endpoints: string;
  } | null;
}) {
  const endpoints =
    (preferences?.gateway_endpoints ?? "").trim();
  const gatewayKey =
    (preferences?.gateway_server_public_key_hex ?? "").trim();

  if (!endpoints || !gatewayKey) {
    return (
      <div className="formNote mutedNote">
        <span>
          Gateway endpoints or public key not configured. Set them in{" "}
          <strong>Preferences → Operator</strong> to generate an install command.
        </span>
      </div>
    );
  }

  const command =
    [
      "curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh | env \\",
      "  VPSMAN_INSTALL_MODE=root \\",
      `  VPSMAN_AGENT_CLIENT_ID=${clientId} \\`,
      `  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=${privateKeyHex} \\`,
      `  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=${gatewayKey} \\`,
      `  VPSMAN_GATEWAY_ENDPOINTS='${endpoints}' \\`,
      "  bash",
    ].join("\n");

  function handleCopy() {
    navigator.clipboard.writeText(command).catch(() => {});
  }

  return (
    <div className="installCommandBlock">
      <div className="sectionHeader compact">
        <strong>Install command</strong>
        <button
          className="secondaryAction compact"
          onClick={handleCopy}
          type="button"
        >
          <Copy size={15} />
          Copy
        </button>
      </div>
      <pre>
        <code>{command}</code>
      </pre>
    </div>
  );
}
