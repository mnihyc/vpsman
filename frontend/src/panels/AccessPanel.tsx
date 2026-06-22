import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  Ban,
  Copy,
  Fingerprint,
  KeyRound,
  LockKeyhole,
  RefreshCw,
  Save,
  ShieldCheck,
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
import { usePanelDisplaySettings } from "../panelDisplay";
import type {
  GatewaySessionRecord,
  OperatorView,
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
  "Privilege unlock",
  "VPS keys",
  "Gateway",
] as const;

type AccessSubpage = (typeof accessSubpages)[number];
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
  onRefresh: () => Promise<void>;
  onRevokeClientKey: (
    clientId: string,
    reason: string | null,
    confirmed: boolean,
    privilegeAssertion: PrivilegeAssertion | null,
  ) => Promise<void>;
  onSetupTotp: (password: string) => Promise<TotpSetupResponse | null>;
  onUpsertAgentIdentity: (
    request: UpsertAgentIdentityRequest,
  ) => Promise<AgentIdentityView>;
  operator: OperatorView | null;
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
  onDisableTotp,
  onRefresh,
  onRevokeClientKey,
  onSetupTotp,
  onUpsertAgentIdentity,
  operator,
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
  const revokedClientCount = lifecycleClients.filter(
    (client) => client.status === "revoked" || client.current_key_revoked,
  ).length;
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
    setActiveSubpage("VPS keys");
    scrollIntoViewSoon(identityFormRef.current);
  }

  function prepareClientKeyRevoke(clientId: string, reason = "") {
    clearRevokeReview();
    setRevokeClientId(clientId);
    setRevokeReason(reason);
    setRevokeError(null);
    setActiveSubpage("VPS keys");
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
          : "Privilege unlock is required",
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
      setRevokeError(privilegeMaterial ? "VPS ID is required" : "Privilege unlock is required");
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
        <div className="sectionHeader heroHeader compactHeroHeader">
          <div>
            <h2>Access control</h2>
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
                  label="Command transport"
                  value="Noise-secured gateway sessions"
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
                <h2>Attention queues</h2>
                <span>Jump from access state to the working table</span>
              </div>
              <div className="accessQueueList">
                <AccessQueueRow
                  action="Review keys"
                  detail={`${keyLifecycleReport?.revocation_count ?? clientKeyRevocations.length} revocations retained`}
                  label="Blocked or revoked keys"
                  onClick={() => setActiveSubpage("VPS keys")}
                  value={`${revokedClientCount} current blocked`}
                />
                <AccessQueueRow
                  action="Open gateway"
                  detail={`${gatewaySessions.length} recent sessions`}
                  label="Gateway sessions"
                  onClick={() => setActiveSubpage("Gateway")}
                  value={`${activeGatewaySessions} active`}
                />
                <AccessQueueRow
                  action="Unlock"
                  detail={privilegeMaterial ? "ready for privileged review" : "required for key lifecycle actions"}
                  label="Privilege state"
                  onClick={() => setActiveSubpage("Privilege unlock")}
                  value={vaultState}
                />
                <AccessQueueRow
                  action="Open access"
                  detail={`token in ${tokenStorageState}`}
                  label="Browser session"
                  onClick={() => setActiveSubpage("Privilege unlock")}
                  value={sessionState}
                />
              </div>
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

        {activeSubpage === "VPS keys" && (
          <div className="workspaceSection accessTableStack">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>Gateway agent identities</h2>
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
                empty="No gateway agent identities"
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
                title="Gateway agent identities"
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
          hidden={activeSubpage !== "VPS keys"}
        >
          <h2>Import identity</h2>
          <span>
            {identityError ?? "Gateway-issued client ID and public key"}
          </span>
        </div>
        <nav
          className="accessSubnav"
          hidden={activeSubpage !== "VPS keys"}
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
          hidden={activeSubpage !== "VPS keys"}
          onSubmit={requestIdentityImport}
          ref={identityFormRef}
        >
          <label>
            <span>Client ID</span>
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
            <span>Noise public key hex</span>
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
          hidden={activeSubpage !== "VPS keys"}
        >
          <h2>Revoke key</h2>
          <span>{revokeError ?? "Block the current gateway key"}</span>
        </div>
        <form
          className="sideForm"
          hidden={activeSubpage !== "VPS keys"}
          onSubmit={requestClientKeyRevoke}
          ref={revokeFormRef}
        >
          <label>
            <span>VPS ID</span>
            <VpsCombobox
              agents={lifecycleVpsOptions}
              ariaLabel="VPS key revoke VPS ID"
              disabled={!canManageOperators || revokePending}
              onChange={(value) => {
                setRevokeClientId(value);
                clearRevokeReview();
              }}
              placeholder="Search VPS key"
              value={revokeClientId}
            />
          </label>
          <label>
            <span>Reason</span>
            <input
              aria-label="VPS key revoke reason"
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
          className="timeline"
          hidden={
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
    element.scrollIntoView({ block: "start", behavior: "smooth" });
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
