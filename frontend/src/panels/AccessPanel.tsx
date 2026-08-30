import {
  useEffect,
  useId,
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
  Plus,
  RefreshCw,
  Save,
  ShieldCheck,
  TerminalSquare,
  UsersRound,
  Trash2,
  Wifi,
  X,
} from "lucide-react";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../components/ActionFeedback";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { AdminRoleBoundary } from "../components/RoleBoundary";
import { TotpEnrollmentQr } from "../components/TotpEnrollmentQr";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { FLEET_DETAIL_LIMIT, formatLowerBoundCount } from "../constants";
import { SystemUsersPanel } from "./SystemPanel";
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
import { RELEASE_TAG } from "../buildInfo";
import { ACCESS_REVOKED_RECOVERY_DETAIL } from "../agentDisplayState";
import type {
  GatewaySessionRecord,
  LifecycleOutcomeRecord,
  OperatorAuthEventRecord,
  OperatorPreferences,
  OperatorView,
  OperatorSessionRecord,
  TotpSetupResponse,
} from "../types";
import type {
  AgentIdentityMutationResponse,
  AgentIdentityView,
  ClientKeyRevocationMutationResponse,
  ClientKeyRevocationView,
  KeyLifecycleClientView,
  KeyLifecycleReportView,
  UpsertAgentIdentityRequest,
} from "../typesAccess";
import type { TerminalSessionRecord } from "../typesTerminal";
import {
  agentIdentityPayloadHashHex,
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
  lifecycleOutcomeFailureReason,
  shortHash,
  statusClass,
} from "../utils";

const accessSubpages = [
  "Overview",
  "Operators",
  "VPS identities",
  "Gateway sessions",
  "Privilege vault",
] as const;

type AccessSubpage = (typeof accessSubpages)[number];
type AccessReleaseSubpage =
  | "overview"
  | "operators"
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
type AgentInstallMode = "root" | "user" | "staged";
type LocalActionFeedback = {
  message: string;
  tone: ActionFeedbackTone;
};

const AGENT_INSTALL_SCRIPT_URL =
  "https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh";
const DEFAULT_AGENT_INSTALL_RELEASE = RELEASE_TAG || "latest";
const INSTALLER_CURL_FLAGS = "-fL --retry 2 --connect-timeout 5 --max-time 30";

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

function lifecycleCompletionFeedback(
  outcomes: LifecycleOutcomeRecord[],
  successMessage: string,
): LocalActionFeedback {
  const failures = outcomes.filter((outcome) => outcome.status !== "completed");
  if (failures.length === 0) {
    return { message: successMessage, tone: "success" };
  }
  return {
    message: `${successMessage} ${failures
      .map((outcome) =>
        lifecycleOutcomeFailureReason(outcome, "Identity change"),
      )
      .join(" ")}`,
    tone: "warning",
  };
}

function clipboardFailureMessage(error: unknown): string {
  const detail =
    error instanceof Error && error.message.trim()
      ? ` Browser reported: ${error.message.trim()}.`
      : "";
  return `Clipboard copy failed.${detail} Allow clipboard access for this origin, or select and copy the value manually.`;
}

type AccessPanelProps = {
  activeSubpage: string;
  apiToken: string;
  error: string | null;
  gatewaySessions: GatewaySessionRecord[];
  initialIdentityWorkflow: "register" | null;
  lastLiveEvent: string;
  loading: boolean;
  onClearSession: () => void;
  onClearOperatorTotp: (
    operatorId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onConfirmTotp: (password: string, code: string) => Promise<void>;
  onCreateOperator: (
    username: string,
    role: string,
    password: string,
    scopes: string[],
    sessionRefreshTtlSecs: number,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onDisableTotp: (password: string, code: string) => Promise<void>;
  onInitialIdentityWorkflowConsumed: () => void;
  onOpenPrivilegeUnlock: () => void;
  onOpenSystemSessions: () => void;
  onOpenTerminalSessions: () => void;
  onRefresh: () => Promise<void>;
  onResetOperatorPassword: (
    operatorId: string,
    password: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onRevokeClientKey: (
    clientId: string,
    reason: string | null,
    confirmed: boolean,
    privilegeAssertion: PrivilegeAssertion | null,
  ) => Promise<ClientKeyRevocationMutationResponse>;
  onRevokeOperatorSession: (
    sessionId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onSetupTotp: (password: string) => Promise<TotpSetupResponse>;
  onSelectSubpage: (subpage: AccessReleaseSubpage) => void;
  onSetOperatorStatus: (
    operatorId: string,
    status: "active" | "disabled" | "deleted",
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onUpdateOperator: (
    operatorId: string,
    role: string,
    scopes: string[],
    sessionRefreshTtlSecs: number,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onUpdateOperatorPreferences: (
    preferences: OperatorPreferences,
  ) => Promise<void>;
  onUpsertAgentIdentity: (
    request: UpsertAgentIdentityRequest,
  ) => Promise<AgentIdentityMutationResponse>;
  operator: OperatorView | null;
  operatorAuthEvents: OperatorAuthEventRecord[];
  operatorSessions: OperatorSessionRecord[];
  operators: OperatorView[];
  clientKeyRevocations: ClientKeyRevocationView[];
  keyLifecycleReport: KeyLifecycleReportView | null;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => Promise<void>;
  terminalSessions: TerminalSessionRecord[];
  wsState: string;
};

function accessSubpageFromRoute(subpage: string): AccessSubpage {
  switch (subpage) {
    case "operators":
      return "Operators";
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
    case "Operators":
      return "operators";
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
    case "Operators":
      return {
        title: "Operators",
        description:
          "Human operator accounts, MFA posture, scopes, and session revocation",
      };
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
          "Live gateway connectivity evidence and reusable installer defaults",
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
  apiToken,
  error,
  gatewaySessions,
  initialIdentityWorkflow,
  lastLiveEvent,
  loading,
  onClearSession,
  onClearOperatorTotp,
  onConfirmTotp,
  onCreateOperator,
  onDisableTotp,
  onInitialIdentityWorkflowConsumed,
  onOpenPrivilegeUnlock,
  onOpenSystemSessions,
  onOpenTerminalSessions,
  onRefresh,
  onResetOperatorPassword,
  onRevokeClientKey,
  onRevokeOperatorSession,
  onSetupTotp,
  onSelectSubpage,
  onSetOperatorStatus,
  onUpdateOperator,
  onUpdateOperatorPreferences,
  onUpsertAgentIdentity,
  operator,
  operatorAuthEvents,
  operatorSessions,
  operators,
  clientKeyRevocations,
  keyLifecycleReport,
  privilegeMaterial,
  setPrivilegeMaterial,
  terminalSessions,
  wsState,
}: AccessPanelProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const identityClientIdHelpId = useId();
  const identityFormRef = useRef<HTMLFormElement | null>(null);
  const revokeFormRef = useRef<HTMLFormElement | null>(null);
  const identityWorkflowRef = useRef<HTMLElement | null>(null);
  const identityWorkflowIntentHandledRef = useRef(false);
  const totpFeedbackRef = useRef<HTMLDivElement | null>(null);
  const totpEnrollmentContextRef = useRef<string | null>(null);
  const totpRequestGenerationRef = useRef(0);
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
  const [generatedPublicKeyHex, setGeneratedPublicKeyHex] = useState<
    string | null
  >(null);
  const [privateKeyHex, setPrivateKeyHex] = useState<string | null>(null);
  const [createdIdentity, setCreatedIdentity] =
    useState<AgentIdentityView | null>(null);
  const [createdIdentityOperation, setCreatedIdentityOperation] = useState<
    "register" | "rotate" | null
  >(null);
  const [createdIdentityPrivateKeyHex, setCreatedIdentityPrivateKeyHex] =
    useState<string | null>(null);
  const [identityCompletion, setIdentityCompletion] =
    useState<LocalActionFeedback | null>(null);
  const [revokeClientId, setRevokeClientId] = useState("");
  const [revokeReason, setRevokeReason] = useState("");
  const [revokePending, setRevokePending] = useState(false);
  const [revokeReviewPending, setRevokeReviewPending] = useState(false);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const [revokeCompletion, setRevokeCompletion] =
    useState<LocalActionFeedback | null>(null);
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
  const currentBearerSession = operatorSessions.find(
    (session) => session.current,
  );
  useEffect(() => {
    const context = `${operator?.id ?? ""}:${currentBearerSession?.id ?? ""}`;
    if (totpEnrollmentContextRef.current === null) {
      totpEnrollmentContextRef.current = context;
      return;
    }
    if (totpEnrollmentContextRef.current === context) {
      return;
    }
    totpEnrollmentContextRef.current = context;
    totpRequestGenerationRef.current += 1;
    setTotpPassword("");
    setTotpCode("");
    setTotpSetup(null);
    setTotpError(null);
    setTotpPending(false);
  }, [currentBearerSession?.id, operator?.id]);
  const adminMfaRisk = operator?.role === "admin" && !operator.totp_enabled;
  const lifecycleClients = keyLifecycleReport?.clients ?? [];
  const nextIdentityClientId = keyLifecycleReport?.suggested_client_id ?? "";
  const lifecycleVpsOptions = useMemo(
    () =>
      lifecycleClients
        .filter((client) => !clientAccessRevoked(client))
        .map((client) => ({
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
  const gatewaySessionsTruncated = gatewaySessions.length >= FLEET_DETAIL_LIMIT;
  const operatorSessionsTruncated =
    operatorSessions.length >= FLEET_DETAIL_LIMIT;
  const operatorAuthEventsTruncated =
    operatorAuthEvents.length >= FLEET_DETAIL_LIMIT;
  const clientKeyRevocationsTruncated =
    clientKeyRevocations.length >= FLEET_DETAIL_LIMIT;
  const terminalSessionsTruncated =
    terminalSessions.length >= FLEET_DETAIL_LIMIT;
  const accessFeedbackMessage =
    error ?? (loading ? "Refreshing access records" : null);
  const accessFeedbackTone = error ? "danger" : "progress";
  const totpStateLabel = operator?.totp_enabled
    ? "enabled"
    : operator?.role === "admin"
      ? "admin MFA required"
      : "recommended account hardening";
  const totpFeedbackMessage =
    totpError ?? (totpPending ? "Updating TOTP" : null);
  const totpFeedbackTone = totpError ? "danger" : "progress";
  useEffect(() => {
    if (!totpFeedbackMessage) return;
    const frame = window.requestAnimationFrame(() => {
      if (totpFeedbackRef.current) {
        scrollIntoViewWithMotion(totpFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [totpFeedbackMessage]);
  const gatewayInstallDefaultsNeedReview =
    canManageOperators &&
    activeGatewaySessions === 0 &&
    !gatewaySessionsTruncated;
  const activeOperatorSessions = operatorSessions.filter(
    isOperatorSessionActive,
  ).length;
  const expiredOperatorSessions = operatorSessions.filter(
    (session) => !session.revoked && isOperatorSessionExpired(session),
  ).length;
  const revokedClientCount =
    lifecycleClients.filter(clientAccessRevoked).length;
  const blockedOrPendingClientCount = lifecycleClients.filter((client) =>
    clientAccessRevoked(client)
      ? true
      : ["blocked", "pending"].includes(client.status.trim().toLowerCase()),
  ).length;
  const currentBearerSessionState = currentBearerSession
    ? operatorSessionStateLabel(currentBearerSession)
    : "Not listed";
  const openTerminalSessions = terminalSessions.filter(isTerminalOpen).length;
  const replayableTerminalSessions = terminalSessions.filter(
    (session) => session.output_next_seq !== null,
  ).length;
  const consoleSessionActive = Boolean(operator || apiToken);
  const consoleSessionValue = operator
    ? `Active as ${operator.username}`
    : apiToken
      ? "Local token loaded"
      : "Not authenticated";
  const consoleSessionDetail = operator
    ? `This browser is operating as ${operator.username}; console stream ${wsState || "unknown"}. Bearer-session inventory is separate.`
    : apiToken
      ? `This browser has a local API token loaded; console stream ${wsState || "unknown"}. Bearer-session inventory is separate.`
      : "This browser has no loaded operator or local API token; bearer-session inventory cannot prove an active console.";
  const bearerSessionValue = canManageOperators
    ? operatorSessions.length === 0
      ? "0 listed"
      : `${formatLowerBoundCount(
          activeOperatorSessions,
          operatorSessionsTruncated,
        )} active / ${formatLowerBoundCount(
          expiredOperatorSessions,
          operatorSessionsTruncated,
        )} expired${operatorSessionsTruncated ? " in loaded sessions" : ""}`
    : "Admin only";
  const bearerSessionDetail = canManageOperators
    ? operatorSessions.length === 0
      ? "No API bearer-session inventory is listed for this operator account."
      : `${formatLowerBoundCount(activeOperatorSessions, operatorSessionsTruncated)} active API bearer session${activeOperatorSessions === 1 ? "" : "s"}${operatorSessionsTruncated ? " in loaded records" : ""} after expiry validation; current bearer record ${currentBearerSessionState}. Console, privilege, terminal, and gateway scopes are separate.`
    : "Bearer-session inventory includes other human operators and is intentionally visible only to admins. This does not mean that zero sessions exist.";
  const terminalSessionDetail =
    terminalSessions.length === 0
      ? "No terminal session records are loaded; terminal shells are managed in Remote and audited separately."
      : `${formatLowerBoundCount(replayableTerminalSessions, terminalSessionsTruncated)} replayable terminal session${replayableTerminalSessions === 1 ? "" : "s"}${terminalSessionsTruncated ? " in loaded records" : ""}; shell streams stay in Remote and audit evidence.`;
  const identityClientIdError = validateIdentityClientId(identityClientId);
  const identityDraftReady =
    canManageOperators &&
    !identityPending &&
    !identityReviewPending &&
    identityClientIdError === null &&
    isFixedHex32(identityPublicKeyHex);
  const revokeClient = lifecycleClients.find(
    (client) => client.client_id === revokeClientId.trim(),
  );
  const revokeTargetError = !revokeClientId.trim()
    ? "VPS ID is required"
    : !revokeClient
      ? "Choose a registered VPS identity"
      : clientAccessRevoked(revokeClient)
        ? `This VPS already has Access revoked. ${ACCESS_REVOKED_RECOVERY_DETAIL}`
        : null;
  const canRevokeClientKey =
    canManageOperators &&
    Boolean(privilegeMaterial) &&
    revokeTargetError === null &&
    !revokePending &&
    !revokeReviewPending;
  const revokeFeedbackMessage =
    revokeError ??
    (revokeReviewPending
      ? "Preparing key revoke review"
      : (revokeCompletion?.message ??
        (revokeClientId.trim() ? revokeTargetError : null)));
  const revokeFeedbackTone: ActionFeedbackTone = revokeError
    ? "danger"
    : revokeReviewPending
      ? "progress"
      : revokeTargetError && revokeClientId.trim()
        ? "warning"
        : (revokeCompletion?.tone ?? "progress");
  const identityFeedbackMessage =
    identityError ??
    (identityReviewPending
      ? "Preparing identity review"
      : identityCompletion?.message);
  const identityFeedbackTone: ActionFeedbackTone = identityError
    ? "danger"
    : identityReviewPending
      ? "progress"
      : (identityCompletion?.tone ?? "progress");
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
        resizeMinSize: 180,
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
        resizeMinSize: 180,
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

  useEffect(() => {
    if (initialIdentityWorkflow === null) {
      identityWorkflowIntentHandledRef.current = false;
      return;
    }
    if (identityWorkflowIntentHandledRef.current) {
      return;
    }
    identityWorkflowIntentHandledRef.current = true;
    prepareNewIdentity();
    onInitialIdentityWorkflowConsumed();
  }, [initialIdentityWorkflow, onInitialIdentityWorkflowConsumed]);

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
    setIdentityCompletion(null);
    setPendingConfirmation((current) =>
      current === "agent-identity" ? null : current,
    );
  }

  function clearRevokeReview() {
    invalidateReviewGeneration();
    setRevokeSnapshot(null);
    setRevokeReviewPending(false);
    setRevokeCompletion(null);
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
    setIdentityClientId(nextIdentityClientId);
    setIdentityPublicKeyHex("");
    setIdentityDisplayName("");
    setIdentityTags("");
    setGeneratedPublicKeyHex(null);
    setPrivateKeyHex(null);
    setCreatedIdentity(null);
    setCreatedIdentityOperation(null);
    setCreatedIdentityPrivateKeyHex(null);
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
    setGeneratedPublicKeyHex(null);
    setPrivateKeyHex(null);
    setCreatedIdentity(null);
    setCreatedIdentityOperation(null);
    setCreatedIdentityPrivateKeyHex(null);
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
    setRevokeCompletion(null);
    openAccessSubpage("VPS identities");
    scrollIdentityWorkflowSoon();
  }

  async function setupTotp() {
    if (!totpPassword) {
      return;
    }
    const generation = totpRequestGenerationRef.current + 1;
    totpRequestGenerationRef.current = generation;
    const operatorId = operator?.id ?? null;
    setTotpPending(true);
    setTotpError(null);
    setTotpCode("");
    setTotpSetup(null);
    try {
      const setup = await onSetupTotp(totpPassword);
      if (totpRequestGenerationRef.current !== generation) {
        return;
      }
      if (!operatorId || setup.operator_id !== operatorId) {
        throw new Error(
          "TOTP setup was returned for a different operator. Refresh the current session before retrying.",
        );
      }
      setTotpSetup(setup);
    } catch (actionError) {
      if (totpRequestGenerationRef.current !== generation) {
        return;
      }
      setTotpError(
        actionError instanceof Error
          ? actionError.message
          : "TOTP setup failed",
      );
    } finally {
      if (totpRequestGenerationRef.current === generation) {
        setTotpPending(false);
      }
    }
  }

  async function confirmTotp() {
    if (!totpPassword || !totpCode) {
      return;
    }
    const generation = totpRequestGenerationRef.current + 1;
    totpRequestGenerationRef.current = generation;
    setTotpPending(true);
    setTotpError(null);
    try {
      await onConfirmTotp(totpPassword, totpCode);
      if (totpRequestGenerationRef.current !== generation) {
        return;
      }
      setTotpPassword("");
      setTotpCode("");
      setTotpSetup(null);
    } catch (actionError) {
      if (totpRequestGenerationRef.current !== generation) {
        return;
      }
      setTotpError(
        actionError instanceof Error
          ? actionError.message
          : "TOTP confirmation failed",
      );
    } finally {
      if (totpRequestGenerationRef.current === generation) {
        setTotpPending(false);
      }
    }
  }

  async function disableTotp() {
    if (!totpPassword || !totpCode) {
      return;
    }
    const generation = totpRequestGenerationRef.current + 1;
    totpRequestGenerationRef.current = generation;
    setTotpPending(true);
    setTotpError(null);
    try {
      await onDisableTotp(totpPassword, totpCode);
      if (totpRequestGenerationRef.current !== generation) {
        return;
      }
      setTotpPassword("");
      setTotpCode("");
      setTotpSetup(null);
      setPendingConfirmation(null);
    } catch (actionError) {
      if (totpRequestGenerationRef.current !== generation) {
        return;
      }
      setTotpError(
        actionError instanceof Error
          ? actionError.message
          : "TOTP disable failed",
      );
    } finally {
      if (totpRequestGenerationRef.current === generation) {
        setTotpPending(false);
      }
    }
  }

  async function handleGenerateKeypair() {
    try {
      const keypair = await generateNoiseKeypair();
      setCreatedIdentity(null);
      setCreatedIdentityPrivateKeyHex(null);
      setIdentityPublicKeyHex(keypair.publicKeyHex);
      setGeneratedPublicKeyHex(keypair.publicKeyHex);
      setPrivateKeyHex(keypair.privateKeyHex);
      setIdentityError(null);
    } catch {
      setIdentityError(
        "Key generation failed. Web Crypto X25519 JWK export is unavailable.",
      );
    }
  }

  function beginAnotherIdentityRegistration() {
    prepareNewIdentity();
    window.requestAnimationFrame(() => {
      identityFormRef.current
        ?.querySelector<HTMLElement>("input, textarea, button")
        ?.focus({ preventScroll: true });
    });
  }

  async function handleCopyPrivateKey() {
    if (!privateKeyHex) {
      return;
    }
    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error("Clipboard API is unavailable");
      }
      await navigator.clipboard.writeText(privateKeyHex);
      setIdentityError(null);
      setIdentityCompletion({
        message: "Private key copied to the clipboard.",
        tone: "success",
      });
    } catch (error) {
      setIdentityError(clipboardFailureMessage(error));
    }
  }

  async function requestIdentityImport(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const clientId = identityClientId.trim();
    if (identityClientIdError) {
      setIdentityError(identityClientIdError);
      return;
    }
    if (!identityDraftReady) {
      setIdentityError("A valid client ID and 64-hex public key are required");
      return;
    }
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
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
      const payloadHash = await agentIdentityPayloadHashHex(snapshotInput);
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: isRotate ? "agent_identity.rotate" : "agent_identity.import",
          confirmed: true,
          resolvedTargets: [clientId],
          target: clientId,
          payloadHash,
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
      const boundPrivateKeyHex =
        generatedPublicKeyHex?.toLowerCase() === snapshot.publicKeyHex
          ? privateKeyHex
          : null;
      const response = await onUpsertAgentIdentity({
        client_id: snapshot.clientId,
        client_public_key_hex: snapshot.publicKeyHex,
        display_name: snapshot.displayName,
        tags: snapshot.tags,
        replace_existing_key: snapshot.replaceExistingKey,
        confirmed: true,
        privilege_assertion: snapshot.privilegeAssertion,
      });
      const completedOperation = snapshot.replaceExistingKey
        ? "rotate"
        : "register";
      setCreatedIdentity(response.identity);
      setCreatedIdentityOperation(completedOperation);
      setCreatedIdentityPrivateKeyHex(boundPrivateKeyHex);
      setIdentityCompletion(
        lifecycleCompletionFeedback(
          response.post_commit,
          snapshot.replaceExistingKey
            ? "VPS key rotated."
            : "VPS identity registered.",
        ),
      );
      setIdentityClientId("");
      setIdentityPublicKeyHex("");
      setIdentityDisplayName("");
      setIdentityTags("");
      setGeneratedPublicKeyHex(null);
      setPrivateKeyHex(null);
      setIdentityMode(completedOperation);
      setIdentitySnapshot(null);
      setPendingConfirmation(null);
      setIdentityWorkflow(completedOperation);
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
        !privilegeMaterial
          ? "Privilege vault unlock is required"
          : (revokeTargetError ?? "This VPS key cannot be revoked"),
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
      const response = await onRevokeClientKey(
        snapshot.clientId,
        snapshot.reason,
        true,
        snapshot.privilegeAssertion,
      );
      setRevokeCompletion(
        lifecycleCompletionFeedback(response.post_commit, "VPS key revoked."),
      );
      setRevokeClientId("");
      setRevokeReason("");
      setRevokeSnapshot(null);
      setPendingConfirmation(null);
      setIdentityWorkflow("revoke");
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
            "Admin MFA is recommended; this page cannot verify role-based enforcement.",
          icon: <ShieldCheck size={16} />,
          label: "Policy recommends MFA",
          onClick: () => openAccessSubpage("Privilege vault"),
          tone: "attention",
          value: "Recommended",
        }
      : null,
    canManageOperators && expiredOperatorSessions > 0
      ? {
          action: "Manage sessions",
          detail: `${formatLowerBoundCount(expiredOperatorSessions, operatorSessionsTruncated)} listed bearer session${expiredOperatorSessions === 1 ? "" : "s"}${operatorSessionsTruncated ? " in loaded records" : ""} expired and are excluded from active-session counts.`,
          icon: <Clock size={16} />,
          label: "Expired bearer sessions",
          onClick: onOpenSystemSessions,
          tone: "attention",
          value: `${formatLowerBoundCount(
            expiredOperatorSessions,
            operatorSessionsTruncated,
          )} expired${operatorSessionsTruncated ? " loaded" : ""}`,
        }
      : null,
    canManageOperators && blockedOrPendingClientCount > 0
      ? {
          action: "Open identities",
          detail:
            "Pending, blocked, or access-revoked VPS identities need operator review. Assign a new key to recover an access-revoked VPS ID.",
          icon: <Fingerprint size={16} />,
          label: "VPS identity attention",
          onClick: () => openAccessSubpage("VPS identities"),
          tone: "attention",
          value: `${blockedOrPendingClientCount} need review`,
        }
      : null,
    gatewayInstallDefaultsNeedReview
      ? {
          action: "Configure defaults",
          detail:
            "Gateway endpoints and the server public key are this operator's reusable agent-installer defaults.",
          icon: <Wifi size={16} />,
          label: "Gateway install defaults",
          onClick: () => openAccessSubpage("Gateway sessions"),
          tone: "attention",
          value: "Review",
        }
      : null,
  ];
  const accessRequiredActions = accessRequiredActionCandidates.filter(
    (item): item is AccessOverviewItem => Boolean(item),
  );

  const accessResponsibilityRows: AccessOverviewItem[] = [
    {
      action: "Open Operators",
      detail: canManageOperators
        ? `${operators.length} operator account${operators.length === 1 ? "" : "s"}; MFA is ${adminMfaRisk ? "recommended for this admin" : "covered by loaded policy evidence"}. Bearer sessions are listed under Session scopes.`
        : "Operator inventory and account governance are intentionally visible only to admins. No count is inferred from unavailable data.",
      icon: <UsersRound size={16} />,
      label: "Operators",
      onClick: () => openAccessSubpage("Operators"),
      tone: canManageOperators
        ? operators.length === 0 || adminMfaRisk
          ? "attention"
          : "ready"
        : "neutral",
      value: canManageOperators
        ? `${operators.length} operator${operators.length === 1 ? "" : "s"}`
        : "Admin only",
    },
    {
      action: "Open identities",
      detail: canManageOperators
        ? `${keyLifecycleReport?.revocation_count ?? formatLowerBoundCount(clientKeyRevocations.length, clientKeyRevocationsTruncated)} revocation records${!keyLifecycleReport && clientKeyRevocationsTruncated ? " loaded" : ""}; ${revokedClientCount} VPS identities have Access revoked.`
        : "VPS public-key registration, rotation, and revocation inventory are intentionally visible only to admins.",
      icon: <Fingerprint size={16} />,
      label: "VPS identities",
      onClick: () => openAccessSubpage("VPS identities"),
      tone: canManageOperators
        ? blockedOrPendingClientCount > 0
          ? "attention"
          : "ready"
        : "neutral",
      value: canManageOperators
        ? `${keyLifecycleReport?.direct_identity_client_count ?? lifecycleClients.length} registered`
        : "Admin only",
    },
  ];

  const accessSessionScopeRows: AccessOverviewItem[] = [
    {
      action: "Refresh access",
      detail: consoleSessionDetail,
      icon: <KeyRound size={16} />,
      label: "Console/browser session",
      onClick: () => void onRefresh(),
      tone: consoleSessionActive ? "ready" : "attention",
      value: consoleSessionValue,
    },
    {
      action: "Open evidence",
      detail: bearerSessionDetail,
      icon: <Clock size={16} />,
      label: "API bearer sessions",
      onClick: onOpenSystemSessions,
      tone: canManageOperators
        ? expiredOperatorSessions > 0 || operatorSessions.length === 0
          ? "attention"
          : operatorSessionsTruncated
            ? "neutral"
            : "ready"
        : "neutral",
      value: bearerSessionValue,
    },
    {
      action: privilegeMaterial ? "Open vault" : "Unlock",
      detail: privilegeMaterial
        ? "Privilege material is local-only and used for request-bound assertions in this browser."
        : "Privilege unlock is local; locking it does not revoke console, bearer, terminal, or gateway sessions.",
      icon: <LockKeyhole size={16} />,
      label: "Privilege unlock",
      onClick: () => openAccessSubpage("Privilege vault"),
      tone: privilegeMaterial
        ? "ready"
        : vaultAvailable
          ? "neutral"
          : "attention",
      value: privilegeMaterial ? "Unlocked" : vaultState,
    },
    {
      action: "Open terminal",
      detail: terminalSessionDetail,
      icon: <TerminalSquare size={16} />,
      label: "Terminal sessions",
      onClick: onOpenTerminalSessions,
      tone: openTerminalSessions > 0 ? "ready" : "neutral",
      value: `${formatLowerBoundCount(
        openTerminalSessions,
        terminalSessionsTruncated,
      )} open / ${formatLowerBoundCount(
        terminalSessions.length,
        terminalSessionsTruncated,
      )} recent${terminalSessionsTruncated ? " loaded" : ""}`,
    },
    {
      action: "Open sessions",
      detail:
        activeGatewaySessions > 0
          ? `${formatLowerBoundCount(gatewaySessions.length, gatewaySessionsTruncated)} recent${gatewaySessionsTruncated ? " loaded" : ""} gateway sessions; installer defaults are managed on the same page.`
          : gatewaySessionsTruncated
            ? "No active gateway session appears in the loaded history; more records may exist."
            : canManageOperators
              ? "No active gateway sessions. Configure reusable installer endpoints and the server public key on the Gateway sessions page."
              : "No active gateway sessions are visible. An admin manages installer defaults on the Gateway sessions page.",
      icon: <Wifi size={16} />,
      label: "Gateway sessions",
      onClick: () => openAccessSubpage("Gateway sessions"),
      tone:
        activeGatewaySessions > 0
          ? "ready"
          : gatewaySessionsTruncated
            ? "neutral"
            : "attention",
      value: `${formatLowerBoundCount(
        activeGatewaySessions,
        gatewaySessionsTruncated,
      )} active / ${formatLowerBoundCount(
        gatewaySessions.length,
        gatewaySessionsTruncated,
      )} recent${gatewaySessionsTruncated ? " loaded" : ""}`,
    },
  ];
  const activePanelHeader = accessPanelHeader(activeSubpage);

  return (
    <div className="workspace accessWorkspace">
      <section className="fleetPanel accessMain">
        <div className="sectionHeader compactSectionHeader">
          <div>
            <h2>{activePanelHeader.title}</h2>
            <span>{activePanelHeader.description}</span>
          </div>
          <div className="headerActionStack">
            <button
              className="secondaryAction"
              data-tooltip-disabled-reason="Access evidence is already refreshing."
              disabled={loading}
              onClick={() => void onRefresh()}
              type="button"
            >
              <RefreshCw size={17} />
              Refresh
            </button>
            <ActionFeedback
              message={accessFeedbackMessage}
              tone={accessFeedbackTone}
            />
          </div>
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

        {activeSubpage === "Operators" &&
          (canManageOperators ? (
            <SystemUsersPanel
              authEvents={operatorAuthEvents}
              authEventsTruncated={operatorAuthEventsTruncated}
              currentOperator={operator}
              onClearOperatorTotp={onClearOperatorTotp}
              onCreateOperator={onCreateOperator}
              onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
              onResetOperatorPassword={onResetOperatorPassword}
              onRevokeOperatorSession={onRevokeOperatorSession}
              onSetOperatorStatus={onSetOperatorStatus}
              onUpdateOperator={onUpdateOperator}
              operators={operators}
              privilegeMaterial={privilegeMaterial}
              sessions={operatorSessions}
              sessionsTruncated={operatorSessionsTruncated}
            />
          ) : (
            <div className="workspaceSection">
              <AdminRoleBoundary
                currentRole={operator?.role}
                detail="Operator inventory, MFA posture, role grants, and bearer-session governance are intentionally visible only to admins."
                title="Operator accounts"
              />
            </div>
          ))}

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
                      {canManageOperators
                        ? "Operators, sessions, identities, gateway, and privilege state have no visible critical warnings in the loaded evidence."
                        : "No action is required in the access evidence visible to this role. Admin-only inventories are not counted as empty or healthy."}
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
                  Operator accounts and VPS identity lifecycle; live authority
                  scopes are separated below.
                </span>
              </div>
              <div className="accessOverviewRows">
                {accessResponsibilityRows.map((item) => (
                  <AccessOverviewRow item={item} key={item.label} />
                ))}
              </div>
            </section>
            <section
              className="controlPanel accessOverviewPanel"
              aria-label="Access session scopes"
            >
              <div className="sectionHeader compact">
                <h2>Session scopes</h2>
                <span>
                  Browser console, API bearer, privilege unlock, terminal, and
                  gateway state are independent evidence domains.
                </span>
              </div>
              <div className="accessOverviewRows">
                {accessSessionScopeRows.map((item) => (
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
                labelPrefix="Access"
                lastPayloadHash={privilegeMaterial ? "unlocked" : null}
                lockPrivilegeLabel="Lock now"
                onOpenUnlock={onOpenPrivilegeUnlock}
                onPrivilegeMaterialChange={setPrivilegeMaterial}
                privilegeMaterial={privilegeMaterial}
                showHandoffState
                showVaultClear={false}
              />
            </section>
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>TOTP</h2>
                <span>{totpStateLabel}</span>
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
              <ActionFeedback
                className="localActionFeedback"
                message={totpFeedbackMessage}
                ref={totpFeedbackRef}
                tone={totpFeedbackTone}
              />
              {operator?.totp_enabled ? (
                <form
                  aria-label="Disable TOTP"
                  className="totpDisablePanel"
                  onSubmit={(event) => {
                    event.preventDefault();
                    if (!totpPending && totpPassword && totpCode) {
                      setPendingConfirmation("totp-disable");
                    }
                  }}
                >
                  <input
                    aria-hidden="true"
                    autoComplete="username"
                    className="srOnly"
                    name="username"
                    readOnly
                    tabIndex={-1}
                    value={operator?.username ?? ""}
                  />
                  <div>
                    <strong>TOTP is enabled</strong>
                    <span>
                      Disabling requires the current password and an
                      authenticator code.
                    </span>
                  </div>
                  <div className="totpActionGrid">
                    <label title="Enter the current account password to authorize disabling TOTP.">
                      <span>Current password</span>
                      <input
                        aria-label="TOTP password"
                        autoComplete="current-password"
                        data-tooltip-disabled-reason="Password entry is unavailable while a TOTP action is in progress."
                        data-tooltip-sensitive="true"
                        disabled={totpPending}
                        id="totp-disable-password"
                        name="totp_disable_password"
                        onChange={(event) => {
                          const nextPassword = event.target.value;
                          if (nextPassword !== totpPassword) {
                            totpRequestGenerationRef.current += 1;
                            setTotpError(null);
                          }
                          setTotpPassword(nextPassword);
                        }}
                        type="password"
                        value={totpPassword}
                      />
                    </label>
                    <label title="Enter the current authenticator code to authorize disabling TOTP.">
                      <span>Authenticator code</span>
                      <input
                        aria-label="TOTP code"
                        autoComplete="one-time-code"
                        data-tooltip-disabled-reason="Authenticator-code entry is unavailable while a TOTP action is in progress."
                        data-tooltip-sensitive="true"
                        disabled={totpPending}
                        id="totp-disable-code"
                        inputMode="numeric"
                        name="totp_disable_code"
                        onChange={(event) => {
                          const nextCode = event.target.value;
                          if (nextCode !== totpCode) {
                            totpRequestGenerationRef.current += 1;
                            setTotpError(null);
                          }
                          setTotpCode(nextCode);
                        }}
                        value={totpCode}
                      />
                    </label>
                    <button
                      className="secondaryAction dangerAction"
                      data-tooltip-disabled-reason={
                        totpPending
                          ? "TOTP disable review is unavailable while another TOTP action is in progress."
                          : !totpPassword
                            ? "Enter the current password before reviewing TOTP disable."
                            : "Enter the current authenticator code before reviewing TOTP disable."
                      }
                      disabled={totpPending || !totpPassword || !totpCode}
                      type="submit"
                    >
                      <Trash2 size={17} />
                      Review disable
                    </button>
                  </div>
                </form>
              ) : (
                <form
                  aria-label="TOTP enrollment sequence"
                  className="totpWorkflow"
                  onSubmit={(event) => {
                    event.preventDefault();
                    if (totpPending || !totpPassword) {
                      return;
                    }
                    if (totpSetup) {
                      if (totpCode) {
                        void confirmTotp();
                      }
                      return;
                    }
                    void setupTotp();
                  }}
                >
                  <input
                    aria-hidden="true"
                    autoComplete="username"
                    className="srOnly"
                    name="username"
                    readOnly
                    tabIndex={-1}
                    value={operator?.username ?? ""}
                  />
                  <ol className="totpStepList">
                    <li className={totpPassword ? "ready" : "active"}>
                      <span>1</span>
                      <strong title="Enter the current account password">
                        Password
                      </strong>
                    </li>
                    <li className={totpSetup ? "ready" : ""}>
                      <span>2</span>
                      <strong title="Scan the QR code or enter the setup key">
                        QR / key
                      </strong>
                    </li>
                    <li className={totpCode ? "ready" : ""}>
                      <span>3</span>
                      <strong title="Enter the authenticator code">Code</strong>
                    </li>
                    <li>
                      <span>4</span>
                      <strong title="Finish TOTP setup">Complete</strong>
                    </li>
                  </ol>
                  <div className="totpActionGrid">
                    <label title="Enter the current account password to generate TOTP setup material.">
                      <span>Current password</span>
                      <input
                        aria-label="TOTP password"
                        autoComplete="current-password"
                        data-tooltip-disabled-reason="Password entry is unavailable while a TOTP action is in progress."
                        data-tooltip-sensitive="true"
                        disabled={totpPending}
                        id="totp-setup-password"
                        name="totp_setup_password"
                        onChange={(event) => {
                          const nextPassword = event.target.value;
                          if (nextPassword !== totpPassword) {
                            totpRequestGenerationRef.current += 1;
                            setTotpPending(false);
                            setTotpCode("");
                            setTotpSetup(null);
                            setTotpError(null);
                          }
                          setTotpPassword(nextPassword);
                        }}
                        type="password"
                        value={totpPassword}
                      />
                    </label>
                    {!totpSetup && (
                      <button
                        className="secondaryAction"
                        data-tooltip-disabled-reason={
                          totpPending
                            ? "TOTP setup is already in progress."
                            : "Enter the current password before generating TOTP setup material."
                        }
                        disabled={totpPending || !totpPassword}
                        type="submit"
                      >
                        <ShieldCheck size={17} />
                        Set up TOTP
                      </button>
                    )}
                    {totpSetup ? (
                      <TotpEnrollmentQr setup={totpSetup} />
                    ) : (
                      <div className="totpSecretPanel muted">
                        <strong>Authenticator QR code</strong>
                        <span>
                          Enter your current password, then choose Set up TOTP
                          to generate a scannable account.
                        </span>
                      </div>
                    )}
                    <label title="Enter the authenticator code from the newly enrolled TOTP account.">
                      <span>Authenticator code</span>
                      <input
                        aria-label="TOTP code"
                        autoComplete="one-time-code"
                        data-tooltip-disabled-reason={
                          totpPending
                            ? "Authenticator-code entry is unavailable while a TOTP action is in progress."
                            : "Generate and scan the TOTP setup material before entering a code."
                        }
                        data-tooltip-sensitive="true"
                        disabled={totpPending || !totpSetup}
                        id="totp-setup-code"
                        inputMode="numeric"
                        name="totp_setup_code"
                        onChange={(event) => {
                          const nextCode = event.target.value;
                          if (nextCode !== totpCode) {
                            totpRequestGenerationRef.current += 1;
                            setTotpError(null);
                          }
                          setTotpCode(nextCode);
                        }}
                        value={totpCode}
                      />
                    </label>
                    <button
                      className="primaryAction"
                      data-tooltip-disabled-reason={
                        totpPending
                          ? "TOTP setup is already in progress."
                          : !totpPassword
                            ? "Enter the current password before completing TOTP setup."
                            : !totpSetup
                              ? "Generate and scan the TOTP setup material before completing setup."
                              : "Enter the current authenticator code before completing TOTP setup."
                      }
                      disabled={totpPending || !totpPassword || !totpCode}
                      type={totpSetup ? "submit" : "button"}
                    >
                      <Save size={17} />
                      Complete setup
                    </button>
                  </div>
                </form>
              )}
              {!operator?.totp_enabled && (
                <div className="totpDisablePanel disabled">
                  <div>
                    <strong>Disable TOTP</strong>
                    <span>
                      No active TOTP factor is recorded for this account.
                    </span>
                  </div>
                  <button
                    className="secondaryAction dangerAction"
                    data-tooltip-disabled-reason="TOTP cannot be disabled because this account has no active TOTP factor."
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

        {activeSubpage === "VPS identities" && canManageOperators && (
          <div className="workspaceSection accessTableStack">
            <section className="controlPanel">
              <div className="sectionHeader compact">
                <h2>VPS identities</h2>
                <span>
                  {keyLifecycleReport?.direct_identity_client_count ??
                    lifecycleClients.length}{" "}
                  registered; {blockedOrPendingClientCount} need review. Select
                  identities, then use Actions to rotate or revoke keys.
                </span>
              </div>
              <ConsoleDataGrid
                actions={[
                  {
                    label: "Rotate",
                    description: (rows) =>
                      rows.length === 1
                        ? `Prefill key rotation for ${rows[0].display_name}.`
                        : "Select exactly one VPS identity to rotate.",
                    disabled: (rows) => rows.length !== 1,
                    icon: <KeyRound size={14} />,
                    onSelect: (rows) => prepareIdentityRotation(rows[0]),
                  },
                  {
                    label: "Revoke",
                    description: (rows) =>
                      rows.length === 1
                        ? rows[0].current_key_revoked ||
                          rows[0].status.trim().toLowerCase() === "revoked"
                          ? `${rows[0].display_name} already has Access revoked; assign a new key to recover this VPS ID.`
                          : `Prefill current key revocation for ${rows[0].display_name}.`
                        : "Select exactly one VPS identity to revoke.",
                    disabled: (rows) =>
                      rows.length !== 1 || clientAccessRevoked(rows[0]),
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
                <span>
                  {formatLowerBoundCount(
                    clientKeyRevocations.length,
                    clientKeyRevocationsTruncated,
                  )}{" "}
                  {clientKeyRevocationsTruncated ? "loaded " : ""}retained
                  records
                </span>
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
                rowsTruncated={clientKeyRevocationsTruncated}
                searchPlaceholder="Search VPS, key hash, reason, or operator"
                selectable={false}
                singleExpandedRow
                storageKey="vpsman.access.revocations"
                title="Client key revocations"
              />
            </section>
          </div>
        )}

        {activeSubpage === "VPS identities" && !canManageOperators && (
          <div className="workspaceSection">
            <AdminRoleBoundary
              currentRole={operator?.role}
              detail="VPS key registration, rotation, revocation, and private install-command handoff are intentionally visible only to admins."
              title="VPS identities"
            />
          </div>
        )}

        {activeSubpage === "Gateway sessions" && (
          <div className="workspaceSection accessTableStack">
            {canManageOperators ? (
              <InstallCommand
                onUpdateOperatorPreferences={onUpdateOperatorPreferences}
                operatorPreferences={operator?.preferences ?? null}
              />
            ) : null}
            {gatewaySessions.length === 0 ? (
              <GatewaySessionEmptyState canConfigure={canManageOperators} />
            ) : (
              <section className="controlPanel">
                <div className="sectionHeader compact">
                  <h2>Gateway sessions</h2>
                  <span>
                    {formatLowerBoundCount(
                      activeGatewaySessions,
                      gatewaySessionsTruncated,
                    )}{" "}
                    active /{" "}
                    {formatLowerBoundCount(
                      gatewaySessions.length,
                      gatewaySessionsTruncated,
                    )}{" "}
                    recent{gatewaySessionsTruncated ? " loaded" : ""}
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
                  rowsTruncated={gatewaySessionsTruncated}
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
          activeSubpage === "Operators" ||
          activeSubpage === "Gateway sessions" ||
          activeSubpage === "Privilege vault" ||
          !canManageOperators ||
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
              title="Close VPS identity workflow"
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
          <h2>
            {createdIdentity
              ? createdIdentityOperation === "rotate"
                ? "VPS key rotated"
                : "VPS registered"
              : identityMode === "rotate"
                ? "Rotate key"
                : "Register VPS"}
          </h2>
          <span>
            {createdIdentity
              ? createdIdentityOperation === "rotate"
                ? "The new public key is saved; review gateway disconnect status below"
                : createdIdentityPrivateKeyHex
                  ? "Copy this VPS install command before starting another registration"
                  : "Registration is complete; use the matching private key from your secure source"
              : identityMode === "rotate"
                ? "Replace the selected VPS public key"
                : "Generate a keypair or import a public key"}
          </span>
        </div>
        {activeSubpage === "VPS identities" &&
          identityWorkflow !== null &&
          identityWorkflow !== "revoke" && (
            <ActionFeedback
              className="localActionFeedback identityActionFeedback"
              message={identityFeedbackMessage}
              tone={identityFeedbackTone}
            />
          )}
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
          {!createdIdentity && (
            <>
              <div className="formNote identityFormGuide">
                <strong>
                  {identityMode === "rotate"
                    ? "Rotation keeps the VPS identity and replaces only the key."
                    : "Register a VPS identity before installing or reconnecting the agent."}
                </strong>
                <span>
                  Generate a keypair for a new install, or paste a pre-generated
                  agent public key. Private key material is shown once and is
                  never saved by the panel.
                </span>
              </div>
              <label>
                <span>VPS client ID</span>
                <input
                  aria-describedby={identityClientIdHelpId}
                  aria-errormessage={
                    identityClientIdError ? identityClientIdHelpId : undefined
                  }
                  aria-invalid={Boolean(identityClientIdError)}
                  aria-label="Agent identity client ID"
                  data-tooltip-disabled-reason={
                    !canManageOperators
                      ? "VPS identities can be changed only by an admin operator."
                      : "The VPS client ID is unavailable while the identity action is in progress."
                  }
                  disabled={!canManageOperators || identityPending}
                  id="agent-identity-client-id"
                  name="agent_identity_client_id"
                  readOnly={identityMode === "rotate"}
                  onChange={(event) => {
                    setIdentityClientId(event.target.value);
                    clearIdentityReview();
                  }}
                  placeholder={
                    identityMode === "rotate"
                      ? "existing VPS ID"
                      : nextIdentityClientId
                  }
                  value={identityClientId}
                />
                <small className="fieldHelp" id={identityClientIdHelpId}>
                  {identityClientIdError ??
                    (identityMode === "rotate"
                      ? "Use the existing VPS ID. Only the current public key is replaced."
                      : `Defaults to the next numbered VPS ID (${nextIdentityClientId}). Editable for imported or existing string IDs.`)}
                </small>
              </label>
              <label className="wideField">
                <span>Noise public key</span>
                <textarea
                  aria-label="Agent identity public key hex"
                  data-tooltip-disabled-reason={
                    !canManageOperators
                      ? "VPS identities can be changed only by an admin operator."
                      : "The Noise public key is unavailable while the identity action is in progress."
                  }
                  disabled={!canManageOperators || identityPending}
                  id="agent-identity-public-key"
                  name="agent_identity_public_key"
                  onChange={(event) => {
                    const value = event.target.value;
                    setIdentityPublicKeyHex(value);
                    if (
                      generatedPublicKeyHex &&
                      value.trim().toLowerCase() !==
                        generatedPublicKeyHex.toLowerCase()
                    ) {
                      setGeneratedPublicKeyHex(null);
                      setPrivateKeyHex(null);
                    }
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
                  data-tooltip-disabled-reason={
                    !canManageOperators
                      ? "VPS keypairs can be generated only by an admin operator."
                      : "A keypair cannot be generated while the identity action is in progress."
                  }
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
                <div
                  className="inlineSecret"
                  data-tooltip-sensitive="true"
                  title="Private key generated for this VPS identity and displayed only once."
                >
                  <strong>Private key - shown once</strong>
                  <div className="secretRow">
                    <input
                      aria-label="Agent identity private key"
                      className="monospace"
                      data-tooltip-sensitive="true"
                      readOnly
                      value={privateKeyHex}
                    />
                    <button
                      className="secondaryAction compact"
                      onClick={() => void handleCopyPrivateKey()}
                      title="Copy the one-time private key to the clipboard."
                      type="button"
                    >
                      <Copy size={15} />
                      Copy
                    </button>
                  </div>
                  <small>
                    Store this key securely. It is not saved by the panel and
                    cannot be recovered.
                  </small>
                </div>
              )}
              <label>
                <span>Display name</span>
                <input
                  aria-label="Agent identity display name"
                  data-tooltip-disabled-reason={
                    identityMode === "rotate"
                      ? "Display name is unchanged during key rotation."
                      : !canManageOperators
                        ? "VPS identities can be changed only by an admin operator."
                        : "Display name is unavailable while the identity action is in progress."
                  }
                  disabled={
                    !canManageOperators ||
                    identityPending ||
                    identityMode === "rotate"
                  }
                  id="agent-identity-display-name"
                  name="agent_identity_display_name"
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
                  data-tooltip-disabled-reason={
                    identityMode === "rotate"
                      ? "Tags are unchanged during key rotation."
                      : !canManageOperators
                        ? "VPS identities can be changed only by an admin operator."
                        : "Tags are unavailable while the identity action is in progress."
                  }
                  disabled={
                    !canManageOperators ||
                    identityPending ||
                    identityMode === "rotate"
                  }
                  id="agent-identity-tags"
                  name="agent_identity_tags"
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
                data-tooltip-disabled-reason={
                  !canManageOperators
                    ? "VPS identities can be changed only by an admin operator."
                    : identityPending || identityReviewPending
                      ? "An identity action is already in progress."
                      : (identityClientIdError ??
                        "Enter a valid 64-character Noise public key before reviewing this identity.")
                }
                disabled={!identityDraftReady}
                title={
                  identityDraftReady && !privilegeMaterial
                    ? "Unlock local privilege; this registration draft will remain intact."
                    : undefined
                }
                type="submit"
              >
                <Fingerprint size={17} />
                {identityReviewPending
                  ? "Preparing review"
                  : !privilegeMaterial
                    ? "Unlock to review"
                    : identityMode === "rotate"
                      ? "Review rotation"
                      : "Review registration"}
              </button>
            </>
          )}
          {createdIdentity && (
            <div
              className="formNote identityRegistrationComplete"
              role="status"
            >
              <strong>
                {createdIdentity.display_name}{" "}
                {createdIdentityOperation === "rotate"
                  ? "key is rotated"
                  : "is registered"}
              </strong>
              <span
                title={`${createdIdentity.client_id} / ${createdIdentity.current_public_key_sha256_hex}`}
              >
                {createdIdentity.client_id} /{" "}
                {shortHash(createdIdentity.current_public_key_sha256_hex)}
              </span>
            </div>
          )}
          {createdIdentity && createdIdentityPrivateKeyHex && (
            <InstallCommand
              clientId={createdIdentity.client_id}
              onUpdateOperatorPreferences={onUpdateOperatorPreferences}
              operatorPreferences={operator?.preferences ?? null}
              privateKeyHex={createdIdentityPrivateKeyHex}
            />
          )}
          {createdIdentity && createdIdentityOperation === "register" && (
            <button
              className="secondaryAction"
              onClick={beginAnotherIdentityRegistration}
              title="Clear this one-time install result and start a separate VPS registration."
              type="button"
            >
              <Plus size={17} />
              Register another VPS
            </button>
          )}
        </form>

        <div
          className="sectionHeader compact"
          hidden={
            activeSubpage !== "VPS identities" || identityWorkflow !== "revoke"
          }
        >
          <h2>Revoke VPS key</h2>
          <span>Block the current VPS gateway key</span>
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
              id="vps-identity-revoke-reason"
              name="vps_identity_revoke_reason"
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
            data-tooltip-disabled-reason={
              !canManageOperators
                ? "VPS identities can be revoked only by an admin operator."
                : !privilegeMaterial
                  ? "Unlock local privilege before revoking a VPS identity key."
                  : revokePending || revokeReviewPending
                    ? "A VPS identity revoke action is already in progress."
                    : (revokeTargetError ??
                      "Choose a registered VPS identity before revoking its key.")
            }
            disabled={!canRevokeClientKey}
            type="submit"
          >
            <Ban size={17} />
            Revoke current key
          </button>
          <ActionFeedback
            className="localActionFeedback accessRevokeActionFeedback"
            message={revokeFeedbackMessage}
            tone={revokeFeedbackTone}
          />
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
            label: "Display name",
            value: identitySnapshot?.replaceExistingKey
              ? "unchanged"
              : (identitySnapshot?.displayName ??
                identitySnapshot?.clientId ??
                ""),
          },
          {
            label: "Tags",
            title: identitySnapshot?.tags.join(", "),
            value: identitySnapshot?.replaceExistingKey
              ? "unchanged"
              : identitySnapshot?.tags.length
                ? identitySnapshot.tags.join(", ")
                : "none",
          },
          {
            label: "Mode",
            value: identitySnapshot?.replaceExistingKey
              ? "key rotation"
              : "new registration",
          },
        ]}
        error={identityError ?? undefined}
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
        detail="The current key is permanently revoked, the VPS remains visible as Access revoked, its live gateway session is disconnected, and active work is stopped. Recover this VPS ID by assigning it a new key; deletion alone permanently retires the VPS identity."
        error={revokeError ?? undefined}
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
        error={totpError ?? undefined}
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

function GatewaySessionEmptyState({ canConfigure }: { canConfigure: boolean }) {
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
        <p>
          {canConfigure
            ? "No active gateway sessions. Configure the gateway endpoint and server key."
            : "No active gateway sessions are visible."}
        </p>
        <span>
          {canConfigure
            ? "Installer defaults can be edited above; a session appears here after an agent connects."
            : "An admin manages installer defaults on this page."}
        </span>
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
        <span
          className="monoValue"
          data-tooltip-empty-reason="The gateway session did not report a Noise public key."
        >
          {session.noise_public_key_hex ?? "-"}
        </span>
      </span>
    </div>
  );
}

function CopyableHash({ label, value }: { label: string; value: string }) {
  const [copyState, setCopyState] = useState<{
    message: string;
    status: "copied" | "failed";
  } | null>(null);

  async function handleCopy(event: MouseEvent<HTMLButtonElement>) {
    event.stopPropagation();
    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error("Clipboard API is unavailable");
      }
      await navigator.clipboard.writeText(value);
      setCopyState({ message: `${label} copied.`, status: "copied" });
    } catch (error) {
      setCopyState({
        message: clipboardFailureMessage(error),
        status: "failed",
      });
    }
  }

  return (
    <button
      aria-label={`Copy ${label}`}
      className="copyHashButton"
      onClick={(event) => void handleCopy(event)}
      title={copyState ? `${copyState.message} Full value: ${value}` : value}
      type="button"
    >
      <span>
        {copyState?.status === "copied"
          ? "Copied"
          : copyState?.status === "failed"
            ? "Copy failed"
            : shortHash(value)}
      </span>
      <Copy size={13} />
    </button>
  );
}

function identityStatus(client: KeyLifecycleClientView): string {
  if (clientAccessRevoked(client)) {
    return "Access revoked";
  }
  if (client.status === "online") {
    return "Identity active";
  }
  return client.status;
}

function clientAccessRevoked(client: KeyLifecycleClientView): boolean {
  return (
    client.current_key_revoked ||
    client.status.trim().toLowerCase() === "revoked"
  );
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

function isTerminalOpen(session: TerminalSessionRecord): boolean {
  return session.state === "opening" || session.state === "open";
}

function isOperatorSessionExpired(session: OperatorSessionRecord): boolean {
  return isPastTime(session.refresh_expires_at);
}

function operatorSessionStateLabel(session: OperatorSessionRecord): string {
  if (session.revoked) {
    return "Revoked";
  }
  if (isPastTime(session.refresh_expires_at)) {
    return "Expired";
  }
  if (isPastTime(session.expires_at)) {
    return "Refresh available";
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

function validateIdentityClientId(value: string): string | null {
  const clientId = value.trim();
  if (!clientId) {
    return "Client ID is required.";
  }
  if (clientId.length > 120) {
    return "Client ID must not exceed 120 ASCII characters.";
  }
  if (!/^[A-Za-z0-9._:-]+$/.test(clientId)) {
    return "Client ID may use only letters, numbers, dot, underscore, colon, and hyphen.";
  }
  return null;
}

function gatewayEndpointsValidationError(value: string): string | null {
  const entries = value
    .split(/[\n,]/)
    .map((entry) => (entry.endsWith("\r") ? entry.slice(0, -1) : entry))
    .filter((entry) => !/^\s*$/.test(entry));
  if (entries.length === 0) {
    return "Gateway endpoints are required before copying the command.";
  }
  if (entries.length > 16) {
    return "Gateway endpoints support at most 16 entries.";
  }

  for (const [index, entry] of entries.entries()) {
    const fields = entry.split("=");
    const entryLabel = `Gateway endpoint ${index + 1}`;
    if (fields.length !== 3 || !fields[0] || !fields[1] || !fields[2]) {
      return `${entryLabel} must use label=host:port=priority.`;
    }
    const [label, tcpAddress, priority] = fields;
    if (label.length > 64 || !/^[A-Za-z0-9._:-]+$/.test(label)) {
      return `${entryLabel} label may use only letters, numbers, dot, underscore, colon, and hyphen.`;
    }
    if (
      tcpAddress.length > 256 ||
      tcpAddress.includes("\0") ||
      /\s/.test(tcpAddress)
    ) {
      return `${entryLabel} host and port cannot contain whitespace.`;
    }
    const port = gatewayEndpointPort(tcpAddress);
    if (port === null) {
      return `${entryLabel} must use a valid IPv4 address, DNS hostname, or bracketed IPv6 literal with a numeric port from 1 to 65535.`;
    }
    const parsedPriority = Number(priority);
    if (
      !/^[0-9]+$/.test(priority) ||
      priority.length > 5 ||
      !Number.isSafeInteger(parsedPriority) ||
      parsedPriority > 65_535
    ) {
      return `${entryLabel} priority must be an integer from 0 to 65535.`;
    }
  }
  return null;
}

function gatewayEndpointPort(tcpAddress: string): number | null {
  let host = "";
  let portText = "";
  if (tcpAddress.startsWith("[")) {
    const closingBracket = tcpAddress.lastIndexOf("]:");
    if (closingBracket <= 1) {
      return null;
    }
    host = tcpAddress.slice(1, closingBracket);
    portText = tcpAddress.slice(closingBracket + 2);
    if (!isValidIpv6Literal(host)) {
      return null;
    }
  } else {
    const separator = tcpAddress.lastIndexOf(":");
    if (separator <= 0) {
      return null;
    }
    host = tcpAddress.slice(0, separator);
    portText = tcpAddress.slice(separator + 1);
    if (
      host.includes(":") ||
      (/^[0-9.]+$/.test(host)
        ? !isValidIpv4Literal(host)
        : !isValidHostname(host))
    ) {
      return null;
    }
  }
  const port = Number(portText);
  return /^[0-9]+$/.test(portText) &&
    portText.length <= 5 &&
    port >= 1 &&
    port <= 65_535
    ? port
    : null;
}

function isValidIpv4Literal(value: string): boolean {
  const octets = value.split(".");
  return (
    octets.length === 4 &&
    octets.every(
      (octet) =>
        /^[0-9]{1,3}$/.test(octet) &&
        (octet.length === 1 || !octet.startsWith("0")) &&
        Number(octet) <= 255,
    )
  );
}

function isValidIpv6Literal(value: string): boolean {
  if (!value.includes(":")) {
    return false;
  }
  let normalized = value;
  if (normalized.includes(".")) {
    const lastColon = normalized.lastIndexOf(":");
    const ipv4Tail = normalized.slice(lastColon + 1);
    if (lastColon < 0 || !isValidIpv4Literal(ipv4Tail)) {
      return false;
    }
    normalized = `${normalized.slice(0, lastColon)}:0:0`;
  }
  if (!/^[0-9A-Fa-f:]+$/.test(normalized)) {
    return false;
  }
  const compression = normalized.indexOf("::");
  if (compression >= 0) {
    if (normalized.indexOf("::", compression + 2) >= 0) {
      return false;
    }
    const left = normalized.slice(0, compression);
    const right = normalized.slice(compression + 2);
    if (left.endsWith(":") || right.startsWith(":")) {
      return false;
    }
    const groups = [
      ...(left ? left.split(":") : []),
      ...(right ? right.split(":") : []),
    ];
    return (
      groups.length < 8 &&
      groups.every((group) => /^[0-9A-Fa-f]{1,4}$/.test(group))
    );
  }
  if (normalized.startsWith(":") || normalized.endsWith(":")) {
    return false;
  }
  const groups = normalized.split(":");
  return (
    groups.length === 8 &&
    groups.every((group) => /^[0-9A-Fa-f]{1,4}$/.test(group))
  );
}

function isValidHostname(value: string): boolean {
  if (value.length > 253) {
    return false;
  }
  const normalized = value.endsWith(".") ? value.slice(0, -1) : value;
  if (
    !normalized ||
    normalized.startsWith(".") ||
    normalized.endsWith(".") ||
    normalized.includes("..")
  ) {
    return false;
  }
  return normalized
    .split(".")
    .every(
      (label) =>
        label.length <= 63 &&
        /^[0-9A-Za-z](?:[0-9A-Za-z-]*[0-9A-Za-z])?$/.test(label),
    );
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
  onUpdateOperatorPreferences,
  operatorPreferences,
  privateKeyHex,
}: {
  clientId?: string;
  onUpdateOperatorPreferences: (
    preferences: OperatorPreferences,
  ) => Promise<void>;
  operatorPreferences: OperatorPreferences | null;
  privateKeyHex?: string;
}) {
  const installControlId = useId().replace(/:/g, "");
  const gatewayKeyInputId = `${installControlId}-gateway-key`;
  const gatewayKeyErrorId = `${gatewayKeyInputId}-error`;
  const gatewayEndpointsInputId = `${installControlId}-gateway-endpoints`;
  const gatewayEndpointsErrorId = `${gatewayEndpointsInputId}-error`;
  const installModeInputId = `${installControlId}-install-mode`;
  const [installMode, setInstallMode] = useState<AgentInstallMode>(
    () => operatorPreferences?.agent_install_mode ?? "root",
  );
  const [gatewayServerPublicKeyHex, setGatewayServerPublicKeyHex] = useState(
    () => operatorPreferences?.gateway_server_public_key_hex ?? "",
  );
  const [gatewayEndpoints, setGatewayEndpoints] = useState(
    () => operatorPreferences?.gateway_endpoints ?? "",
  );
  const [savePending, setSavePending] = useState(false);
  const [installFeedback, setInstallFeedback] =
    useState<LocalActionFeedback | null>(null);
  const savedGatewayServerPublicKeyHex =
    operatorPreferences?.gateway_server_public_key_hex ?? "";
  const savedGatewayEndpoints = operatorPreferences?.gateway_endpoints ?? "";
  const savedInstallMode = operatorPreferences?.agent_install_mode ?? "root";
  const normalizedGatewayServerPublicKeyHex = gatewayServerPublicKeyHex.trim();
  const normalizedGatewayEndpoints =
    normalizeGatewayEndpointList(gatewayEndpoints);
  const hasInstallIdentity = Boolean(clientId && privateKeyHex);
  const gatewayKeyValid = isFixedHex32(normalizedGatewayServerPublicKeyHex);
  const gatewayEndpointsError = gatewayEndpointsValidationError(
    normalizedGatewayEndpoints,
  );
  const gatewayEndpointsValid = gatewayEndpointsError === null;
  const gatewayDefaultsEmpty =
    !normalizedGatewayServerPublicKeyHex && !normalizedGatewayEndpoints;
  const gatewayDefaultsValid =
    gatewayDefaultsEmpty || (gatewayKeyValid && gatewayEndpointsValid);
  const showGatewayValidation =
    hasInstallIdentity ||
    Boolean(normalizedGatewayServerPublicKeyHex || normalizedGatewayEndpoints);
  const gatewayKeyInvalid = showGatewayValidation && !gatewayKeyValid;
  const gatewayEndpointsInvalid =
    showGatewayValidation && !gatewayEndpointsValid;
  const canBuildCommand =
    hasInstallIdentity && gatewayKeyValid && gatewayEndpointsValid;
  const gatewayValidationDescription =
    [
      gatewayKeyInvalid ? gatewayKeyErrorId : null,
      gatewayEndpointsInvalid ? gatewayEndpointsErrorId : null,
    ]
      .filter(Boolean)
      .join(" ") || undefined;
  const installValidationDescription = hasInstallIdentity
    ? gatewayValidationDescription
    : undefined;
  const gatewayDefaultsDirty =
    normalizedGatewayServerPublicKeyHex !==
      savedGatewayServerPublicKeyHex.trim() ||
    normalizedGatewayEndpoints !==
      normalizeGatewayEndpointList(savedGatewayEndpoints) ||
    installMode !== savedInstallMode;
  const canSaveGatewayDefaults =
    operatorPreferences !== null &&
    gatewayDefaultsDirty &&
    gatewayDefaultsValid &&
    !savePending;
  const saveGatewayDefaultsLabel = savePending
    ? "Saving"
    : operatorPreferences === null
      ? "Defaults unavailable"
      : gatewayDefaultsDirty
        ? "Save defaults"
        : "Defaults saved";
  const saveGatewayDefaultsTitle = gatewayDefaultsDirty
    ? "Save the reusable gateway key, endpoints, and install mode for this operator."
    : "The gateway key, endpoints, and install mode already match the saved defaults.";
  const installCommand = canBuildCommand
    ? buildAgentInstallCommand({
        clientId: clientId!,
        gatewayEndpoints: normalizedGatewayEndpoints,
        gatewayServerPublicKeyHex: normalizedGatewayServerPublicKeyHex,
        installMode,
        privateKeyHex: privateKeyHex!,
      })
    : [
        "Enter the gateway server public key and endpoints to generate",
        "the paste-ready agent install command.",
      ].join(" ");
  const foregroundStartCommand =
    'env VPSMAN_AGENT_STATE_DIR="$PWD/vpsman-agent/state" ' +
    '"$PWD/vpsman-agent/bin/vpsman-agent" ' +
    '--config "$PWD/vpsman-agent/config/agent.toml" run';

  useEffect(() => {
    setGatewayServerPublicKeyHex(
      operatorPreferences?.gateway_server_public_key_hex ?? "",
    );
    setGatewayEndpoints(operatorPreferences?.gateway_endpoints ?? "");
    setInstallMode(operatorPreferences?.agent_install_mode ?? "root");
  }, [
    operatorPreferences?.gateway_endpoints,
    operatorPreferences?.gateway_server_public_key_hex,
    operatorPreferences?.agent_install_mode,
  ]);

  async function handleCopy() {
    if (!canBuildCommand) {
      return;
    }
    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error("Clipboard API is unavailable");
      }
      await navigator.clipboard.writeText(installCommand);
      setInstallFeedback({
        message: "Agent install command copied to the clipboard.",
        tone: "success",
      });
    } catch (error) {
      setInstallFeedback({
        message: clipboardFailureMessage(error),
        tone: "danger",
      });
    }
  }

  async function handleSaveGatewayDefaults() {
    if (!canSaveGatewayDefaults || operatorPreferences === null) {
      return;
    }
    setSavePending(true);
    setInstallFeedback(null);
    try {
      await onUpdateOperatorPreferences({
        ...operatorPreferences,
        agent_install_mode: installMode,
        gateway_endpoints: normalizedGatewayEndpoints,
        gateway_server_public_key_hex:
          normalizedGatewayServerPublicKeyHex || null,
      });
      setInstallFeedback({
        message: "Gateway install defaults saved for this operator.",
        tone: "success",
      });
    } catch (error) {
      setInstallFeedback({
        message:
          error instanceof Error
            ? error.message
            : "Gateway install defaults were not saved because the browser returned no failure detail. Refresh preferences and retry.",
        tone: "danger",
      });
    } finally {
      setSavePending(false);
    }
  }

  return (
    <div
      aria-describedby={installValidationDescription}
      aria-label={
        hasInstallIdentity
          ? "Agent install command"
          : "Gateway installer defaults"
      }
      className="installCommandBlock"
    >
      <div className="installCommandHeader">
        <div>
          <strong>
            {hasInstallIdentity
              ? "Agent install command"
              : "Gateway installer defaults"}
          </strong>
          <span>
            {hasInstallIdentity
              ? "Uses the stable repository installer, which resolves the selected agent release through version.json. The private key is shown once and is not saved by the console; the copied line contains it, so use a trusted shell with history disabled and clear the clipboard afterward."
              : "Saved per operator and reused when this console generates agent install commands. These values do not change the gateway listener or private key."}
          </span>
        </div>
        <div className="sectionActions">
          <button
            aria-describedby={gatewayValidationDescription}
            className="secondaryAction compact"
            disabled={!canSaveGatewayDefaults}
            onClick={handleSaveGatewayDefaults}
            title={saveGatewayDefaultsTitle}
            type="button"
          >
            <Save size={15} />
            {saveGatewayDefaultsLabel}
          </button>
          {hasInstallIdentity ? (
            <button
              aria-describedby={installValidationDescription}
              className="secondaryAction compact"
              data-tooltip-disabled-reason={
                installValidationDescription
                  ? "Enter a valid gateway public key and gateway endpoint before copying the install command."
                  : "A one-time VPS identity and private key are required before copying an install command."
              }
              disabled={!canBuildCommand}
              onClick={() => void handleCopy()}
              title={
                canBuildCommand
                  ? "Copy the complete one-line install command."
                  : undefined
              }
              type="button"
            >
              <Copy size={15} />
              Copy command
            </button>
          ) : null}
        </div>
      </div>
      <div className="installCommandControls">
        <label
          htmlFor={gatewayKeyInputId}
          title="Gateway server public key hex used by the agent to authenticate the gateway."
        >
          <span>Gateway public key</span>
          <input
            aria-describedby={gatewayKeyInvalid ? gatewayKeyErrorId : undefined}
            aria-errormessage={
              gatewayKeyInvalid ? gatewayKeyErrorId : undefined
            }
            aria-invalid={gatewayKeyInvalid}
            aria-label="Gateway server public key hex"
            aria-required={hasInstallIdentity}
            className="monospace"
            id={gatewayKeyInputId}
            name="gateway_server_public_key_hex"
            onChange={(event) => {
              setGatewayServerPublicKeyHex(event.target.value);
              setInstallFeedback(null);
            }}
            placeholder="64 hex characters"
            value={gatewayServerPublicKeyHex}
          />
        </label>
        <label
          htmlFor={gatewayEndpointsInputId}
          title="Comma- or newline-separated gateway endpoints accepted by the installer."
        >
          <span>Gateway endpoints</span>
          <textarea
            aria-describedby={
              gatewayEndpointsInvalid ? gatewayEndpointsErrorId : undefined
            }
            aria-errormessage={
              gatewayEndpointsInvalid ? gatewayEndpointsErrorId : undefined
            }
            aria-invalid={gatewayEndpointsInvalid}
            aria-label="Gateway endpoints"
            aria-required={hasInstallIdentity}
            id={gatewayEndpointsInputId}
            name="gateway_endpoints"
            onChange={(event) => {
              setGatewayEndpoints(event.target.value);
              setInstallFeedback(null);
            }}
            placeholder="primary=gw.example.com:9443=10"
            rows={2}
            value={gatewayEndpoints}
          />
        </label>
        <label
          htmlFor={installModeInputId}
          title="Root and user service modes start through systemd. Stage only writes the agent files and shows the foreground start command."
        >
          <span>Install mode</span>
          <select
            aria-label="Install mode"
            id={installModeInputId}
            name="agent_install_mode"
            onChange={(event) => {
              setInstallMode(event.target.value as AgentInstallMode);
              setInstallFeedback(null);
            }}
            value={installMode}
          >
            <option value="root">Root service</option>
            <option value="user">User service</option>
            <option value="staged">Stage only (no systemd)</option>
          </select>
        </label>
      </div>
      <div aria-live="polite" className="installCommandValidation">
        {gatewayKeyInvalid ? (
          <small className="installCommandHint warn" id={gatewayKeyErrorId}>
            {normalizedGatewayServerPublicKeyHex.length === 0
              ? "Gateway public key is required before saving defaults or copying the command."
              : "Gateway public key must be exactly 64 hex characters."}
          </small>
        ) : null}
        {gatewayEndpointsInvalid ? (
          <small
            className="installCommandHint warn"
            id={gatewayEndpointsErrorId}
          >
            {gatewayEndpointsError}
          </small>
        ) : null}
      </div>
      <ActionFeedback
        className="localActionFeedback"
        message={
          savePending
            ? "Saving gateway install defaults"
            : installFeedback?.message
        }
        tone={savePending ? "progress" : installFeedback?.tone}
      />
      {hasInstallIdentity ? (
        <pre
          data-tooltip-sensitive="true"
          title="Paste-ready install command containing the one-time private key; copy it only into a trusted shell."
        >
          <code>{installCommand}</code>
        </pre>
      ) : null}
      {hasInstallIdentity && installMode === "staged" ? (
        <div className="installCommandFollowup" role="note">
          <strong>Then run in foreground</strong>
          <code title="Foreground command for a staged unprivileged installation.">
            {foregroundStartCommand}
          </code>
        </div>
      ) : null}
    </div>
  );
}

function normalizeGatewayEndpointList(value: string): string {
  return value
    .split(/[\n,]/)
    .map((entry) => entry.trim())
    .filter(Boolean)
    .join("\n");
}

function buildAgentInstallCommand({
  clientId,
  gatewayEndpoints,
  gatewayServerPublicKeyHex,
  installMode,
  privateKeyHex,
}: {
  clientId: string;
  gatewayEndpoints: string;
  gatewayServerPublicKeyHex: string;
  installMode: AgentInstallMode;
  privateKeyHex: string;
}): string {
  const installerMode = installMode === "staged" ? "unprivileged" : installMode;
  const environment = [
    ["VPSMAN_AGENT_RELEASE", DEFAULT_AGENT_INSTALL_RELEASE],
    ["VPSMAN_INSTALL_MODE", installerMode],
    ["VPSMAN_AGENT_CLIENT_ID", clientId],
    ["VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX", privateKeyHex],
    ["VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX", gatewayServerPublicKeyHex],
    ["VPSMAN_GATEWAY_ENDPOINTS", gatewayEndpoints],
  ];
  if (installMode === "staged") {
    environment.push(["VPSMAN_AGENT_ENABLE_SERVICE", "0"]);
  }
  return [
    'agent_install_tmp="$(mktemp -d)" &&',
    `(trap 'rm -rf -- "$agent_install_tmp"' EXIT;`,
    `curl ${INSTALLER_CURL_FLAGS} ${shellQuote(AGENT_INSTALL_SCRIPT_URL)} -o "$agent_install_tmp/install-agent.sh" &&`,
    "env",
    ...environment.map(([name, value]) => `${name}=${shellQuote(value)}`),
    'bash "$agent_install_tmp/install-agent.sh")',
  ].join(" ");
}

function shellQuote(value: string): string {
  return `'${value.replace(/'/g, "'\\''")}'`;
}
