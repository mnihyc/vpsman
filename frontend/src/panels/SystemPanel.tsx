import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Database,
  KeyRound,
  LockKeyhole,
  Network,
  Pencil,
  RefreshCw,
  Save,
  ServerCog,
  SlidersHorizontal,
  ShieldCheck,
  TimerReset,
  Trash2,
  UserPlus,
  UserX,
  X,
} from "lucide-react";
import { ConsoleDataGrid, type ConsoleDataGridColumn } from "../components/ConsoleDataGrid";
import { parse, stringify, type TomlTable } from "smol-toml";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { TimeSeriesChart, type TimeSeriesChartLine } from "../components/TimeSeriesChart";
import { dashboardChartColors } from "../colorPalette";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  operatorDbPayloadHashHex,
  textPayloadHashHex,
  type PrivilegeAssertion,
  type PrivilegeMaterial,
} from "../privilege";
import { useReviewGenerationGuard, waitForReviewRender } from "../hooks/useReviewGenerationGuard";
import type {
  ActiveView,
  JsonValue,
  OperatorAuthEventRecord,
  OperatorView,
  OperatorSessionRecord,
  SuiteConfigResponse,
  SuiteConfigUpdateResponse,
  SuiteConfigValidateResponse,
  SuiteConfigValidationRecord,
  SystemDashboardRecord,
  SystemMetricSeriesRecord,
  TagView,
} from "../types";
import type { SystemDashboardPointDensity, SystemDashboardWindow } from "../hooks/useSystemData";
import { PreferencesPanel } from "./PreferencesPanel";
import { formatTime, shortId, statusClass } from "../utils";

type SystemPanelProps = {
  activeSubpage: string;
  dashboard: SystemDashboardRecord | null;
  dashboardError: string | null;
  dashboardLoading: boolean;
  dashboardPointDensity: SystemDashboardPointDensity;
  dashboardWindow: SystemDashboardWindow;
  onDashboardPointDensityChange: (density: SystemDashboardPointDensity) => void;
  onDashboardRefresh: () => void;
  onDashboardWindowChange: (window: SystemDashboardWindow) => void;
  onClearOperatorTotp: (
    operatorId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onCreateOperator: (
    username: string,
    role: string,
    password: string,
    scopes: string[],
    sessionRefreshTtlSecs: number,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onLoadSuiteConfig: () => void;
  onOpenPrivilegeUnlock: () => void;
  onResetOperatorPassword: (
    operatorId: string,
    password: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onRevokeOperatorSession: (
    sessionId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onSelectView: (view: ActiveView, subpage?: string) => void;
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
  onUpdateSuiteConfig: (
    toml: string,
    privilegeAssertion: unknown,
  ) => Promise<SuiteConfigUpdateResponse>;
  onValidateSuiteConfig: (toml: string) => Promise<SuiteConfigValidateResponse>;
  operator: OperatorView | null;
  operatorAuthEvents: OperatorAuthEventRecord[];
  operatorSessions: OperatorSessionRecord[];
  operators: OperatorView[];
  privilegeMaterial: PrivilegeMaterial | null;
  suiteConfig: SuiteConfigResponse | null;
  suiteConfigError: string | null;
  suiteConfigLoading: boolean;
  tags: TagView[];
};

const dashboardWindows: Array<{ label: string; value: SystemDashboardWindow }> = [
  { label: "15m", value: "15m" },
  { label: "1h", value: "1h" },
  { label: "6h", value: "6h" },
  { label: "24h", value: "24h" },
  { label: "7d", value: "7d" },
  { label: "30d", value: "30d" },
];
const pointDensityOptions: Array<{ label: string; value: SystemDashboardPointDensity }> = [
  { label: "Compact", value: "compact" },
  { label: "Balanced", value: "balanced" },
  { label: "Dense", value: "dense" },
];

type SystemHealthTone = "critical" | "warning" | "ok" | "info" | "neutral";

type SystemAttentionItem = {
  detail: string;
  label: string;
  tone: SystemHealthTone;
  value: string;
};

type SystemThresholdItem = {
  label: string;
  tone?: SystemHealthTone;
  value: string;
};

const operatorRoleOptions = ["viewer", "operator", "admin"];
const commonScopeOptions = [
  "fleet:read",
  "jobs:read",
  "backups:read",
  "terminal:read",
  "integrations:read",
  "templates:read",
  "schedules:read",
  "config:read",
  "network:read",
  "audit:read",
  "jobs:write",
  "inventory:write",
  "schedules:write",
  "backups:write",
  "network:write",
  "config:write",
  "integrations:write",
  "templates:write",
  "history:write",
  "*",
];
const defaultSessionTtlDays = 365;
const operatorHelpText = {
  username:
    "Login username. Existing operator usernames are locked in the editor; create a new operator for a new login name.",
  createPassword:
    "Initial operator password. It must be at least 12 characters and is used only when creating the user.",
  newPassword:
    "Replacement password. Save does not read or send this field; use Reset password to apply it and revoke existing sessions.",
  role:
    "Role controls the default permission bundle. Admin grants full operator control; operator and viewer are narrower unless scopes override them.",
  sessionRefreshTtl:
    "Refresh-token/session lifetime in days for newly issued operator sessions. This is not the short access-token expiry shown in Audit / Sessions.",
  scopes:
    "Optional scope override. Leave empty for role defaults. Accepts comma or space separated scopes such as fleet:read, jobs:write, config:read, or *.",
  shortcuts:
    "Append a scope token to the override field. These shortcuts are editable text helpers, not immutable presets.",
  save:
    "Save role, scopes, and refresh-token session TTL only. This action never changes the password field.",
  resetPassword:
    "Apply the New password field, clear existing TOTP secret material, then revoke existing sessions for this operator.",
  clearTotp:
    "Remove stored TOTP secret material and revoke existing sessions. The user must enroll TOTP again before using it.",
  enable:
    "Allow this disabled operator to log in again.",
  disable:
    "Block login and revoke existing sessions without deleting the operator record.",
  delete:
    "Delete this operator record for login purposes, block login, and revoke existing sessions. The backend keeps the username reserved.",
  create:
    "Create the operator record. Password is required for creation and is not shown in the confirmation message.",
  sessionAccessExpires:
    "Short access-token expiry for this bearer session.",
  sessionRefreshExpires:
    "Refresh-token/session expiry. This is controlled by the user's refresh TTL setting.",
};

type ConfigFieldKind = "checkbox" | "number" | "text";

type ConfigFieldSpec = {
  defaultValue: string;
  help: string;
  kind: ConfigFieldKind;
  label: string;
  path: string;
  required?: boolean;
  rule: string;
};

type ConfigSectionSpec = {
  description: string;
  fields: ConfigFieldSpec[];
  id: string;
  title: string;
};

type ConfigImpact = "hot" | "restart" | "unknown";

const suiteConfigSections: ConfigSectionSpec[] = [
  {
    description: "Private operator API listener, gateway control socket, and artifact thresholds.",
    id: "api",
    title: "API",
    fields: [
      {
        defaultValue: "127.0.0.1:8080",
        help: "Private HTTP API bind address for the console and control-plane API. Exposing this beyond private networks changes the attack surface.",
        kind: "text",
        label: "Private API bind",
        path: "api.bind",
        required: true,
        rule: "host:port or loopback bind",
      },
      {
        defaultValue: "unix:/var/lib/vpsman/gateway-control.sock",
        help: "Gateway control endpoint used by the API for gateway operations. Keep it private to the control host or trusted container network.",
        kind: "text",
        label: "Gateway control URL",
        path: "api.gateway_control_url",
        required: true,
        rule: "unix: path or private URL",
      },
      {
        defaultValue: "32768",
        help: "Job output at or above this size becomes an artifact instead of inline response text, reducing table payload size.",
        kind: "number",
        label: "Output artifact threshold",
        path: "api.job_output_artifact_min_bytes",
        rule: "bytes, 0 or greater",
      },
      {
        defaultValue: "unset",
        help: "Maximum accepted artifact size for API-managed artifacts when configured by the backend.",
        kind: "number",
        label: "Artifact max bytes",
        path: "api.artifact_max_bytes",
        rule: "bytes, 0 or greater",
      },
      {
        defaultValue: "false",
        help: "Blocks agent update packages that are not registered in the update registry.",
        kind: "checkbox",
        label: "Require registered agent updates",
        path: "api.require_registered_agent_updates",
        rule: "boolean",
      },
    ],
  },
  {
    description: "Agent listener, control socket, API forwarding URL, and gateway identity.",
    id: "gateway",
    title: "Gateway",
    fields: [
      {
        defaultValue: "0.0.0.0:9443",
        help: "Gateway listener for agent connections. Public binds require firewall and key controls.",
        kind: "text",
        label: "Agent bind",
        path: "gateway.bind",
        required: true,
        rule: "host:port",
      },
      {
        defaultValue: "unix:/var/lib/vpsman/gateway-control.sock",
        help: "Local gateway control listener used by the API. Prefer a Unix socket or private bind.",
        kind: "text",
        label: "Control bind",
        path: "gateway.control_bind",
        required: true,
        rule: "unix: path or private bind",
      },
      {
        defaultValue: "http://api:8080",
        help: "API URL the gateway forwards control-plane traffic to. This should resolve from the gateway runtime.",
        kind: "text",
        label: "API URL",
        path: "gateway.api_url",
        required: true,
        rule: "URL reachable by gateway",
      },
      {
        defaultValue: "compose-gateway",
        help: "Stable gateway identity used in logs and routing evidence.",
        kind: "text",
        label: "Gateway ID",
        path: "gateway.gateway_id",
        required: true,
        rule: "stable non-empty identifier",
      },
      {
        defaultValue: "60",
        help: "Grace window for reconnecting agents before the gateway treats a connection as gone.",
        kind: "number",
        label: "Reconnect grace seconds",
        path: "gateway.reconnect_grace_secs",
        rule: "seconds, 0 or greater",
      },
    ],
  },
  {
    description: "Worker cadence, leases, schedule timeout, and offline reconciliation.",
    id: "worker",
    title: "Worker",
    fields: [
      {
        defaultValue: "30",
        help: "Main worker loop cadence. Lower values react faster but increase database and control-plane churn.",
        kind: "number",
        label: "Tick seconds",
        path: "worker.tick_secs",
        rule: "seconds, 1 or greater",
      },
      {
        defaultValue: "60",
        help: "Lease duration for worker ownership. Too short can cause churn; too long delays failover.",
        kind: "number",
        label: "Worker lease seconds",
        path: "worker.worker_lease_secs",
        rule: "seconds, 1 or greater",
      },
      {
        defaultValue: "300",
        help: "Time before a missing agent heartbeat is treated as offline by worker reconciliation.",
        kind: "number",
        label: "Offline timeout seconds",
        path: "worker.agent_offline_timeout_secs",
        rule: "seconds, 1 or greater",
      },
      {
        defaultValue: "30",
        help: "Maximum timeout used when worker-created schedule jobs do not specify a tighter value.",
        kind: "number",
        label: "Schedule job max timeout",
        path: "worker.schedule_job_max_timeout_secs",
        rule: "seconds, 1 or greater",
      },
    ],
  },
  {
    description: "Control-plane pool and dispatch limits for 20-50 long-lived VPS operation.",
    id: "capacity",
    title: "Capacity",
    fields: [
      {
        defaultValue: "32",
        help: "API database connection pool. Increase only with database capacity and operator concurrency in mind.",
        kind: "number",
        label: "API DB pool",
        path: "capacity.api_db_pool",
        rule: "integer, 1 or greater",
      },
      {
        defaultValue: "8",
        help: "Worker database connection pool for schedules, telemetry, and background reconciliation.",
        kind: "number",
        label: "Worker DB pool",
        path: "capacity.worker_db_pool",
        rule: "integer, 1 or greater",
      },
      {
        defaultValue: "128",
        help: "Maximum dispatch batch size per cycle. Larger batches move work faster but can spike gateway pressure.",
        kind: "number",
        label: "Dispatcher batch",
        path: "capacity.dispatcher_batch",
        rule: "integer, 1 or greater",
      },
      {
        defaultValue: "64",
        help: "Maximum targets in flight at once. Tune with queue depth, gateway retries, and agent latency.",
        kind: "number",
        label: "Dispatcher in-flight",
        path: "capacity.dispatcher_in_flight",
        rule: "integer, 1 or greater",
      },
    ],
  },
  {
    description: "Backup artifact location and optional object-store client settings.",
    id: "storage",
    title: "Storage",
    fields: [
      {
        defaultValue: "/var/lib/vpsman/objects/backups",
        help: "Local backup object store directory used when artifacts are kept on the server.",
        kind: "text",
        label: "Backup object store dir",
        path: "storage.backup_object_store_dir",
        required: true,
        rule: "absolute path",
      },
      {
        defaultValue: "unset",
        help: "S3-compatible object endpoint for remote artifact storage.",
        kind: "text",
        label: "Object endpoint",
        path: "storage.object_endpoint",
        rule: "URL or unset",
      },
      {
        defaultValue: "unset",
        help: "Remote bucket name for object artifacts.",
        kind: "text",
        label: "Object bucket",
        path: "storage.object_bucket",
        rule: "bucket name or unset",
      },
      {
        defaultValue: "unset",
        help: "Remote object-store region when the provider requires it.",
        kind: "text",
        label: "Object region",
        path: "storage.object_region",
        rule: "region string or unset",
      },
    ],
  },
  {
    description: "Secret-file references. Values point to mounted files; secret contents stay hidden.",
    id: "secrets",
    title: "Secrets",
    fields: [
      {
        defaultValue: "/run/secrets/vpsman_internal_token",
        help: "File containing the internal API token used by trusted control-plane services.",
        kind: "text",
        label: "Internal token file",
        path: "secrets.internal_token_file",
        required: true,
        rule: "absolute file path",
      },
      {
        defaultValue: "/run/secrets/vpsman_gateway_private_key_hex",
        help: "File containing the gateway private key material. The UI only stores the path.",
        kind: "text",
        label: "Gateway key file",
        path: "secrets.gateway_private_key_file",
        required: true,
        rule: "absolute file path",
      },
      {
        defaultValue: "/run/secrets/vpsman_privilege_verifier_key_hex",
        help: "File containing privilege verifier material used for privileged review assertions.",
        kind: "text",
        label: "Privilege verifier file",
        path: "secrets.privilege_verifier_key_file",
        required: true,
        rule: "absolute file path",
      },
      {
        defaultValue: "unset",
        help: "File containing remote object-store secret key material when object storage is configured.",
        kind: "text",
        label: "Object secret key file",
        path: "secrets.object_secret_key_file",
        rule: "absolute file path or unset",
      },
    ],
  },
  {
    description: "Network and internal HTTP timing controls that shape retry and deadline behavior.",
    id: "timeouts",
    title: "Timeouts",
    fields: [
      {
        defaultValue: "30",
        help: "Time allowed for dispatch acknowledgement before work is considered delayed.",
        kind: "number",
        label: "Dispatch ack seconds",
        path: "timeout.dispatch_ack_secs",
        rule: "seconds, 1 or greater",
      },
      {
        defaultValue: "15",
        help: "Timeout for posting gateway events to the API.",
        kind: "number",
        label: "Event post seconds",
        path: "timeout.event_post_secs",
        rule: "seconds, 1 or greater",
      },
      {
        defaultValue: "15",
        help: "Read timeout for internal HTTP calls among control-plane services.",
        kind: "number",
        label: "Internal HTTP read seconds",
        path: "timeout.internal_http_read_secs",
        rule: "seconds, 1 or greater",
      },
      {
        defaultValue: "300",
        help: "Timeout used by dashboard and worker capacity logic for agent-offline interpretation.",
        kind: "number",
        label: "Agent offline seconds",
        path: "timeout.agent_offline_secs",
        rule: "seconds, 1 or greater",
      },
    ],
  },
];

export function SystemPanel({
  activeSubpage,
  dashboard,
  dashboardError,
  dashboardLoading,
  dashboardPointDensity,
  dashboardWindow,
  onDashboardPointDensityChange,
  onDashboardRefresh,
  onDashboardWindowChange,
  onClearOperatorTotp,
  onCreateOperator,
  onLoadSuiteConfig,
  onOpenPrivilegeUnlock,
  onResetOperatorPassword,
  onRevokeOperatorSession,
  onSelectView,
  onSetOperatorStatus,
  onUpdateOperator,
  onUpdateSuiteConfig,
  onValidateSuiteConfig,
  operator,
  operatorAuthEvents,
  operatorSessions,
  operators,
  privilegeMaterial,
  suiteConfig,
  suiteConfigError,
  suiteConfigLoading,
  tags,
}: SystemPanelProps) {
  if (activeSubpage === "config") {
    return (
      <SystemConfigPanel
        config={suiteConfig}
        error={suiteConfigError}
        loading={suiteConfigLoading}
        onLoad={onLoadSuiteConfig}
        onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
        onSelectView={onSelectView}
        onUpdate={onUpdateSuiteConfig}
        onValidate={onValidateSuiteConfig}
        privilegeMaterial={privilegeMaterial}
      />
    );
  }
  if (activeSubpage === "users") {
    return (
      <SystemUsersPanel
        authEvents={operatorAuthEvents}
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
      />
    );
  }
  if (activeSubpage === "sessions") {
    return (
      <SystemSessionsPanel
        authEvents={operatorAuthEvents}
        onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
        onRevokeOperatorSession={onRevokeOperatorSession}
        privilegeMaterial={privilegeMaterial}
        sessions={operatorSessions}
      />
    );
  }
  if (activeSubpage === "operator") {
    return <PreferencesPanel operator={operator} tags={tags} />;
  }
  if (activeSubpage === "capacity") {
    return (
      <SystemCapacityPanel
        dashboard={dashboard}
        error={dashboardError}
        loading={dashboardLoading}
        onPointDensityChange={onDashboardPointDensityChange}
        onRefresh={onDashboardRefresh}
        onWindowChange={onDashboardWindowChange}
        pointDensity={dashboardPointDensity}
        window={dashboardWindow}
      />
    );
  }
  return (
    <SystemDashboardPanel
      dashboard={dashboard}
      error={dashboardError}
      loading={dashboardLoading}
      onPointDensityChange={onDashboardPointDensityChange}
      onRefresh={onDashboardRefresh}
      onWindowChange={onDashboardWindowChange}
      pointDensity={dashboardPointDensity}
      window={dashboardWindow}
    />
  );
}

type PendingUserAction =
  | {
      kind: "create";
      username: string;
      role: string;
      password: string;
      scopes: string[];
      sessionRefreshTtlSecs: number;
      adminRisk: boolean;
      privilege: OperatorPrivilegeSnapshot;
    }
  | {
      kind: "update";
      operator: OperatorView;
      role: string;
      scopes: string[];
      sessionRefreshTtlSecs: number;
      adminRisk: boolean;
      privilege: OperatorPrivilegeSnapshot;
    }
  | {
      kind: "status";
      operators: OperatorView[];
      status: "active" | "disabled" | "deleted";
      adminRisk: boolean;
      privileges: Record<string, OperatorPrivilegeSnapshot>;
    }
  | {
      kind: "password";
      operator: OperatorView;
      password: string;
      adminRisk: boolean;
      privilege: OperatorPrivilegeSnapshot;
    }
  | {
      kind: "totp";
      operators: OperatorView[];
      adminRisk: boolean;
      privileges: Record<string, OperatorPrivilegeSnapshot>;
    }
  | {
      kind: "sessions";
      operator: OperatorView;
      sessions: OperatorSessionRecord[];
      adminRisk: boolean;
      privileges: Record<string, OperatorPrivilegeSnapshot>;
    };

type OperatorPrivilegeSnapshot = {
  payloadHashHex: string;
  privilegeAssertion: PrivilegeAssertion;
};

type OperatorAccessSummary = {
  activeSessions: number;
  failedLogins: number;
  lastFailure: OperatorAuthEventRecord | null;
  lastLogin: OperatorAuthEventRecord | null;
  revokableSessions: OperatorSessionRecord[];
};

type PendingSessionRevoke = {
  sessions: OperatorSessionRecord[];
  adminRisk: boolean;
  privileges: Record<string, OperatorPrivilegeSnapshot>;
};

type SessionEnrichment = {
  authEvent: OperatorAuthEventRecord | null;
  browser: string;
  device: string;
  location: string;
  remoteIp: string;
  riskDetail: string;
  riskLabel: string;
  riskTone: SystemHealthTone;
  userAgent: string;
};

type AuthFailureGroup = {
  count: number;
  firstAt: string;
  key: string;
  lastAt: string;
  reason: string;
  remoteIp: string;
  riskDetail: string;
  riskLabel: string;
  username: string;
  userAgent: string;
};

function FieldLabel({ help, label }: { help: string; label: string }) {
  return (
    <span className="fieldLabelWithHelp">
      <span>{label}</span>
      <span
        aria-label={`${label} help`}
        className="fieldHelpIcon"
        role="img"
        tabIndex={0}
        title={help}
      >
        ?
      </span>
    </span>
  );
}

function SystemUsersPanel({
  authEvents,
  currentOperator,
  onClearOperatorTotp,
  onCreateOperator,
  onOpenPrivilegeUnlock,
  onResetOperatorPassword,
  onRevokeOperatorSession,
  onSetOperatorStatus,
  onUpdateOperator,
  operators,
  privilegeMaterial,
  sessions,
}: {
  authEvents: OperatorAuthEventRecord[];
  currentOperator: OperatorView | null;
  onClearOperatorTotp: (
    operatorId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onCreateOperator: (
    username: string,
    role: string,
    password: string,
    scopes: string[],
    sessionRefreshTtlSecs: number,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onOpenPrivilegeUnlock: () => void;
  onResetOperatorPassword: (
    operatorId: string,
    password: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onRevokeOperatorSession: (
    sessionId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
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
  operators: OperatorView[];
  privilegeMaterial: PrivilegeMaterial | null;
  sessions: OperatorSessionRecord[];
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selectedOperator = operators.find((item) => item.id === selectedId) ?? null;
  const [editorMode, setEditorMode] = useState<"closed" | "create" | "edit">("closed");
  const [draftUsername, setDraftUsername] = useState("");
  const [draftPassword, setDraftPassword] = useState("");
  const [draftRole, setDraftRole] = useState("operator");
  const [draftScopes, setDraftScopes] = useState("");
  const [draftSessionTtlDays, setDraftSessionTtlDays] = useState(defaultSessionTtlDays);
  const [pendingAction, setPendingAction] = useState<PendingUserAction | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const canManageUsers = currentOperator?.role === "admin";
  const accessSummaries = useMemo(
    () => buildOperatorAccessSummaries(operators, sessions, authEvents),
    [authEvents, operators, sessions],
  );
  const selectedAccessSummary = selectedOperator ? accessSummaries[selectedOperator.id] : null;
  const activeOperators = operators.filter((operator) => operator.status !== "deleted");
  const adminOperators = activeOperators.filter((operator) => operator.role === "admin");
  const adminWithoutMfaCount = adminOperators.filter((operator) => !operator.totp_enabled).length;
  const adminLongTtlCount = adminOperators.filter(
    (operator) => operator.session_refresh_ttl_secs > 30 * 86_400,
  ).length;
  const maxAdminTtlDays = adminOperators.reduce(
    (max, operator) => Math.max(max, secondsToDays(operator.session_refresh_ttl_secs)),
    0,
  );
  const explicitScopeUsers = activeOperators.filter((operator) => operator.scopes.length > 0).length;
  const visibleRoles = Array.from(new Set(activeOperators.map((operator) => operator.role))).sort();
  const activeSessionCount = sessions.filter((session) => !session.revoked).length;
  const revokableSessionCount = sessions.filter((session) => !session.current && !session.revoked).length;
  const authFailureCount = authEvents.filter((event) => event.result !== "success").length;
  const governanceTone = adminWithoutMfaCount > 0 || adminLongTtlCount > 0 ? "warning" : "ok";
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();

  useEffect(() => {
    if (editorMode !== "edit" || !selectedOperator) {
      return;
    }
    setDraftUsername(selectedOperator.username);
    setDraftPassword("");
    setDraftRole(selectedOperator.role);
    setDraftScopes(selectedOperator.scopes.join(", "));
    setDraftSessionTtlDays(secondsToDays(selectedOperator.session_refresh_ttl_secs));
    setActionError(null);
    setPendingAction(null);
    invalidateReviewGeneration();
  }, [editorMode, selectedOperator, invalidateReviewGeneration]);

  useEffect(() => {
    if (editorMode === "edit" && selectedId && !operators.some((operator) => operator.id === selectedId)) {
      setSelectedId(null);
      setEditorMode("closed");
    }
    setPendingAction(null);
    invalidateReviewGeneration();
  }, [editorMode, operators, selectedId, invalidateReviewGeneration]);

  const userColumns = useMemo<ConsoleDataGridColumn<OperatorView>[]>(
    () => [
      {
        id: "username",
        header: "Username",
        cell: (row) => <span className="operatorRecordName">{row.username}</span>,
        searchValue: (row) => row.username,
        sortValue: (row) => row.username,
      },
      {
        id: "status",
        header: "Status",
        cell: (row) => <span className={`statusPill ${statusClass(row.status)}`}>{row.status}</span>,
        searchValue: (row) => row.status,
        sortValue: (row) => row.status,
      },
      {
        id: "role",
        header: "Role",
        cell: (row) => <span className={`statusPill ${statusClass(row.role)}`}>{row.role}</span>,
        searchValue: (row) => row.role,
        sortValue: (row) => row.role,
      },
      {
        id: "mfa",
        header: "MFA",
        cell: (row) => (
          <span className={`status ${row.totp_enabled ? "ok" : row.role === "admin" ? "warn" : "neutral"}`}>
            {row.totp_enabled ? "enabled" : row.role === "admin" ? "admin off" : "off"}
          </span>
        ),
        searchValue: (row) => (row.totp_enabled ? "mfa enabled totp enabled" : "mfa off totp off"),
        sortValue: (row) => (row.totp_enabled ? 1 : 0),
      },
      {
        id: "ttl",
        header: "Session TTL",
        cell: (row) => (
          <span
            className={`status ${row.role === "admin" && row.session_refresh_ttl_secs > 30 * 86_400 ? "warn" : "neutral"}`}
            title={operatorHelpText.sessionRefreshTtl}
          >
            {secondsToDays(row.session_refresh_ttl_secs)}d
          </span>
        ),
        sortValue: (row) => row.session_refresh_ttl_secs,
      },
      {
        id: "sessions",
        header: "Active sessions",
        cell: (row) => `${accessSummaries[row.id]?.activeSessions ?? 0}`,
        sortValue: (row) => accessSummaries[row.id]?.activeSessions ?? 0,
      },
      {
        id: "lastLogin",
        header: "Last login",
        cell: (row) => formatAuthEventTime(accessSummaries[row.id]?.lastLogin),
        searchValue: (row) => formatAuthEventTime(accessSummaries[row.id]?.lastLogin),
        sortValue: (row) => accessSummaries[row.id]?.lastLogin?.created_at ?? "",
      },
      {
        id: "failures",
        header: "Failures",
        cell: (row) => `${accessSummaries[row.id]?.failedLogins ?? 0}`,
        sortValue: (row) => accessSummaries[row.id]?.failedLogins ?? 0,
      },
      {
        id: "created",
        header: "Created",
        cell: (row) => formatTime(row.created_at),
        sortValue: (row) => row.created_at,
      },
    ],
    [accessSummaries],
  );

  function invalidateUserReview() {
    setPendingAction(null);
    setReviewPending(false);
    invalidateReviewGeneration();
  }

  function setSelectedOperatorId(nextId: string | null) {
    invalidateUserReview();
    setSelectedId(nextId);
    setEditorMode(nextId ? "edit" : "closed");
  }

  function resetCreateDraft() {
    invalidateUserReview();
    setSelectedId(null);
    setEditorMode("create");
    setDraftUsername("");
    setDraftPassword("");
    setDraftRole("operator");
    setDraftScopes("");
    setDraftSessionTtlDays(defaultSessionTtlDays);
    setActionError(null);
  }

  function closeEditor() {
    invalidateUserReview();
    setSelectedId(null);
    setEditorMode("closed");
    setActionError(null);
  }

  async function requestPendingAction(
    builder: (material: PrivilegeMaterial) => Promise<PendingUserAction>,
  ) {
    if (!privilegeMaterial) {
      setActionError("Local privilege unlock is required");
      onOpenPrivilegeUnlock();
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    setReviewPending(true);
    setActionError(null);
    try {
      await waitForReviewRender();
      const action = await builder(privilegeMaterial);
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingAction(action);
    } catch (error) {
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setActionError(error instanceof Error ? error.message : "Privilege assertion failed");
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewPending(false);
      }
    }
  }

  async function buildOperatorPrivilegeSnapshot(
    material: PrivilegeMaterial,
    input: {
      action: string;
      target: string;
      username?: string | null;
      role?: string | null;
      scopes?: string[];
      sessionRefreshTtlSecs?: number | null;
      status?: string | null;
      adminRisk: boolean;
    },
  ): Promise<OperatorPrivilegeSnapshot> {
    const payloadHashHex = await operatorDbPayloadHashHex({
      action: input.action,
      target: input.target,
      username: input.username ?? null,
      role: input.role ?? null,
      scopes: input.scopes ?? [],
      sessionRefreshTtlSecs: input.sessionRefreshTtlSecs ?? null,
      status: input.status ?? null,
      adminRiskAcknowledged: input.adminRisk,
    });
    const privilegeAssertion = await buildPrivilegeAssertion({
      intent: canonicalDbPrivilegeIntent({
        action: input.action,
        confirmed: true,
        payloadHash: payloadHashHex,
        resolvedTargets: [input.target],
        target: input.target,
      }),
      privilegeMaterial: material,
    });
    return { payloadHashHex, privilegeAssertion };
  }

  async function submitCreate() {
    const username = draftUsername.trim();
    const password = draftPassword;
    if (!username || password.length < 12) {
      setActionError("Username and a 12+ character password are required");
      return;
    }
    const scopes = parseScopeList(draftScopes);
    const sessionRefreshTtlSecs = daysToSeconds(draftSessionTtlDays);
    const adminRisk = draftRole === "admin";
    await requestPendingAction(async (material) => ({
      kind: "create",
      username,
      role: draftRole,
      password,
      scopes,
      sessionRefreshTtlSecs,
      adminRisk,
      privilege: await buildOperatorPrivilegeSnapshot(material, {
        action: "operator.create",
        target: username,
        username,
        role: draftRole,
        scopes,
        sessionRefreshTtlSecs,
        adminRisk,
      }),
    }));
  }

  async function submitUpdate() {
    if (!selectedOperator) {
      return;
    }
    const role = draftRole;
    const scopes = parseScopeList(draftScopes);
    const sessionRefreshTtlSecs = daysToSeconds(draftSessionTtlDays);
    const adminRisk = selectedOperator.role === "admin" || role === "admin";
    await requestPendingAction(async (material) => ({
      kind: "update",
      operator: selectedOperator,
      role,
      scopes,
      sessionRefreshTtlSecs,
      adminRisk,
      privilege: await buildOperatorPrivilegeSnapshot(material, {
        action: "operator.update",
        target: selectedOperator.id,
        role,
        scopes,
        sessionRefreshTtlSecs,
        adminRisk,
      }),
    }));
  }

  async function submitStatus(status: "active" | "disabled" | "deleted") {
    if (!selectedOperator) {
      return;
    }
    await submitBulkStatus([selectedOperator], status);
  }

  async function submitBulkStatus(rows: OperatorView[], status: "active" | "disabled" | "deleted") {
    const operatorsToChange = rows.filter((operator) => operator.status !== "deleted");
    if (operatorsToChange.length === 0) {
      return;
    }
    const adminRisk = operatorsToChange.some((operator) => operator.role === "admin");
    const action = status === "active" ? "operator.enable" : status === "disabled" ? "operator.disable" : "operator.delete";
    await requestPendingAction(async (material) => {
      const privileges = Object.fromEntries(
        await Promise.all(
          operatorsToChange.map(async (operator) => [
            operator.id,
            await buildOperatorPrivilegeSnapshot(material, {
              action,
              target: operator.id,
              status,
              adminRisk,
            }),
          ]),
        ),
      );
      return {
        kind: "status",
        operators: operatorsToChange,
        status,
        adminRisk,
        privileges,
      };
    });
  }

  async function submitPasswordReset() {
    if (!selectedOperator || draftPassword.length < 12) {
      setActionError("A 12+ character replacement password is required");
      return;
    }
    const password = draftPassword;
    const adminRisk = selectedOperator.role === "admin";
    await requestPendingAction(async (material) => ({
      kind: "password",
      operator: selectedOperator,
      password,
      adminRisk,
      privilege: await buildOperatorPrivilegeSnapshot(material, {
        action: "operator.password_reset",
        target: selectedOperator.id,
        adminRisk,
      }),
    }));
  }

  async function submitTotpClear() {
    if (!selectedOperator) {
      return;
    }
    await submitBulkTotpClear([selectedOperator]);
  }

  async function submitSelectedSessionRevoke() {
    if (!selectedOperator || !selectedAccessSummary) {
      return;
    }
    const sessionsToRevoke = selectedAccessSummary.revokableSessions;
    if (sessionsToRevoke.length === 0) {
      setActionError("No non-current active sessions are available to revoke for this user");
      return;
    }
    const adminRisk = selectedOperator.role === "admin" ||
      sessionsToRevoke.some((session) => session.operator_role === "admin");
    await requestPendingAction(async (material) => {
      const privileges = Object.fromEntries(
        await Promise.all(
          sessionsToRevoke.map(async (session) => {
            const payloadHashHex = await operatorDbPayloadHashHex({
              action: "operator_session.revoke",
              target: session.id,
              adminRiskAcknowledged: adminRisk,
            });
            const privilegeAssertion = await buildPrivilegeAssertion({
              intent: canonicalDbPrivilegeIntent({
                action: "operator_session.revoke",
                confirmed: true,
                payloadHash: payloadHashHex,
                resolvedTargets: [session.id],
                target: session.id,
              }),
              privilegeMaterial: material,
            });
            return [session.id, { payloadHashHex, privilegeAssertion }];
          }),
        ),
      );
      return {
        kind: "sessions",
        operator: selectedOperator,
        sessions: sessionsToRevoke,
        adminRisk,
        privileges,
      };
    });
  }

  async function submitBulkTotpClear(rows: OperatorView[]) {
    const operatorsToChange = rows.filter((operator) => operator.totp_enabled && operator.status !== "deleted");
    if (operatorsToChange.length === 0) {
      return;
    }
    const adminRisk = operatorsToChange.some((operator) => operator.role === "admin");
    await requestPendingAction(async (material) => {
      const privileges = Object.fromEntries(
        await Promise.all(
          operatorsToChange.map(async (operator) => [
            operator.id,
            await buildOperatorPrivilegeSnapshot(material, {
              action: "operator.totp_clear",
              target: operator.id,
              adminRisk,
            }),
          ]),
        ),
      );
      return {
        kind: "totp",
        operators: operatorsToChange,
        adminRisk,
        privileges,
      };
    });
  }

  async function confirmUserAction() {
    if (!pendingAction) {
      return;
    }
    setActionPending(true);
    setActionError(null);
    try {
      if (pendingAction.kind === "create") {
        await onCreateOperator(
          pendingAction.username,
          pendingAction.role,
          pendingAction.password,
          pendingAction.scopes,
          pendingAction.sessionRefreshTtlSecs,
          pendingAction.adminRisk,
          pendingAction.privilege.privilegeAssertion,
        );
        resetCreateDraft();
      } else if (pendingAction.kind === "update") {
        await onUpdateOperator(
          pendingAction.operator.id,
          pendingAction.role,
          pendingAction.scopes,
          pendingAction.sessionRefreshTtlSecs,
          pendingAction.adminRisk,
          pendingAction.privilege.privilegeAssertion,
        );
      } else if (pendingAction.kind === "status") {
        for (const operator of pendingAction.operators) {
          await onSetOperatorStatus(
            operator.id,
            pendingAction.status,
            pendingAction.adminRisk,
            pendingAction.privileges[operator.id].privilegeAssertion,
          );
        }
      } else if (pendingAction.kind === "password") {
        await onResetOperatorPassword(
          pendingAction.operator.id,
          pendingAction.password,
          pendingAction.adminRisk,
          pendingAction.privilege.privilegeAssertion,
        );
        setDraftPassword("");
      } else if (pendingAction.kind === "totp") {
        for (const operator of pendingAction.operators) {
          await onClearOperatorTotp(
            operator.id,
            pendingAction.adminRisk,
            pendingAction.privileges[operator.id].privilegeAssertion,
          );
        }
      } else {
        for (const session of pendingAction.sessions) {
          await onRevokeOperatorSession(
            session.id,
            pendingAction.adminRisk,
            pendingAction.privileges[session.id].privilegeAssertion,
          );
        }
      }
      setPendingAction(null);
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "User action failed");
    } finally {
      setActionPending(false);
    }
  }

  const editingDeleted = selectedOperator?.status === "deleted";

  return (
    <div className="workspace singleColumn systemWorkspace systemUsersWorkspace">
      <section className="controlPanel systemUserGovernancePanel" aria-label="Operator governance overview">
        <div className="sectionHeader fleetInstancesHeader">
          <div>
            <h2>Identity governance</h2>
            <span>MFA policy, session TTL risk, RBAC scope posture, sessions, and authentication evidence.</span>
          </div>
          <ConsoleStatusBadge tone={governanceTone}>
            {governanceTone === "ok" ? "Policy healthy" : "Needs review"}
          </ConsoleStatusBadge>
        </div>
        <div className="systemPostureGrid operatorPostureGrid">
          <SystemPostureTile
            detail="Policy target: admin accounts require MFA. Backend policy enforcement is not exposed yet, so this page flags current admin gaps."
            icon={<ShieldCheck size={18} />}
            label="MFA required policy"
            tone={adminWithoutMfaCount > 0 ? "warning" : "ok"}
            value={adminWithoutMfaCount > 0 ? `${adminWithoutMfaCount} admin off` : "admins covered"}
          />
          <SystemPostureTile
            detail={`Admin refresh TTL should stay at or below 30d; longest visible admin TTL is ${maxAdminTtlDays || 0}d.`}
            icon={<TimerReset size={18} />}
            label="Admin session TTL"
            tone={adminLongTtlCount > 0 ? "warning" : "ok"}
            value={adminLongTtlCount > 0 ? `${adminLongTtlCount} over 30d` : "<=30d"}
          />
          <SystemPostureTile
            detail="Role defaults remain server-defined; explicit scope overrides are visible and admin grants require privilege review."
            icon={<LockKeyhole size={18} />}
            label="Role model"
            tone={visibleRoles.length > 1 || explicitScopeUsers > 0 ? "info" : "warning"}
            value={`${visibleRoles.length} roles / ${explicitScopeUsers} scoped`}
          />
          <SystemPostureTile
            detail={`${revokableSessionCount} non-current active sessions can be revoked here or reviewed in Audit / Sessions.`}
            icon={<Activity size={18} />}
            label="Bearer sessions"
            tone={activeSessionCount > 0 ? "info" : "neutral"}
            value={`${activeSessionCount} active`}
          />
          <SystemPostureTile
            detail="Counts come from loaded authentication history; use Audit / Sessions for the full auth event table."
            icon={<AlertTriangle size={18} />}
            label="Auth failures"
            tone={authFailureCount > 0 ? "warning" : "ok"}
            value={authFailureCount === 1 ? "1 failure" : `${authFailureCount} failures`}
          />
          <SystemPostureTile
            detail="Password age, invite/locked state, and API-token inventory are not exposed by the current operator API."
            icon={<KeyRound size={18} />}
            label="Evidence gaps"
            tone="warning"
            value="3 backend gaps"
          />
        </div>
        <div className="operatorRoleMatrix" aria-label="RBAC role model">
          {operatorRoleOptions.map((role) => {
            const roleCount = activeOperators.filter((operator) => operator.role === role).length;
            return (
              <div className={role === "admin" ? "attention" : "neutral"} key={role}>
                <strong>{role}</strong>
                <span>{roleDescription(role)}</span>
                <small>{roleCount} users · {role === "admin" ? "extra confirmation on grants" : "scope overrides supported"}</small>
              </div>
            );
          })}
        </div>
      </section>
      <section className="controlPanel systemUsersTablePanel">
        <div className="sectionHeader fleetInstancesHeader">
          <div>
            <h2>Users</h2>
            <span>{operators.length} operator records</span>
          </div>
          <span className="sectionContext">
            {editorMode === "edit" && selectedOperator
              ? `Editing ${selectedOperator.username}`
              : editorMode === "create"
                ? "Creating new operator"
                : "Use New or row actions"}
          </span>
        </div>
        <ConsoleDataGrid
          actions={[
            {
              label: "Edit selected",
              description: (rows) =>
                rows.length === 1
                  ? `Load ${rows[0].username} into the editor.`
                  : "Select exactly one operator to edit.",
              disabled: (rows) => rows.length !== 1,
              icon: <Pencil size={14} />,
              onSelect: (rows) => setSelectedOperatorId(rows[0].id),
            },
            {
              label: "Enable selected",
              description: (rows) =>
                rows.length === 1
                  ? `Allow ${rows[0].username} to log in again.`
                  : `Allow ${rows.length} disabled operators to log in again.`,
              disabled: (rows) => reviewPending || actionPending || !canManageUsers || rows.length === 0 || rows.some((row) => row.status !== "disabled"),
              icon: <CheckCircle2 size={14} />,
              onSelect: (rows) => void submitBulkStatus(rows, "active"),
            },
            {
              label: "Disable selected",
              description: (rows) =>
                rows.length === 1
                  ? `Block ${rows[0].username} login and revoke existing sessions.`
                  : `Block login and revoke existing sessions for ${rows.length} operators.`,
              disabled: (rows) => reviewPending || actionPending || !canManageUsers || rows.length === 0 || rows.some((row) => row.status !== "active"),
              icon: <UserX size={14} />,
              onSelect: (rows) => void submitBulkStatus(rows, "disabled"),
              tone: "danger",
            },
            {
              label: "Delete selected",
              description: (rows) =>
                rows.length === 1
                  ? `Delete ${rows[0].username} for login purposes and revoke existing sessions.`
                  : `Delete ${rows.length} operators for login purposes and revoke existing sessions.`,
              disabled: (rows) => reviewPending || actionPending || !canManageUsers || rows.length === 0 || rows.some((row) => row.status === "deleted"),
              icon: <Trash2 size={14} />,
              onSelect: (rows) => void submitBulkStatus(rows, "deleted"),
              tone: "danger",
            },
            {
              label: "Clear TOTP selected",
              description: (rows) =>
                rows.length === 1
                  ? `Remove stored TOTP secret material for ${rows[0].username} and revoke existing sessions.`
                  : `Remove stored TOTP secret material and revoke sessions for ${rows.length} operators.`,
              disabled: (rows) => reviewPending || actionPending || !canManageUsers || rows.length === 0 || rows.some((row) => !row.totp_enabled || row.status === "deleted"),
              icon: <ShieldCheck size={14} />,
              onSelect: (rows) => void submitBulkTotpClear(rows),
            },
          ]}
          columns={userColumns}
          defaultPageSize={12}
          empty="No operators"
          expandOnRowClick
          getRowId={(row) => row.id}
          itemLabel="users"
          onOpenRow={(row) => setSelectedOperatorId(row.id)}
          renderExpandedRow={(row) => (
            <OperatorDetailGrid operator={row} summary={accessSummaries[row.id]} />
          )}
          renderSelectionPanel={(rows) => <OperatorSelectionPanel rows={rows} />}
          rows={operators}
          searchPlaceholder="Search username, role, status, or TOTP"
          singleExpandedRow
          storageKey="vpsman.system.users"
          title="Users"
          toolbarActions={
            <button
              className="secondaryAction compactAction"
              onClick={resetCreateDraft}
              title="Clear the editor and prepare a new operator record."
              type="button"
            >
              <UserPlus size={16} />
              <span>New</span>
            </button>
          }
        />
      </section>

      {editorMode !== "closed" && (
      <section className="controlPanel operatorEditorPanel" aria-label="Operator user editor">
        <div className="sectionHeader fleetInstancesHeader">
          <div>
            <h2>{selectedOperator ? "Edit user" : "Create user"}</h2>
            <span>{actionError ?? (reviewPending ? "Preparing review" : canManageUsers ? "Ready" : "Admin role required for changes")}</span>
          </div>
          <div className="sectionActions">
            {selectedOperator && (
            <span className="sectionContext">
              {selectedOperator.status} · {selectedOperator.role} · {secondsToDays(selectedOperator.session_refresh_ttl_secs)}d session TTL
            </span>
            )}
            <button
              aria-label="Close operator editor"
              className="iconButton"
              onClick={closeEditor}
              title="Close editor"
              type="button"
            >
              <X size={16} />
            </button>
          </div>
        </div>
        <div className="operatorEditorBody">
          {selectedOperator && selectedAccessSummary ? (
            <OperatorAccessEvidencePanel
              operator={selectedOperator}
              onRevokeSessions={() => void submitSelectedSessionRevoke()}
              pending={reviewPending || actionPending}
              summary={selectedAccessSummary}
              userCanManage={canManageUsers}
            />
          ) : null}
          {draftRole === "admin" && (!selectedOperator || selectedOperator.role !== "admin") ? (
            <div className="operatorAdminGuardrail" role="status">
              <AlertTriangle size={16} />
              <span>
                Admin role grant requires local privilege unlock, danger confirmation, reviewed scopes, MFA enrollment, and a short refresh TTL.
              </span>
            </div>
          ) : null}
          <div className="operatorEditorFields">
            <label>
              <FieldLabel help={operatorHelpText.username} label="Username" />
              <input
                aria-label="Operator username"
                disabled={Boolean(selectedOperator)}
                onChange={(event) => {
                  invalidateUserReview();
                  setDraftUsername(event.target.value);
                }}
                title={operatorHelpText.username}
                value={draftUsername}
              />
            </label>
            <label>
              <FieldLabel
                help={selectedOperator ? operatorHelpText.newPassword : operatorHelpText.createPassword}
                label={selectedOperator ? "New password" : "Password"}
              />
              <input
                aria-label="Operator password"
                disabled={!canManageUsers || editingDeleted}
                minLength={12}
                onChange={(event) => {
                  invalidateUserReview();
                  setDraftPassword(event.target.value);
                }}
                placeholder={selectedOperator ? "Only fill to reset" : "12+ characters"}
                title={selectedOperator ? operatorHelpText.newPassword : operatorHelpText.createPassword}
                type="password"
                value={draftPassword}
              />
            </label>
            <label>
              <FieldLabel help={operatorHelpText.role} label="Role" />
              <select
                aria-label="Operator role"
                disabled={!canManageUsers || editingDeleted}
                onChange={(event) => {
                  invalidateUserReview();
                  setDraftRole(event.target.value);
                }}
                title={operatorHelpText.role}
                value={draftRole}
              >
                {operatorRoleOptions.map((role) => (
                  <option key={role} value={role}>{role}</option>
                ))}
              </select>
            </label>
            <label>
              <FieldLabel help={operatorHelpText.sessionRefreshTtl} label="Session TTL days" />
              <input
                aria-label="Session refresh TTL days"
                disabled={!canManageUsers || editingDeleted}
                max={3650}
                min={1}
                onChange={(event) => {
                  invalidateUserReview();
                  setDraftSessionTtlDays(Number(event.target.value));
                }}
                title={operatorHelpText.sessionRefreshTtl}
                type="number"
                value={draftSessionTtlDays}
              />
            </label>
          </div>

          <div className="operatorScopeEditor">
            <label>
              <FieldLabel help={operatorHelpText.scopes} label="Scopes" />
              <textarea
                aria-label="Operator scopes"
                disabled={!canManageUsers || editingDeleted}
                onChange={(event) => {
                  invalidateUserReview();
                  setDraftScopes(event.target.value);
                }}
                placeholder="Leave empty for role defaults"
                rows={4}
                title={operatorHelpText.scopes}
                value={draftScopes}
              />
            </label>
            <div
              className="operatorScopeShortcuts"
              aria-label="Scope shortcuts"
              title={operatorHelpText.shortcuts}
            >
              {commonScopeOptions.map((scope) => (
                <button
                  className="tagChip"
                  disabled={!canManageUsers || editingDeleted}
                  key={scope}
                  onClick={() => {
                    invalidateUserReview();
                    setDraftScopes(addScopeToken(draftScopes, scope));
                  }}
                  title={scope === "*" ? "Append * to grant all operator scopes." : `Append ${scope} to the scope override field.`}
                  type="button"
                >
                  {scope}
                </button>
              ))}
            </div>
          </div>

          <div className="operatorEditorActions">
            {selectedOperator ? (
              <>
                <button
                  className="secondaryAction"
                  disabled={!canManageUsers || editingDeleted || reviewPending || actionPending}
                  onClick={() => void submitUpdate()}
                  title={operatorHelpText.save}
                  type="button"
                >
                  <Save size={17} />
                  Save
                </button>
                <button
                  className="secondaryAction"
                  disabled={!canManageUsers || editingDeleted || draftPassword.length < 12 || reviewPending || actionPending}
                  onClick={() => void submitPasswordReset()}
                  title={operatorHelpText.resetPassword}
                  type="button"
                >
                  <KeyRound size={17} />
                  Reset password
                </button>
                <button
                  className="secondaryAction"
                  disabled={!canManageUsers || editingDeleted || !selectedOperator.totp_enabled || reviewPending || actionPending}
                  onClick={() => void submitTotpClear()}
                  title={operatorHelpText.clearTotp}
                  type="button"
                >
                  <ShieldCheck size={17} />
                  Clear TOTP
                </button>
                <button
                  className="secondaryAction"
                  disabled={!canManageUsers || selectedOperator.status !== "disabled" || reviewPending || actionPending}
                  onClick={() => void submitStatus("active")}
                  title={operatorHelpText.enable}
                  type="button"
                >
                  <CheckCircle2 size={17} />
                  Enable
                </button>
                <button
                  className="secondaryAction dangerAction"
                  disabled={!canManageUsers || selectedOperator.status !== "active" || reviewPending || actionPending}
                  onClick={() => void submitStatus("disabled")}
                  title={operatorHelpText.disable}
                  type="button"
                >
                  <UserX size={17} />
                  Disable
                </button>
                <button
                  className="secondaryAction dangerAction"
                  disabled={!canManageUsers || editingDeleted || reviewPending || actionPending}
                  onClick={() => void submitStatus("deleted")}
                  title={operatorHelpText.delete}
                  type="button"
                >
                  <Trash2 size={17} />
                  Delete
                </button>
              </>
            ) : (
              <button
                className="secondaryAction"
                disabled={!canManageUsers || reviewPending || actionPending}
                onClick={() => void submitCreate()}
                title={operatorHelpText.create}
                type="button"
              >
                <UserPlus size={17} />
                Create
              </button>
            )}
          </div>
        </div>
      </section>
      )}
      <ConfirmationPrompt
        confirmLabel={pendingUserActionLabel(pendingAction)}
        detail={pendingUserActionDetail(pendingAction)}
        items={pendingUserActionItems(pendingAction)}
        onCancel={() => setPendingAction(null)}
        onConfirm={() => void confirmUserAction()}
        open={pendingAction !== null}
        pending={actionPending}
        title={pendingAction?.adminRisk ? "Confirm admin user action" : "Confirm user action"}
        tone={pendingAction?.adminRisk ? "danger" : "normal"}
      />
    </div>
  );
}

function SystemSessionsPanel({
  authEvents,
  onOpenPrivilegeUnlock,
  onRevokeOperatorSession,
  privilegeMaterial,
  sessions,
}: {
  authEvents: OperatorAuthEventRecord[];
  onOpenPrivilegeUnlock: () => void;
  onRevokeOperatorSession: (
    sessionId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  privilegeMaterial: PrivilegeMaterial | null;
  sessions: OperatorSessionRecord[];
}) {
  const [pendingRevoke, setPendingRevoke] = useState<PendingSessionRevoke | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [authFilter, setAuthFilter] = useState<"all" | "failures" | "success" | "suspicious">("all");
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const sessionEnrichment = useMemo(
    () => buildSessionEnrichmentMap(sessions, authEvents),
    [authEvents, sessions],
  );
  const failureGroups = useMemo(() => buildAuthFailureGroups(authEvents), [authEvents]);
  const filteredAuthEvents = useMemo(
    () => authEvents.filter((event) => authEventMatchesFilter(event, authFilter, failureGroups)),
    [authEvents, authFilter, failureGroups],
  );
  const activeSessions = sessions.filter((session) => !session.revoked);
  const currentSessions = activeSessions.filter((session) => session.current).length;
  const revokableSessions = activeSessions.filter((session) => !session.current).length;
  const adminSessions = activeSessions.filter((session) => session.operator_role === "admin").length;
  const enrichedSessions = activeSessions.filter((session) => sessionEnrichment[session.id]?.authEvent).length;
  const suspiciousSessions = activeSessions.filter(
    (session) => sessionEnrichment[session.id]?.riskTone === "warning" ||
      sessionEnrichment[session.id]?.riskTone === "critical",
  ).length;
  const authFailureCount = authEvents.filter((event) => event.result !== "success").length;
  const uniqueRemoteIps = new Set(
    authEvents.map((event) => event.remote_ip).filter((ip): ip is string => Boolean(ip)),
  ).size;
  const sessionColumns = useMemo<ConsoleDataGridColumn<OperatorSessionRecord>[]>(
    () => [
      {
        id: "operator",
        header: "User",
        cell: (row) => (
          <span className="sessionIdentityCell">
            <strong>{row.operator_username}</strong>
            <small>{shortId(row.id)}</small>
          </span>
        ),
        searchValue: (row) => `${row.operator_username} ${row.id}`,
        sortValue: (row) => row.operator_username,
      },
      {
        id: "role",
        header: "Role",
        cell: (row) => <span className={`status ${row.operator_role === "admin" ? "warn" : "neutral"}`}>{row.operator_role}</span>,
        searchValue: (row) => row.operator_role,
      },
      {
        id: "network",
        header: "IP / location",
        cell: (row) => <SessionNetworkCell enrichment={sessionEnrichment[row.id]} />,
        searchValue: (row) => `${sessionEnrichment[row.id]?.remoteIp ?? ""} ${sessionEnrichment[row.id]?.location ?? ""}`,
      },
      {
        id: "device",
        header: "Browser / device",
        cell: (row) => <SessionDeviceCell enrichment={sessionEnrichment[row.id]} />,
        searchValue: (row) => `${sessionEnrichment[row.id]?.browser ?? ""} ${sessionEnrichment[row.id]?.device ?? ""} ${sessionEnrichment[row.id]?.userAgent ?? ""}`,
      },
      {
        id: "created",
        header: "Created",
        cell: (row) => <SessionDateCell value={row.created_at} />,
        sortValue: (row) => row.created_at,
      },
      {
        id: "access",
        header: "Access expires",
        cell: (row) => <SessionDateCell help={operatorHelpText.sessionAccessExpires} value={row.expires_at} />,
        sortValue: (row) => row.expires_at,
      },
      {
        id: "refresh",
        header: "Refresh expires",
        cell: (row) => <SessionDateCell help={operatorHelpText.sessionRefreshExpires} value={row.refresh_expires_at} />,
        sortValue: (row) => row.refresh_expires_at,
      },
      {
        id: "state",
        header: "State",
        cell: (row) => <span className={`status ${row.current ? "info" : row.revoked ? "warn" : "ok"}`}>{row.current ? "current" : row.revoked ? "revoked" : "active"}</span>,
        searchValue: (row) => row.current ? "current" : row.revoked ? "revoked" : "active",
      },
      {
        id: "risk",
        header: "Risk",
        cell: (row) => <SessionRiskCell enrichment={sessionEnrichment[row.id]} />,
        searchValue: (row) => `${sessionEnrichment[row.id]?.riskLabel ?? ""} ${sessionEnrichment[row.id]?.riskDetail ?? ""}`,
      },
      {
        id: "revoke",
        header: "Revoke",
        cell: (row) => (
          <button
            aria-label={`Revoke session for ${row.operator_username}`}
            className="secondaryAction compactAction sessionInlineRevoke"
            disabled={reviewPending || pending || row.current || row.revoked}
            onClick={(event) => {
              event.stopPropagation();
              void requestSessionRevoke([row]);
            }}
            title={
              row.current
                ? "Current session cannot be revoked from this table."
                : row.revoked
                  ? "Session is already revoked."
                  : `Revoke ${row.operator_username}'s bearer session.`
            }
            type="button"
          >
            <UserX size={15} />
            <span>Revoke</span>
          </button>
        ),
      },
    ],
    [pending, reviewPending, sessionEnrichment],
  );
  const eventColumns = useMemo<ConsoleDataGridColumn<OperatorAuthEventRecord>[]>(
    () => [
      { id: "time", header: "Time", cell: (row) => <SessionDateCell value={row.created_at} />, sortValue: (row) => row.created_at },
      { id: "username", header: "Username", cell: (row) => row.username, searchValue: (row) => row.username },
      {
        id: "result",
        header: "Result",
        cell: (row) => <span className={`status ${authEventRiskTone(row)}`}>{row.result}</span>,
        searchValue: (row) => row.result,
      },
      { id: "reason", header: "Reason", cell: (row) => <AuthReasonCell event={row} failureGroups={failureGroups} />, searchValue: (row) => row.reason },
      { id: "remote", header: "Remote IP", cell: (row) => row.remote_ip ?? "not recorded", searchValue: (row) => row.remote_ip },
      { id: "agent", header: "Browser / device", cell: (row) => <AuthUserAgentCell event={row} />, searchValue: (row) => row.user_agent },
      { id: "session", header: "Session", cell: (row) => shortId(row.session_id), searchValue: (row) => row.session_id },
    ],
    [failureGroups],
  );

  useEffect(() => {
    setPendingRevoke(null);
    setReviewPending(false);
    invalidateReviewGeneration();
  }, [sessions, invalidateReviewGeneration]);

  async function requestSessionRevoke(rows: OperatorSessionRecord[]) {
    const sessionsToRevoke = rows.filter((session) => !session.current && !session.revoked);
    if (sessionsToRevoke.length === 0) {
      return;
    }
    if (!privilegeMaterial) {
      setError("Local privilege unlock is required");
      onOpenPrivilegeUnlock();
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    const adminRisk = sessionsToRevoke.some((session) => session.operator_role === "admin");
    setReviewPending(true);
    setError(null);
    try {
      await waitForReviewRender();
      const privileges = Object.fromEntries(
        await Promise.all(
          sessionsToRevoke.map(async (session) => {
            const payloadHashHex = await operatorDbPayloadHashHex({
              action: "operator_session.revoke",
              target: session.id,
              adminRiskAcknowledged: adminRisk,
            });
            const privilegeAssertion = await buildPrivilegeAssertion({
              intent: canonicalDbPrivilegeIntent({
                action: "operator_session.revoke",
                confirmed: true,
                payloadHash: payloadHashHex,
                resolvedTargets: [session.id],
                target: session.id,
              }),
              privilegeMaterial,
            });
            return [session.id, { payloadHashHex, privilegeAssertion }];
          }),
        ),
      );
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingRevoke({
        sessions: sessionsToRevoke,
        adminRisk,
        privileges,
      });
    } catch (actionError) {
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setError(actionError instanceof Error ? actionError.message : "Privilege assertion failed");
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewPending(false);
      }
    }
  }

  async function confirmSessionRevoke() {
    if (!pendingRevoke || pendingRevoke.sessions.length === 0) {
      return;
    }
    setPending(true);
    setError(null);
    try {
      for (const session of pendingRevoke.sessions) {
        await onRevokeOperatorSession(
          session.id,
          pendingRevoke.adminRisk,
          pendingRevoke.privileges[session.id].privilegeAssertion,
        );
      }
      setPendingRevoke(null);
    } catch (actionError) {
      setError(actionError instanceof Error ? actionError.message : "Session revoke failed");
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <section className="controlPanel sessionSecurityOverview" aria-label="Session evidence security overview">
          <div className="sectionHeader fleetInstancesHeader">
            <div>
              <h2>Session security posture</h2>
              <span>Active bearer sessions, revocation readiness, IP/device evidence, and suspicious authentication signals.</span>
            </div>
            <ConsoleStatusBadge tone={suspiciousSessions > 0 || failureGroups.length > 0 ? "warning" : "ok"}>
              {suspiciousSessions > 0 || failureGroups.length > 0 ? "Needs review" : "Healthy"}
            </ConsoleStatusBadge>
          </div>
          <div className="systemPostureGrid sessionPostureGrid">
            <SystemPostureTile
              detail={`${currentSessions} current browser session; ${revokableSessions} non-current sessions can be revoked without ending the current console session.`}
              icon={<Activity size={18} />}
              label="Active sessions"
              tone={activeSessions.length > 0 ? "info" : "neutral"}
              value={`${activeSessions.length} active`}
            />
            <SystemPostureTile
              detail="Admin sessions are higher-risk and require admin-risk acknowledgement when revoked."
              icon={<ShieldCheck size={18} />}
              label="Admin sessions"
              tone={adminSessions > 0 ? "warning" : "ok"}
              value={`${adminSessions} admin`}
            />
            <SystemPostureTile
              detail={`${enrichedSessions} active sessions have login-event IP and user-agent evidence; missing evidence is flagged per row.`}
              icon={<Network size={18} />}
              label="IP/device evidence"
              tone={enrichedSessions === activeSessions.length ? "ok" : "warning"}
              value={`${enrichedSessions}/${activeSessions.length} enriched`}
            />
            <SystemPostureTile
              detail={`${uniqueRemoteIps} remote IPs are visible. Geo lookup and impossible-travel detection need backend location enrichment.`}
              icon={<ServerCog size={18} />}
              label="Location enrichment"
              tone="warning"
              value="Geo not exposed"
            />
            <SystemPostureTile
              detail={`${authFailureCount} non-success authentication events across ${failureGroups.length} grouped failure patterns.`}
              icon={<AlertTriangle size={18} />}
              label="Suspicious auth"
              tone={authFailureCount > 0 ? "warning" : "ok"}
              value={authFailureCount === 1 ? "1 failure" : `${authFailureCount} failures`}
            />
            <SystemPostureTile
              detail="The table exposes row-level revoke actions and still supports bulk revoke from selected rows."
              icon={<UserX size={18} />}
              label="Revocation"
              tone={revokableSessions > 0 ? "info" : "ok"}
              value={`${revokableSessions} revokable`}
            />
          </div>
        </section>
        <section className="controlPanel">
          <div className="sectionHeader compact">
            <h2>Sessions</h2>
            <span>{reviewPending ? "Preparing review" : `${sessions.length} recent sessions`}</span>
          </div>
          {error && <div className="panelError">{error}</div>}
          <ConsoleDataGrid
            actions={[
              {
                label: "Revoke selected",
                description: (rows) =>
                  rows.length === 1
                    ? `Revoke the bearer session for ${rows[0].operator_username}.`
                    : `Revoke ${rows.length} selected bearer sessions.`,
                tone: "danger",
                icon: <UserX size={14} />,
                disabled: (rows) => reviewPending || pending || rows.length === 0 || rows.some((row) => row.current || row.revoked),
                onSelect: (rows) => void requestSessionRevoke(rows),
              },
            ]}
            columns={sessionColumns}
            defaultPageSize={12}
            empty="No operator sessions"
            expandOnRowClick
            getRowId={(row) => row.id}
            itemLabel="sessions"
            renderExpandedRow={(row) => <SessionDetailGrid enrichment={sessionEnrichment[row.id]} session={row} />}
            renderSelectionPanel={(rows) => (
              <SessionSelectionPanel rows={rows} />
            )}
            rowActions={[
              {
                label: "Revoke session",
                description: (rows) =>
                  `Revoke the bearer session for ${rows[0].operator_username}.`,
                tone: "danger",
                icon: <UserX size={14} />,
                disabled: (rows) =>
                  reviewPending ||
                  pending ||
                  rows.length === 0 ||
                  rows.some((row) => row.current || row.revoked),
                onSelect: (rows) => void requestSessionRevoke(rows),
              },
            ]}
            rows={sessions}
            searchPlaceholder="Search user, role, IP, browser, device, state, or risk"
            singleExpandedRow
            storageKey="vpsman.system.sessions"
            title="Sessions"
          />
        </section>
        <section className="controlPanel">
          <div className="sectionHeader compact sessionAuthHeader">
            <div>
              <h2>Authentication history</h2>
              <span>{filteredAuthEvents.length} of {authEvents.length} login results</span>
            </div>
            <div className="segmented sessionAuthFilters" role="group" aria-label="Authentication history filters">
              {[
                { label: "All", value: "all" },
                { label: "Failures", value: "failures" },
                { label: "Success", value: "success" },
                { label: "Suspicious", value: "suspicious" },
              ].map((option) => (
                <button
                  aria-pressed={authFilter === option.value}
                  className={authFilter === option.value ? "active" : ""}
                  key={option.value}
                  onClick={() => setAuthFilter(option.value as typeof authFilter)}
                  type="button"
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>
          {failureGroups.length > 0 ? (
            <div className="authFailureGroups" aria-label="Authentication failure groups">
              <div className="dashboardSideRailHeader">
                <strong>Grouped failures</strong>
                <span>{failureGroups.length} patterns</span>
              </div>
              {failureGroups.map((group) => (
                <div className="authFailureGroupRow" key={group.key}>
                  <span className="status warn">{group.riskLabel}</span>
                  <strong>{group.username}</strong>
                  <b>{group.count === 1 ? "1 attempt" : `${group.count} attempts`}</b>
                  <p>
                    {group.reason} from {group.remoteIp}; last {formatTime(group.lastAt)}. {group.riskDetail}
                  </p>
                </div>
              ))}
            </div>
          ) : null}
          <ConsoleDataGrid
            columns={eventColumns}
            defaultPageSize={12}
            empty="No authentication events"
            expandOnRowClick
            getRowId={(row) => row.id}
            itemLabel="events"
            renderExpandedRow={(row) => <AuthEventDetailGrid event={row} />}
            rows={filteredAuthEvents}
            searchPlaceholder="Search username, result, reason, remote IP, browser, device, or session"
            selectable={false}
            singleExpandedRow
            storageKey="vpsman.system.authEvents"
            title="Authentication history"
          />
        </section>
      </div>
      <ConfirmationPrompt
        confirmLabel={(pendingRevoke?.sessions.length ?? 0) === 1 ? "Revoke session" : "Revoke sessions"}
        detail={
          pendingRevoke?.sessions.some((session) => session.operator_role === "admin")
            ? "This revokes an admin user's bearer session. Existing browser state for that session will stop working after the current access token expires or is checked again."
            : (pendingRevoke?.sessions.length ?? 0) === 1
              ? "This revokes the selected bearer session."
              : "This revokes the selected bearer sessions."
        }
        items={[
          { label: "Sessions", value: pendingRevoke?.sessions.length ?? 0 },
          { label: "Users", value: pendingRevoke?.sessions.map((session) => session.operator_username).join(", ") || "-" },
          { label: "Admin sessions", value: pendingRevoke?.sessions.filter((session) => session.operator_role === "admin").length ?? 0 },
          {
            label: "Payload",
            value: pendingRevoke?.sessions[0]
              ? shortId(pendingRevoke.privileges[pendingRevoke.sessions[0].id].payloadHashHex)
              : "-",
          },
        ]}
        onCancel={() => setPendingRevoke(null)}
        onConfirm={() => void confirmSessionRevoke()}
        open={Boolean(pendingRevoke)}
        pending={pending}
        title={pendingRevoke?.sessions.some((session) => session.operator_role === "admin") ? "Confirm admin session revoke" : "Confirm session revoke"}
        tone="danger"
      />
    </div>
  );
}

function OperatorAccessEvidencePanel({
  onRevokeSessions,
  operator,
  pending,
  summary,
  userCanManage,
}: {
  onRevokeSessions: () => void;
  operator: OperatorView;
  pending: boolean;
  summary: OperatorAccessSummary;
  userCanManage: boolean;
}) {
  const ttlDays = secondsToDays(operator.session_refresh_ttl_secs);
  const adminTtlRisk = operator.role === "admin" && ttlDays > 30;
  return (
    <section className="operatorAccessEvidencePanel" aria-label="Operator access evidence">
      <div className="dashboardSideRailHeader">
        <strong>Access evidence</strong>
        <span>{operator.username}</span>
      </div>
      <div className="operatorEvidenceGrid">
        <OperatorEvidenceTile
          label="MFA"
          tone={operator.totp_enabled ? "ok" : operator.role === "admin" ? "warning" : "neutral"}
          value={operator.totp_enabled ? "enabled" : operator.role === "admin" ? "Admin MFA is off" : "off"}
        />
        <OperatorEvidenceTile
          label="Refresh TTL"
          tone={adminTtlRisk ? "warning" : "neutral"}
          value={`${ttlDays}d${adminTtlRisk ? " admin risk" : ""}`}
        />
        <OperatorEvidenceTile
          label="Active sessions"
          tone={summary.activeSessions > 0 ? "info" : "neutral"}
          value={`${summary.activeSessions}`}
        />
        <OperatorEvidenceTile
          label="Failed logins"
          tone={summary.failedLogins > 0 ? "warning" : "ok"}
          value={`${summary.failedLogins}`}
        />
        <OperatorEvidenceTile
          label="Last login"
          value={formatAuthEventTime(summary.lastLogin)}
        />
        <OperatorEvidenceTile
          label="Lifecycle"
          tone={operator.status === "active" ? "ok" : "warning"}
          value={operatorLifecycleLabel(operator)}
        />
        <OperatorEvidenceTile
          label="Password age"
          tone="warning"
          value="not exposed"
        />
        <OperatorEvidenceTile
          label="API tokens"
          tone="warning"
          value="not exposed"
        />
      </div>
      <div className="operatorEvidenceFooter">
        <span>
          Invite and locked states are not separate backend fields; the visible lifecycle is active, disabled, or deleted.
        </span>
        <button
          className="secondaryAction compactAction"
          disabled={!userCanManage || pending || summary.revokableSessions.length === 0}
          onClick={onRevokeSessions}
          title={
            summary.revokableSessions.length > 0
              ? `Revoke ${summary.revokableSessions.length} non-current active sessions for ${operator.username}.`
              : "No non-current active sessions are available to revoke for this user."
          }
          type="button"
        >
          <UserX size={16} />
          <span>Revoke sessions</span>
        </button>
      </div>
    </section>
  );
}

function OperatorEvidenceTile({
  label,
  tone = "neutral",
  value,
}: {
  label: string;
  tone?: SystemHealthTone;
  value: string;
}) {
  return (
    <span className={`operatorEvidenceTile ${tone}`}>
      <small>{label}</small>
      <strong>{value}</strong>
    </span>
  );
}

function OperatorDetailGrid({
  operator,
  summary,
}: {
  operator: OperatorView;
  summary?: OperatorAccessSummary;
}) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>User</strong>
        <span>{operator.username}</span>
      </span>
      <span>
        <strong>ID</strong>
        <span className="monoValue">{operator.id}</span>
      </span>
      <span>
        <strong>Status</strong>
        <span>{operator.status}</span>
      </span>
      <span>
        <strong>Role</strong>
        <span>{operator.role}</span>
      </span>
      <span>
        <strong>Session TTL</strong>
        <span>{secondsToDays(operator.session_refresh_ttl_secs)}d</span>
      </span>
      <span>
        <strong>TOTP</strong>
        <span>{operator.totp_enabled ? "enabled" : "off"}</span>
      </span>
      <span>
        <strong>Active sessions</strong>
        <span>{summary?.activeSessions ?? 0}</span>
      </span>
      <span>
        <strong>Last login</strong>
        <span>{formatAuthEventTime(summary?.lastLogin ?? null)}</span>
      </span>
      <span>
        <strong>Failed logins</strong>
        <span>{summary?.failedLogins ?? 0}</span>
      </span>
      <span>
        <strong>Scopes</strong>
        <span>{operator.scopes.length > 0 ? operator.scopes.join(", ") : "role defaults"}</span>
      </span>
      <span>
        <strong>Created</strong>
        <span>{formatTime(operator.created_at)}</span>
      </span>
      <span>
        <strong>Disabled</strong>
        <span>{operator.disabled_at ? formatTime(operator.disabled_at) : "no"}</span>
      </span>
      <span>
        <strong>Deleted</strong>
        <span>{operator.deleted_at ? formatTime(operator.deleted_at) : "no"}</span>
      </span>
      <span>
        <strong>Password age</strong>
        <span>not exposed</span>
      </span>
      <span>
        <strong>Invite / lock</strong>
        <span>not exposed separately</span>
      </span>
      <span>
        <strong>API tokens</strong>
        <span>not exposed</span>
      </span>
    </div>
  );
}

function OperatorSelectionPanel({ rows }: { rows: OperatorView[] }) {
  const adminCount = rows.filter((operator) => operator.role === "admin").length;
  const activeCount = rows.filter((operator) => operator.status === "active").length;
  const totpCount = rows.filter((operator) => operator.totp_enabled).length;
  return (
    <div className="gridSelectionSummary">
      <span>
        <strong>{rows.length}</strong>
        selected
      </span>
      <span>
        <strong>{adminCount}</strong>
        admin
      </span>
      <span>
        <strong>{activeCount}</strong>
        active
      </span>
      <span>
        <strong>{totpCount}</strong>
        TOTP
      </span>
    </div>
  );
}

function SessionDateCell({ help, value }: { help?: string; value: string }) {
  const date = new Date(value);
  const dateLabel = Number.isNaN(date.getTime())
    ? value
    : date.toLocaleDateString(undefined, {
        day: "2-digit",
        month: "short",
        year: "numeric",
      });
  const timeLabel = Number.isNaN(date.getTime())
    ? ""
    : date.toLocaleTimeString(undefined, {
        hour: "2-digit",
        minute: "2-digit",
        timeZoneName: "short",
      });
  return (
    <span className="sessionDateCell" title={help ? `${help} ${value}` : value}>
      <strong>{dateLabel}</strong>
      {timeLabel ? <small>{timeLabel}</small> : null}
    </span>
  );
}

function SessionNetworkCell({ enrichment }: { enrichment?: SessionEnrichment }) {
  const remoteIp = enrichment?.remoteIp ?? "IP not recorded";
  const location = enrichment?.location ?? "Geo not exposed";
  return (
    <span className="sessionNetworkCell" title={`${remoteIp} / ${location}`}>
      <strong>{remoteIp}</strong>
      <small>{location}</small>
    </span>
  );
}

function SessionDeviceCell({ enrichment }: { enrichment?: SessionEnrichment }) {
  return (
    <span className="sessionDeviceCell" title={enrichment?.userAgent ?? "Login user agent not recorded"}>
      <strong>{enrichment?.browser ?? "Unknown browser"}</strong>
      <small>{enrichment?.device ?? "Device not recorded"}</small>
    </span>
  );
}

function SessionRiskCell({ enrichment }: { enrichment?: SessionEnrichment }) {
  return (
    <span className={`status ${enrichment?.riskTone ?? "neutral"}`} title={enrichment?.riskDetail}>
      {enrichment?.riskLabel ?? "No login evidence"}
    </span>
  );
}

function AuthReasonCell({
  event,
  failureGroups,
}: {
  event: OperatorAuthEventRecord;
  failureGroups: AuthFailureGroup[];
}) {
  const repeated = failureGroups.find((group) => authFailureGroupKey(event) === group.key && group.count > 1);
  const reason = event.reason ?? "accepted";
  return (
    <span className="authReasonCell" title={`${reason} - ${authEventRiskLabel(event)}`}>
      <strong>{reason}</strong>
      {repeated ? <small>{repeated.count} repeated attempts</small> : <small>{authEventRiskLabel(event)}</small>}
    </span>
  );
}

function AuthUserAgentCell({ event }: { event: OperatorAuthEventRecord }) {
  const parsed = parseUserAgent(event.user_agent);
  return (
    <span className="sessionDeviceCell" title={event.user_agent ?? "User agent not recorded"}>
      <strong>{parsed.browser}</strong>
      <small>{parsed.device}</small>
    </span>
  );
}

function SessionDetailGrid({
  enrichment,
  session,
}: {
  enrichment?: SessionEnrichment;
  session: OperatorSessionRecord;
}) {
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>User</strong>
        <span>{session.operator_username}</span>
      </span>
      <span>
        <strong>Session ID</strong>
        <span className="monoValue">{session.id}</span>
      </span>
      <span>
        <strong>Operator ID</strong>
        <span className="monoValue">{session.operator_id}</span>
      </span>
      <span>
        <strong>Role</strong>
        <span>{session.operator_role}</span>
      </span>
      <span>
        <strong>Remote IP</strong>
        <span>{enrichment?.remoteIp ?? "not recorded"}</span>
      </span>
      <span>
        <strong>Location</strong>
        <span>{enrichment?.location ?? "Geo not exposed"}</span>
      </span>
      <span>
        <strong>Browser</strong>
        <span>{enrichment?.browser ?? "Unknown browser"}</span>
      </span>
      <span>
        <strong>Device</strong>
        <span>{enrichment?.device ?? "Device not recorded"}</span>
      </span>
      <span>
        <strong>User agent</strong>
        <span>{enrichment?.userAgent ?? "not recorded"}</span>
      </span>
      <span>
        <strong>Risk</strong>
        <span>{enrichment?.riskLabel ?? "No login evidence"}</span>
      </span>
      <span>
        <strong>State</strong>
        <span>{session.current ? "current" : session.revoked ? "revoked" : "active"}</span>
      </span>
      <span>
        <strong>Created</strong>
        <span>{formatTime(session.created_at)}</span>
      </span>
      <span>
        <strong>Access expires</strong>
        <span>{formatTime(session.expires_at)}</span>
      </span>
      <span>
        <strong>Refresh expires</strong>
        <span>{formatTime(session.refresh_expires_at)}</span>
      </span>
      <span>
        <strong>Revoked</strong>
        <span>{session.revoked_at ? formatTime(session.revoked_at) : "no"}</span>
      </span>
    </div>
  );
}

function SessionSelectionPanel({ rows }: { rows: OperatorSessionRecord[] }) {
  const revokable = rows.filter((session) => !session.current && !session.revoked).length;
  const current = rows.filter((session) => session.current).length;
  const revoked = rows.filter((session) => session.revoked).length;
  return (
    <div className="gridSelectionSummary">
      <span>
        <strong>{rows.length}</strong>
        selected
      </span>
      <span>
        <strong>{revokable}</strong>
        revokable
      </span>
      <span>
        <strong>{current}</strong>
        current
      </span>
      <span>
        <strong>{revoked}</strong>
        revoked
      </span>
    </div>
  );
}

function AuthEventDetailGrid({ event }: { event: OperatorAuthEventRecord }) {
  const parsed = parseUserAgent(event.user_agent);
  return (
    <div className="consoleInlineDetailGrid">
      <span>
        <strong>Time</strong>
        <span>{formatTime(event.created_at)}</span>
      </span>
      <span>
        <strong>User</strong>
        <span>{event.username}</span>
      </span>
      <span>
        <strong>Operator ID</strong>
        <span className="monoValue">{event.operator_id ?? "n/a"}</span>
      </span>
      <span>
        <strong>Result</strong>
        <span>{event.result}</span>
      </span>
      <span>
        <strong>Reason</strong>
        <span>{event.reason ?? "-"}</span>
      </span>
      <span>
        <strong>Remote IP</strong>
        <span>{event.remote_ip ?? "-"}</span>
      </span>
      <span>
        <strong>Risk</strong>
        <span>{authEventRiskLabel(event)}</span>
      </span>
      <span>
        <strong>Browser</strong>
        <span>{parsed.browser}</span>
      </span>
      <span>
        <strong>Device</strong>
        <span>{parsed.device}</span>
      </span>
      <span>
        <strong>Session</strong>
        <span className="monoValue">{event.session_id ?? "-"}</span>
      </span>
      <span>
        <strong>User agent</strong>
        <span>{event.user_agent ?? "-"}</span>
      </span>
    </div>
  );
}

function buildOperatorAccessSummaries(
  operators: OperatorView[],
  sessions: OperatorSessionRecord[],
  authEvents: OperatorAuthEventRecord[],
): Record<string, OperatorAccessSummary> {
  const summaries: Record<string, OperatorAccessSummary> = {};
  for (const operator of operators) {
    summaries[operator.id] = {
      activeSessions: 0,
      failedLogins: 0,
      lastFailure: null,
      lastLogin: null,
      revokableSessions: [],
    };
  }
  for (const session of sessions) {
    const summary = summaries[session.operator_id];
    if (!summary || session.revoked) {
      continue;
    }
    summary.activeSessions += 1;
    if (!session.current) {
      summary.revokableSessions.push(session);
    }
  }
  const sortedEvents = [...authEvents].sort(
    (left, right) => Date.parse(right.created_at) - Date.parse(left.created_at),
  );
  for (const event of sortedEvents) {
    const operator = operators.find(
      (candidate) =>
        candidate.id === event.operator_id ||
        candidate.username === event.username,
    );
    if (!operator) {
      continue;
    }
    const summary = summaries[operator.id];
    if (!summary) {
      continue;
    }
    if (event.result === "success") {
      summary.lastLogin ??= event;
    } else {
      summary.failedLogins += 1;
      summary.lastFailure ??= event;
    }
  }
  return summaries;
}

function buildSessionEnrichmentMap(
  sessions: OperatorSessionRecord[],
  authEvents: OperatorAuthEventRecord[],
): Record<string, SessionEnrichment> {
  const loginEventsBySession = new Map(
    authEvents
      .filter((event) => event.session_id && event.result === "success")
      .sort((left, right) => Date.parse(right.created_at) - Date.parse(left.created_at))
      .map((event) => [event.session_id as string, event]),
  );
  return Object.fromEntries(
    sessions.map((session) => {
      const authEvent = loginEventsBySession.get(session.id) ?? null;
      const parsed = parseUserAgent(authEvent?.user_agent ?? null);
      const remoteIp = authEvent?.remote_ip ?? "IP not recorded";
      const risk = sessionRisk(session, authEvent);
      return [
        session.id,
        {
          authEvent,
          browser: parsed.browser,
          device: parsed.device,
          location: authEvent?.remote_ip ? "Geo not exposed" : "Location not recorded",
          remoteIp,
          riskDetail: risk.detail,
          riskLabel: risk.label,
          riskTone: risk.tone,
          userAgent: authEvent?.user_agent ?? "User agent not recorded",
        },
      ];
    }),
  );
}

function buildAuthFailureGroups(authEvents: OperatorAuthEventRecord[]): AuthFailureGroup[] {
  const groups = new Map<string, AuthFailureGroup>();
  for (const event of authEvents) {
    if (event.result === "success") {
      continue;
    }
    const key = authFailureGroupKey(event);
    const existing = groups.get(key);
    if (existing) {
      existing.count += 1;
      if (Date.parse(event.created_at) < Date.parse(existing.firstAt)) {
        existing.firstAt = event.created_at;
      }
      if (Date.parse(event.created_at) > Date.parse(existing.lastAt)) {
        existing.lastAt = event.created_at;
      }
      continue;
    }
    const risk = authFailureRisk(event, 1);
    groups.set(key, {
      count: 1,
      firstAt: event.created_at,
      key,
      lastAt: event.created_at,
      reason: event.reason ?? event.result,
      remoteIp: event.remote_ip ?? "IP not recorded",
      riskDetail: risk.detail,
      riskLabel: risk.label,
      username: event.username,
      userAgent: event.user_agent ?? "User agent not recorded",
    });
  }
  for (const group of groups.values()) {
    const risk = authFailureRisk(
      {
        created_at: group.lastAt,
        id: group.key,
        operator_id: null,
        reason: group.reason,
        remote_ip: group.remoteIp === "IP not recorded" ? null : group.remoteIp,
        result: "failure",
        session_id: null,
        user_agent: group.userAgent === "User agent not recorded" ? null : group.userAgent,
        username: group.username,
      },
      group.count,
    );
    group.riskLabel = risk.label;
    group.riskDetail = risk.detail;
  }
  return Array.from(groups.values()).sort((left, right) => Date.parse(right.lastAt) - Date.parse(left.lastAt));
}

function authEventMatchesFilter(
  event: OperatorAuthEventRecord,
  filter: "all" | "failures" | "success" | "suspicious",
  failureGroups: AuthFailureGroup[],
): boolean {
  if (filter === "all") {
    return true;
  }
  if (filter === "success") {
    return event.result === "success";
  }
  if (filter === "failures") {
    return event.result !== "success";
  }
  return authEventRiskTone(event, failureGroups) === "warning" || authEventRiskTone(event, failureGroups) === "critical";
}

function authFailureGroupKey(event: OperatorAuthEventRecord): string {
  return [
    event.username || "unknown",
    event.remote_ip || "no-ip",
    event.reason || event.result || "failure",
  ].join("|");
}

function authEventRiskTone(
  event: OperatorAuthEventRecord,
  failureGroups: AuthFailureGroup[] = [],
): SystemHealthTone {
  if (event.result === "success") {
    return event.remote_ip && event.user_agent ? "ok" : "warning";
  }
  const group = failureGroups.find((item) => item.key === authFailureGroupKey(event));
  if (event.result === "throttled" || (group?.count ?? 0) >= 5) {
    return "critical";
  }
  return "warning";
}

function authEventRiskLabel(event: OperatorAuthEventRecord): string {
  if (event.result === "success") {
    return event.remote_ip && event.user_agent ? "Accepted login" : "Accepted; evidence incomplete";
  }
  if (!event.operator_id) {
    return "Unknown user";
  }
  if (event.result === "throttled") {
    return "Throttled";
  }
  return "Failed login";
}

function authFailureRisk(
  event: Pick<OperatorAuthEventRecord, "operator_id" | "reason" | "result" | "username"> & Partial<OperatorAuthEventRecord>,
  count: number,
): { detail: string; label: string } {
  if (event.result === "throttled" || count >= 5) {
    return {
      detail: "High-volume or throttled authentication failures need investigation.",
      label: "High-risk failure",
    };
  }
  if (!event.operator_id) {
    return {
      detail: "No operator matched this username; verify whether this is mistyped access or credential probing.",
      label: count > 1 ? "Repeated unknown user" : "Unknown user",
    };
  }
  return {
    detail: "Failure maps to a known operator; review reason, IP, and user agent.",
    label: count > 1 ? "Repeated failure" : "Failed login",
  };
}

function sessionRisk(
  session: OperatorSessionRecord,
  authEvent: OperatorAuthEventRecord | null,
): { detail: string; label: string; tone: SystemHealthTone } {
  if (session.revoked) {
    return { detail: "This session has already been revoked.", label: "Revoked", tone: "neutral" };
  }
  if (!authEvent) {
    return {
      detail: "No matching successful login event was loaded for this session.",
      label: "Missing login evidence",
      tone: "warning",
    };
  }
  if (!authEvent.remote_ip || !authEvent.user_agent) {
    return {
      detail: "The login event is missing IP or user-agent evidence.",
      label: "Evidence incomplete",
      tone: "warning",
    };
  }
  if (session.operator_role === "admin" && !session.current) {
    return {
      detail: "Non-current admin sessions should be reviewed and revoked when no longer needed.",
      label: "Admin review",
      tone: "warning",
    };
  }
  if (session.current) {
    return { detail: "This is the current console session.", label: "Current", tone: "info" };
  }
  return { detail: "Login event, IP, and user agent are present.", label: "Normal", tone: "ok" };
}

function parseUserAgent(userAgent: string | null): { browser: string; device: string } {
  if (!userAgent) {
    return { browser: "Unknown browser", device: "Device not recorded" };
  }
  const lower = userAgent.toLowerCase();
  let browser = "Unknown browser";
  if (lower.includes("playwright")) {
    browser = "Playwright";
  } else if (lower.includes("edg/")) {
    browser = "Edge";
  } else if (lower.includes("chrome/") || lower.includes("chromium/")) {
    browser = "Chrome";
  } else if (lower.includes("firefox/")) {
    browser = "Firefox";
  } else if (lower.includes("safari/")) {
    browser = "Safari";
  } else if (lower.includes("curl/")) {
    browser = "curl";
  }
  let device = "Desktop browser";
  if (lower.includes("mobile") || lower.includes("android") || lower.includes("iphone")) {
    device = "Mobile browser";
  } else if (lower.includes("playwright")) {
    device = "Browser automation";
  } else if (lower.includes("curl/")) {
    device = "CLI client";
  }
  return { browser, device };
}

function formatAuthEventTime(event: OperatorAuthEventRecord | null | undefined): string {
  return event ? formatTime(event.created_at) : "not recorded";
}

function operatorLifecycleLabel(operator: OperatorView): string {
  if (operator.deleted_at) {
    return `deleted ${formatTime(operator.deleted_at)}`;
  }
  if (operator.disabled_at) {
    return `disabled ${formatTime(operator.disabled_at)}`;
  }
  return operator.status;
}

function roleDescription(role: string): string {
  if (role === "admin") {
    return "Full operator control, user lifecycle, config, and privileged operations.";
  }
  if (role === "operator") {
    return "Day-to-day fleet operation with write scopes when granted.";
  }
  if (role === "viewer") {
    return "Read-oriented inspection role for dashboards, history, and evidence.";
  }
  return "Custom backend role visible in operator records.";
}

function parseScopeList(value: string): string[] {
  return value
    .split(/[,\s]+/)
    .map((scope) => scope.trim())
    .filter(Boolean)
    .filter((scope, index, scopes) => scopes.indexOf(scope) === index);
}

function addScopeToken(current: string, scope: string): string {
  const scopes = parseScopeList(current);
  if (!scopes.includes(scope)) {
    scopes.push(scope);
  }
  return scopes.join(", ");
}

function daysToSeconds(value: number): number {
  return Math.max(1, Math.min(3650, Math.round(value || defaultSessionTtlDays))) * 24 * 60 * 60;
}

function secondsToDays(value: number): number {
  return Math.max(1, Math.round(value / (24 * 60 * 60)));
}

function pendingUserActionLabel(action: PendingUserAction | null): string {
  if (!action) {
    return "Confirm";
  }
  switch (action.kind) {
    case "create":
      return "Create user";
    case "update":
      return "Save user";
    case "status":
      if (action.operators.length === 1) {
        return action.status === "active" ? "Enable user" : action.status === "disabled" ? "Disable user" : "Delete user";
      }
      return action.status === "active" ? "Enable users" : action.status === "disabled" ? "Disable users" : "Delete users";
    case "password":
      return "Reset password";
    case "totp":
      return action.operators.length === 1 ? "Clear TOTP" : "Clear TOTP secrets";
    case "sessions":
      return action.sessions.length === 1 ? "Revoke session" : "Revoke sessions";
  }
}

function pendingUserActionDetail(action: PendingUserAction | null): ReactNode {
  if (!action) {
    return "";
  }
  const adminDetail = action.adminRisk
    ? " This action targets or grants admin privileges; verify that the selected account and role change are intentional."
    : "";
  switch (action.kind) {
    case "create":
      return `Create an operator record for ${action.username}.${adminDetail}`;
    case "update":
      return `Update role, scopes, and session lifetime for ${action.operator.username}.${adminDetail}`;
    case "status":
      return `${pendingUserActionLabel(action)} for ${formatOperatorSelection(action.operators)}.${adminDetail}`;
    case "password":
      return `Replace the password, clear stored TOTP secret material, and revoke existing sessions for ${action.operator.username}.${adminDetail}`;
    case "totp":
      return `Clear stored TOTP secret material and revoke existing sessions for ${formatOperatorSelection(action.operators)}.${adminDetail}`;
    case "sessions":
      return `Revoke ${action.sessions.length} non-current active sessions for ${action.operator.username}.${adminDetail}`;
  }
}

function pendingUserActionItems(action: PendingUserAction | null): Array<{ label: string; value: ReactNode }> {
  if (!action) {
    return [];
  }
  if (action.kind === "create") {
    return [
      { label: "Username", value: action.username },
      { label: "Role", value: action.role },
      { label: "Session TTL", value: `${secondsToDays(action.sessionRefreshTtlSecs)}d` },
      { label: "Scopes", value: action.scopes.length ? action.scopes.join(", ") : "role defaults" },
      { label: "Payload", value: shortId(action.privilege.payloadHashHex) },
    ];
  }
  if (action.kind === "update") {
    return [
      { label: "Username", value: action.operator.username },
      { label: "Role", value: action.role },
      { label: "Session TTL", value: `${secondsToDays(action.sessionRefreshTtlSecs)}d` },
      { label: "Scopes", value: action.scopes.length ? action.scopes.join(", ") : "role defaults" },
      { label: "Payload", value: shortId(action.privilege.payloadHashHex) },
    ];
  }
  if (action.kind === "password") {
    return [
      { label: "Username", value: action.operator.username },
      { label: "Role", value: action.operator.role },
      { label: "Payload", value: shortId(action.privilege.payloadHashHex) },
    ];
  }
  if (action.kind === "status" || action.kind === "totp") {
    const firstOperator = action.operators[0];
    return [
      { label: action.operators.length === 1 ? "Username" : "Users", value: formatOperatorSelection(action.operators) },
      { label: "Count", value: action.operators.length },
      {
        label: "Payload",
        value: firstOperator ? shortId(action.privileges[firstOperator.id].payloadHashHex) : "-",
      },
    ];
  }
  if (action.kind === "sessions") {
    const firstSession = action.sessions[0];
    return [
      { label: "Username", value: action.operator.username },
      { label: "Sessions", value: action.sessions.length },
      { label: "Admin sessions", value: action.sessions.filter((session) => session.operator_role === "admin").length },
      {
        label: "Payload",
        value: firstSession ? shortId(action.privileges[firstSession.id].payloadHashHex) : "-",
      },
    ];
  }
  return [];
}

function formatOperatorSelection(operators: OperatorView[]): string {
  if (operators.length === 0) {
    return "-";
  }
  const names = operators.map((operator) => operator.username);
  if (names.length <= 4) {
    return names.join(", ");
  }
  return `${names.slice(0, 4).join(", ")} +${names.length - 4} more`;
}

function SystemDashboardPanel({
  dashboard,
  error,
  loading,
  onPointDensityChange,
  onRefresh,
  onWindowChange,
  pointDensity,
  window,
}: {
  dashboard: SystemDashboardRecord | null;
  error: string | null;
  loading: boolean;
  onPointDensityChange: (density: SystemDashboardPointDensity) => void;
  onRefresh: () => void;
  onWindowChange: (window: SystemDashboardWindow) => void;
  pointDensity: SystemDashboardPointDensity;
  window: SystemDashboardWindow;
}) {
  const series = dashboard?.series ?? [];
  const dbPressure = dashboard?.current.db_pool.max_connections
    ? dashboard.current.db_pool.in_use_connections / dashboard.current.db_pool.max_connections
    : 0;
  const lifecycleFailures =
    (dashboard?.current.targets.control_timeout_last_24h ?? 0) +
    (dashboard?.current.targets.agent_timeout_last_24h ?? 0) +
    (dashboard?.current.targets.agent_lost_last_24h ?? 0);
  const dbPressurePercent = Math.round(dbPressure * 100);
  const queueDepth = dashboard?.current.dispatch.queue_depth ?? 0;
  const dispatcherInFlight = dashboard?.capacity.dispatcher_in_flight ?? null;
  const dispatcherBatch = dashboard?.capacity.dispatcher_batch ?? null;
  const gatewayEvents = dashboard?.current.gateway_events;
  const gatewayDropped = gatewayEvents?.dropped_events ?? 0;
  const gatewayCriticalFailures = gatewayEvents?.critical_failures ?? 0;
  const gatewayRetries = gatewayEvents?.retry_attempts ?? 0;
  const gatewayQueueDepth = gatewayEvents?.current_queue_depth ?? 0;
  const gatewayOldestAge = gatewayEvents?.oldest_event_age_secs ?? 0;
  const gatewayRejected = gatewayEvents?.rejected_agent_connections ?? 0;
  const profileLimit = extractCapacityProfileLimit(dashboard?.notes ?? []);
  const dbTone = dbPressure >= 0.85 ? "critical" : dbPressure >= 0.7 ? "warning" : "ok";
  const dispatchTone = dispatchHealthTone(queueDepth, dispatcherInFlight, dispatcherBatch);
  const deadlineTone =
    (dashboard?.current.targets.deadline_expired_active ?? 0) > 0
      ? "critical"
      : lifecycleFailures > 0
        ? "warning"
        : "ok";
  const gatewayTone = gatewayHealthTone(gatewayEvents);
  const postureTone = mostSevereTone([dbTone, dispatchTone, deadlineTone, gatewayTone]);
  const profileRatio =
    profileLimit && dispatcherInFlight ? Math.round((dispatcherInFlight / profileLimit) * 100) : null;
  const capacityForecast =
    profileLimit && dispatcherInFlight && dispatcherBatch
      ? `${profileLimit}-VPS profile; ${dispatcherInFlight} in-flight (${profileRatio}% of profile) and ${dispatcherBatch} batch.`
      : profileLimit
        ? `${profileLimit}-VPS profile detected; set dispatcher limits in Suite config to complete the plan.`
        : "No capacity profile note in this sample; configure limits from observed fleet size.";
  const recommendedCapacity =
    profileLimit && dispatcherInFlight && dispatcherBatch
      ? dispatcherInFlight >= profileLimit && dispatcherBatch >= dispatcherInFlight
        ? "Current limits cover the profile."
        : `Raise dispatcher in-flight to at least ${profileLimit} and keep batch >= in-flight.`
      : "Recommended profile unavailable until capacity limits are loaded.";
  const attentionItems = buildSystemAttentionItems({
    dbPressurePercent,
    dbTone,
    dispatchTone,
    gatewayCriticalFailures,
    gatewayDropped,
    gatewayOldestAge,
    gatewayQueueDepth,
    gatewayRejected,
    gatewayRetries,
    gatewayTone,
    lifecycleFailures,
    queueDepth,
  });
  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <div className="dashboardToolbar">
          <div>
            <h2>Control-plane metrics</h2>
            <span>
              {dashboard
                ? `${dashboard.bucket_secs}s rollups / generated ${new Date(dashboard.generated_at).toLocaleTimeString()}`
                : "Control-plane metrics loading"}
            </span>
          </div>
          <div className="dashboardToolbarActions">
            <label className="dashboardToolbarSelect">
              <span>Points</span>
              <select
                aria-label="System overview point density"
                onChange={(event) => onPointDensityChange(event.target.value as SystemDashboardPointDensity)}
                value={pointDensity}
              >
                {pointDensityOptions.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </label>
            <div className="timeRangeTabs" aria-label="System overview time range">
              {dashboardWindows.map((option) => (
                <button
                  aria-pressed={window === option.value}
                  className={window === option.value ? "active" : ""}
                  key={option.value}
                  onClick={() => onWindowChange(option.value)}
                  type="button"
                >
                  {option.label}
                </button>
              ))}
            </div>
            <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
              <RefreshCw size={16} />
              <span>{loading ? "Refreshing" : "Refresh"}</span>
            </button>
          </div>
        </div>
        {error && <div className="panelError">{error}</div>}
        {dashboard?.notes.length ? <div className="panelWarning">{dashboard.notes.join("; ")}</div> : null}

        <section className="dashboardSection systemDashboardOverview" aria-label="System overview operations overview">
          <div className="dashboardSectionHeader">
            <div>
              <h2>Control-plane posture</h2>
              <span>Threshold interpretation, capacity planning, and drilldown coverage before the raw metric curves.</span>
            </div>
            <ConsoleStatusBadge tone={postureTone}>{systemToneLabel(postureTone)}</ConsoleStatusBadge>
          </div>
          <div className="systemPostureGrid">
            <SystemPostureTile
              detail={`${dashboard?.current.db_pool.in_use_connections ?? 0} of ${dashboard?.current.db_pool.max_connections ?? 0} connections in use; warn at 70%, critical at 85%.`}
              icon={<Database size={18} />}
              label="DB capacity"
              tone={dbTone}
              value={`${dbPressurePercent}% in use`}
            />
            <SystemPostureTile
              detail={`${dispatcherInFlight ?? "unset"} in-flight limit / ${dispatcherBatch ?? "unset"} batch; queued work should stay below active dispatch capacity.`}
              icon={<Activity size={18} />}
              label="Dispatch pressure"
              tone={dispatchTone}
              value={`${queueDepth} queued`}
            />
            <SystemPostureTile
              detail={`${dashboard?.current.targets.deadline_expired_active ?? 0} active expired; ${lifecycleFailures} timeout or loss events in the last 24h.`}
              icon={<TimerReset size={18} />}
              label="Deadlines"
              tone={deadlineTone}
              value={`${lifecycleFailures} failures`}
            />
            <SystemPostureTile
              detail={`${gatewayQueueDepth} queued / ${secondsOrUnset(gatewayOldestAge)} oldest; ${gatewayDropped} dropped, ${gatewayRetries} retries, ${gatewayRejected} rejected connects.`}
              icon={<Network size={18} />}
              label="Gateway queue"
              tone={gatewayTone}
              value={gatewayEvents?.status ?? "unavailable"}
            />
            <SystemPostureTile
              detail={recommendedCapacity}
              icon={<ServerCog size={18} />}
              label="Capacity plan"
              tone={profileLimit ? "info" : "warning"}
              value={profileLimit ? `${profileLimit} VPS expected max` : "Profile unset"}
            />
            <SystemPostureTile
              detail="Metric rollups are available here; raw alert overlays and per-series log links need backend event/log endpoints."
              icon={<ShieldCheck size={18} />}
              label="Drilldown coverage"
              tone="warning"
              value={`${series.length} rollup series`}
            />
          </div>
          <div className="systemCapacityStrip" aria-label="System overview capacity forecast">
            <div>
              <strong>Capacity forecast</strong>
              <span>{capacityForecast}</span>
            </div>
            <div>
              <strong>Configured limit</strong>
              <span>{`${valueOrUnset(dispatcherInFlight)} in-flight / ${valueOrUnset(dispatcherBatch)} batch`}</span>
            </div>
            <div>
              <strong>Expected max</strong>
              <span>{profileLimit ? `${profileLimit} long-lived VPSs` : "Not declared"}</span>
            </div>
          </div>
          <div className="systemAttentionQueue" aria-label="System overview attention queue">
            <div className="dashboardSideRailHeader">
              <strong>What needs attention</strong>
              <span>{attentionItems.length ? `${attentionItems.length} signals` : "No active signals"}</span>
            </div>
            {attentionItems.length ? (
              attentionItems.map((item) => (
                <SystemAttentionRow item={item} key={item.label} />
              ))
            ) : (
              <div className="systemAttentionEmpty">
                <CheckCircle2 size={16} />
                <span>No control-plane thresholds are currently breached in this sample.</span>
              </div>
            )}
          </div>
        </section>

        <SystemMetricSection
          badge={`${dashboard?.current.dispatch.queue_depth ?? 0} queued`}
          badgeTone={dispatchTone}
          icon={<Activity size={18} />}
          insight="Dispatch queue should drain below active in-flight capacity; a growing queue while retries climb is an operator signal."
          title="Dispatch Lifecycle"
          subtitle="Queued, dispatching, running, retry, and active job pressure."
          metrics={[
            { label: "Active jobs", value: String(dashboard?.current.dispatch.active_jobs ?? 0) },
            { label: "Dispatch queue", value: String(dashboard?.current.dispatch.queue_depth ?? 0) },
            { label: "Active targets", value: String(dashboard?.current.targets.active ?? 0) },
            { label: "Retried targets", value: String(dashboard?.current.dispatch.retried_targets ?? 0) },
          ]}
          thresholds={[
            { label: "Capacity", tone: "info", value: `${valueOrUnset(dispatcherInFlight)} in-flight` },
            { label: "Batch", tone: "info", value: `${valueOrUnset(dispatcherBatch)} max` },
            { label: "Attention", tone: "warning", value: "queue persists" },
          ]}
          lines={chartLines(series, [
            "dispatch.queue_depth",
            "targets.dispatching",
            "targets.running",
            "dispatch.retried_targets",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <SystemMetricSection
          badge={`${dashboard?.current.targets.deadline_expired_active ?? 0} expired`}
          badgeTone={deadlineTone}
          icon={<AlertTriangle size={18} />}
          insight="Deadline curves separate active expiry from historical control and agent timeout causes so operators can triage control-plane versus VPS-side failure."
          title="Deadlines"
          subtitle="Control deadline expiry, agent timeouts, agent loss, and canceled outcomes."
          metrics={[
            { label: "Deadline timeouts", value: String(lifecycleFailures) },
            { label: "Control timed out", value: String(dashboard?.current.targets.control_timeout_last_24h ?? 0) },
            { label: "Agent timed out", value: String(dashboard?.current.targets.agent_timeout_last_24h ?? 0) },
            { label: "Agent lost", value: String(dashboard?.current.targets.agent_lost_last_24h ?? 0) },
            { label: "Agent offline timeout", value: secondsOrUnset(dashboard?.capacity.agent_offline_secs) },
          ]}
          thresholds={[
            { label: "Warn", tone: "warning", value: "any 24h timeout" },
            { label: "Critical", tone: "critical", value: "active deadline expiry" },
            { label: "Agent offline", tone: "info", value: secondsOrUnset(dashboard?.capacity.agent_offline_secs) },
          ]}
          lines={chartLines(series, [
            "targets.deadline_expired_active",
            "targets.control_timeout_last_24h",
            "targets.agent_timeout_last_24h",
            "targets.agent_lost_last_24h",
            "targets.canceled_last_24h",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <SystemMetricSection
          badge={dashboard?.current.gateway_events.status ?? "unavailable"}
          badgeTone={gatewayTone}
          icon={<Network size={18} />}
          insight="Gateway charts focus on queue depth, retry, drop, and rejection signals. Delivered totals stay in the current table so they do not flatten smaller failure curves."
          title="Gateway Events"
          subtitle="Gateway-to-API forwarding backlog, deliveries, retries, and per-target queues."
          metrics={[
            { label: "Status", value: dashboard?.current.gateway_events.status ?? "unavailable" },
            { label: "Queue depth", value: valueOrUnset(dashboard?.current.gateway_events.current_queue_depth) },
            { label: "Oldest age", value: secondsOrUnset(dashboard?.current.gateway_events.oldest_event_age_secs) },
            { label: "Dropped", value: valueOrUnset(dashboard?.current.gateway_events.dropped_events) },
            { label: "Critical failures", value: valueOrUnset(dashboard?.current.gateway_events.critical_failures) },
            { label: "Telemetry coalesced", value: valueOrUnset(dashboard?.current.gateway_events.dropped_by_reason?.coalesced) },
            { label: "Protocol conflicts", value: valueOrUnset(dashboard?.current.gateway_events.dropped_by_reason?.protocol_conflict) },
            { label: "Target queue full", value: valueOrUnset(dashboard?.current.gateway_events.dropped_by_reason?.target_queue_full) },
            { label: "Retained output trunc", value: valueOrUnset(dashboard?.current.gateway_events.retained_output_truncated_events) },
            { label: "Rejected connects", value: valueOrUnset(dashboard?.current.gateway_events.rejected_agent_connections) },
            { label: "Delivered", value: valueOrUnset(dashboard?.current.gateway_events.delivered_events) },
            { label: "Event retries", value: valueOrUnset(dashboard?.current.gateway_events.retry_attempts) },
          ]}
          thresholds={[
            { label: "Live", tone: gatewayEvents?.status === "live" ? "ok" : "warning", value: gatewayEvents?.status ?? "unavailable" },
            { label: "Critical", tone: "critical", value: "queue full / expired drops" },
            { label: "Attention", tone: "warning", value: "any retry or drop" },
          ]}
          lines={chartLines(series, [
            "gateway_events.current_queue_depth",
            "gateway_events.oldest_event_age_secs",
            "gateway_events.dropped_events",
            "gateway_events.critical_failures",
            "gateway_events.rejected_agent_connections",
            "gateway_events.retry_attempts",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <SystemMetricSection
          badge={`${dashboard?.current.cancellations.awaiting_ack ?? 0} waiting`}
          badgeTone={(dashboard?.current.cancellations.awaiting_ack ?? 0) > 0 ? "warning" : "ok"}
          icon={<TimerReset size={18} />}
          insight="Cancellation health is about acknowledgement lag: sent cancels should converge to agent acks without accumulating waiting state."
          title="Cancellations"
          subtitle="Operator cancel delivery and explicit agent acknowledgement state."
          metrics={[
            { label: "Requested", value: String(dashboard?.current.cancellations.requested ?? 0) },
            { label: "Sent", value: String(dashboard?.current.cancellations.sent ?? 0) },
            { label: "Cancel acks", value: String(dashboard?.current.cancellations.acked ?? 0) },
            { label: "Awaiting ack", value: String(dashboard?.current.cancellations.awaiting_ack ?? 0) },
          ]}
          thresholds={[
            { label: "Healthy", tone: "ok", value: "0 awaiting ack" },
            { label: "Attention", tone: "warning", value: "ack backlog grows" },
          ]}
          lines={chartLines(series, [
            "cancellations.requested",
            "cancellations.sent",
            "cancellations.acked",
            "cancellations.awaiting_ack",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <section className="dashboardSection">
          <div className="dashboardSectionHeader">
            <div>
              <h2>Service Health</h2>
              <span>Current timeout and internal HTTP posture from suite config.</span>
            </div>
            <ConsoleStatusBadge tone={dashboard?.current.gateway_events.status === "live" ? "ok" : "warning"}>
              {dashboard?.current.gateway_events.status ?? "unavailable"}
            </ConsoleStatusBadge>
          </div>
          <div className="dashboardCardGrid operationalGrid">
            <SystemStatusTile icon={<ServerCog size={18} />} label="Dispatch ack" value={secondsOrUnset(dashboard?.capacity.dispatch_ack_secs)} />
            <SystemStatusTile icon={<Network size={18} />} label="Event post" value={secondsOrUnset(dashboard?.capacity.event_post_secs)} />
            <SystemStatusTile icon={<TimerReset size={18} />} label="Internal HTTP read" value={secondsOrUnset(dashboard?.capacity.internal_http_read_secs)} />
            <SystemStatusTile icon={<TimerReset size={18} />} label="Control grace" value={secondsOrUnset(dashboard?.capacity.control_deadline_grace_secs)} />
            <SystemStatusTile icon={<Activity size={18} />} label="Schedule job" value={secondsOrUnset(dashboard?.capacity.worker_schedule_job_max_timeout_secs)} />
          </div>
        </section>
      </div>
    </div>
  );
}

function SystemCapacityPanel({
  dashboard,
  error,
  loading,
  onPointDensityChange,
  onRefresh,
  onWindowChange,
  pointDensity,
  window,
}: {
  dashboard: SystemDashboardRecord | null;
  error: string | null;
  loading: boolean;
  onPointDensityChange: (density: SystemDashboardPointDensity) => void;
  onRefresh: () => void;
  onWindowChange: (window: SystemDashboardWindow) => void;
  pointDensity: SystemDashboardPointDensity;
  window: SystemDashboardWindow;
}) {
  const series = dashboard?.series ?? [];
  const dbPressure = dashboard?.current.db_pool.max_connections
    ? dashboard.current.db_pool.in_use_connections / dashboard.current.db_pool.max_connections
    : 0;
  const dbPressurePercent = Math.round(dbPressure * 100);
  const dbTone = dbPressure >= 0.85 ? "critical" : dbPressure >= 0.7 ? "warning" : "ok";
  const queueDepth = dashboard?.current.dispatch.queue_depth ?? 0;
  const dispatcherInFlight = dashboard?.capacity.dispatcher_in_flight ?? null;
  const dispatcherBatch = dashboard?.capacity.dispatcher_batch ?? null;
  const dispatchTone = dispatchHealthTone(queueDepth, dispatcherInFlight, dispatcherBatch);
  const gatewayEvents = dashboard?.current.gateway_events;
  const gatewayTone = gatewayHealthTone(gatewayEvents);
  const profileLimit = extractCapacityProfileLimit(dashboard?.notes ?? []);
  const profileRatio =
    profileLimit && dispatcherInFlight ? Math.round((dispatcherInFlight / profileLimit) * 100) : null;
  const capacityTone = mostSevereTone([dbTone, dispatchTone, gatewayTone]);
  const configuredLimit = `${valueOrUnset(dispatcherInFlight)} in-flight / ${valueOrUnset(dispatcherBatch)} batch`;
  const capacityForecast =
    profileLimit && dispatcherInFlight && dispatcherBatch
      ? `${profileLimit}-VPS profile; ${dispatcherInFlight} in-flight (${profileRatio}% of profile) and ${dispatcherBatch} batch.`
      : profileLimit
        ? `${profileLimit}-VPS profile detected; set dispatcher limits in Suite config to complete the plan.`
        : "Capacity profile is not declared in the current system dashboard notes.";
  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <div className="dashboardToolbar">
          <div>
            <h2>Capacity telemetry</h2>
            <span>
              {dashboard
                ? `${dashboard.bucket_secs}s rollups / generated ${new Date(dashboard.generated_at).toLocaleTimeString()}`
                : "Capacity telemetry loading"}
            </span>
          </div>
          <div className="dashboardToolbarActions">
            <label className="dashboardToolbarSelect">
              <span>Points</span>
              <select
                aria-label="System capacity point density"
                onChange={(event) => onPointDensityChange(event.target.value as SystemDashboardPointDensity)}
                value={pointDensity}
              >
                {pointDensityOptions.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </label>
            <div className="timeRangeTabs" aria-label="System capacity time range">
              {dashboardWindows.map((option) => (
                <button
                  aria-pressed={window === option.value}
                  className={window === option.value ? "active" : ""}
                  key={option.value}
                  onClick={() => onWindowChange(option.value)}
                  type="button"
                >
                  {option.label}
                </button>
              ))}
            </div>
            <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
              <RefreshCw size={16} />
              <span>{loading ? "Refreshing" : "Refresh"}</span>
            </button>
          </div>
        </div>
        {error && <div className="panelError">{error}</div>}
        {dashboard?.notes.length ? <div className="panelWarning">{dashboard.notes.join("; ")}</div> : null}

        <section className="dashboardSection systemDashboardOverview" aria-label="System capacity posture overview">
          <div className="dashboardSectionHeader">
            <div>
              <h2>Capacity posture</h2>
              <span>Control-plane limits, queue pressure, and capacity API gaps before fleet-resource metrics.</span>
            </div>
            <ConsoleStatusBadge tone={capacityTone}>{systemToneLabel(capacityTone)}</ConsoleStatusBadge>
          </div>
          <div className="systemPostureGrid">
            <SystemPostureTile
              detail={`${dashboard?.current.db_pool.in_use_connections ?? 0} of ${dashboard?.current.db_pool.max_connections ?? 0} DB connections are in use; warn at 70%, critical at 85%.`}
              icon={<Database size={18} />}
              label="DB pool capacity"
              tone={dbTone}
              value={`${dbPressurePercent}% in use`}
            />
            <SystemPostureTile
              detail={`${queueDepth} queued targets against ${configuredLimit}; queue depth should drain below active in-flight capacity.`}
              icon={<Activity size={18} />}
              label="Dispatch capacity"
              tone={dispatchTone}
              value={`${queueDepth} queued`}
            />
            <SystemPostureTile
              detail={`${gatewayEvents?.current_queue_depth ?? 0} gateway events queued; oldest event ${secondsOrUnset(gatewayEvents?.oldest_event_age_secs)}.`}
              icon={<Network size={18} />}
              label="Gateway backlog"
              tone={gatewayTone}
              value={gatewayEvents?.status ?? "unavailable"}
            />
            <SystemPostureTile
              detail={capacityForecast}
              icon={<ServerCog size={18} />}
              label="Configured profile"
              tone={profileLimit ? "info" : "warning"}
              value={profileLimit ? `${profileLimit} VPS expected max` : "Profile unset"}
            />
            <SystemPostureTile
              detail="Object-store bytes and retained artifact pressure are not exposed by the system dashboard API yet; System / Maintenance owns cleanup previews."
              icon={<Database size={18} />}
              label="Artifact storage"
              tone="warning"
              value="API gap"
            />
            <SystemPostureTile
              detail="Retention pressure needs current retained bytes, oldest object age, and prune backlog from the backend before this page can show authoritative pressure."
              icon={<TimerReset size={18} />}
              label="Retention pressure"
              tone="warning"
              value="API gap"
            />
            <SystemPostureTile
              detail="Worker lag is not exposed in the dashboard response yet; schedule timeout and queue signals are shown as available proxies only."
              icon={<AlertTriangle size={18} />}
              label="Worker lag"
              tone="warning"
              value="API gap"
            />
          </div>
          <div className="systemCapacityStrip" aria-label="System capacity configured limits">
            <div>
              <strong>Dispatch capacity</strong>
              <span>{configuredLimit}</span>
            </div>
            <div>
              <strong>DB pools</strong>
              <span>{`${valueOrUnset(dashboard?.capacity.api_db_pool)} API / ${valueOrUnset(dashboard?.capacity.worker_db_pool)} worker`}</span>
            </div>
            <div>
              <strong>Expected max</strong>
              <span>{profileLimit ? `${profileLimit} long-lived VPSs` : "Not declared"}</span>
            </div>
          </div>
        </section>

        <SystemMetricSection
          badge={`${dbPressurePercent}% in use`}
          badgeTone={dbTone}
          icon={<Database size={18} />}
          insight="Capacity charts compare active DB usage with configured connection ceilings; sustained pressure above 70% needs pool review."
          title="Capacity"
          subtitle="Database pool pressure and configured control-plane limits."
          metrics={[
            { label: "API DB pool", value: valueOrUnset(dashboard?.capacity.api_db_pool) },
            { label: "Worker DB pool", value: valueOrUnset(dashboard?.capacity.worker_db_pool) },
            { label: "Dispatcher in-flight", value: valueOrUnset(dashboard?.capacity.dispatcher_in_flight) },
            { label: "Dispatcher batch", value: valueOrUnset(dashboard?.capacity.dispatcher_batch) },
          ]}
          thresholds={[
            { label: "Warn", tone: "warning", value: "70% pool pressure" },
            { label: "Critical", tone: "critical", value: "85% pool pressure" },
            { label: "Profile", tone: "info", value: profileLimit ? `${profileLimit} VPS expected max` : "unset" },
          ]}
          lines={chartLines(series, [
            "db_pool.in_use_connections",
            "db_pool.open_connections",
            "db_pool.idle_connections",
            "db_pool.max_connections",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <SystemMetricSection
          badge={`${queueDepth} queued`}
          badgeTone={dispatchTone}
          icon={<Activity size={18} />}
          insight="Dispatch queue should drain below active in-flight capacity; a growing queue while retries climb is an operator signal."
          title="Dispatch capacity"
          subtitle="Queued, dispatching, running, retry, and active job pressure."
          metrics={[
            { label: "Active jobs", value: String(dashboard?.current.dispatch.active_jobs ?? 0) },
            { label: "Dispatch queue", value: String(queueDepth) },
            { label: "Active targets", value: String(dashboard?.current.targets.active ?? 0) },
            { label: "Retried targets", value: String(dashboard?.current.dispatch.retried_targets ?? 0) },
          ]}
          thresholds={[
            { label: "Capacity", tone: "info", value: `${valueOrUnset(dispatcherInFlight)} in-flight` },
            { label: "Batch", tone: "info", value: `${valueOrUnset(dispatcherBatch)} max` },
            { label: "Attention", tone: "warning", value: "queue persists" },
          ]}
          lines={chartLines(series, [
            "dispatch.queue_depth",
            "targets.dispatching",
            "targets.running",
            "dispatch.retried_targets",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <SystemMetricSection
          badge={gatewayEvents?.status ?? "unavailable"}
          badgeTone={gatewayTone}
          icon={<Network size={18} />}
          insight="Gateway queue pressure is control-plane capacity: target queue saturation, retries, and rejected connections are shown without mixing in VPS CPU or memory."
          title="Gateway queue"
          subtitle="Gateway-to-API forwarding backlog, retries, drops, and per-target queue saturation."
          metrics={[
            { label: "Status", value: gatewayEvents?.status ?? "unavailable" },
            { label: "Queue depth", value: valueOrUnset(gatewayEvents?.current_queue_depth) },
            { label: "Oldest age", value: secondsOrUnset(gatewayEvents?.oldest_event_age_secs) },
            { label: "Dropped", value: valueOrUnset(gatewayEvents?.dropped_events) },
            { label: "Critical failures", value: valueOrUnset(gatewayEvents?.critical_failures) },
            { label: "Target queue full", value: valueOrUnset(gatewayEvents?.dropped_by_reason?.target_queue_full) },
            { label: "Rejected connects", value: valueOrUnset(gatewayEvents?.rejected_agent_connections) },
            { label: "Event retries", value: valueOrUnset(gatewayEvents?.retry_attempts) },
          ]}
          thresholds={[
            { label: "Live", tone: gatewayEvents?.status === "live" ? "ok" : "warning", value: gatewayEvents?.status ?? "unavailable" },
            { label: "Critical", tone: "critical", value: "queue full / expired drops" },
            { label: "Attention", tone: "warning", value: "any retry or drop" },
          ]}
          lines={chartLines(series, [
            "gateway_events.current_queue_depth",
            "gateway_events.oldest_event_age_secs",
            "gateway_events.dropped_events",
            "gateway_events.critical_failures",
            "gateway_events.rejected_agent_connections",
            "gateway_events.retry_attempts",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <section className="dashboardSection" aria-label="System capacity unavailable telemetry">
          <div className="dashboardSectionHeader">
            <div>
              <h2>Unavailable capacity telemetry</h2>
              <span>Fields the release shape needs before artifact, retention, and worker pressure can be authoritative.</span>
            </div>
            <ConsoleStatusBadge tone="warning">API gaps</ConsoleStatusBadge>
          </div>
          <div className="dashboardCardGrid operationalGrid">
            <SystemStatusTile icon={<Database size={18} />} label="Artifact storage bytes" value="not exposed" />
            <SystemStatusTile icon={<TimerReset size={18} />} label="Retention prune backlog" value="not exposed" />
            <SystemStatusTile icon={<Activity size={18} />} label="Worker lag seconds" value="not exposed" />
            <SystemStatusTile icon={<ServerCog size={18} />} label="Maintenance cleanup owner" value="System / Maintenance" />
          </div>
        </section>
      </div>
    </div>
  );
}

function SystemMetricSection({
  badge,
  badgeTone = "info",
  icon,
  insight,
  lines,
  metrics,
  subtitle,
  thresholds = [],
  title,
  valueFormatter,
}: {
  badge: string;
  badgeTone?: SystemHealthTone;
  icon: ReactNode;
  insight?: string;
  lines: { lines: TimeSeriesChartLine[]; times: string[] };
  metrics: Array<{ label: string; value: string }>;
  subtitle: string;
  thresholds?: SystemThresholdItem[];
  title: string;
  valueFormatter: (value: number | null) => string;
}) {
  return (
    <section className="dashboardSection">
      <div className="dashboardSectionHeader">
        <div>
          <h2>{title}</h2>
          <span>{subtitle}</span>
        </div>
        <ConsoleStatusBadge tone={badgeTone}>{badge}</ConsoleStatusBadge>
      </div>
      <div className="dashboardNetworkPanel systemMetricPanel">
        <div className="dashboardCurveCard">
          <div className="dashboardChartHeader">
            <span className="systemSectionTitle">{icon}{title} curves</span>
            {thresholds.length ? (
              <div className="systemThresholdChips" aria-label={`${title} thresholds`}>
                {thresholds.map((threshold) => (
                  <span className={threshold.tone ?? "neutral"} key={threshold.label}>
                    <strong>{threshold.label}</strong>
                    {threshold.value}
                  </span>
                ))}
              </div>
            ) : null}
          </div>
          {insight ? <p className="systemChartInsight">{insight}</p> : null}
          <TimeSeriesChart
            ariaLabel={`${title} system metrics`}
            emptyLabel="No durable system metric samples in this time range"
            lines={lines.lines}
            times={lines.times}
            valueFormatter={valueFormatter}
          />
        </div>
        <div className="dashboardTopClients systemMetricTable">
          <div className="dashboardSideRailHeader">
            <strong>Current</strong>
            <span>{metrics.length} values</span>
          </div>
          {metrics.map((metric) => (
            <div className="dashboardClientRow staticRow" key={metric.label}>
              <span>
                <strong>{metric.label}</strong>
              </span>
              <b>{metric.value}</b>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function SystemPostureTile({
  detail,
  icon,
  label,
  tone,
  value,
}: {
  detail: string;
  icon: ReactNode;
  label: string;
  tone: SystemHealthTone;
  value: string;
}) {
  return (
    <div className={`systemPostureTile ${tone}`}>
      <span className="dashboardMetricIcon">{icon}</span>
      <div>
        <small>{label}</small>
        <strong>{value}</strong>
        <p>{detail}</p>
      </div>
    </div>
  );
}

function SystemAttentionRow({ item }: { item: SystemAttentionItem }) {
  return (
    <div className={`systemAttentionRow ${item.tone}`}>
      <span>{systemToneLabel(item.tone)}</span>
      <strong>{item.label}</strong>
      <b>{item.value}</b>
      <p>{item.detail}</p>
    </div>
  );
}

function SystemStatusTile({ icon, label, value }: { icon: ReactNode; label: string; value: string }) {
  return (
    <div className="dashboardMetricCard neutral staticCard">
      <span className="dashboardMetricIcon">{icon}</span>
      <span>
        <small>{label}</small>
        <strong>{value}</strong>
      </span>
    </div>
  );
}

function SystemConfigPanel({
  config,
  error,
  loading,
  onLoad,
  onOpenPrivilegeUnlock,
  onSelectView,
  onUpdate,
  onValidate,
  privilegeMaterial,
}: {
  config: SuiteConfigResponse | null;
  error: string | null;
  loading: boolean;
  onLoad: () => void;
  onOpenPrivilegeUnlock: () => void;
  onSelectView: (view: ActiveView, subpage?: string) => void;
  onUpdate: (toml: string, privilegeAssertion: unknown) => Promise<SuiteConfigUpdateResponse>;
  onValidate: (toml: string) => Promise<SuiteConfigValidateResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
}) {
  const [draftToml, setDraftToml] = useState("");
  const [validation, setValidation] = useState<SuiteConfigValidateResponse | null>(null);
  const [configMessage, setConfigMessage] = useState<string | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [editorMode, setEditorMode] = useState<"form" | "toml">("form");
  const [activeConfigSection, setActiveConfigSection] = useState(suiteConfigSections[0].id);
  const parsedDraft = useMemo(() => parseTomlDraft(draftToml), [draftToml]);
  const currentConfigDraft = useMemo(
    () => config ? parseTomlDraft(config.toml) : null,
    [config],
  );
  const dirty = Boolean(config && draftToml !== config.toml);
  const changedKeys = validation?.changed_keys ?? [];
  const activeValidation = validation?.validation ?? config?.validation ?? null;
  const hotReloadFields = activeValidation?.hot_reload_fields ?? [];
  const restartRequiredFields = activeValidation?.restart_required_fields ?? [];
  const hotReloadCount = suiteConfigSections.flatMap((section) => section.fields).filter(
    (field) => configFieldImpact(field.path, activeValidation) === "hot",
  ).length;
  const restartRequiredCount = suiteConfigSections.flatMap((section) => section.fields).filter(
    (field) => configFieldImpact(field.path, activeValidation) === "restart",
  ).length;
  const changedHotReloadFields = changedKeys.filter((key) => configFieldImpact(key, activeValidation) === "hot");
  const changedRestartRequiredFields = changedKeys.filter((key) => configFieldImpact(key, activeValidation) === "restart");
  const changedUnknownImpactFields = changedKeys.filter((key) => configFieldImpact(key, activeValidation) === "unknown");
  const validationState = validation
    ? validation.validation.valid
      ? "validated"
      : "invalid"
    : config?.validation.valid
      ? "loaded"
      : "invalid";
  const reviewDisabled = pending || !dirty || !validation || !privilegeMaterial || !validation.validation.valid;
  const nextAction = systemConfigNextAction({
    dirty,
    pending,
    privilegeReady: Boolean(privilegeMaterial),
    reviewDisabled,
    validation,
  });

  useEffect(() => {
    if (config) {
      setDraftToml(config.toml);
      setValidation(null);
      setConfigMessage(null);
      setConfigError(null);
      setConfirmOpen(false);
    }
  }, [config]);

  async function validateDraft() {
    setPending(true);
    setConfigError(null);
    setConfigMessage(null);
    try {
      const result = await onValidate(draftToml);
      setValidation(result);
      setConfigMessage(`Validation passed; ${result.changed_keys.length} changed key${result.changed_keys.length === 1 ? "" : "s"}.`);
    } catch (validateError) {
      setValidation(null);
      setConfigError(validateError instanceof Error ? validateError.message : "Suite config validation failed");
    } finally {
      setPending(false);
    }
  }

  async function saveDraft() {
    if (!privilegeMaterial) {
      setConfigError("Local privilege unlock is required");
      return;
    }
    if (!validation) {
      setConfigError("Validate the current TOML before saving");
      return;
    }
    if (!validation.validation.valid) {
      setConfigError("Fix validation errors before saving");
      return;
    }
    setPending(true);
    setConfigError(null);
    setConfigMessage(null);
    try {
      const payloadHash = await textPayloadHashHex(draftToml);
      const intent = canonicalDbPrivilegeIntent({
        action: "suite_config.update",
        confirmed: true,
        payloadHash,
        target: "suite_config",
      });
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent,
        privilegeMaterial,
      });
      const response = await onUpdate(draftToml, privilegeAssertion);
      const auditNote = response.audit_status === "applied_recorded" ? "" : ` Audit status: ${response.audit_status}.`;
      setConfigMessage(`Saved suite config; changed keys: ${response.changed_keys.join(", ") || "none"}.${auditNote}`);
      setConfirmOpen(false);
      onLoad();
    } catch (saveError) {
      setConfigError(saveError instanceof Error ? saveError.message : "Suite config save failed");
    } finally {
      setPending(false);
    }
  }

  function updateField(path: string, value: unknown) {
    if (!parsedDraft.ok) {
      setConfigError(parsedDraft.error);
      return;
    }
    const next = cloneTable(parsedDraft.table);
    setTomlPath(next, path.split("."), value);
    setDraftToml(stringify(next));
    setValidation(null);
    setConfirmOpen(false);
  }

  function selectConfigSection(sectionId: string) {
    setActiveConfigSection(sectionId);
    document.getElementById(`suite-config-${sectionId}`)?.scrollIntoView({
      block: "start",
      behavior: "smooth",
    });
  }

  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <section className="fleetPanel systemConfigOverview">
          <div className="sectionHeader">
            <div>
              <h2>Suite config</h2>
              <span>{config?.path ?? "Suite TOML path"} / {config?.exists ? "file exists" : "new file"}</span>
            </div>
            <div className="buttonCluster">
              <button className="secondaryAction compactAction" disabled={loading || pending} onClick={onLoad} type="button">
                <RefreshCw size={16} />
                <span>{loading ? "Loading" : "Reload"}</span>
              </button>
              <button className="secondaryAction compactAction" disabled={pending || !draftToml.trim()} onClick={validateDraft} type="button">
                <CheckCircle2 size={16} />
                <span>Validate</span>
              </button>
              <button
                className="primaryAction compactAction"
                disabled={reviewDisabled}
                onClick={() => setConfirmOpen(true)}
                title={reviewDisabled ? nextAction.detail : "Review changed suite config keys, reload impact, and privilege state."}
                type="button"
              >
                <Save size={16} />
                <span>Review save</span>
              </button>
            </div>
          </div>
          {error && <div className="panelError">{error}</div>}
          {configError && <div className="panelError">{configError}</div>}
          {configMessage && <div className="panelSuccess">{configMessage}</div>}
          {config && (
            <div className="systemConfigSummary">
              <SystemConfigStatusItem icon={<SlidersHorizontal size={17} />} label="State" value={dirty ? "draft" : validationState} tone={dirty ? "warning" : validationState === "invalid" ? "critical" : "ok"} />
              <SystemConfigStatusItem icon={<CheckCircle2 size={17} />} label="Changed keys" value={validation ? String(changedKeys.length) : "not validated"} tone={validation ? "info" : "neutral"} />
              <SystemConfigStatusItem icon={<RefreshCw size={17} />} label="Hot reload" value={`${hotReloadCount} fields`} tone="info" />
              <SystemConfigStatusItem icon={<AlertTriangle size={17} />} label="Restart required" value={`${restartRequiredCount} fields`} tone={restartRequiredCount ? "warning" : "ok"} />
              <SystemConfigStatusItem icon={<LockKeyhole size={17} />} label="Privilege" value={privilegeMaterial ? "unlocked" : "locked"} tone={privilegeMaterial ? "ok" : "warning"} />
            </div>
          )}
        </section>

        <section className="systemConfigOwnership" aria-label="Suite config ownership boundary">
          <div className="systemConfigOwnershipItem">
            <span><ServerCog size={17} /></span>
            <div>
              <strong>System scope</strong>
              <p>Suite TOML controls API, gateway, worker, capacity, storage, secrets, and control-plane timeouts.</p>
            </div>
          </div>
          <div className="systemConfigOwnershipItem">
            <span><SlidersHorizontal size={17} /></span>
            <div>
              <strong>Runtime config scope</strong>
              <p>Per-VPS runtime reads, overrides, patches, templates, and rules stay in Config workflows.</p>
            </div>
            <div className="systemConfigOwnershipActions">
              <button className="secondaryAction compactAction" onClick={() => onSelectView("Config", "per_vps")} type="button">
                Open Config / Per-VPS
              </button>
              <button className="secondaryAction compactAction" onClick={() => onSelectView("Config", "bulk_patch")} type="button">
                Open Config / Bulk patch
              </button>
            </div>
          </div>
          <div className="systemConfigOwnershipItem">
            <span><ShieldCheck size={17} /></span>
            <div>
              <strong>Save contract</strong>
              <p>Every suite change follows structured edit, validation, redacted diff, privilege unlock, save, reload or restart, and audit evidence.</p>
            </div>
          </div>
        </section>

        <div className="systemConfigBody">
          <aside className="systemConfigSideNav" aria-label="Suite config sections">
            <strong>Sections</strong>
            {suiteConfigSections.map((section) => (
              <button
                aria-current={activeConfigSection === section.id ? "true" : undefined}
                className={activeConfigSection === section.id ? "active" : ""}
                key={section.id}
                onClick={() => selectConfigSection(section.id)}
                type="button"
              >
                <span>{section.title}</span>
                <small>{section.fields.length} fields</small>
              </button>
            ))}
            <button
              aria-current={activeConfigSection === "review" ? "true" : undefined}
              className={activeConfigSection === "review" ? "active" : ""}
              onClick={() => selectConfigSection("review")}
              type="button"
            >
              <span>Review</span>
              <small>{nextAction.label}</small>
            </button>
          </aside>

          <div className="systemConfigMain">
            <section className="dashboardSection systemConfigEditor">
              <div className="dashboardSectionHeader">
                <div>
                  <h2>Suite editor</h2>
                  <span>{editorMode === "form" ? "Structured sections with help, defaults, current value, validation rule, and reload impact." : "Advanced TOML editor for settings not covered by structured controls."}</span>
                </div>
                <div className="editorModeGroup">
                  <ConsoleStatusBadge tone={parsedDraft.ok ? "ok" : "warning"}>
                    {parsedDraft.ok ? "TOML parsed" : "TOML invalid"}
                  </ConsoleStatusBadge>
                  <div className="segmented" role="group" aria-label="Suite config editor mode">
                    <button aria-pressed={editorMode === "form"} className={editorMode === "form" ? "selected" : ""} onClick={() => setEditorMode("form")} type="button">
                      Fields
                    </button>
                    <button aria-pressed={editorMode === "toml"} className={editorMode === "toml" ? "selected" : ""} onClick={() => setEditorMode("toml")} type="button">
                      Advanced TOML
                    </button>
                  </div>
                </div>
              </div>
              {!parsedDraft.ok && (
                <div className="panelWarning systemConfigNotice">
                  Structured controls are paused until the TOML parses. Use the advanced TOML editor to repair the document.
                </div>
              )}
              {editorMode === "form" ? (
                <div className="systemConfigSectionStack">
                  {suiteConfigSections.map((section) => (
                    <ConfigSection
                      activeValidation={activeValidation}
                      currentDraft={currentConfigDraft}
                      key={section.id}
                      onChange={updateField}
                      parsedDraft={parsedDraft}
                      section={section}
                    />
                  ))}
                </div>
              ) : (
                <div className="systemTomlEditor">
                  <div className="systemTomlNotes">
                    <span>{config?.hot_reload_note ?? "Hot-reload notes unavailable"}</span>
                    <span>{config?.restart_required_note ?? "Restart notes unavailable"}</span>
                  </div>
                  <textarea
                    aria-label="Suite config TOML"
                    className="systemConfigToml"
                    onChange={(event) => {
                      setDraftToml(event.target.value);
                      setValidation(null);
                      setConfirmOpen(false);
                    }}
                    spellCheck={false}
                    value={draftToml}
                  />
                </div>
              )}
            </section>

            <section
              className="dashboardSection systemConfigReview"
              id="suite-config-review"
              aria-label="Suite config validation and save review"
            >
              <div className="dashboardSectionHeader">
                <div>
                  <h2>Review and save</h2>
                  <span>Edit, validate, unlock, review diff, save, then follow reload/restart and audit evidence.</span>
                </div>
                <ConsoleStatusBadge tone={validation?.validation.valid ? "ok" : dirty ? "warning" : "neutral"}>
                  {validation ? `${changedKeys.length} changed` : dirty ? "Draft" : "No draft"}
                </ConsoleStatusBadge>
              </div>

              <div className="systemReviewStack">
                <ConfigSaveStepper
                  auditRecorded={configMessage?.includes("Saved suite config") ?? false}
                  dirty={dirty}
                  pending={pending}
                  privilegeReady={Boolean(privilegeMaterial)}
                  validated={Boolean(validation?.validation.valid)}
                />

                <div className={`systemNextAction ${nextAction.tone}`}>
                  <strong>{nextAction.label}</strong>
                  <span>{nextAction.detail}</span>
                  {nextAction.action === "validate" ? (
                    <button className="secondaryAction compactAction" disabled={pending || !draftToml.trim()} onClick={validateDraft} type="button">
                      <CheckCircle2 size={16} />
                      Validate
                    </button>
                  ) : nextAction.action === "unlock" ? (
                    <button className="secondaryAction compactAction" onClick={onOpenPrivilegeUnlock} type="button">
                      <LockKeyhole size={16} />
                      Open Privilege Vault
                    </button>
                  ) : nextAction.action === "review" ? (
                    <button
                      className="primaryAction compactAction"
                      disabled={reviewDisabled}
                      onClick={() => setConfirmOpen(true)}
                      title={reviewDisabled ? nextAction.detail : "Review changed suite config keys, reload impact, and privilege state."}
                      type="button"
                    >
                      <Save size={16} />
                      Review save
                    </button>
                  ) : null}
                </div>

                <div className="systemReviewBlock">
                  <h3>Changed keys</h3>
                  <div className="chipList compactChipList">
                    {changedKeys.map((key) => (
                      <span className={configFieldImpact(key, activeValidation)} key={key}>
                        {key}
                      </span>
                    ))}
                    {validation && changedKeys.length === 0 ? <span>No changes</span> : null}
                    {!validation ? <span>Validate draft first</span> : null}
                  </div>
                </div>

                <div className="systemImpactGrid" aria-label="Suite config reload and restart plan">
                  <ImpactList title="Hot reload after save" fields={changedHotReloadFields} emptyLabel="No changed hot-reload fields" />
                  <ImpactList title="Restart required after save" fields={changedRestartRequiredFields} emptyLabel="No changed restart-only fields" />
                  <ImpactList title="Impact not reported" fields={changedUnknownImpactFields} emptyLabel="All changed keys have a reported impact" />
                </div>

                <div className="systemReviewBlock">
                  <h3>Privilege</h3>
                  <div className={`privilegeGateBox ${privilegeMaterial ? "ready" : ""}`}>
                    <LockKeyhole size={18} />
                    <span>{privilegeMaterial ? "Privilege unlocked for this browser session" : "Open Privilege Vault before saving suite config"}</span>
                    {!privilegeMaterial && (
                      <button className="secondaryAction compactAction" onClick={onOpenPrivilegeUnlock} type="button">
                        Open Privilege Vault
                      </button>
                    )}
                  </div>
                </div>

                <div className="systemReviewBlock">
                  <h3>Save</h3>
                  <button
                    className="primaryAction wideAction"
                    disabled={reviewDisabled}
                    onClick={() => setConfirmOpen(true)}
                    title={reviewDisabled ? nextAction.detail : "Review changed suite config keys, reload impact, and privilege state."}
                    type="button"
                  >
                    <Save size={16} />
                    <span>{pending ? "Saving" : "Review save"}</span>
                  </button>
                </div>

                <details className="systemAdvancedDiff">
                  <summary>Advanced redacted JSON diff</summary>
                  <div className="systemDiffPreview">
                    <div>
                      <h3>Current redacted</h3>
                      <pre className="jsonPreview compactJsonPreview">{formatJson(config?.redacted ?? validation?.old_redacted ?? null)}</pre>
                    </div>
                    <div>
                      <h3>Draft redacted</h3>
                      <pre className="jsonPreview compactJsonPreview">{formatJson(validation?.redacted ?? null)}</pre>
                    </div>
                  </div>
                </details>
              </div>
              <ConfirmationPrompt
                confirmLabel="Save suite config"
                detail="This writes the suite TOML, may hot-reload runtime settings, and may require service restarts for restart-only keys."
                error={configError}
                items={[
                  { label: "Changed keys", value: String(changedKeys.length) },
                  { label: "Hot reload changed", value: String(changedHotReloadFields.length) },
                  { label: "Restart required changed", value: String(changedRestartRequiredFields.length) },
                  { label: "Privilege", value: privilegeMaterial ? "Unlocked locally" : "Locked" },
                ]}
                onCancel={() => setConfirmOpen(false)}
                onConfirm={() => void saveDraft()}
                open={confirmOpen}
                pending={pending}
                title="Confirm suite config save"
                tone="danger"
              />
            </section>
          </div>
        </div>
      </div>
    </div>
  );
}

type ParsedTomlDraft = { ok: true; table: TomlTable } | { ok: false; error: string };

function SystemConfigStatusItem({
  icon,
  label,
  tone,
  value,
}: {
  icon: ReactNode;
  label: string;
  tone: "critical" | "info" | "neutral" | "ok" | "warning";
  value: string;
}) {
  return (
    <div className={`systemConfigStatusItem ${tone}`}>
      <span>{icon}</span>
      <small>{label}</small>
      <strong>{value}</strong>
    </div>
  );
}

function ImpactList({ emptyLabel, fields, title }: { emptyLabel: string; fields: string[]; title: string }) {
  return (
    <div className="systemImpactList">
      <h3>{title}</h3>
      <ul>
        {fields.slice(0, 8).map((field) => <li key={field}>{field}</li>)}
        {fields.length === 0 ? <li>{emptyLabel}</li> : null}
        {fields.length > 8 ? <li>{fields.length - 8} more fields</li> : null}
      </ul>
    </div>
  );
}

function ConfigSection({
  activeValidation,
  currentDraft,
  onChange,
  parsedDraft,
  section,
}: {
  activeValidation: SuiteConfigValidationRecord | null;
  currentDraft: ParsedTomlDraft | null;
  onChange: (path: string, value: unknown) => void;
  parsedDraft: ParsedTomlDraft;
  section: ConfigSectionSpec;
}) {
  return (
    <section
      className="systemConfigGroup"
      id={`suite-config-${section.id}`}
      aria-label={`${section.title} suite config fields`}
    >
      <div className="systemConfigGroupHeader">
        <div>
          <h3>{section.title}</h3>
          <p>{section.description}</p>
        </div>
        <span>{section.fields.length} fields</span>
      </div>
      <div className="systemConfigFieldList">
        {section.fields.map((field) => (
          <ConfigFieldControl
            activeValidation={activeValidation}
            currentDraft={currentDraft}
            field={field}
            key={field.path}
            onChange={onChange}
            parsedDraft={parsedDraft}
          />
        ))}
      </div>
    </section>
  );
}

function ConfigFieldControl({
  activeValidation,
  currentDraft,
  field,
  onChange,
  parsedDraft,
}: {
  activeValidation: SuiteConfigValidationRecord | null;
  currentDraft: ParsedTomlDraft | null;
  field: ConfigFieldSpec;
  onChange: (path: string, value: unknown) => void;
  parsedDraft: ParsedTomlDraft;
}) {
  const pathParts = field.path.split(".");
  const draftValue = parsedDraft.ok ? getTomlPath(parsedDraft.table, pathParts) : undefined;
  const currentValue = currentDraft?.ok ? getTomlPath(currentDraft.table, pathParts) : undefined;
  const currentLabel = formatConfigValue(currentValue);
  const draftLabel = formatConfigValue(draftValue);
  const impact = configFieldImpact(field.path, activeValidation);
  const controlId = `suite-config-field-${field.path.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  return (
    <div className="systemConfigFieldRow">
      <div className="systemConfigFieldText">
        <label htmlFor={controlId}>
          {field.label}
          {field.required ? <span>required</span> : null}
        </label>
        <p>{field.help}</p>
        <dl>
          <div>
            <dt>Current</dt>
            <dd title={currentLabel}>{currentLabel}</dd>
          </div>
          <div>
            <dt>Default</dt>
            <dd title={field.defaultValue}>{field.defaultValue}</dd>
          </div>
          <div>
            <dt>Validation</dt>
            <dd title={field.rule}>{field.rule}</dd>
          </div>
          <div>
            <dt>Impact</dt>
            <dd><ConfigImpactBadge impact={impact} /></dd>
          </div>
        </dl>
      </div>
      <div className="systemConfigFieldControl">
        {field.kind === "checkbox" ? (
          <label className="checkLine inlineCheck">
            <input
              checked={draftValue === true}
              disabled={!parsedDraft.ok}
              id={controlId}
              onChange={(event) => onChange(field.path, event.target.checked)}
              type="checkbox"
            />
            <span>{draftValue === true ? "Enabled" : "Disabled"}</span>
          </label>
        ) : (
          <input
            aria-describedby={`${controlId}-meta`}
            disabled={!parsedDraft.ok}
            id={controlId}
            min={field.kind === "number" ? 0 : undefined}
            onChange={(event) => {
              const next = event.target.value.trim();
              onChange(field.path, next ? field.kind === "number" ? Number(next) : event.target.value : undefined);
            }}
            title={draftLabel}
            type={field.kind === "number" ? "number" : "text"}
            value={field.kind === "number" ? formatNumberInputValue(draftValue) : formatTextInputValue(draftValue)}
          />
        )}
        <span id={`${controlId}-meta`} className="systemConfigPath" title={field.path}>{field.path}</span>
      </div>
    </div>
  );
}

function ConfigImpactBadge({ impact }: { impact: ConfigImpact }) {
  const label = impact === "restart" ? "Restart required" : impact === "hot" ? "Hot reload" : "Not reported";
  return (
    <span className={`configImpactBadge ${impact}`}>{label}</span>
  );
}

function ConfigSaveStepper({
  auditRecorded,
  dirty,
  pending,
  privilegeReady,
  validated,
}: {
  auditRecorded: boolean;
  dirty: boolean;
  pending: boolean;
  privilegeReady: boolean;
  validated: boolean;
}) {
  const steps = [
    { label: "Edit", complete: dirty || validated || pending || auditRecorded },
    { label: "Validate", complete: validated || auditRecorded },
    { label: "Unlock", complete: privilegeReady || auditRecorded },
    { label: "Review", complete: validated && privilegeReady },
    { label: "Save", complete: auditRecorded, active: pending },
    { label: "Reload / restart / audit", complete: auditRecorded },
  ];
  return (
    <ol className="systemConfigStepper" aria-label="Suite config save flow">
      {steps.map((step) => (
        <li className={`${step.complete ? "complete" : ""} ${step.active ? "active" : ""}`} key={step.label}>
          <span>{step.complete ? <CheckCircle2 size={14} /> : <span aria-hidden="true" />}</span>
          <strong>{step.label}</strong>
        </li>
      ))}
    </ol>
  );
}

function configFieldImpact(
  path: string,
  validation: SuiteConfigValidationRecord | null | undefined,
): ConfigImpact {
  if (!validation) {
    return "unknown";
  }
  if (validation.restart_required_fields.some((pattern) => configPathMatches(path, pattern))) {
    return "restart";
  }
  if (validation.hot_reload_fields.some((pattern) => configPathMatches(path, pattern))) {
    return "hot";
  }
  return "unknown";
}

function configPathMatches(path: string, pattern: string): boolean {
  if (pattern === path) {
    return true;
  }
  if (!pattern.includes("*")) {
    return false;
  }
  const [prefix, suffix = ""] = pattern.split("*");
  return path.startsWith(prefix) && path.endsWith(suffix);
}

function formatConfigValue(value: unknown): string {
  if (value === undefined || value === null || value === "") {
    return "unset";
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  if (typeof value === "number") {
    return String(value);
  }
  if (typeof value === "string") {
    return value;
  }
  return "configured";
}

function formatNumberInputValue(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value) ? String(value) : "";
}

function formatTextInputValue(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function systemConfigNextAction({
  dirty,
  pending,
  privilegeReady,
  reviewDisabled,
  validation,
}: {
  dirty: boolean;
  pending: boolean;
  privilegeReady: boolean;
  reviewDisabled: boolean;
  validation: SuiteConfigValidateResponse | null;
}): {
  action: "none" | "review" | "unlock" | "validate";
  detail: string;
  label: string;
  tone: "neutral" | "ok" | "warning";
} {
  if (pending) {
    return {
      action: "none",
      detail: "A validation or save request is already running.",
      label: "Working",
      tone: "neutral",
    };
  }
  if (!dirty) {
    return {
      action: "none",
      detail: "No draft changes are waiting. Edit a field or use Advanced TOML to start a save flow.",
      label: "No draft changes",
      tone: "neutral",
    };
  }
  if (!validation) {
    return {
      action: "validate",
      detail: "Validate the draft before review so changed keys and reload impact are authoritative.",
      label: "Next: validate draft",
      tone: "warning",
    };
  }
  if (!validation.validation.valid) {
    return {
      action: "validate",
      detail: "Validation failed. Fix TOML or field values, then validate again.",
      label: "Fix validation errors",
      tone: "warning",
    };
  }
  if (!privilegeReady) {
    return {
      action: "unlock",
      detail: "Suite config writes require local privilege unlock before review can proceed.",
      label: "Next: unlock privilege",
      tone: "warning",
    };
  }
  if (!reviewDisabled) {
    return {
      action: "review",
      detail: "Changed keys, reload plan, and privilege state are ready for final confirmation.",
      label: "Next: review save",
      tone: "ok",
    };
  }
  return {
    action: "none",
    detail: "The save flow is waiting for a valid draft, privilege, and review state.",
    label: "Review unavailable",
    tone: "neutral",
  };
}

function extractCapacityProfileLimit(notes: string[]): number | null {
  for (const note of notes) {
    const match = note.match(/(\d+)\s*[- ]?\s*VPS/i);
    if (match) {
      const parsed = Number(match[1]);
      return Number.isFinite(parsed) ? parsed : null;
    }
  }
  return null;
}

function dispatchHealthTone(
  queueDepth: number,
  dispatcherInFlight: number | null,
  dispatcherBatch: number | null,
): SystemHealthTone {
  if (queueDepth <= 0) {
    return "ok";
  }
  const hardLimit = dispatcherBatch ?? dispatcherInFlight;
  if (hardLimit && queueDepth >= hardLimit) {
    return "critical";
  }
  if (dispatcherInFlight && queueDepth >= Math.ceil(dispatcherInFlight * 0.5)) {
    return "warning";
  }
  return "info";
}

function gatewayHealthTone(
  gatewayEvents: SystemDashboardRecord["current"]["gateway_events"] | undefined,
): SystemHealthTone {
  if (!gatewayEvents || gatewayEvents.status !== "live") {
    return "warning";
  }
  const criticalFailures =
    (gatewayEvents.critical_failures ?? 0) +
    gatewayEvents.critical_failures_by_reason.global_queue_full +
    gatewayEvents.critical_failures_by_reason.target_queue_full +
    gatewayEvents.critical_failures_by_reason.expired;
  const hardDrops =
    gatewayEvents.dropped_by_reason.global_queue_full +
    gatewayEvents.dropped_by_reason.target_queue_full +
    (gatewayEvents.expired_events ?? 0);
  if (criticalFailures > 0 || hardDrops > 0) {
    return "critical";
  }
  const softSignals =
    (gatewayEvents.dropped_events ?? 0) +
    (gatewayEvents.retry_attempts ?? 0) +
    (gatewayEvents.telemetry_dropped_events ?? 0) +
    (gatewayEvents.retained_output_truncated_events ?? 0) +
    (gatewayEvents.rejected_agent_connections ?? 0) +
    (gatewayEvents.current_queue_depth ?? 0) +
    Math.min(1, gatewayEvents.oldest_event_age_secs ?? 0);
  return softSignals > 0 ? "warning" : "ok";
}

function mostSevereTone(tones: SystemHealthTone[]): SystemHealthTone {
  if (tones.includes("critical")) {
    return "critical";
  }
  if (tones.includes("warning")) {
    return "warning";
  }
  if (tones.includes("info")) {
    return "info";
  }
  return tones.includes("ok") ? "ok" : "neutral";
}

function systemToneLabel(tone: SystemHealthTone): string {
  if (tone === "critical") {
    return "Critical";
  }
  if (tone === "warning") {
    return "Needs attention";
  }
  if (tone === "ok") {
    return "Healthy";
  }
  if (tone === "info") {
    return "Monitoring";
  }
  return "No data";
}

function buildSystemAttentionItems({
  dbPressurePercent,
  dbTone,
  dispatchTone,
  gatewayCriticalFailures,
  gatewayDropped,
  gatewayOldestAge,
  gatewayQueueDepth,
  gatewayRejected,
  gatewayRetries,
  gatewayTone,
  lifecycleFailures,
  queueDepth,
}: {
  dbPressurePercent: number;
  dbTone: SystemHealthTone;
  dispatchTone: SystemHealthTone;
  gatewayCriticalFailures: number;
  gatewayDropped: number;
  gatewayOldestAge: number;
  gatewayQueueDepth: number;
  gatewayRejected: number;
  gatewayRetries: number;
  gatewayTone: SystemHealthTone;
  lifecycleFailures: number;
  queueDepth: number;
}): SystemAttentionItem[] {
  const items: SystemAttentionItem[] = [];
  if (dbTone !== "ok") {
    items.push({
      detail: "DB pool pressure crossed the dashboard threshold; validate pool sizing before sustained operator traffic.",
      label: "DB pool pressure",
      tone: dbTone,
      value: `${dbPressurePercent}%`,
    });
  }
  if (dispatchTone !== "ok") {
    items.push({
      detail: "Queued dispatch work exists in the current sample; confirm the queue is draining and retries are not climbing.",
      label: "Dispatch queue",
      tone: dispatchTone,
      value: `${queueDepth} queued`,
    });
  }
  if (lifecycleFailures > 0) {
    items.push({
      detail: "Control, agent timeout, or lost-agent outcomes were recorded in the last 24 hours.",
      label: "Deadline timeouts",
      tone: "warning",
      value: `${lifecycleFailures} events`,
    });
  }
  if (gatewayTone !== "ok") {
    items.push({
      detail: `${gatewayQueueDepth} queued, ${gatewayOldestAge}s oldest, ${gatewayDropped} dropped, ${gatewayRetries} retries, ${gatewayRejected} rejected connects.`,
      label: "Gateway delivery",
      tone: gatewayTone,
      value: gatewayCriticalFailures > 0 ? `${gatewayCriticalFailures} critical` : `${gatewayDropped} dropped`,
    });
  }
  return items;
}

function chartLines(series: SystemMetricSeriesRecord[], metrics: string[]): { lines: TimeSeriesChartLine[]; times: string[] } {
  const selected = metrics
    .map((metric) => series.find((entry) => entry.metric === metric))
    .filter((entry): entry is SystemMetricSeriesRecord => Boolean(entry));
  const times = Array.from(new Set(selected.flatMap((entry) => entry.points.map((point) => point.bucket_start))))
    .sort((left, right) => Date.parse(left) - Date.parse(right));
  const lines = selected.map((entry, index) => {
    const points = new Map(entry.points.map((point) => [point.bucket_start, point.latest_value]));
    return {
      color: dashboardChartColors[index % dashboardChartColors.length],
      label: entry.label,
      values: times.map((time) => points.get(time) ?? null),
    };
  });
  return { lines, times };
}

function parseTomlDraft(toml: string): ParsedTomlDraft {
  try {
    return { ok: true, table: parse(toml) as TomlTable };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : "Invalid TOML" };
  }
}

function cloneTable(table: TomlTable): TomlTable {
  return JSON.parse(JSON.stringify(table)) as TomlTable;
}

function getTomlPath(table: TomlTable, path: string[]): unknown {
  let current: unknown = table;
  for (const part of path) {
    if (!current || typeof current !== "object" || Array.isArray(current)) {
      return undefined;
    }
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}

function setTomlPath(table: TomlTable, path: string[], value: unknown) {
  let current = table as Record<string, unknown>;
  for (const part of path.slice(0, -1)) {
    if (!current[part] || typeof current[part] !== "object" || Array.isArray(current[part])) {
      current[part] = {};
    }
    current = current[part] as Record<string, unknown>;
  }
  const key = path[path.length - 1];
  if (value === undefined || value === null || value === "") {
    delete current[key];
  } else {
    current[key] = value;
  }
}

function formatNumber(value: number | null | undefined): string {
  return value === null || value === undefined ? "No data" : String(Math.round(value));
}

function valueOrUnset(value: number | null | undefined): string {
  return value === null || value === undefined ? "unset" : String(value);
}

function secondsOrUnset(value: number | null | undefined): string {
  return value === null || value === undefined ? "unset" : `${value}s`;
}

function formatJson(value: JsonValue | null): string {
  return value === null ? "Validate draft to preview redacted JSON." : JSON.stringify(value, null, 2);
}
