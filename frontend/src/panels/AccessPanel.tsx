import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
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
  X,
} from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
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
type AccessOverviewTone = "attention" | "neutral" | "ready";
type IdentityWorkflow = "register" | "rotate" | "revoke" | null;

type AccessOverviewItem = {
  action: string;
  detail: string;
  icon: ReactNode;
  label: string;
  onClick: () => void;
  tone: AccessOverviewTone;
  value: string;
};

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

function accessPanelHeader(subpage: AccessSubpage): {
  title: string;
  description: string;
} {
  switch (subpage) {
    case "VPS identities":
      return {
        title: "VPS identity registry",
        description:
          "Agent identity registration, key rotation, revocation, and install evidence",
      };
    case "Gateway sessions":
      return {
        title: "Gateway session inventory",
        description:
          "Live gateway connectivity evidence and shared gateway configuration",
      };
    case "Privilege vault":
      return {
        title: "Privilege workflow",
        description:
          "Local unlock state, request-bound assertions, and vault controls",
      };
    default:
      return {
        title: "Access overview",
        description:
          "Direct gateway identities, browser session state, and live access streams",
      };
  }
}

export function AccessPanel({
  activeSubpage: routeSubpage,
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
  setPrivilegeMaterial,
  wsState,
}: AccessPanelProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const identityFormRef = useRef<HTMLFormElement | null>(null);
  const revokeFormRef = useRef<HTMLFormElement | null>(null);
  const identityWorkflowRef = useRef<HTMLElement | null>(null);
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
  const [identityWorkflow, setIdentityWorkflow] =
    useState<IdentityWorkflow>(null);
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
  const vaultState = privilegeMaterial
    ? "Privilege unlocked"
    : vaultAvailable
      ? "Saved local privilege vault"
      : "No saved local vault";
  const currentSession =
    operatorSessions.find((session) => session.current) ?? operatorSessions[0];
  const adminMfaRisk = operator?.role === "admin" && !operator.totp_enabled;
  const gatewayInstallDefaultsReady = Boolean(
    operator?.preferences.gateway_endpoints.trim() &&
    operator.preferences.gateway_server_public_key_hex?.trim(),
  );
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
    isOperatorSessionActive,
  ).length;
  const expiredOperatorSessions = operatorSessions.filter(
    (session) => !session.revoked && isOperatorSessionExpired(session),
  ).length;
  const revokedClientCount = lifecycleClients.filter(
    (client) => client.status === "revoked" || client.current_key_revoked,
  ).length;
  const blockedOrPendingClientCount = lifecycleClients.filter((client) =>
    ["blocked", "pending", "revoked"].includes(identityStatus(client)),
  ).length;
  const currentSessionState = currentSession
    ? operatorSessionStateLabel(currentSession)
    : "Not listed";
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
  const identityColumns = useMemo<
    ConsoleDataGridColumn<KeyLifecycleClientView>[]
  >(
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
            <CopyableHash
              label="current key fingerprint"
              value={client.current_public_key_sha256_hex}
            />
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
            ? `${formatTime(client.latest_revoked_at)} · ${revocationReasonLabel(client.latest_revocation_reason)}`
            : "none",
        searchValue: (client) =>
          `${client.latest_revoked_at ?? ""} ${client.latest_revocation_reason ?? ""}`,
        sortValue: (client) => client.latest_revoked_at ?? "",
        size: 260,
      },
    ],
    [vpsNameDisplayMode],
  );
  const revocationColumns = useMemo<
    ConsoleDataGridColumn<ClientKeyRevocationView>[]
  >(
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
          <CopyableHash
            label="revoked key fingerprint"
            value={revocation.public_key_sha256_hex}
          />
        ),
        searchValue: (revocation) => revocation.public_key_sha256_hex,
        size: 180,
      },
      {
        id: "reason",
        header: "Reason",
        cell: (revocation) => revocationReasonLabel(revocation.reason),
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
  const gatewaySessionColumns = useMemo<
    ConsoleDataGridColumn<GatewaySessionRecord>[]
  >(
    () => [
      {
        id: "gateway",
        header: "Gateway",
        cell: (session) => session.gateway_id,
        searchValue: (session) => session.gateway_id,
        sortValue: (session) => session.gateway_id,
        size: 160,
      },
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
        id: "state",
        header: "State",
        cell: (session) => (
          <span className={`statusPill ${statusClass(session.status)}`}>
            {gatewaySessionStateLabel(session.status)}
          </span>
        ),
        searchValue: (session) => session.status,
        sortValue: (session) => session.status,
        size: 120,
      },
      {
        id: "connected",
        header: "Connected",
        cell: (session) => formatTime(session.started_at),
        sortValue: (session) => session.started_at,
        size: 180,
      },
      {
        id: "lastActivity",
        header: "Last activity",
        cell: (session) => formatTime(session.last_seen_at),
        sortValue: (session) => session.last_seen_at,
        size: 190,
      },
      {
        id: "remoteIp",
        header: "Remote IP",
        cell: (session) => session.remote_ip ?? "not reported",
        searchValue: (session) => session.remote_ip ?? "",
        sortValue: (session) => session.remote_ip ?? "",
        size: 150,
      },
      {
        id: "version",
        header: "Version",
        cell: (session) => session.agent_version || "unknown",
        searchValue: (session) => session.agent_version,
        sortValue: (session) => session.agent_version,
        size: 150,
      },
    ],
    [lifecycleNameById],
  );

  useEffect(() => {
    setActiveSubpage(accessSubpageFromRoute(routeSubpage));
  }, [routeSubpage]);

  function openAccessSubpage(subpage: AccessSubpage) {
    if (subpage !== "VPS identities") {
      setIdentityWorkflow(null);
    }
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
    setPendingConfirmation((current) =>
      current === "key-revoke" ? null : current,
    );
  }

  function scrollIdentityWorkflowSoon() {
    window.setTimeout(() => scrollIntoViewSoon(identityWorkflowRef.current), 0);
  }

  function closeIdentityWorkflow() {
    setIdentityWorkflow(null);
    clearIdentityReview();
    clearRevokeReview();
  }

  function prepareNewIdentity() {
    clearIdentityReview();
    clearRevokeReview();
    setIdentityMode("register");
    setIdentityWorkflow("register");
    setIdentityClientId("");
    setIdentityPublicKeyHex("");
    setIdentityDisplayName("");
    setIdentityTags("");
    setPrivateKeyHex(null);
    setCreatedIdentity(null);
    setIdentityError(null);
    openAccessSubpage("VPS identities");
    scrollIdentityWorkflowSoon();
  }

  function prepareIdentityRotation(client: KeyLifecycleClientView) {
    clearIdentityReview();
    clearRevokeReview();
    setIdentityMode("rotate");
    setIdentityWorkflow("rotate");
    setIdentityClientId(client.client_id);
    setIdentityPublicKeyHex("");
    setIdentityDisplayName("");
    setIdentityTags("");
    setPrivateKeyHex(null);
    setCreatedIdentity(null);
    setIdentityError(null);
    openAccessSubpage("VPS identities");
    scrollIdentityWorkflowSoon();
  }

  function prepareClientKeyRevoke(clientId: string, reason = "") {
    clearIdentityReview();
    clearRevokeReview();
    setIdentityWorkflow("revoke");
    setRevokeClientId(clientId);
    setRevokeReason(reason);
    setRevokeError(null);
    openAccessSubpage("VPS identities");
    scrollIdentityWorkflowSoon();
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
      setIdentityError(
        "Key generation failed — browser may not support Web Crypto",
      );
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
      setIdentityWorkflow(snapshot.replaceExistingKey ? null : "register");
    } catch (actionError) {
      setIdentityError(
        actionError instanceof Error
          ? actionError.message
          : "VPS identity update failed",
      );
    } finally {
      setIdentityPending(false);
    }
  }

  async function requestClientKeyRevoke(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const clientId = revokeClientId.trim();
    if (!canRevokeClientKey) {
      setRevokeError(
        privilegeMaterial
          ? "VPS ID is required"
          : "Privilege vault unlock is required",
      );
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
      setIdentityWorkflow(null);
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

  const accessRequiredActionCandidates: Array<AccessOverviewItem | null> = [
    adminMfaRisk
      ? {
          action: "Set up MFA",
          detail:
            "Admin MFA is recommended; policy enforcement is not exposed by the API.",
          icon: <ShieldCheck size={16} />,
          label: "Policy recommends MFA",
          onClick: () => openAccessSubpage("Privilege vault"),
          tone: "attention",
          value: "Recommended",
        }
      : null,
    expiredOperatorSessions > 0
      ? {
          action: "Manage sessions",
          detail: `${expiredOperatorSessions} listed bearer session${expiredOperatorSessions === 1 ? "" : "s"} expired and are excluded from active-session counts.`,
          icon: <Clock size={16} />,
          label: "Expired bearer sessions",
          onClick: onOpenSystemSessions,
          tone: "attention",
          value: `${expiredOperatorSessions} expired`,
        }
      : null,
    blockedOrPendingClientCount > 0
      ? {
          action: "Open identities",
          detail:
            "Pending, revoked, or blocked VPS identities need operator review.",
          icon: <Fingerprint size={16} />,
          label: "VPS identity attention",
          onClick: () => openAccessSubpage("VPS identities"),
          tone: "attention",
          value: `${blockedOrPendingClientCount} need review`,
        }
      : null,
    gatewayInstallDefaultsReady
      ? null
      : {
          action: "Open Preferences",
          detail:
            "Gateway endpoints and server public key are needed for generated agent install commands.",
          icon: <Wifi size={16} />,
          label: "Gateway install defaults",
          onClick: onOpenSystemPreferences,
          tone: "attention",
          value: "Missing",
        },
    privilegeMaterial
      ? null
      : {
          action: "Unlock",
          detail: "No saved local vault; enter privilege secret when needed.",
          icon: <LockKeyhole size={16} />,
          label: "Privilege state",
          onClick: () => openAccessSubpage("Privilege vault"),
          tone: vaultAvailable ? "neutral" : "attention",
          value: vaultState,
        },
  ];
  const accessRequiredActions = accessRequiredActionCandidates.filter(
    (item): item is AccessOverviewItem => Boolean(item),
  );

  const accessResponsibilityRows: AccessOverviewItem[] = [
    {
      action: "Open Operators",
      detail: `${operators.length} operators; ${activeOperatorSessions} active session${activeOperatorSessions === 1 ? "" : "s"} after expiry validation; current session ${currentSessionState}.`,
      icon: <UsersRound size={16} />,
      label: "Operators and active sessions",
      onClick: onOpenSystemUsers,
      tone:
        operators.length === 0 || expiredOperatorSessions > 0 || adminMfaRisk
          ? "attention"
          : "ready",
      value: `${operators.length} operators / ${activeOperatorSessions} active`,
    },
    {
      action: "Open identities",
      detail: `${keyLifecycleReport?.revocation_count ?? clientKeyRevocations.length} revocation records; ${revokedClientCount} current keys blocked.`,
      icon: <Fingerprint size={16} />,
      label: "VPS identities",
      onClick: () => openAccessSubpage("VPS identities"),
      tone: blockedOrPendingClientCount > 0 ? "attention" : "ready",
      value: `${keyLifecycleReport?.direct_identity_client_count ?? lifecycleClients.length} registered`,
    },
    {
      action: "Open sessions",
      detail: gatewayInstallDefaultsReady
        ? `${gatewaySessions.length} recent gateway sessions; install defaults configured.`
        : "Gateway install defaults are missing endpoint or server-key settings.",
      icon: <Wifi size={16} />,
      label: "Gateway sessions",
      onClick: () => openAccessSubpage("Gateway sessions"),
      tone: gatewayInstallDefaultsReady ? "ready" : "attention",
      value: `${activeGatewaySessions} active / ${gatewaySessions.length} recent`,
    },
    {
      action: privilegeMaterial ? "Open vault" : "Unlock",
      detail: privilegeMaterial
        ? "Privilege material is local-only and used for request-bound assertions."
        : "No saved local vault; enter privilege secret when needed.",
      icon: <LockKeyhole size={16} />,
      label: "Privilege state",
      onClick: () => openAccessSubpage("Privilege vault"),
      tone: privilegeMaterial
        ? "ready"
        : vaultAvailable
          ? "neutral"
          : "attention",
      value: privilegeMaterial ? "Unlocked for this browser" : vaultState,
    },
  ];
  const activePanelHeader = accessPanelHeader(activeSubpage);

  return (
    <div className="workspace accessWorkspace">
      <section className="fleetPanel accessMain">
        <div className="sectionHeader compactSectionHeader">
          <div>
            <h2>{activePanelHeader.title}</h2>
            <span>
              {error ??
                (loading
                  ? "Refreshing access records"
                  : activePanelHeader.description)}
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
          <div className="workspaceSection accessOverviewFocus">
            <section
              className="controlPanel accessOverviewPanel"
              aria-label="Access actions required"
            >
              <div className="sectionHeader compact">
                <h2>Actions required</h2>
                <span>
                  Critical access items first; each action opens the canonical
                  operating page.
                </span>
              </div>
              {accessRequiredActions.length > 0 ? (
                <div className="accessOverviewRows">
                  {accessRequiredActions.map((item) => (
                    <AccessOverviewRow item={item} key={item.label} />
                  ))}
                </div>
              ) : (
                <div className="accessOverviewEmpty">
                  <ShieldCheck size={18} />
                  <span>
                    <strong>No immediate access actions</strong>
                    <small>
                      Operators, sessions, identities, gateway, and privilege
                      state have no visible critical warnings in the loaded
                      evidence.
                    </small>
                  </span>
                </div>
              )}
            </section>
            <section
              className="controlPanel accessOverviewPanel"
              aria-label="Access overview responsibilities"
            >
              <div className="sectionHeader compact">
                <h2>Access responsibilities</h2>
                <span>
                  Operators and sessions, VPS identities, gateway sessions, and
                  privilege state.
                </span>
              </div>
              <div className="accessOverviewRows">
                {accessResponsibilityRows.map((item) => (
                  <AccessOverviewRow item={item} key={item.label} />
                ))}
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
                clearVaultLabel="Clear local vault"
                labelPrefix="Access"
                lastPayloadHash={privilegeMaterial ? "unlocked" : null}
                lockPrivilegeLabel="Lock now"
                onPrivilegeMaterialChange={setPrivilegeMaterial}
                onVaultAvailabilityChange={setVaultAvailable}
                privilegeMaterial={privilegeMaterial}
                unlockLabel="Unlock saved vault"
                usePrivilegeLabel="Unlock"
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
              <div
                className={`accessRiskNotice ${adminMfaRisk ? "attention" : "ready"}`}
              >
                {adminMfaRisk ? (
                  <AlertTriangle size={17} />
                ) : (
                  <ShieldCheck size={17} />
                )}
                <div>
                  <strong>
                    {adminMfaRisk ? "Admin MFA is off" : "MFA posture recorded"}
                  </strong>
                  <span>
                    {adminMfaRisk
                      ? "Production admin accounts should require TOTP before long-lived access or privileged workflows."
                      : operator?.totp_enabled
                        ? "This operator account has TOTP enabled."
                        : "TOTP is available for this operator account."}
                  </span>
                </div>
              </div>
              {operator?.totp_enabled ? (
                <div className="totpDisablePanel">
                  <div>
                    <strong>TOTP is enabled</strong>
                    <span>
                      Disabling requires the current password and an
                      authenticator code.
                    </span>
                  </div>
                  <div className="totpActionGrid">
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
              ) : (
                <div
                  aria-label="TOTP enrollment sequence"
                  className="totpWorkflow"
                >
                  <ol className="totpStepList">
                    <li className={totpPassword ? "ready" : "active"}>
                      <span>1</span>
                      <strong>Password</strong>
                    </li>
                    <li className={totpSetup ? "ready" : ""}>
                      <span>2</span>
                      <strong>QR/secret</strong>
                    </li>
                    <li className={totpCode ? "ready" : ""}>
                      <span>3</span>
                      <strong>Enter code</strong>
                    </li>
                    <li>
                      <span>4</span>
                      <strong>Complete</strong>
                    </li>
                  </ol>
                  <div className="totpActionGrid">
                    <label>
                      <span>Current password</span>
                      <input
                        aria-label="TOTP password"
                        onChange={(event) => setTotpPassword(event.target.value)}
                        type="password"
                        value={totpPassword}
                      />
                    </label>
                    <button
                      className="secondaryAction"
                      disabled={totpPending || !totpPassword}
                      onClick={() => void setupTotp()}
                      type="button"
                    >
                      <ShieldCheck size={17} />
                      Generate setup
                    </button>
                    {totpSetup ? (
                      <div className="totpSecretPanel">
                        <strong>Authenticator secret</strong>
                        <span>{totpSetup.secret_base32}</span>
                        <small>{totpSetup.otpauth_uri}</small>
                      </div>
                    ) : (
                      <div className="totpSecretPanel muted">
                        <strong>Authenticator secret</strong>
                        <span>Generate setup after entering the password.</span>
                      </div>
                    )}
                    <label>
                      <span>Authenticator code</span>
                      <input
                        aria-label="TOTP code"
                        disabled={!totpSetup}
                        onChange={(event) => setTotpCode(event.target.value)}
                        value={totpCode}
                      />
                    </label>
                    <button
                      className="primaryAction"
                      disabled={totpPending || !totpPassword || !totpCode}
                      onClick={() => void confirmTotp()}
                      type="button"
                    >
                      <Save size={17} />
                      Complete setup
                    </button>
                  </div>
                </div>
              )}
              {!operator?.totp_enabled && (
                <div className="totpDisablePanel disabled">
                  <div>
                    <strong>Disable TOTP</strong>
                    <span>No active TOTP factor is recorded for this account.</span>
                  </div>
                  <button
                    className="secondaryAction dangerAction"
                    disabled
                    type="button"
                  >
                    <Trash2 size={17} />
                    Review disable
                  </button>
                </div>
              )}
            </section>
          </div>
        )}

        {activeSubpage === "VPS identities" && (
          <div className="workspaceSection accessTableStack">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>VPS identities</h2>
                <span>
                  {keyLifecycleReport?.direct_identity_client_count ??
                    lifecycleClients.length}{" "}
                  registered; {blockedOrPendingClientCount} need review. Use row
                  actions to rotate or revoke keys.
                </span>
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
                    onSelect: (rows) =>
                      prepareClientKeyRevoke(rows[0].client_id),
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
                    onSelect: (rows) =>
                      prepareClientKeyRevoke(rows[0].client_id),
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
                    <span>Register VPS</span>
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
            {gatewaySessions.length === 0 ? (
              <GatewaySessionEmptyState onOpenGatewaySettings={onOpenSystemConfig} />
            ) : (
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
                  searchPlaceholder="Search gateway, VPS, state, remote IP, or version"
                  selectable={false}
                  singleExpandedRow
                  storageKey="vpsman.access.gatewaySessions"
                  title="Gateway sessions"
                />
              </section>
            )}
          </div>
        )}
      </section>

      <aside
        className={`fleetPanel accessInspector${activeSubpage === "VPS identities" ? " identityWorkflowPanel" : ""}`}
        hidden={
          activeSubpage === "Overview" ||
          activeSubpage === "Gateway sessions" ||
          activeSubpage === "Privilege vault" ||
          (activeSubpage === "VPS identities" && identityWorkflow === null)
        }
        ref={identityWorkflowRef}
      >
        <div className="accessConfigHeading identityWorkflowHeader">
          <div>
            <strong>
              {activeSubpage === "VPS identities"
                ? identityWorkflow === "revoke"
                  ? "Revoke VPS key"
                  : identityMode === "rotate"
                    ? "Rotate VPS key"
                    : "Register VPS"
                : "Access actions"}
            </strong>
            <span>
              {activeSubpage === "VPS identities"
                ? canManageOperators
                  ? "One focused identity workflow; review before mutation"
                  : "Admin role required"
                : canManageOperators
                  ? "Admin controls"
                  : "Admin role required"}
            </span>
          </div>
          {activeSubpage === "VPS identities" && (
            <button
              aria-label="Close VPS identity workflow"
              className="secondaryAction compact"
              onClick={closeIdentityWorkflow}
              type="button"
            >
              <X size={15} />
              Close
            </button>
          )}
        </div>

        <div
          className="sectionHeader compact"
          hidden={
            activeSubpage !== "VPS identities" ||
            identityWorkflow === null ||
            identityWorkflow === "revoke"
          }
        >
          <h2>{identityMode === "rotate" ? "Rotate key" : "Register VPS"}</h2>
          <span>
            {identityError ??
              (identityMode === "rotate"
                ? "Replace the selected VPS public key"
                : "Generate a keypair or import a public key")}
          </span>
        </div>
        <form
          className="sideForm"
          hidden={
            activeSubpage !== "VPS identities" ||
            identityWorkflow === null ||
            identityWorkflow === "revoke"
          }
          onSubmit={requestIdentityImport}
          ref={identityFormRef}
        >
          <div className="formNote identityFormGuide">
            <strong>
              {identityMode === "rotate"
                ? "Rotation keeps the VPS identity and replaces only the key."
                : "Register a VPS identity before installing or reconnecting the agent."}
            </strong>
            <span>
              Generate a keypair for a new install, or paste a pre-generated
              agent public key. Private key material is shown once and is never
              saved by the panel.
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
                identityMode === "rotate" ? "existing VPS ID" : "new VPS ID"
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
              64 hex characters. Use Generate keypair for a new install, or
              paste the agent public key for a pre-generated identity.
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
              <strong>Private key - shown once</strong>
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
                Store this key securely. It is not saved by the panel and cannot
                be recovered.
              </small>
            </div>
          )}
          <label>
            <span>Display name</span>
            <input
              aria-label="Agent identity display name"
              disabled={
                !canManageOperators ||
                identityPending ||
                identityMode === "rotate"
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
                !canManageOperators ||
                identityPending ||
                identityMode === "rotate"
              }
              onChange={(event) => {
                setIdentityTags(event.target.value);
                clearIdentityReview();
              }}
              placeholder={
                identityMode === "rotate"
                  ? "unchanged"
                  : "country:JP, role:edge"
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
                ? "Review rotation"
                : "Review registration"}
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
          hidden={
            activeSubpage !== "VPS identities" || identityWorkflow !== "revoke"
          }
        >
          <h2>Revoke VPS key</h2>
          <span>{revokeError ?? "Block the current VPS gateway key"}</span>
        </div>
        <form
          className="sideForm"
          hidden={
            activeSubpage !== "VPS identities" || identityWorkflow !== "revoke"
          }
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

      </aside>

      <ConfirmationPrompt
        confirmLabel={
          identitySnapshot?.replaceExistingKey ? "Rotate key" : "Register VPS"
        }
        detail={
          identitySnapshot?.replaceExistingKey
            ? "This replaces the stored VPS public key, disconnects the old gateway session, and marks old active work lost."
            : "This registers a VPS client ID and public key for gateway identity lifecycle management. It does not create a token and does not give the agent a panel endpoint."
        }
        items={[
          { label: "Client", value: identitySnapshot?.clientId ?? "" },
          {
            label: "Public key",
            title: identitySnapshot?.publicKeyHex,
            value: identitySnapshot
              ? shortHash(identitySnapshot.publicKeyHex)
              : "",
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
        open={
          pendingConfirmation === "agent-identity" && Boolean(identitySnapshot)
        }
        pending={identityPending}
        title={
          identitySnapshot?.replaceExistingKey
            ? "Confirm client key rotation"
            : "Confirm VPS identity registration"
        }
      />
      <ConfirmationPrompt
        confirmLabel="Revoke key"
        detail="The current stored public key is revoked, the VPS is hidden as revoked, the live gateway session is disconnected, and old active work is marked lost. Revoked or deleted identities cannot be reused through direct import."
        items={[
          { label: "VPS", value: revokeSnapshot?.clientId ?? "" },
          {
            label: "Reason",
            value: revokeSnapshot?.reason ?? "operator request",
          },
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

function AccessOverviewRow({ item }: { item: AccessOverviewItem }) {
  return (
    <div className={`accessOverviewRow ${item.tone}`}>
      <span className="accessOverviewIcon">{item.icon}</span>
      <span className="accessOverviewText">
        <strong>{item.label}</strong>
        <small>{item.detail}</small>
      </span>
      <span className="accessOverviewValue">{item.value}</span>
      <button
        className="secondaryAction compact"
        onClick={item.onClick}
        type="button"
      >
        {item.action}
      </button>
    </div>
  );
}

function GatewaySessionEmptyState({
  onOpenGatewaySettings,
}: {
  onOpenGatewaySettings: () => void;
}) {
  return (
    <section
      aria-label="Gateway sessions empty state"
      className="controlPanel gatewaySessionEmpty"
    >
      <div className="gatewaySessionEmptyIcon">
        <Wifi size={20} />
      </div>
      <div>
        <h2>Gateway sessions</h2>
        <p>No active gateway sessions. Configure the gateway endpoint and server key.</p>
        <span>Gateway defaults are managed from shared system configuration.</span>
      </div>
      <div className="gatewaySessionEmptyActions">
        <button
          className="primaryAction compact"
          onClick={onOpenGatewaySettings}
          type="button"
        >
          Gateway settings
        </button>
      </div>
    </section>
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
        {client.current_public_key_sha256_hex ? (
          <CopyableHash
            label="current key fingerprint"
            value={client.current_public_key_sha256_hex}
          />
        ) : (
          <span>none</span>
        )}
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
        <span>
          {client.latest_revocation_reason
            ? revocationReasonLabel(client.latest_revocation_reason)
            : "none"}
        </span>
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
        <CopyableHash
          label="revoked key fingerprint"
          value={revocation.public_key_sha256_hex}
        />
      </span>
      <span>
        <strong>Reason</strong>
        <span>{revocationReasonLabel(revocation.reason)}</span>
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
        <span>{gatewaySessionStateLabel(session.status)}</span>
      </span>
      <span>
        <strong>Remote IP</strong>
        <span>{session.remote_ip ?? "not reported"}</span>
      </span>
      <span>
        <strong>Version</strong>
        <span>{session.agent_version || "unknown"}</span>
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
        <span>
          {session.ended_at ? formatTime(session.ended_at) : "active"}
        </span>
      </span>
      <span>
        <strong>End reason</strong>
        <span>
          {session.end_reason ?? (session.ended_at ? "ended" : "active")}
        </span>
      </span>
      <span>
        <strong>Noise key</strong>
        <span className="monoValue">
          {session.noise_public_key_hex ?? "n/a"}
        </span>
      </span>
    </div>
  );
}

function CopyableHash({ label, value }: { label: string; value: string }) {
  function handleCopy(event: MouseEvent<HTMLButtonElement>) {
    event.stopPropagation();
    navigator.clipboard.writeText(value).catch(() => {});
  }

  return (
    <button
      aria-label={`Copy ${label}`}
      className="copyHashButton"
      onClick={handleCopy}
      title={value}
      type="button"
    >
      <span>{shortHash(value)}</span>
      <Copy size={13} />
    </button>
  );
}

function identityStatus(client: KeyLifecycleClientView): string {
  return client.current_key_revoked ? "blocked" : client.status;
}

function gatewaySessionStateLabel(status: string): string {
  if (status === "active") return "Active";
  if (status === "ended") return "Ended";
  if (status === "expired") return "Expired";
  return status
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function isOperatorSessionActive(session: OperatorSessionRecord): boolean {
  return !session.revoked && !isOperatorSessionExpired(session);
}

function isOperatorSessionExpired(session: OperatorSessionRecord): boolean {
  return (
    isPastTime(session.expires_at) || isPastTime(session.refresh_expires_at)
  );
}

function operatorSessionStateLabel(session: OperatorSessionRecord): string {
  if (session.revoked) {
    return "Revoked";
  }
  if (isPastTime(session.refresh_expires_at)) {
    return "Expired";
  }
  if (isPastTime(session.expires_at)) {
    return "Access expired";
  }
  return session.current ? "Current" : "Active";
}

function isPastTime(value: string): boolean {
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) && timestamp <= Date.now();
}

function revocationReasonLabel(value: string | null | undefined): string {
  if (!value?.trim()) {
    return "Operator request";
  }
  const normalized = value.trim().toLowerCase();
  if (normalized.includes("fixture") && normalized.includes("rebuild")) {
    return "Host rebuild";
  }
  return value
    .trim()
    .replace(/[_-]+/g, " ")
    .replace(/\s+/g, " ")
    .replace(/^./, (match) => match.toUpperCase());
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
  const endpoints = (preferences?.gateway_endpoints ?? "").trim();
  const gatewayKey = (preferences?.gateway_server_public_key_hex ?? "").trim();

  if (!endpoints || !gatewayKey) {
    return (
      <div className="formNote mutedNote">
        <span>
          Gateway endpoint or server key settings are not configured, so the
          panel cannot generate a complete install command yet.
        </span>
      </div>
    );
  }

  const command = [
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
