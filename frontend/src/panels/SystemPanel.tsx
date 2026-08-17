import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
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
  Search,
  ServerCog,
  SlidersHorizontal,
  ShieldCheck,
  TimerReset,
  Trash2,
  UserPlus,
  UserX,
  X,
} from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { formatLowerBoundCount } from "../constants";
import { parse, stringify, type TomlTable } from "smol-toml";
import { ActionFeedback } from "../components/ActionFeedback";
import { handleTabListKeyDown, tabId } from "../components/AccessibleTabs";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { AdminRoleBoundary } from "../components/RoleBoundary";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "../components/TimeSeriesChart";
import { dashboardChartColors } from "../colorPalette";
import { scrollIntoViewWithMotion } from "../motion";
import {
  dashboardWindowAccessibleLabel,
  dashboardWindowLabel,
  dashboardWindowOptions,
} from "../dashboardQuery";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  operatorDbPayloadHashHex,
  textPayloadHashHex,
  type PrivilegeAssertion,
  type PrivilegeMaterial,
} from "../privilege";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
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
import type {
  SystemDashboardPointDensity,
  SystemDashboardWindow,
} from "../hooks/useSystemData";
import { PreferencesPanel } from "./PreferencesPanel";
import {
  formatCompactTime,
  formatFullTime,
  formatTime,
  shortId,
  statusClass,
} from "../utils";

type SystemPanelProps = {
  activeSubpage: string;
  accessError: string | null;
  accessLoading: boolean;
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
  operatorAuthEventsTruncated: boolean;
  operatorSessions: OperatorSessionRecord[];
  operatorSessionsTruncated: boolean;
  operators: OperatorView[];
  privilegeMaterial: PrivilegeMaterial | null;
  suiteConfig: SuiteConfigResponse | null;
  suiteConfigError: string | null;
  suiteConfigLoading: boolean;
  tags: TagView[];
};

const pointDensityOptions: Array<{
  label: string;
  value: SystemDashboardPointDensity;
}> = [
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

type CapacitySubsystem = "database" | "dispatch" | "gateway";

type CapacityFactorItem = {
  detail: string;
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
  "sharing:read",
  "audit:read",
  "jobs:write",
  "inventory:write",
  "schedules:write",
  "backups:write",
  "network:write",
  "sharing:write",
  "config:write",
  "integrations:write",
  "templates:write",
  "history:write",
  "*",
];
const defaultSessionTtlDays = 365;
const defaultAdminSessionTtlDays = 30;

function defaultSessionTtlDaysForRole(role: string): number {
  return role === "admin" ? defaultAdminSessionTtlDays : defaultSessionTtlDays;
}
const operatorHelpText = {
  username:
    "Login username. Existing operator usernames are locked in the editor; create a new operator for a new login name.",
  createPassword:
    "Initial operator password. It must be at least 12 characters and is used only when creating the user.",
  newPassword:
    "Replacement password. Save does not read or send this field; use Reset password to apply it and revoke existing sessions.",
  role: "Role controls the default permission bundle. Admin grants full operator control; operator and viewer are narrower unless scopes override them.",
  sessionRefreshTtl:
    "Refresh-token/session lifetime in days for newly issued operator sessions. This is not the short access-token expiry shown in Audit / Sessions.",
  scopes:
    "Optional scope override. Leave empty for role defaults. Accepts comma or space separated scopes such as fleet:read, jobs:write, config:read, or *.",
  shortcuts:
    "Append a scope token to the override field. These shortcuts are editable text helpers, not immutable presets.",
  save: "Save role, scopes, and refresh-token session TTL only. This action never changes the password field.",
  resetPassword:
    "Apply the New password field, clear existing TOTP secret material, then revoke existing sessions for this operator.",
  clearTotp:
    "Remove stored TOTP secret material and revoke existing sessions. The user must enroll TOTP again before using it.",
  enable: "Allow this disabled operator to log in again.",
  disable:
    "Block login and revoke existing sessions without deleting the operator record.",
  delete:
    "Delete this operator record for login purposes, block login, and revoke existing sessions. The username remains reserved.",
  create:
    "Create the operator record. Password is required for creation and is not shown in the confirmation message.",
  sessionAccessExpires: "Short access-token expiry for this bearer session.",
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
    description:
      "Private operator API listener, gateway control socket, and artifact thresholds.",
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
        help: "Maximum accepted artifact size for API-managed artifacts when configured by the control plane.",
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
    description:
      "Agent listener, control socket, API forwarding URL, and gateway identity.",
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
    description:
      "Shared address pools used when tunnel plans request automatic endpoint allocation.",
    id: "network",
    title: "Network",
    fields: [
      {
        defaultValue: "unset",
        help: "Global IPv4 CIDR used to allocate missing tunnel endpoint addresses. Leave empty to require explicit IPv4 endpoints per plan.",
        kind: "text",
        label: "Tunnel IPv4 allocation pool",
        path: "network.tunnel_ipv4_allocation_pool_cidr",
        rule: "IPv4 CIDR with at least two addresses, or unset",
      },
      {
        defaultValue: "unset",
        help: "Global IPv6 CIDR used to allocate missing tunnel endpoint addresses. Leave empty to require explicit IPv6 endpoints per plan.",
        kind: "text",
        label: "Tunnel IPv6 allocation pool",
        path: "network.tunnel_ipv6_allocation_pool_cidr",
        rule: "IPv6 CIDR with at least two addresses, or unset",
      },
    ],
  },
  {
    description:
      "Worker cadence, leases, schedule timeout, and offline reconciliation.",
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
    description:
      "Control-plane pool and dispatch limits for 20-50 long-lived VPS operation.",
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
    description:
      "Backup artifact location and optional object-store client settings.",
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
    description:
      "Secret-file references. Values point to mounted files; secret contents stay hidden.",
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
    description:
      "Network and internal HTTP timing controls that shape retry and deadline behavior.",
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
  accessError,
  accessLoading,
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
  operatorAuthEventsTruncated,
  operatorSessions,
  operatorSessionsTruncated,
  operators,
  privilegeMaterial,
  suiteConfig,
  suiteConfigError,
  suiteConfigLoading,
  tags,
}: SystemPanelProps) {
  if (
    operator?.role !== "admin" &&
    ["config", "users", "sessions"].includes(activeSubpage)
  ) {
    const title =
      activeSubpage === "config"
        ? "Suite configuration"
        : activeSubpage === "sessions"
          ? "Operator sessions"
          : "Operator accounts";
    return (
      <div className="workspace singleColumn systemWorkspace">
        <AdminRoleBoundary
          currentRole={operator?.role}
          detail="This page can change control-plane policy or inspect authority records and is intentionally visible only to admins."
          title={title}
        />
      </div>
    );
  }
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
        authEventsTruncated={operatorAuthEventsTruncated}
        currentOperator={operator}
        loadError={accessError}
        loadLoading={accessLoading}
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
    );
  }
  if (activeSubpage === "sessions") {
    return (
      <SystemSessionsPanel
        authEvents={operatorAuthEvents}
        authEventsTruncated={operatorAuthEventsTruncated}
        loadError={accessError}
        loadLoading={accessLoading}
        onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
        onRevokeOperatorSession={onRevokeOperatorSession}
        privilegeMaterial={privilegeMaterial}
        sessions={operatorSessions}
        sessionsTruncated={operatorSessionsTruncated}
      />
    );
  }
  if (activeSubpage === "operator") {
    return (
      <PreferencesPanel
        onSelectView={onSelectView}
        operator={operator}
        tags={tags}
      />
    );
  }
  if (activeSubpage === "capacity") {
    return (
      <SystemCapacityPanel
        dashboard={dashboard}
        error={dashboardError}
        loading={dashboardLoading}
        onSelectView={onSelectView}
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

export function SystemUsersPanel({
  authEvents,
  authEventsTruncated,
  currentOperator,
  loadError,
  loadLoading,
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
  sessionsTruncated,
}: {
  authEvents: OperatorAuthEventRecord[];
  authEventsTruncated: boolean;
  currentOperator: OperatorView | null;
  loadError?: string | null;
  loadLoading?: boolean;
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
  sessionsTruncated: boolean;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selectedOperator =
    operators.find((item) => item.id === selectedId) ?? null;
  const [editorMode, setEditorMode] = useState<"closed" | "create" | "edit">(
    "closed",
  );
  const [draftUsername, setDraftUsername] = useState("");
  const [draftPassword, setDraftPassword] = useState("");
  const [draftRole, setDraftRole] = useState("operator");
  const [draftScopes, setDraftScopes] = useState("");
  const [draftSessionTtlDays, setDraftSessionTtlDays] = useState(
    defaultSessionTtlDays,
  );
  const [draftSessionTtlCustomized, setDraftSessionTtlCustomized] =
    useState(false);
  const [pendingAction, setPendingAction] = useState<PendingUserAction | null>(
    null,
  );
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionStatus, setActionStatus] = useState<string | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const operatorEditorRef = useRef<HTMLElement | null>(null);
  const operatorUsernameRef = useRef<HTMLInputElement | null>(null);
  const operatorActionOutcomeRef = useRef<HTMLDivElement | null>(null);
  const previousOperatorActionOutcomeRef = useRef<string | null>(null);
  const canManageUsers = currentOperator?.role === "admin";
  const accessSummaries = useMemo(
    () => buildOperatorAccessSummaries(operators, sessions, authEvents),
    [authEvents, operators, sessions],
  );
  const selectedAccessSummary = selectedOperator
    ? accessSummaries[selectedOperator.id]
    : null;
  const visibleOperators = operators.filter(
    (operator) => operator.status !== "deleted",
  );
  const adminOperators = visibleOperators.filter(
    (operator) => operator.role === "admin",
  );
  const adminWithoutMfaCount = adminOperators.filter(
    (operator) => !operator.totp_enabled,
  ).length;
  const adminLongTtlCount = adminOperators.filter(
    (operator) => operator.session_refresh_ttl_secs > 30 * 86_400,
  ).length;
  const maxAdminTtlDays = adminOperators.reduce(
    (max, operator) =>
      Math.max(max, secondsToDays(operator.session_refresh_ttl_secs)),
    0,
  );
  const explicitScopeUsers = visibleOperators.filter(
    (operator) => operator.scopes.length > 0,
  ).length;
  const visibleRoles = Array.from(
    new Set(visibleOperators.map((operator) => operator.role)),
  ).sort();
  const customRoles = visibleRoles.filter(
    (role) => !operatorRoleOptions.includes(role),
  );
  const activeSessionCount = sessions.filter(isOperatorSessionUsable).length;
  const expiredSessionCount = sessions.filter(
    (session) => !session.revoked && isOperatorSessionExpired(session),
  ).length;
  const revokableSessionCount = sessions.filter(
    (session) => !session.current && isOperatorSessionUsable(session),
  ).length;
  const authFailureEvents = authEvents.filter(
    (event) => event.result !== "success",
  );
  const knownAuthFailureCount = visibleOperators.reduce(
    (count, operator) =>
      count + (accessSummaries[operator.id]?.failedLogins ?? 0),
    0,
  );
  const unknownAuthFailureCount =
    authFailureEvents.length - knownAuthFailureCount;
  const governanceTone =
    adminWithoutMfaCount > 0 || adminLongTtlCount > 0
      ? "warning"
      : sessionsTruncated || authEventsTruncated
        ? "info"
        : "ok";
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const createDraftError =
    editorMode === "create"
      ? !draftUsername.trim()
        ? "Enter a username"
        : draftPassword.length < 12
          ? "Enter a password with at least 12 characters"
          : !Number.isFinite(draftSessionTtlDays) ||
              draftSessionTtlDays < 1 ||
              draftSessionTtlDays > 3650
            ? "Session TTL must be from 1 to 3650 days"
            : null
      : null;
  const operatorActionFeedbackMessage =
    actionError ??
    (reviewPending
      ? "Preparing review"
      : actionPending
        ? "Applying user action"
        : actionStatus);
  const operatorActionFeedbackTone = actionError
    ? "danger"
    : reviewPending || actionPending
      ? "progress"
      : "success";
  const operatorActionOutcome = pendingAction
    ? null
    : (actionError ?? actionStatus);

  useEffect(() => {
    if (!operatorActionOutcome) {
      previousOperatorActionOutcomeRef.current = null;
      return;
    }
    if (previousOperatorActionOutcomeRef.current === operatorActionOutcome) {
      return;
    }
    previousOperatorActionOutcomeRef.current = operatorActionOutcome;
    const frame = window.requestAnimationFrame(() => {
      if (operatorActionOutcomeRef.current) {
        scrollIntoViewWithMotion(operatorActionOutcomeRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [operatorActionOutcome]);

  useEffect(() => {
    if (editorMode === "closed") return;
    const timeout = window.setTimeout(() => {
      operatorEditorRef.current?.scrollIntoView({
        behavior: "smooth",
        block: "start",
      });
      if (editorMode === "create") {
        operatorUsernameRef.current?.focus();
      } else {
        operatorEditorRef.current
          ?.querySelector<HTMLElement>(
            "form input:not(:disabled), form select:not(:disabled), form textarea:not(:disabled), form button:not(:disabled)",
          )
          ?.focus();
      }
    }, 0);
    return () => window.clearTimeout(timeout);
  }, [editorMode, selectedId]);

  useEffect(() => {
    if (editorMode !== "edit" || !selectedOperator) {
      return;
    }
    setDraftUsername(selectedOperator.username);
    setDraftPassword("");
    setDraftRole(selectedOperator.role);
    setDraftScopes(selectedOperator.scopes.join(", "));
    const sessionTtlDays = secondsToDays(
      selectedOperator.session_refresh_ttl_secs,
    );
    setDraftSessionTtlDays(sessionTtlDays);
    setDraftSessionTtlCustomized(
      sessionTtlDays !== defaultSessionTtlDaysForRole(selectedOperator.role),
    );
    setActionError(null);
    setPendingAction(null);
    invalidateReviewGeneration();
  }, [editorMode, selectedOperator, invalidateReviewGeneration]);

  useEffect(() => {
    if (
      editorMode === "edit" &&
      selectedId &&
      !operators.some((operator) => operator.id === selectedId)
    ) {
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
        header: "User",
        cell: (row) => (
          <span className="operatorRecordName">
            <strong>{row.username}</strong>
            <small title={row.id}>{shortId(row.id)}</small>
          </span>
        ),
        searchValue: (row) => row.username,
        sortValue: (row) => row.username,
        minSize: 190,
      },
      {
        id: "status",
        header: "Status",
        cell: (row) => (
          <span className={`statusPill ${statusClass(row.status)}`}>
            {operatorStatusLabel(row.status)}
          </span>
        ),
        searchValue: (row) => row.status,
        sortValue: (row) => row.status,
        minSize: 96,
      },
      {
        id: "role",
        header: "Role",
        cell: (row) => (
          <span className={`statusPill ${statusClass(row.role)}`}>
            {operatorRoleLabel(row.role)}
          </span>
        ),
        searchValue: (row) => row.role,
        sortValue: (row) => row.role,
        minSize: 105,
      },
      {
        id: "mfa",
        header: "MFA",
        cell: (row) => {
          const mfaState = operatorMfaState(row);
          return (
            <span
              className={`status operatorMfaStatus ${mfaState.statusClass}`}
              title={mfaState.detail}
            >
              {mfaState.label}
            </span>
          );
        },
        searchValue: (row) => operatorMfaState(row).label,
        sortValue: (row) => (row.totp_enabled ? 1 : 0),
        minSize: 172,
      },
      {
        id: "sessions",
        header: "Active sessions",
        cell: (row) =>
          `${formatLowerBoundCount(
            accessSummaries[row.id]?.activeSessions ?? 0,
            sessionsTruncated,
          )}${sessionsTruncated ? " loaded" : ""}`,
        sortValue: (row) => accessSummaries[row.id]?.activeSessions ?? 0,
        minSize: 112,
      },
      {
        id: "lastLogin",
        header: "Last login",
        cell: (row) => (
          <OperatorAuthTimeValue
            event={accessSummaries[row.id]?.lastLogin}
            historyTruncated={authEventsTruncated}
          />
        ),
        searchValue: (row) =>
          formatAuthEventTime(
            accessSummaries[row.id]?.lastLogin,
            authEventsTruncated,
          ),
        sortValue: (row) =>
          accessSummaries[row.id]?.lastLogin?.created_at ?? "",
        minSize: 120,
      },
    ],
    [
      accessSummaries,
      actionPending,
      canManageUsers,
      privilegeMaterial,
      reviewPending,
      sessionsTruncated,
    ],
  );

  function invalidateUserReview() {
    setPendingAction(null);
    setReviewPending(false);
    setActionError(null);
    setActionStatus(null);
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
    setDraftSessionTtlCustomized(false);
    setActionError(null);
  }

  function closeEditor() {
    invalidateUserReview();
    setSelectedId(null);
    setEditorMode("closed");
    setActionError(null);
    setActionStatus(null);
  }

  async function requestPendingAction(
    builder: (material: PrivilegeMaterial) => Promise<PendingUserAction>,
  ) {
    if (!privilegeMaterial) {
      setActionError(null);
      setActionStatus(null);
      onOpenPrivilegeUnlock();
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    setReviewPending(true);
    setActionError(null);
    setActionStatus(null);
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
      setActionError(
        error instanceof Error ? error.message : "Privilege assertion failed",
      );
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

  async function submitBulkStatus(
    rows: OperatorView[],
    status: "active" | "disabled" | "deleted",
  ) {
    const operatorsToChange = rows.filter(
      (operator) => operator.status !== "deleted",
    );
    if (operatorsToChange.length === 0) {
      return;
    }
    const adminRisk = operatorsToChange.some(
      (operator) => operator.role === "admin",
    );
    const action =
      status === "active"
        ? "operator.enable"
        : status === "disabled"
          ? "operator.disable"
          : "operator.delete";
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
    await submitOperatorSessionRevoke(selectedOperator, selectedAccessSummary);
  }

  async function submitOperatorSessionRevoke(
    operator: OperatorView,
    summary: OperatorAccessSummary | undefined,
  ) {
    const sessionsToRevoke = summary?.revokableSessions ?? [];
    if (sessionsToRevoke.length === 0) {
      setActionError(
        "No non-current active sessions are available to revoke for this user",
      );
      return;
    }
    const adminRisk =
      operator.role === "admin" ||
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
        operator,
        sessions: sessionsToRevoke,
        adminRisk,
        privileges,
      };
    });
  }

  async function submitBulkTotpClear(rows: OperatorView[]) {
    const operatorsToChange = rows.filter(
      (operator) => operator.totp_enabled && operator.status !== "deleted",
    );
    if (operatorsToChange.length === 0) {
      return;
    }
    const adminRisk = operatorsToChange.some(
      (operator) => operator.role === "admin",
    );
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
        const username = pendingAction.username;
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
        setActionStatus(`Created operator ${username}`);
      } else if (pendingAction.kind === "update") {
        await onUpdateOperator(
          pendingAction.operator.id,
          pendingAction.role,
          pendingAction.scopes,
          pendingAction.sessionRefreshTtlSecs,
          pendingAction.adminRisk,
          pendingAction.privilege.privilegeAssertion,
        );
        setActionStatus(`Updated operator ${pendingAction.operator.username}`);
      } else if (pendingAction.kind === "status") {
        for (const operator of pendingAction.operators) {
          await onSetOperatorStatus(
            operator.id,
            pendingAction.status,
            pendingAction.adminRisk,
            pendingAction.privileges[operator.id].privilegeAssertion,
          );
        }
        const statusVerb =
          pendingAction.status === "active"
            ? "Enabled"
            : pendingAction.status === "disabled"
              ? "Disabled"
              : "Deleted";
        setActionStatus(
          `${statusVerb} ${pendingAction.operators.length} operator${pendingAction.operators.length === 1 ? "" : "s"}`,
        );
      } else if (pendingAction.kind === "password") {
        await onResetOperatorPassword(
          pendingAction.operator.id,
          pendingAction.password,
          pendingAction.adminRisk,
          pendingAction.privilege.privilegeAssertion,
        );
        setDraftPassword("");
        setActionStatus(
          `Reset password for ${pendingAction.operator.username}`,
        );
      } else if (pendingAction.kind === "totp") {
        for (const operator of pendingAction.operators) {
          await onClearOperatorTotp(
            operator.id,
            pendingAction.adminRisk,
            pendingAction.privileges[operator.id].privilegeAssertion,
          );
        }
        setActionStatus(
          `Cleared TOTP for ${pendingAction.operators.length} operator${pendingAction.operators.length === 1 ? "" : "s"}`,
        );
      } else {
        for (const session of pendingAction.sessions) {
          await onRevokeOperatorSession(
            session.id,
            pendingAction.adminRisk,
            pendingAction.privileges[session.id].privilegeAssertion,
          );
        }
        setActionStatus(
          `Revoked ${pendingAction.sessions.length} bearer session${pendingAction.sessions.length === 1 ? "" : "s"} for ${pendingAction.operator.username}`,
        );
      }
      setPendingAction(null);
    } catch (error) {
      setActionError(
        error instanceof Error ? error.message : "User action failed",
      );
    } finally {
      setActionPending(false);
    }
  }

  const editingDeleted = selectedOperator?.status === "deleted";

  return (
    <div className="workspace singleColumn systemWorkspace systemUsersWorkspace">
      <ActionFeedback
        className="localActionFeedback"
        message={
          loadError ??
          (loadLoading ? "Refreshing operator access records" : null)
        }
        tone={loadError ? "danger" : "progress"}
      />
      <section
        className="controlPanel systemUserGovernancePanel"
        aria-label="Operator governance overview"
      >
        <div className="sectionHeader fleetInstancesHeader">
          <div>
            <h2>Operator access policy</h2>
            <span>
              Human operator accounts, MFA posture, refresh-token lifetime,
              sessions, and loaded authentication evidence.
            </span>
          </div>
          <ConsoleStatusBadge tone={governanceTone}>
            {governanceTone === "ok"
              ? "Ready"
              : governanceTone === "info"
                ? "Loaded evidence"
                : "Needs review"}
          </ConsoleStatusBadge>
        </div>
        <div className="systemPostureGrid operatorPostureGrid operatorPolicyGrid">
          <SystemPostureTile
            detail="This console reports each operator's TOTP state. It cannot verify role-based enforcement, so the page reports MFA as recommended rather than enforced."
            icon={<ShieldCheck size={18} />}
            label="MFA policy"
            tone={adminWithoutMfaCount > 0 ? "warning" : "ok"}
            value={
              adminWithoutMfaCount > 0
                ? `${adminWithoutMfaCount} admin needs MFA`
                : "MFA enabled for admins"
            }
          />
          <SystemPostureTile
            detail={`Refresh TTL controls long-lived browser session renewal, not the short access token. Admin target is <=30d; longest visible admin TTL is ${maxAdminTtlDays || 0}d.`}
            icon={<TimerReset size={18} />}
            label="Refresh TTL policy"
            tone={adminLongTtlCount > 0 ? "warning" : "ok"}
            value={
              adminLongTtlCount > 0
                ? `${adminLongTtlCount} admin over target`
                : "Admins <=30d"
            }
          />
          <SystemPostureTile
            detail={`${visibleOperators.length} visible operator record${visibleOperators.length === 1 ? "" : "s"}. Standard roles are Viewer, Operator, and Admin; ${explicitScopeUsers} operator${explicitScopeUsers === 1 ? " has" : "s have"} explicit scope overrides${customRoles.length > 0 ? `, and ${customRoles.length} custom role${customRoles.length === 1 ? " is" : "s are"} loaded` : ""}.`}
            icon={<LockKeyhole size={18} />}
            label="Role model"
            tone={customRoles.length > 0 ? "warning" : "info"}
            value={`${operatorRoleOptions.length} standard roles`}
          />
          <SystemPostureTile
            detail={`${formatLowerBoundCount(revokableSessionCount, sessionsTruncated)} non-current active bearer session${revokableSessionCount === 1 ? "" : "s"}${sessionsTruncated ? " in loaded records" : ""} can be revoked here or reviewed in Audit / Sessions; ${formatLowerBoundCount(expiredSessionCount, sessionsTruncated)} expired bearer session${expiredSessionCount === 1 ? "" : "s"} excluded from${sessionsTruncated ? " loaded" : ""} active counts.`}
            icon={<Activity size={18} />}
            label="Bearer sessions"
            tone={activeSessionCount > 0 ? "info" : "neutral"}
            value={`${formatLowerBoundCount(
              activeSessionCount,
              sessionsTruncated,
            )} active${sessionsTruncated ? " loaded" : ""}`}
          />
          <SystemPostureTile
            detail={`${formatLowerBoundCount(knownAuthFailureCount, authEventsTruncated)} failure events map to visible operators; ${formatLowerBoundCount(Math.max(0, unknownAuthFailureCount), authEventsTruncated)}${authEventsTruncated ? " loaded" : ""} failures are unknown usernames. Per-user counts below use the same${authEventsTruncated ? " loaded" : ""} auth history.`}
            icon={<AlertTriangle size={18} />}
            label={
              authEventsTruncated
                ? "Auth failures in loaded history"
                : "Auth failures"
            }
            tone={
              authFailureEvents.length > 0
                ? "warning"
                : authEventsTruncated
                  ? "info"
                  : "ok"
            }
            value={
              authFailureEvents.length === 1
                ? `1${authEventsTruncated ? " loaded" : ""} failure`
                : `${formatLowerBoundCount(
                    authFailureEvents.length,
                    authEventsTruncated,
                  )}${authEventsTruncated ? " loaded" : ""} failures`
            }
          />
        </div>
        <div className="operatorPolicyNotice">
          <strong>Policy evidence boundary</strong>
          <span>
            Password age, invite state, locked state, and API-token inventory
            are unavailable. They are not counted as healthy or unhealthy
            posture here.
          </span>
        </div>
        <div className="operatorRoleMatrix" aria-label="RBAC role model">
          {operatorRoleOptions.map((role) => {
            const roleCount = visibleOperators.filter(
              (operator) => operator.role === role,
            ).length;
            return (
              <div
                className={role === "admin" ? "attention" : "neutral"}
                key={role}
              >
                <strong>{operatorRoleLabel(role)}</strong>
                <span>{roleDescription(role)}</span>
                <small>
                  {roleCount} user{roleCount === 1 ? "" : "s"} ·{" "}
                  {role === "admin"
                    ? "extra confirmation on grants"
                    : "scope overrides supported"}
                </small>
              </div>
            );
          })}
          {customRoles.map((role) => {
            const roleCount = visibleOperators.filter(
              (operator) => operator.role === role,
            ).length;
            return (
              <div className="attention" key={role}>
                <strong>{operatorRoleLabel(role)}</strong>
                <span>{roleDescription(role)}</span>
                <small>
                  {roleCount} user{roleCount === 1 ? "" : "s"} · custom role
                </small>
              </div>
            );
          })}
        </div>
      </section>
      <section className="controlPanel systemUsersTablePanel">
        <div className="sectionHeader fleetInstancesHeader">
          <div>
            <h2>Operator accounts</h2>
            <span>
              {operators.length} operator record
              {operators.length === 1 ? "" : "s"}
            </span>
          </div>
          <span className="sectionContext">
            {editorMode === "edit" && selectedOperator
              ? `Editing ${selectedOperator.username}`
              : editorMode === "create"
                ? "Creating new operator"
                : "Use New or select rows, then Actions"}
          </span>
        </div>
        {editorMode === "closed" ? (
          <ActionFeedback
            className="localActionFeedback"
            message={operatorActionFeedbackMessage}
            ref={operatorActionOutcomeRef}
            tone={operatorActionFeedbackTone}
          />
        ) : null}
        <ConsoleDataGrid
          actions={[
            {
              label: "Edit",
              description: (rows) =>
                rows.length === 1
                  ? `Load ${rows[0].username} into the editor.`
                  : "Select exactly one operator to edit.",
              disabled: (rows) => rows.length !== 1,
              icon: <Pencil size={14} />,
              onSelect: (rows) => setSelectedOperatorId(rows[0].id),
            },
            {
              label: "Revoke sessions",
              description: (rows) =>
                rows.length === 1
                  ? `Revoke non-current active sessions for ${rows[0].username}.`
                  : "Select exactly one operator whose sessions should be revoked.",
              disabled: (rows) =>
                rows.length !== 1 ||
                reviewPending ||
                actionPending ||
                !canManageUsers ||
                (accessSummaries[rows[0].id]?.revokableSessions.length ?? 0) ===
                  0,
              icon: <UserX size={14} />,
              onSelect: (rows) =>
                void submitOperatorSessionRevoke(
                  rows[0],
                  accessSummaries[rows[0].id],
                ),
            },
            {
              label: "Enable",
              description: (rows) =>
                rows.length === 1
                  ? `Allow ${rows[0].username} to log in again.`
                  : `Allow ${rows.length} disabled operators to log in again.`,
              disabled: (rows) =>
                reviewPending ||
                actionPending ||
                !canManageUsers ||
                rows.length === 0 ||
                rows.some((row) => row.status !== "disabled"),
              icon: <CheckCircle2 size={14} />,
              onSelect: (rows) => void submitBulkStatus(rows, "active"),
            },
            {
              label: "Disable",
              description: (rows) =>
                rows.length === 1
                  ? `Block ${rows[0].username} login and revoke existing sessions.`
                  : `Block login and revoke existing sessions for ${rows.length} operators.`,
              disabled: (rows) =>
                reviewPending ||
                actionPending ||
                !canManageUsers ||
                rows.length === 0 ||
                rows.some((row) => row.status !== "active"),
              icon: <UserX size={14} />,
              onSelect: (rows) => void submitBulkStatus(rows, "disabled"),
              tone: "danger",
            },
            {
              label: "Delete",
              description: (rows) =>
                rows.length === 1
                  ? `Delete ${rows[0].username} for login purposes and revoke existing sessions.`
                  : `Delete ${rows.length} operators for login purposes and revoke existing sessions.`,
              disabled: (rows) =>
                reviewPending ||
                actionPending ||
                !canManageUsers ||
                rows.length === 0 ||
                rows.some((row) => row.status === "deleted"),
              icon: <Trash2 size={14} />,
              onSelect: (rows) => void submitBulkStatus(rows, "deleted"),
              tone: "danger",
            },
            {
              label: "Clear TOTP",
              description: (rows) =>
                rows.length === 1
                  ? `Remove stored TOTP secret material for ${rows[0].username} and revoke existing sessions.`
                  : `Remove stored TOTP secret material and revoke sessions for ${rows.length} operators.`,
              disabled: (rows) =>
                reviewPending ||
                actionPending ||
                !canManageUsers ||
                rows.length === 0 ||
                rows.some(
                  (row) => !row.totp_enabled || row.status === "deleted",
                ),
              icon: <ShieldCheck size={14} />,
              onSelect: (rows) => void submitBulkTotpClear(rows),
            },
          ]}
          columns={userColumns}
          defaultPageSize={12}
          empty="No operators"
          expandOnRowClick
          getRowId={(row) => row.id}
          itemLabel="operators"
          renderExpandedRow={(row) => (
            <OperatorDetailGrid
              authEventsTruncated={authEventsTruncated}
              operator={row}
              sessionsTruncated={sessionsTruncated}
              summary={accessSummaries[row.id]}
            />
          )}
          renderSelectionPanel={(rows) => (
            <OperatorSelectionPanel rows={rows} />
          )}
          rows={operators}
          searchPlaceholder="Search username, role, status, MFA, or session evidence"
          singleExpandedRow
          storageKey="vpsman.system.users"
          title="Operator accounts"
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
        <section
          className="controlPanel operatorEditorPanel"
          aria-label="Operator user editor"
          ref={operatorEditorRef}
        >
          <div className="sectionHeader fleetInstancesHeader">
            <div>
              <h2>{selectedOperator ? "Edit user" : "Create user"}</h2>
              <span>
                {canManageUsers ? "Ready" : "Admin role required for changes"}
              </span>
            </div>
            <div className="sectionActions">
              {selectedOperator && (
                <span className="sectionContext">
                  {operatorStatusLabel(selectedOperator.status)} ·{" "}
                  {operatorRoleLabel(selectedOperator.role)} ·{" "}
                  {operatorRefreshTtlLabel(selectedOperator)}
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
          <form
            className="operatorEditorBody"
            onSubmit={(event) => event.preventDefault()}
          >
            <ActionFeedback
              className="localActionFeedback"
              message={operatorActionFeedbackMessage}
              ref={operatorActionOutcomeRef}
              tone={operatorActionFeedbackTone}
            />
            {selectedOperator && selectedAccessSummary ? (
              <OperatorAccessEvidencePanel
                authEventsTruncated={authEventsTruncated}
                operator={selectedOperator}
                onRevokeSessions={() => void submitSelectedSessionRevoke()}
                pending={reviewPending || actionPending}
                summary={selectedAccessSummary}
                sessionsTruncated={sessionsTruncated}
                userCanManage={canManageUsers}
              />
            ) : null}
            {draftRole === "admin" &&
            (!selectedOperator || selectedOperator.role !== "admin") ? (
              <div className="operatorAdminGuardrail" role="status">
                <AlertTriangle size={16} />
                <span>
                  Admin role grants require local privilege unlock, one
                  confirmation, reviewed scopes, and a refresh TTL that matches
                  the admin policy target.
                </span>
              </div>
            ) : null}
            <div className="operatorEditorFields">
              <label>
                <FieldLabel help={operatorHelpText.username} label="Username" />
                <input
                  aria-label="Operator username"
                  autoComplete="username"
                  data-tooltip-disabled-reason="Username is immutable after an operator account is created."
                  disabled={Boolean(selectedOperator)}
                  onChange={(event) => {
                    invalidateUserReview();
                    setDraftUsername(event.target.value);
                  }}
                  ref={operatorUsernameRef}
                  value={draftUsername}
                />
              </label>
              <label>
                <FieldLabel
                  help={
                    selectedOperator
                      ? operatorHelpText.newPassword
                      : operatorHelpText.createPassword
                  }
                  label={selectedOperator ? "New password" : "Password"}
                />
                <input
                  aria-label="Operator password"
                  autoComplete="new-password"
                  data-tooltip-disabled-reason={operatorFieldDisabledReason(
                    canManageUsers,
                    editingDeleted,
                  )}
                  data-tooltip-sensitive="true"
                  disabled={!canManageUsers || editingDeleted}
                  minLength={12}
                  onChange={(event) => {
                    invalidateUserReview();
                    setDraftPassword(event.target.value);
                  }}
                  placeholder={
                    selectedOperator ? "Only fill to reset" : "12+ characters"
                  }
                  type="password"
                  value={draftPassword}
                />
              </label>
              <label>
                <FieldLabel help={operatorHelpText.role} label="Role" />
                <select
                  aria-label="Operator role"
                  data-tooltip-disabled-reason={operatorFieldDisabledReason(
                    canManageUsers,
                    editingDeleted,
                  )}
                  disabled={!canManageUsers || editingDeleted}
                  onChange={(event) => {
                    invalidateUserReview();
                    const nextRole = event.target.value;
                    if (!draftSessionTtlCustomized) {
                      setDraftSessionTtlDays(
                        defaultSessionTtlDaysForRole(nextRole),
                      );
                    }
                    setDraftRole(nextRole);
                  }}
                  value={draftRole}
                >
                  {operatorRoleOptions.map((role) => (
                    <option key={role} value={role}>
                      {role}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <FieldLabel
                  help={operatorHelpText.sessionRefreshTtl}
                  label="Session TTL days"
                />
                <input
                  aria-label="Session refresh TTL days"
                  data-tooltip-disabled-reason={operatorFieldDisabledReason(
                    canManageUsers,
                    editingDeleted,
                  )}
                  disabled={!canManageUsers || editingDeleted}
                  max={3650}
                  min={1}
                  onChange={(event) => {
                    invalidateUserReview();
                    setDraftSessionTtlCustomized(true);
                    setDraftSessionTtlDays(Number(event.target.value));
                  }}
                  type="number"
                  value={draftSessionTtlDays}
                />
              </label>
            </div>

            {draftRole === "admin" &&
              draftSessionTtlDays > defaultAdminSessionTtlDays && (
                <ActionFeedback
                  className="localActionFeedback"
                  message={`Admin session TTL is above the ${defaultAdminSessionTtlDays}-day policy target. You may continue only if this longer renewal window is intentional.`}
                  tone="warning"
                />
              )}

            <div className="operatorScopeEditor">
              <label>
                <FieldLabel help={operatorHelpText.scopes} label="Scopes" />
                <textarea
                  aria-label="Operator scopes"
                  data-tooltip-disabled-reason={operatorFieldDisabledReason(
                    canManageUsers,
                    editingDeleted,
                  )}
                  disabled={!canManageUsers || editingDeleted}
                  onChange={(event) => {
                    invalidateUserReview();
                    setDraftScopes(event.target.value);
                  }}
                  placeholder="Leave empty for role defaults"
                  rows={4}
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
                    data-tooltip-disabled-reason={operatorFieldDisabledReason(
                      canManageUsers,
                      editingDeleted,
                    )}
                    disabled={!canManageUsers || editingDeleted}
                    key={scope}
                    onClick={() => {
                      invalidateUserReview();
                      setDraftScopes(addScopeToken(draftScopes, scope));
                    }}
                    title={
                      scope === "*"
                        ? "Append * to grant all operator scopes."
                        : `Append ${scope} to the scope override field.`
                    }
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
                    disabled={
                      !canManageUsers ||
                      editingDeleted ||
                      reviewPending ||
                      actionPending
                    }
                    onClick={() => void submitUpdate()}
                    title={operatorHelpText.save}
                    type="button"
                  >
                    <Save size={17} />
                    Save
                  </button>
                  <button
                    className="secondaryAction"
                    disabled={
                      !canManageUsers ||
                      editingDeleted ||
                      draftPassword.length < 12 ||
                      reviewPending ||
                      actionPending
                    }
                    onClick={() => void submitPasswordReset()}
                    title={operatorHelpText.resetPassword}
                    type="button"
                  >
                    <KeyRound size={17} />
                    Reset password
                  </button>
                  <button
                    className="secondaryAction"
                    disabled={
                      !canManageUsers ||
                      editingDeleted ||
                      !selectedOperator.totp_enabled ||
                      reviewPending ||
                      actionPending
                    }
                    onClick={() => void submitTotpClear()}
                    title={operatorHelpText.clearTotp}
                    type="button"
                  >
                    <ShieldCheck size={17} />
                    Clear TOTP
                  </button>
                  <button
                    className="secondaryAction"
                    disabled={
                      !canManageUsers ||
                      selectedOperator.status !== "disabled" ||
                      reviewPending ||
                      actionPending
                    }
                    onClick={() => void submitStatus("active")}
                    title={operatorHelpText.enable}
                    type="button"
                  >
                    <CheckCircle2 size={17} />
                    Enable
                  </button>
                  <button
                    className="secondaryAction dangerAction"
                    disabled={
                      !canManageUsers ||
                      selectedOperator.status !== "active" ||
                      reviewPending ||
                      actionPending
                    }
                    onClick={() => void submitStatus("disabled")}
                    title={operatorHelpText.disable}
                    type="button"
                  >
                    <UserX size={17} />
                    Disable
                  </button>
                  <button
                    className="secondaryAction dangerAction"
                    disabled={
                      !canManageUsers ||
                      editingDeleted ||
                      reviewPending ||
                      actionPending
                    }
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
                  disabled={
                    !canManageUsers ||
                    reviewPending ||
                    actionPending ||
                    Boolean(createDraftError)
                  }
                  onClick={() => void submitCreate()}
                  title={createDraftError ?? operatorHelpText.create}
                  type="button"
                >
                  <UserPlus size={17} />
                  Create
                </button>
              )}
            </div>
          </form>
        </section>
      )}
      <ConfirmationPrompt
        confirmLabel={pendingUserActionLabel(pendingAction)}
        detail={pendingUserActionDetail(pendingAction)}
        error={actionError ?? undefined}
        items={pendingUserActionItems(pendingAction)}
        onCancel={() => setPendingAction(null)}
        onConfirm={() => void confirmUserAction()}
        open={pendingAction !== null}
        pending={actionPending}
        title={
          pendingAction?.adminRisk
            ? "Confirm admin user action"
            : "Confirm user action"
        }
        tone={pendingAction?.adminRisk ? "danger" : "normal"}
      />
    </div>
  );
}

function SystemSessionsPanel({
  authEvents,
  authEventsTruncated,
  loadError,
  loadLoading,
  onOpenPrivilegeUnlock,
  onRevokeOperatorSession,
  privilegeMaterial,
  sessions,
  sessionsTruncated,
}: {
  authEvents: OperatorAuthEventRecord[];
  authEventsTruncated: boolean;
  loadError: string | null;
  loadLoading: boolean;
  onOpenPrivilegeUnlock: () => void;
  onRevokeOperatorSession: (
    sessionId: string,
    adminRiskAcknowledged: boolean,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  privilegeMaterial: PrivilegeMaterial | null;
  sessions: OperatorSessionRecord[];
  sessionsTruncated: boolean;
}) {
  const [pendingRevoke, setPendingRevoke] =
    useState<PendingSessionRevoke | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sessionStatus, setSessionStatus] = useState<string | null>(null);
  const sessionActionOutcomeRef = useRef<HTMLDivElement | null>(null);
  const previousSessionActionOutcomeRef = useRef<string | null>(null);
  const [authFilter, setAuthFilter] = useState<
    "all" | "failures" | "success" | "suspicious"
  >("all");
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const sessionEnrichment = useMemo(
    () => buildSessionEnrichmentMap(sessions, authEvents),
    [authEvents, sessions],
  );
  const failureGroups = useMemo(
    () => buildAuthFailureGroups(authEvents),
    [authEvents],
  );
  const filteredAuthEvents = useMemo(
    () =>
      authEvents.filter((event) =>
        authEventMatchesFilter(event, authFilter, failureGroups),
      ),
    [authEvents, authFilter, failureGroups],
  );
  const activeSessions = sessions.filter(isOperatorSessionUsable);
  const expiredSessions = sessions.filter(
    (session) => !session.revoked && isOperatorSessionExpired(session),
  );
  const currentSessions = activeSessions.filter(
    (session) => session.current,
  ).length;
  const revokableSessions = activeSessions.filter(
    (session) => !session.current,
  ).length;
  const adminSessions = activeSessions.filter(
    (session) => session.operator_role === "admin",
  ).length;
  const enrichedSessions = activeSessions.filter(
    (session) => sessionEnrichment[session.id]?.authEvent,
  ).length;
  const suspiciousSessions = activeSessions.filter(
    (session) =>
      sessionEnrichment[session.id]?.riskTone === "warning" ||
      sessionEnrichment[session.id]?.riskTone === "critical",
  ).length;
  const authFailureCount = authEvents.filter(
    (event) => event.result !== "success",
  ).length;
  const sessionActionFeedbackMessage =
    error ??
    (reviewPending
      ? "Preparing session revoke review"
      : pending
        ? "Revoking sessions"
        : sessionStatus);
  const sessionActionFeedbackTone = error
    ? "danger"
    : reviewPending || pending
      ? "progress"
      : "success";
  const sessionActionOutcome = pendingRevoke ? null : (error ?? sessionStatus);
  const uniqueRemoteIps = new Set(
    authEvents
      .map((event) => event.remote_ip)
      .filter((ip): ip is string => Boolean(ip)),
  ).size;
  const sessionColumns = useMemo<
    ConsoleDataGridColumn<OperatorSessionRecord>[]
  >(
    () => [
      {
        id: "operator",
        header: "User",
        cell: (row) => (
          <span className="sessionIdentityCell">
            <strong>{row.operator_username}</strong>
            <small title={row.id}>{shortId(row.id)}</small>
          </span>
        ),
        searchValue: (row) => `${row.operator_username} ${row.id}`,
        sortValue: (row) => row.operator_username,
      },
      {
        id: "role",
        header: "Role",
        cell: (row) => (
          <span
            className={`status ${row.operator_role === "admin" ? "warn" : "neutral"}`}
          >
            {row.operator_role}
          </span>
        ),
        searchValue: (row) => row.operator_role,
      },
      {
        id: "network",
        header: "IP / location",
        cell: (row) => (
          <SessionNetworkCell enrichment={sessionEnrichment[row.id]} />
        ),
        searchValue: (row) =>
          `${sessionEnrichment[row.id]?.remoteIp ?? ""} ${sessionEnrichment[row.id]?.location ?? ""}`,
      },
      {
        id: "device",
        header: "Browser / device",
        cell: (row) => (
          <SessionDeviceCell enrichment={sessionEnrichment[row.id]} />
        ),
        searchValue: (row) =>
          `${sessionEnrichment[row.id]?.browser ?? ""} ${sessionEnrichment[row.id]?.device ?? ""} ${sessionEnrichment[row.id]?.userAgent ?? ""}`,
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
        cell: (row) => (
          <SessionDateCell
            help={operatorHelpText.sessionAccessExpires}
            value={row.expires_at}
          />
        ),
        sortValue: (row) => row.expires_at,
      },
      {
        id: "refresh",
        header: "Refresh expires",
        cell: (row) => (
          <SessionDateCell
            help={operatorHelpText.sessionRefreshExpires}
            value={row.refresh_expires_at}
          />
        ),
        sortValue: (row) => row.refresh_expires_at,
      },
      {
        id: "state",
        header: "State",
        cell: (row) => (
          <span className={`status ${operatorSessionStateTone(row)}`}>
            {operatorSessionStateLabel(row)}
          </span>
        ),
        searchValue: (row) => operatorSessionStateLabel(row),
      },
      {
        id: "risk",
        header: "Risk",
        cell: (row) => (
          <SessionRiskCell enrichment={sessionEnrichment[row.id]} />
        ),
        searchValue: (row) =>
          `${sessionEnrichment[row.id]?.riskLabel ?? ""} ${sessionEnrichment[row.id]?.riskDetail ?? ""}`,
      },
    ],
    [pending, privilegeMaterial, reviewPending, sessionEnrichment],
  );
  const eventColumns = useMemo<
    ConsoleDataGridColumn<OperatorAuthEventRecord>[]
  >(
    () => [
      {
        id: "time",
        header: "Time",
        cell: (row) => <SessionDateCell value={row.created_at} />,
        sortValue: (row) => row.created_at,
      },
      {
        id: "username",
        header: "Username",
        cell: (row) => row.username,
        searchValue: (row) => row.username,
      },
      {
        id: "result",
        header: "Result",
        cell: (row) => (
          <span className={`status ${authEventRiskTone(row)}`}>
            {row.result}
          </span>
        ),
        searchValue: (row) => row.result,
      },
      {
        id: "reason",
        header: "Reason",
        cell: (row) => (
          <AuthReasonCell event={row} failureGroups={failureGroups} />
        ),
        searchValue: (row) => row.reason,
      },
      {
        id: "remote",
        header: "Remote IP",
        cell: (row) => row.remote_ip ?? "not recorded",
        searchValue: (row) => row.remote_ip,
      },
      {
        id: "agent",
        header: "Browser / device",
        cell: (row) => <AuthUserAgentCell event={row} />,
        searchValue: (row) => row.user_agent,
      },
      {
        id: "session",
        header: "Session",
        cell: (row) => (
          <span title={row.session_id ?? "No session ID"}>
            {shortId(row.session_id)}
          </span>
        ),
        searchValue: (row) => row.session_id,
      },
    ],
    [failureGroups],
  );

  useEffect(() => {
    setPendingRevoke(null);
    setReviewPending(false);
    invalidateReviewGeneration();
  }, [sessions, invalidateReviewGeneration]);

  useEffect(() => {
    if (!sessionActionOutcome) {
      previousSessionActionOutcomeRef.current = null;
      return;
    }
    if (previousSessionActionOutcomeRef.current === sessionActionOutcome) {
      return;
    }
    previousSessionActionOutcomeRef.current = sessionActionOutcome;
    const frame = window.requestAnimationFrame(() => {
      if (sessionActionOutcomeRef.current) {
        scrollIntoViewWithMotion(sessionActionOutcomeRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [sessionActionOutcome]);

  async function requestSessionRevoke(rows: OperatorSessionRecord[]) {
    setSessionStatus(null);
    const sessionsToRevoke = rows.filter(
      (session) => !session.current && isOperatorSessionUsable(session),
    );
    if (sessionsToRevoke.length === 0) {
      return;
    }
    if (!privilegeMaterial) {
      setError("Local privilege unlock is required");
      onOpenPrivilegeUnlock();
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    const adminRisk = sessionsToRevoke.some(
      (session) => session.operator_role === "admin",
    );
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
      setError(
        actionError instanceof Error
          ? actionError.message
          : "Privilege assertion failed",
      );
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
      const revokedCount = pendingRevoke.sessions.length;
      for (const session of pendingRevoke.sessions) {
        await onRevokeOperatorSession(
          session.id,
          pendingRevoke.adminRisk,
          pendingRevoke.privileges[session.id].privilegeAssertion,
        );
      }
      setSessionStatus(
        `Revoked ${revokedCount} bearer session${revokedCount === 1 ? "" : "s"}`,
      );
      setPendingRevoke(null);
    } catch (actionError) {
      setError(
        actionError instanceof Error
          ? actionError.message
          : "Session revoke failed",
      );
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="workspace singleColumn systemWorkspace">
      <ActionFeedback
        className="localActionFeedback"
        message={
          loadError ??
          (loadLoading ? "Refreshing session and authentication records" : null)
        }
        tone={loadError ? "danger" : "progress"}
      />
      <div className="workspaceStack">
        <section
          className="controlPanel sessionSecurityOverview"
          aria-label="Session evidence security overview"
        >
          <div className="sectionHeader fleetInstancesHeader">
            <div>
              <h2>Session security posture</h2>
              <span>
                Active bearer sessions, revocation readiness, IP/device
                evidence, and suspicious authentication signals.
              </span>
            </div>
            <ConsoleStatusBadge
              tone={
                suspiciousSessions > 0 || failureGroups.length > 0
                  ? "warning"
                  : sessionsTruncated || authEventsTruncated
                    ? "info"
                    : "ok"
              }
            >
              {suspiciousSessions > 0 || failureGroups.length > 0
                ? "Needs review"
                : sessionsTruncated || authEventsTruncated
                  ? "Loaded records clear"
                  : "Healthy"}
            </ConsoleStatusBadge>
          </div>
          <div className="systemPostureGrid sessionPostureGrid">
            <SystemPostureTile
              detail={`${formatLowerBoundCount(currentSessions, sessionsTruncated)} current usable browser session${sessionsTruncated ? " in loaded records" : ""}; ${formatLowerBoundCount(revokableSessions, sessionsTruncated)} non-current active bearer sessions can be revoked without ending the current console session; ${formatLowerBoundCount(expiredSessions.length, sessionsTruncated)} expired bearer session${expiredSessions.length === 1 ? "" : "s"} excluded.`}
              icon={<Activity size={18} />}
              label="Active sessions"
              tone={activeSessions.length > 0 ? "info" : "neutral"}
              value={`${formatLowerBoundCount(
                activeSessions.length,
                sessionsTruncated,
              )} active${sessionsTruncated ? " loaded" : ""}`}
            />
            <SystemPostureTile
              detail="Admin sessions are higher-risk and require admin-risk acknowledgement when revoked."
              icon={<ShieldCheck size={18} />}
              label="Admin sessions"
              tone={
                adminSessions > 0
                  ? "warning"
                  : sessionsTruncated
                    ? "info"
                    : "ok"
              }
              value={`${formatLowerBoundCount(
                adminSessions,
                sessionsTruncated,
              )} admin${sessionsTruncated ? " loaded" : ""}`}
            />
            <SystemPostureTile
              detail={`${formatLowerBoundCount(enrichedSessions, sessionsTruncated)} active bearer sessions${sessionsTruncated ? " in loaded records" : ""} have login-event IP and user-agent evidence; missing evidence is flagged per row.`}
              icon={<Network size={18} />}
              label="IP/device evidence"
              tone={
                sessionsTruncated
                  ? "info"
                  : enrichedSessions === activeSessions.length
                    ? "ok"
                    : "warning"
              }
              value={`${enrichedSessions}/${activeSessions.length}${sessionsTruncated ? " loaded" : ""} enriched`}
            />
            <SystemPostureTile
              detail={`${formatLowerBoundCount(uniqueRemoteIps, authEventsTruncated)} remote IPs are visible${authEventsTruncated ? " in loaded auth history" : ""}. IP locations are unavailable, so impossible-travel detection is not evaluated.`}
              icon={<ServerCog size={18} />}
              label="Location enrichment"
              tone="neutral"
              value="Unavailable"
            />
            <SystemPostureTile
              detail={`${formatLowerBoundCount(authFailureCount, authEventsTruncated)} non-success authentication events across ${formatLowerBoundCount(failureGroups.length, authEventsTruncated)} grouped failure patterns${authEventsTruncated ? " in loaded history" : ""}.`}
              icon={<AlertTriangle size={18} />}
              label="Suspicious auth"
              tone={
                authFailureCount > 0
                  ? "warning"
                  : authEventsTruncated
                    ? "info"
                    : "ok"
              }
              value={
                authFailureCount === 1
                  ? `1${authEventsTruncated ? " loaded" : ""} failure`
                  : `${formatLowerBoundCount(
                      authFailureCount,
                      authEventsTruncated,
                    )}${authEventsTruncated ? " loaded" : ""} failures`
              }
            />
            <SystemPostureTile
              detail="Select one or more active bearer sessions, then use Actions to review revocation."
              icon={<UserX size={18} />}
              label="Revocation"
              tone={
                revokableSessions > 0
                  ? "info"
                  : sessionsTruncated
                    ? "info"
                    : "ok"
              }
              value={`${formatLowerBoundCount(
                revokableSessions,
                sessionsTruncated,
              )} revocable${sessionsTruncated ? " loaded" : ""}`}
            />
          </div>
        </section>
        <section className="controlPanel">
          <div className="sectionHeader compact">
            <h2>Sessions</h2>
            <span>
              {formatLowerBoundCount(sessions.length, sessionsTruncated)} recent
              {sessionsTruncated ? " loaded" : ""} sessions
            </span>
          </div>
          <ActionFeedback
            className="localActionFeedback systemSessionActionFeedback"
            message={sessionActionFeedbackMessage}
            ref={sessionActionOutcomeRef}
            tone={sessionActionFeedbackTone}
          />
          <ConsoleDataGrid
            actions={[
              {
                label: "Revoke",
                description: (rows) =>
                  rows.length === 1
                    ? `Revoke the bearer session for ${rows[0].operator_username}.`
                    : `Revoke ${rows.length} selected bearer sessions.`,
                tone: "danger",
                icon: <UserX size={14} />,
                disabled: (rows) =>
                  reviewPending ||
                  pending ||
                  rows.length === 0 ||
                  rows.some(
                    (row) => row.current || !isOperatorSessionUsable(row),
                  ),
                onSelect: (rows) => void requestSessionRevoke(rows),
              },
            ]}
            columns={sessionColumns}
            defaultPageSize={12}
            empty="No operator sessions"
            expandOnRowClick
            getRowId={(row) => row.id}
            itemLabel="sessions"
            renderExpandedRow={(row) => (
              <SessionDetailGrid
                enrichment={sessionEnrichment[row.id]}
                session={row}
              />
            )}
            renderSelectionPanel={(rows) => (
              <SessionSelectionPanel rows={rows} />
            )}
            rows={sessions}
            rowsTruncated={sessionsTruncated}
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
              <span>
                {authEventsTruncated
                  ? `${filteredAuthEvents.length} matching from ${formatLowerBoundCount(
                      authEvents.length,
                      true,
                    )} loaded login results`
                  : `${filteredAuthEvents.length} of ${authEvents.length} login results`}
              </span>
            </div>
            <div
              className="segmented sessionAuthFilters"
              role="group"
              aria-label="Authentication history filters"
            >
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
                  onClick={() =>
                    setAuthFilter(option.value as typeof authFilter)
                  }
                  type="button"
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>
          {failureGroups.length > 0 ? (
            <div
              className="authFailureGroups"
              aria-label="Authentication failure groups"
            >
              <div className="dashboardSideRailHeader">
                <strong>Grouped failures</strong>
                <span>
                  {formatLowerBoundCount(
                    failureGroups.length,
                    authEventsTruncated,
                  )}
                  {authEventsTruncated ? " loaded" : ""} patterns
                </span>
              </div>
              {failureGroups.map((group) => (
                <div className="authFailureGroupRow" key={group.key}>
                  <span className="status warn">{group.riskLabel}</span>
                  <strong>{group.username}</strong>
                  <b>
                    {formatLowerBoundCount(group.count, authEventsTruncated)}
                    {authEventsTruncated ? " loaded" : ""} attempt
                    {group.count === 1 && !authEventsTruncated ? "" : "s"}
                  </b>
                  <p>
                    {group.reason} from {group.remoteIp}; last{" "}
                    {formatTime(group.lastAt)}. {group.riskDetail}
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
            rowsTruncated={authEventsTruncated}
            searchPlaceholder="Search username, result, reason, remote IP, browser, device, or session"
            selectable={false}
            singleExpandedRow
            storageKey="vpsman.system.authEvents"
            title="Authentication history"
          />
        </section>
      </div>
      <ConfirmationPrompt
        confirmLabel={
          (pendingRevoke?.sessions.length ?? 0) === 1
            ? "Revoke session"
            : "Revoke sessions"
        }
        detail={
          pendingRevoke?.sessions.some(
            (session) => session.operator_role === "admin",
          )
            ? "This revokes an admin user's bearer session. Existing browser state for that session will stop working after the current access token expires or is checked again."
            : (pendingRevoke?.sessions.length ?? 0) === 1
              ? "This revokes the selected bearer session."
              : "This revokes the selected bearer sessions."
        }
        error={error ?? undefined}
        items={[
          { label: "Sessions", value: pendingRevoke?.sessions.length ?? 0 },
          {
            label: "Operators",
            value:
              pendingRevoke?.sessions
                .map((session) => session.operator_username)
                .join(", ") || "-",
          },
          {
            label: "Admin sessions",
            value:
              pendingRevoke?.sessions.filter(
                (session) => session.operator_role === "admin",
              ).length ?? 0,
          },
          {
            label: "Payload",
            title: pendingRevoke?.sessions[0]
              ? pendingRevoke.privileges[pendingRevoke.sessions[0].id]
                  .payloadHashHex
              : undefined,
            value: pendingRevoke?.sessions[0]
              ? shortId(
                  pendingRevoke.privileges[pendingRevoke.sessions[0].id]
                    .payloadHashHex,
                )
              : "-",
          },
        ]}
        onCancel={() => setPendingRevoke(null)}
        onConfirm={() => void confirmSessionRevoke()}
        open={Boolean(pendingRevoke)}
        pending={pending}
        title={
          pendingRevoke?.sessions.some(
            (session) => session.operator_role === "admin",
          )
            ? "Confirm admin session revoke"
            : "Confirm session revoke"
        }
        tone="danger"
      />
    </div>
  );
}

function OperatorAccessEvidencePanel({
  authEventsTruncated,
  onRevokeSessions,
  operator,
  pending,
  sessionsTruncated,
  summary,
  userCanManage,
}: {
  authEventsTruncated: boolean;
  onRevokeSessions: () => void;
  operator: OperatorView;
  pending: boolean;
  sessionsTruncated: boolean;
  summary: OperatorAccessSummary;
  userCanManage: boolean;
}) {
  const ttlDays = secondsToDays(operator.session_refresh_ttl_secs);
  const adminTtlRisk = operator.role === "admin" && ttlDays > 30;
  const mfaState = operatorMfaState(operator);
  return (
    <section
      className="operatorAccessEvidencePanel"
      aria-label="Operator access evidence"
    >
      <div className="dashboardSideRailHeader">
        <strong>Access evidence</strong>
        <span>{operator.username}</span>
      </div>
      <div className="operatorEvidenceGrid">
        <OperatorEvidenceTile
          label="MFA"
          detail={mfaState.detail}
          tone={mfaState.tone}
          value={mfaState.label}
        />
        <OperatorEvidenceTile
          label="Refresh TTL"
          detail={operatorRefreshTtlDetail(operator)}
          tone={adminTtlRisk ? "warning" : "neutral"}
          value={operatorRefreshTtlLabel(operator)}
        />
        <OperatorEvidenceTile
          label="Active sessions"
          tone={
            summary.activeSessions > 0 || sessionsTruncated ? "info" : "neutral"
          }
          value={`${formatLowerBoundCount(
            summary.activeSessions,
            sessionsTruncated,
          )}${sessionsTruncated ? " loaded" : ""}`}
        />
        <OperatorEvidenceTile
          label="Failed logins"
          tone={
            summary.failedLogins > 0
              ? "warning"
              : authEventsTruncated
                ? "info"
                : "ok"
          }
          value={
            authEventsTruncated && summary.failedLogins === 0
              ? "None in loaded history"
              : `${formatLowerBoundCount(
                  summary.failedLogins,
                  authEventsTruncated,
                )}${authEventsTruncated ? " loaded" : ""}`
          }
        />
        <OperatorEvidenceTile
          label="Last login"
          value={formatAuthEventTime(summary.lastLogin, authEventsTruncated)}
        />
        <OperatorEvidenceTile
          label="Lifecycle"
          tone={operator.status === "active" ? "ok" : "warning"}
          value={operatorLifecycleLabel(operator)}
        />
      </div>
      <div className="operatorEvidenceFooter">
        <span>
          Password age, invite state, locked state, and API-token inventory are
          unavailable for this operator.
        </span>
        <button
          className="secondaryAction compactAction"
          disabled={
            !userCanManage || pending || summary.revokableSessions.length === 0
          }
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
  detail,
  label,
  tone = "neutral",
  value,
}: {
  detail?: string;
  label: string;
  tone?: SystemHealthTone;
  value: string;
}) {
  return (
    <span className={`operatorEvidenceTile ${tone}`} title={detail}>
      <small>{label}</small>
      <strong>{value}</strong>
    </span>
  );
}

function OperatorDetailGrid({
  authEventsTruncated,
  operator,
  sessionsTruncated,
  summary,
}: {
  authEventsTruncated: boolean;
  operator: OperatorView;
  sessionsTruncated: boolean;
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
        <span>{operatorStatusLabel(operator.status)}</span>
      </span>
      <span>
        <strong>Role</strong>
        <span>{operatorRoleLabel(operator.role)}</span>
      </span>
      <span>
        <strong>Refresh TTL</strong>
        <span title={operatorRefreshTtlDetail(operator)}>
          {operatorRefreshTtlLabel(operator)}
        </span>
      </span>
      <span>
        <strong>MFA</strong>
        <span title={operatorMfaState(operator).detail}>
          {operatorMfaState(operator).label}
        </span>
      </span>
      <span>
        <strong>Active sessions</strong>
        <span>
          {formatLowerBoundCount(
            summary?.activeSessions ?? 0,
            sessionsTruncated,
          )}
          {sessionsTruncated ? " loaded" : ""}
        </span>
      </span>
      <span>
        <strong>Last login</strong>
        <span>
          <OperatorAuthTimeValue
            event={summary?.lastLogin ?? null}
            historyTruncated={authEventsTruncated}
          />
        </span>
      </span>
      <span>
        <strong>Auth failures</strong>
        <span>
          {authEventsTruncated && (summary?.failedLogins ?? 0) === 0
            ? "None in loaded history"
            : `${formatLowerBoundCount(
                summary?.failedLogins ?? 0,
                authEventsTruncated,
              )}${authEventsTruncated ? " loaded" : ""}`}
        </span>
      </span>
      <span>
        <strong>Scopes</strong>
        <span>
          {operator.scopes.length > 0
            ? operator.scopes.join(", ")
            : "role defaults"}
        </span>
      </span>
      <span>
        <strong>Created</strong>
        <span>
          <FullTimeValue value={operator.created_at} />
        </span>
      </span>
      <span>
        <strong>Disabled</strong>
        <span>
          {operator.disabled_at ? (
            <FullTimeValue value={operator.disabled_at} />
          ) : (
            "No"
          )}
        </span>
      </span>
      <span>
        <strong>Deleted</strong>
        <span>
          {operator.deleted_at ? (
            <FullTimeValue value={operator.deleted_at} />
          ) : (
            "No"
          )}
        </span>
      </span>
      <span>
        <strong>API evidence boundary</strong>
        <span>
          Password age, invite state, locked state, and API tokens are not
          exposed.
        </span>
      </span>
    </div>
  );
}

function OperatorSelectionPanel({ rows }: { rows: OperatorView[] }) {
  const adminCount = rows.filter(
    (operator) => operator.role === "admin",
  ).length;
  const activeCount = rows.filter(
    (operator) => operator.status === "active",
  ).length;
  const totpCount = rows.filter((operator) => operator.totp_enabled).length;
  return (
    <div className="gridSelectionSummary">
      <span>
        <strong>{rows.length}</strong>
        selected
      </span>
      <span>
        <strong>{adminCount}</strong>
        Admin role
      </span>
      <span>
        <strong>{activeCount}</strong>
        Active
      </span>
      <span>
        <strong>{totpCount}</strong>
        MFA enabled
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

function SessionNetworkCell({
  enrichment,
}: {
  enrichment?: SessionEnrichment;
}) {
  const remoteIp = enrichment?.remoteIp ?? "IP not recorded";
  const location = enrichment?.location ?? "Location unavailable";
  return (
    <span className="sessionNetworkCell" title={`${remoteIp} / ${location}`}>
      <strong>{remoteIp}</strong>
      <small>{location}</small>
    </span>
  );
}

function SessionDeviceCell({ enrichment }: { enrichment?: SessionEnrichment }) {
  return (
    <span
      className="sessionDeviceCell"
      title={enrichment?.userAgent ?? "Login user agent not recorded"}
    >
      <strong>{enrichment?.browser ?? "Unknown browser"}</strong>
      <small>{enrichment?.device ?? "Device not recorded"}</small>
    </span>
  );
}

function SessionRiskCell({ enrichment }: { enrichment?: SessionEnrichment }) {
  return (
    <span
      className={`status ${enrichment?.riskTone ?? "neutral"}`}
      title={enrichment?.riskDetail}
    >
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
  const repeated = failureGroups.find(
    (group) => authFailureGroupKey(event) === group.key && group.count > 1,
  );
  const reason = event.reason ?? "accepted";
  return (
    <span
      className="authReasonCell"
      title={`${reason} - ${authEventRiskLabel(event)}`}
    >
      <strong>{reason}</strong>
      {repeated ? (
        <small>{repeated.count} repeated attempts</small>
      ) : (
        <small>{authEventRiskLabel(event)}</small>
      )}
    </span>
  );
}

function AuthUserAgentCell({ event }: { event: OperatorAuthEventRecord }) {
  const parsed = parseUserAgent(event.user_agent);
  return (
    <span
      className="sessionDeviceCell"
      title={event.user_agent ?? "User agent not recorded"}
    >
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
        <span>{enrichment?.location ?? "Location unavailable"}</span>
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
        <span>{operatorSessionStateLabel(session)}</span>
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
        <span>
          {session.revoked_at ? formatTime(session.revoked_at) : "no"}
        </span>
      </span>
    </div>
  );
}

function SessionSelectionPanel({ rows }: { rows: OperatorSessionRecord[] }) {
  const revokable = rows.filter(
    (session) => !session.current && isOperatorSessionUsable(session),
  ).length;
  const current = rows.filter((session) => session.current).length;
  const revoked = rows.filter((session) => session.revoked).length;
  const expired = rows.filter(
    (session) => !session.revoked && isOperatorSessionExpired(session),
  ).length;
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
      <span>
        <strong>{expired}</strong>
        expired
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
        <span
          className="monoValue"
          data-tooltip-empty-reason="This authentication event is not linked to an operator record."
        >
          {event.operator_id ?? "-"}
        </span>
      </span>
      <span>
        <strong>Result</strong>
        <span>{event.result}</span>
      </span>
      <span>
        <strong>Reason</strong>
        <span data-tooltip-empty-reason="No authentication reason was recorded for this event.">
          {event.reason ?? "-"}
        </span>
      </span>
      <span>
        <strong>Remote IP</strong>
        <span data-tooltip-empty-reason="No remote IP address was recorded for this authentication event.">
          {event.remote_ip ?? "-"}
        </span>
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
        <span
          className="monoValue"
          data-tooltip-empty-reason="This authentication event is not linked to a bearer session."
        >
          {event.session_id ?? "-"}
        </span>
      </span>
      <span>
        <strong>User agent</strong>
        <span data-tooltip-empty-reason="No browser or client user agent was recorded for this authentication event.">
          {event.user_agent ?? "-"}
        </span>
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
    if (!summary || !isOperatorSessionUsable(session)) {
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
      .sort(
        (left, right) =>
          Date.parse(right.created_at) - Date.parse(left.created_at),
      )
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
          location: authEvent?.remote_ip
            ? "Location unavailable"
            : "Location not recorded",
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

function buildAuthFailureGroups(
  authEvents: OperatorAuthEventRecord[],
): AuthFailureGroup[] {
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
        user_agent:
          group.userAgent === "User agent not recorded"
            ? null
            : group.userAgent,
        username: group.username,
      },
      group.count,
    );
    group.riskLabel = risk.label;
    group.riskDetail = risk.detail;
  }
  return Array.from(groups.values()).sort(
    (left, right) => Date.parse(right.lastAt) - Date.parse(left.lastAt),
  );
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
  return (
    authEventRiskTone(event, failureGroups) === "warning" ||
    authEventRiskTone(event, failureGroups) === "critical"
  );
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
  const group = failureGroups.find(
    (item) => item.key === authFailureGroupKey(event),
  );
  if (event.result === "throttled" || (group?.count ?? 0) >= 5) {
    return "critical";
  }
  return "warning";
}

function authEventRiskLabel(event: OperatorAuthEventRecord): string {
  if (event.result === "success") {
    return event.remote_ip && event.user_agent
      ? "Accepted login"
      : "Accepted; evidence incomplete";
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
  event: Pick<
    OperatorAuthEventRecord,
    "operator_id" | "reason" | "result" | "username"
  > &
    Partial<OperatorAuthEventRecord>,
  count: number,
): { detail: string; label: string } {
  if (event.result === "throttled" || count >= 5) {
    return {
      detail:
        "High-volume or throttled authentication failures need investigation.",
      label: "High-risk failure",
    };
  }
  if (!event.operator_id) {
    return {
      detail:
        "No operator matched this username; verify whether this is mistyped access or credential probing.",
      label: count > 1 ? "Repeated unknown user" : "Unknown user",
    };
  }
  return {
    detail:
      "Failure maps to a known operator; review reason, IP, and user agent.",
    label: count > 1 ? "Repeated failure" : "Failed login",
  };
}

function sessionRisk(
  session: OperatorSessionRecord,
  authEvent: OperatorAuthEventRecord | null,
): { detail: string; label: string; tone: SystemHealthTone } {
  if (session.revoked) {
    return {
      detail: "This session has already been revoked.",
      label: "Revoked",
      tone: "neutral",
    };
  }
  if (isOperatorSessionExpired(session)) {
    return {
      detail:
        "This bearer session is expired and excluded from active-session and active-revoke counts.",
      label: "Expired bearer",
      tone: "neutral",
    };
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
      detail:
        "Non-current admin sessions should be reviewed and revoked when no longer needed.",
      label: "Admin review",
      tone: "warning",
    };
  }
  if (session.current) {
    return {
      detail: "This is the current console session.",
      label: "Current",
      tone: "info",
    };
  }
  return {
    detail: "Login event, IP, and user agent are present.",
    label: "Normal",
    tone: "ok",
  };
}

function parseUserAgent(userAgent: string | null): {
  browser: string;
  device: string;
} {
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
  if (
    lower.includes("mobile") ||
    lower.includes("android") ||
    lower.includes("iphone")
  ) {
    device = "Mobile browser";
  } else if (lower.includes("playwright")) {
    device = "Browser automation";
  } else if (lower.includes("curl/")) {
    device = "CLI client";
  }
  return { browser, device };
}

function operatorRoleLabel(role: string): string {
  if (role === "admin") return "Admin";
  if (role === "operator") return "Operator";
  if (role === "viewer") return "Viewer";
  return role.trim() || "Unknown role";
}

function operatorStatusLabel(status: string): string {
  if (status === "active") return "Active";
  if (status === "disabled") return "Disabled";
  if (status === "deleted") return "Deleted";
  return status.trim() || "Unknown";
}

function operatorSessionStateLabel(session: OperatorSessionRecord): string {
  if (session.revoked) {
    return "revoked";
  }
  if (isPastTime(session.refresh_expires_at)) {
    return "refresh expired";
  }
  if (isPastTime(session.expires_at)) {
    return "refreshable";
  }
  return session.current ? "current" : "active";
}

function operatorSessionStateTone(session: OperatorSessionRecord): string {
  if (session.revoked) {
    return "warn";
  }
  if (isOperatorSessionExpired(session)) {
    return "neutral";
  }
  return session.current ? "info" : "ok";
}

function isOperatorSessionUsable(session: OperatorSessionRecord): boolean {
  return !session.revoked && !isOperatorSessionExpired(session);
}

function isOperatorSessionExpired(session: OperatorSessionRecord): boolean {
  return isPastTime(session.refresh_expires_at);
}

function isPastTime(value: string): boolean {
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) && timestamp <= Date.now();
}

function operatorMfaState(operator: OperatorView): {
  detail: string;
  label: string;
  statusClass: "neutral" | "ok" | "warn";
  tone: SystemHealthTone;
} {
  if (operator.totp_enabled) {
    return {
      detail: "TOTP is enrolled for this operator.",
      label: "MFA enabled",
      statusClass: "ok",
      tone: "ok",
    };
  }
  if (operator.role === "admin") {
    return {
      detail:
        "Admin account has no TOTP enrollment. Enroll TOTP before long-lived access or privileged workflows.",
      label: "Policy recommends MFA",
      statusClass: "warn",
      tone: "warning",
    };
  }
  return {
    detail: "TOTP is not enrolled for this non-admin operator.",
    label: "MFA off",
    statusClass: "neutral",
    tone: "neutral",
  };
}

function operatorRefreshTtlLabel(operator: OperatorView): string {
  return operatorRefreshTtlLabelFor(
    operator.role,
    operator.session_refresh_ttl_secs,
  );
}

function operatorRefreshTtlLabelFor(
  role: string,
  sessionRefreshTtlSecs: number,
): string {
  const ttlDays = secondsToDays(sessionRefreshTtlSecs);
  if (role === "admin" && ttlDays > 30) {
    return `${ttlDays}d - over admin target`;
  }
  if (role === "admin") {
    return `${ttlDays}d - within admin target`;
  }
  return `${ttlDays}d refresh TTL`;
}

function operatorRefreshTtlDetail(operator: OperatorView): string {
  const ttlDays = secondsToDays(operator.session_refresh_ttl_secs);
  if (operator.role === "admin") {
    return `Refresh-token lifetime is ${ttlDays} days. Admin policy target is <=30 days. This is separate from the short access-token expiry in Audit / Sessions.`;
  }
  return `Refresh-token lifetime is ${ttlDays} days. This is separate from the short access-token expiry in Audit / Sessions.`;
}

function FullTimeValue({ value }: { value: string }) {
  return (
    <time dateTime={value} title={formatFullTime(value)}>
      {formatTime(value)}
    </time>
  );
}

function OperatorAuthTimeValue({
  event,
  historyTruncated = false,
}: {
  event: OperatorAuthEventRecord | null | undefined;
  historyTruncated?: boolean;
}) {
  if (!event) {
    return (
      <span
        title={
          historyTruncated
            ? "No successful login event appears for this operator in the loaded authentication history; older events may exist."
            : "No successful login event is recorded for this operator."
        }
      >
        {historyTruncated ? "None in loaded history" : "Never"}
      </span>
    );
  }
  return (
    <time dateTime={event.created_at} title={formatFullTime(event.created_at)}>
      {formatCompactTime(event.created_at)}
    </time>
  );
}

function formatAuthEventTime(
  event: OperatorAuthEventRecord | null | undefined,
  historyTruncated = false,
): string {
  return event
    ? formatTime(event.created_at)
    : historyTruncated
      ? "None in loaded history"
      : "Not recorded";
}

function operatorLifecycleLabel(operator: OperatorView): string {
  if (operator.deleted_at) {
    return `Deleted ${formatFullTime(operator.deleted_at)}`;
  }
  if (operator.disabled_at) {
    return `Disabled ${formatFullTime(operator.disabled_at)}`;
  }
  return operatorStatusLabel(operator.status);
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
  return "Custom role retained from operator records.";
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
  return (
    Math.max(1, Math.min(3650, Math.round(value || defaultSessionTtlDays))) *
    24 *
    60 *
    60
  );
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
        return action.status === "active"
          ? "Enable user"
          : action.status === "disabled"
            ? "Disable user"
            : "Delete user";
      }
      return action.status === "active"
        ? "Enable users"
        : action.status === "disabled"
          ? "Disable users"
          : "Delete users";
    case "password":
      return "Reset password";
    case "totp":
      return action.operators.length === 1
        ? "Clear TOTP"
        : "Clear TOTP secrets";
    case "sessions":
      return action.sessions.length === 1
        ? "Revoke session"
        : "Revoke sessions";
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

function pendingUserActionItems(
  action: PendingUserAction | null,
): Array<{ label: string; title?: string; value: ReactNode }> {
  if (!action) {
    return [];
  }
  if (action.kind === "create") {
    return [
      { label: "Username", value: action.username },
      { label: "Role", value: operatorRoleLabel(action.role) },
      {
        label: "Refresh TTL",
        value: operatorRefreshTtlLabelFor(
          action.role,
          action.sessionRefreshTtlSecs,
        ),
      },
      {
        label: "Scopes",
        value: action.scopes.length
          ? action.scopes.join(", ")
          : "role defaults",
      },
      {
        label: "Payload",
        title: action.privilege.payloadHashHex,
        value: shortId(action.privilege.payloadHashHex),
      },
    ];
  }
  if (action.kind === "update") {
    return [
      { label: "Username", value: action.operator.username },
      { label: "Role", value: operatorRoleLabel(action.role) },
      {
        label: "Refresh TTL",
        value: operatorRefreshTtlLabelFor(
          action.role,
          action.sessionRefreshTtlSecs,
        ),
      },
      {
        label: "Scopes",
        value: action.scopes.length
          ? action.scopes.join(", ")
          : "role defaults",
      },
      {
        label: "Payload",
        title: action.privilege.payloadHashHex,
        value: shortId(action.privilege.payloadHashHex),
      },
    ];
  }
  if (action.kind === "password") {
    return [
      { label: "Username", value: action.operator.username },
      { label: "Role", value: operatorRoleLabel(action.operator.role) },
      {
        label: "Payload",
        title: action.privilege.payloadHashHex,
        value: shortId(action.privilege.payloadHashHex),
      },
    ];
  }
  if (action.kind === "status" || action.kind === "totp") {
    const firstOperator = action.operators[0];
    return [
      {
        label: action.operators.length === 1 ? "Username" : "Operators",
        value: formatOperatorSelection(action.operators),
      },
      { label: "Count", value: action.operators.length },
      {
        label: "Payload",
        title: firstOperator
          ? action.privileges[firstOperator.id].payloadHashHex
          : undefined,
        value: firstOperator
          ? shortId(action.privileges[firstOperator.id].payloadHashHex)
          : "-",
      },
    ];
  }
  if (action.kind === "sessions") {
    const firstSession = action.sessions[0];
    return [
      { label: "Username", value: action.operator.username },
      { label: "Sessions", value: action.sessions.length },
      {
        label: "Admin sessions",
        value: action.sessions.filter(
          (session) => session.operator_role === "admin",
        ).length,
      },
      {
        label: "Payload",
        title: firstSession
          ? action.privileges[firstSession.id].payloadHashHex
          : undefined,
        value: firstSession
          ? shortId(action.privileges[firstSession.id].payloadHashHex)
          : "-",
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
  const generatedLabel = dashboard
    ? formatFullTime(dashboard.generated_at)
    : "dashboard loading";
  const dbPressure = dashboard?.current.db_pool.max_connections
    ? dashboard.current.db_pool.in_use_connections /
      dashboard.current.db_pool.max_connections
    : null;
  const lifecycleFailures = dashboard
    ? dashboard.current.targets.control_timeout_last_24h +
      dashboard.current.targets.agent_timeout_last_24h +
      dashboard.current.targets.agent_lost_last_24h
    : null;
  const dbPressurePercent =
    dbPressure === null ? null : Math.round(dbPressure * 100);
  const queueDepth = dashboard?.current.dispatch.queue_depth ?? null;
  const dispatcherInFlight = dashboard?.capacity.dispatcher_in_flight ?? null;
  const dispatcherBatch = dashboard?.capacity.dispatcher_batch ?? null;
  const gatewayEvents = dashboard?.current.gateway_events;
  const gatewayDropped = gatewayEvents?.dropped_events ?? 0;
  const gatewayCriticalFailures = gatewayEvents?.critical_failures ?? 0;
  const gatewayRetries = gatewayEvents?.retry_attempts ?? 0;
  const gatewayQueueDepth = gatewayEvents?.current_queue_depth ?? 0;
  const gatewayOldestAgeSeconds = gatewayEvents?.oldest_event_age_secs ?? null;
  const gatewayOldestAgeLabel =
    gatewayOldestAgeSeconds === null
      ? "No queued events"
      : `${gatewayOldestAgeSeconds}s`;
  const gatewayRejected = gatewayEvents?.rejected_agent_connections ?? 0;
  const profileLimit = extractCapacityProfileLimit(dashboard?.notes ?? []);
  const dbTone: SystemHealthTone =
    dbPressure === null
      ? "neutral"
      : dbPressure >= 0.85
        ? "critical"
        : dbPressure >= 0.7
          ? "warning"
          : "ok";
  const dispatchTone: SystemHealthTone =
    queueDepth === null
      ? "neutral"
      : dispatchHealthTone(queueDepth, dispatcherInFlight, dispatcherBatch);
  const deadlineTone =
    !dashboard || lifecycleFailures === null
      ? "neutral"
      : dashboard.current.targets.deadline_expired_active > 0
        ? "critical"
        : lifecycleFailures > 0
          ? "warning"
          : "ok";
  const gatewayTone = dashboard ? gatewayHealthTone(gatewayEvents) : "neutral";
  const postureTone = mostSevereTone([
    dbTone,
    dispatchTone,
    deadlineTone,
    gatewayTone,
  ]);
  const recommendedCapacity =
    profileLimit && dispatcherInFlight && dispatcherBatch
      ? dispatcherInFlight >= profileLimit &&
        dispatcherBatch >= dispatcherInFlight
        ? "Current limits cover the profile."
        : `Raise dispatcher in-flight to at least ${profileLimit} and keep batch >= in-flight.`
      : "Recommended profile unavailable until capacity limits are loaded.";
  const attentionItems = dashboard
    ? buildSystemAttentionItems({
        dbPressurePercent: dbPressurePercent ?? 0,
        dbTone,
        dispatchTone,
        gatewayCriticalFailures,
        gatewayDropped,
        gatewayOldestAgeLabel,
        gatewayQueueDepth,
        gatewayRejected,
        gatewayRetries,
        gatewayTone,
        lifecycleFailures: lifecycleFailures ?? 0,
        queueDepth: queueDepth ?? 0,
      })
    : [];
  const dataCoverage =
    series.length > 0
      ? `${series.length} rollup series; latest sample ${formatCompactTime(dashboard?.generated_at ?? "")}`
      : "No durable metric samples in this range";
  const retainedResolutionLabel = dashboard
    ? `${formatDurationLabel(dashboard.effective_resolution_secs)} coarsest source resolution; ${formatDurationLabel(dashboard.bucket_secs)} chart buckets`
    : "retained history resolution loading";
  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <div className="dashboardToolbar">
          <div>
            <h2>Control-plane overview</h2>
            <span>
              {dashboard
                ? `${retainedResolutionLabel} / ${dataCoverage} / generated ${generatedLabel}`
                : "Control-plane metrics loading"}
            </span>
          </div>
          <div className="dashboardToolbarActions">
            <label className="dashboardToolbarSelect">
              <span>Points</span>
              <select
                aria-label="System overview point density"
                onChange={(event) =>
                  onPointDensityChange(
                    event.target.value as SystemDashboardPointDensity,
                  )
                }
                value={pointDensity}
              >
                {pointDensityOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
            <div
              className="timeRangeTabs"
              aria-label="System overview time range"
            >
              {dashboardWindowOptions.map((option) => (
                <button
                  aria-label={dashboardWindowAccessibleLabel(option)}
                  aria-pressed={window === option}
                  className={window === option ? "active" : ""}
                  key={option}
                  onClick={() => onWindowChange(option)}
                  title={dashboardWindowAccessibleLabel(option)}
                  type="button"
                >
                  {dashboardWindowLabel(option)}
                </button>
              ))}
            </div>
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason="System overview data is already refreshing."
              disabled={loading}
              onClick={onRefresh}
              type="button"
            >
              <RefreshCw size={16} />
              <span>{loading ? "Refreshing" : "Refresh"}</span>
            </button>
          </div>
        </div>
        <ActionFeedback
          className="localActionFeedback dashboardActionFeedback systemDashboardActionFeedback"
          message={error}
          tone="danger"
        />
        {dashboard &&
        dashboard.effective_resolution_secs > dashboard.requested_step_secs ? (
          <p className="observabilitySparseNotice" role="status">
            {`Older system evidence is retained at ${formatDurationLabel(dashboard.effective_resolution_secs)} resolution; missing detail is not interpolated.`}
          </p>
        ) : null}

        <section
          className="dashboardSection systemDashboardOverview"
          aria-label="System overview operations overview"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2>Service health</h2>
              <span>
                Database, dispatch, gateway, and worker state for the control
                plane.
              </span>
            </div>
            <ConsoleStatusBadge tone={postureTone}>
              {systemToneLabel(postureTone)}
            </ConsoleStatusBadge>
          </div>
          <div className="systemPostureGrid">
            <SystemPostureTile
              detail={
                dashboard
                  ? `${dashboard.current.db_pool.in_use_connections} of ${dashboard.current.db_pool.max_connections} connections in use; warn at 70%, critical at 85%.`
                  : "Database pool evidence is unavailable."
              }
              icon={<Database size={18} />}
              label="Database"
              tone={dbTone}
              value={
                dbPressurePercent === null
                  ? "Unknown"
                  : `${dbPressurePercent}% in use`
              }
            />
            <SystemPostureTile
              detail={
                dashboard
                  ? "Dispatch health summarized from current queue depth; configured limits live in System / Capacity."
                  : "Dispatch queue evidence is unavailable."
              }
              icon={<Activity size={18} />}
              label="Control-plane queue"
              tone={dispatchTone}
              value={queueDepth === null ? "Unknown" : `${queueDepth} queued`}
            />
            <SystemPostureTile
              detail={
                dashboard && lifecycleFailures !== null
                  ? `${dashboard.current.targets.deadline_expired_active} active expired; ${lifecycleFailures} timeout or loss event${lifecycleFailures === 1 ? "" : "s"} in the last 24h.`
                  : "Worker lifecycle evidence is unavailable."
              }
              icon={<TimerReset size={18} />}
              label="Worker"
              tone={deadlineTone}
              value={
                lifecycleFailures === null
                  ? "Unknown"
                  : `${lifecycleFailures} failure${lifecycleFailures === 1 ? "" : "s"}`
              }
            />
            <SystemPostureTile
              detail={
                dashboard
                  ? `${gatewayQueueDepth} queued / ${gatewayOldestAgeLabel}; ${gatewayDropped} dropped, ${gatewayRetries} retries, ${gatewayRejected} rejected connects.`
                  : "Gateway delivery evidence is unavailable."
              }
              icon={<Network size={18} />}
              label="Gateway"
              tone={gatewayTone}
              value={
                dashboard
                  ? (gatewayEvents?.status ?? "Not configured")
                  : "Unknown"
              }
            />
          </div>

          <div
            className="systemAttentionQueue"
            aria-label="System overview attention queue"
          >
            <div className="dashboardSideRailHeader">
              <strong>What needs attention</strong>
              <span>
                {attentionItems.length
                  ? `${attentionItems.length} signal${attentionItems.length === 1 ? "" : "s"}`
                  : dashboard
                    ? "No active signals"
                    : "Evidence unavailable"}
              </span>
            </div>
            {attentionItems.length ? (
              attentionItems.map((item) => (
                <SystemAttentionRow item={item} key={item.label} />
              ))
            ) : (
              <div className="systemAttentionEmpty">
                {dashboard ? (
                  <CheckCircle2 size={16} />
                ) : (
                  <AlertTriangle size={16} />
                )}
                <span>
                  {dashboard
                    ? "No control-plane thresholds are currently breached in this sample."
                    : "Control-plane health is unavailable until dashboard data loads."}
                </span>
              </div>
            )}
          </div>

          <details className="systemDiagnosticsDisclosure">
            <summary>
              <span>Diagnostics</span>
              <small>Capacity and drilldown notes hidden from posture</small>
            </summary>
            <div className="systemDiagnosticsGrid">
              <SystemDiagnosticsRow
                detail={recommendedCapacity}
                label="Capacity profile"
                value={
                  profileLimit
                    ? `${profileLimit} VPS expected max`
                    : "Not configured"
                }
              />
              <SystemDiagnosticsRow
                detail="Detailed capacity curves and threshold tuning belong in System / Capacity."
                label="Dispatch limits"
                value={`${valueOrNotConfigured(dispatcherInFlight)} in-flight / ${valueOrNotConfigured(dispatcherBatch)} batch`}
              />
              <SystemDiagnosticsRow
                detail="Series-level alert overlays and linked logs are unavailable."
                label="Chart drilldowns"
                value={`${series.length} rollup series only`}
              />
              <SystemDiagnosticsRow
                detail={dashboard?.notes.join("; ") || "No dashboard notes."}
                label="Dashboard notes"
                value={dashboard?.notes.length ? "Available" : "None"}
              />
            </div>
          </details>
        </section>

        <SystemMetricSection
          badge={
            dashboard
              ? `${dashboard.current.dispatch.queue_depth} queued`
              : "Unknown"
          }
          badgeTone={dispatchTone}
          icon={<Activity size={18} />}
          insight="Overview shows one selected chart. Capacity-only curves stay in System / Capacity."
          title="Selected chart - Dispatch queue"
          subtitle="Queued, dispatching, running, and retry pressure."
          metrics={[
            {
              label: "Active jobs",
              value: dashboard
                ? String(dashboard.current.dispatch.active_jobs)
                : "Unknown",
            },
            {
              label: "Dispatch queue",
              value: dashboard
                ? String(dashboard.current.dispatch.queue_depth)
                : "Unknown",
            },
            {
              label: "Active targets",
              value: dashboard
                ? String(dashboard.current.targets.active)
                : "Unknown",
            },
            {
              label: "Retried targets",
              value: dashboard
                ? String(dashboard.current.dispatch.retried_targets)
                : "Unknown",
            },
          ]}
          lines={chartLines(
            series,
            [
              "dispatch.queue_depth",
              "targets.dispatching",
              "targets.running",
              "dispatch.retried_targets",
            ],
            dashboard?.bucket_secs ?? 60,
          )}
          valueFormatter={(value) => formatNumber(value)}
        />
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
  onSelectView,
  onWindowChange,
  pointDensity,
  window,
}: {
  dashboard: SystemDashboardRecord | null;
  error: string | null;
  loading: boolean;
  onPointDensityChange: (density: SystemDashboardPointDensity) => void;
  onRefresh: () => void;
  onSelectView: (view: ActiveView, subpage?: string) => void;
  onWindowChange: (window: SystemDashboardWindow) => void;
  pointDensity: SystemDashboardPointDensity;
  window: SystemDashboardWindow;
}) {
  const [activeSubsystem, setActiveSubsystem] =
    useState<CapacitySubsystem>("dispatch");
  const series = dashboard?.series ?? [];
  const sampleCoverage = systemSeriesCoverageSummary(series);
  const dbPressure = dashboard?.current.db_pool.max_connections
    ? dashboard.current.db_pool.in_use_connections /
      dashboard.current.db_pool.max_connections
    : null;
  const dbPressurePercent =
    dbPressure === null ? null : Math.round(dbPressure * 100);
  const dbTone: SystemHealthTone =
    dbPressure === null
      ? "neutral"
      : dbPressure >= 0.85
        ? "critical"
        : dbPressure >= 0.7
          ? "warning"
          : "ok";
  const queueDepth = dashboard?.current.dispatch.queue_depth ?? null;
  const dispatcherInFlight = dashboard?.capacity.dispatcher_in_flight ?? null;
  const dispatcherBatch = dashboard?.capacity.dispatcher_batch ?? null;
  const dispatchQueueGrowth = seriesDelta(series, "dispatch.queue_depth");
  const dispatchModel = dispatchCapacityHealth({
    queueDepth,
    queueGrowth: dispatchQueueGrowth.delta,
    dispatcherInFlight,
    dispatcherBatch,
    oldestAgeSecs: null,
  });
  const gatewayEvents = dashboard?.current.gateway_events;
  const gatewayQueueGrowth = seriesDelta(
    series,
    "gateway_events.current_queue_depth",
  );
  const gatewayModel = dashboard
    ? gatewayCapacityHealth({
        gatewayEvents,
        queueGrowth: gatewayQueueGrowth.delta,
      })
    : {
        reason: "gateway evidence unavailable",
        tone: "neutral" as const,
      };
  const profileLimit = extractCapacityProfileLimit(dashboard?.notes ?? []);
  const profileRatio =
    profileLimit && dispatcherInFlight
      ? Math.round((dispatcherInFlight / profileLimit) * 100)
      : null;
  const capacityTone = mostSevereTone([
    dbTone,
    dispatchModel.tone,
    gatewayModel.tone,
  ]);
  const configuredLimit = dashboard
    ? `${valueOrNotConfigured(dispatcherInFlight)} in-flight / ${valueOrNotConfigured(dispatcherBatch)} batch`
    : "Unknown";
  const capacityForecast = !dashboard
    ? "Capacity evidence is unavailable."
    : profileLimit && dispatcherInFlight && dispatcherBatch
      ? `${profileLimit}-VPS profile; ${dispatcherInFlight} in-flight (${profileRatio}% of profile) and ${dispatcherBatch} batch.`
      : profileLimit
        ? `${profileLimit}-VPS profile detected; set dispatcher limits in Suite config to complete the plan.`
        : "No fleet capacity profile is configured.";
  const activeChart =
    activeSubsystem === "database" ? (
      <SystemMetricSection
        badge={
          dbPressurePercent === null
            ? "Unknown"
            : `${dbPressurePercent}% in use`
        }
        badgeTone={dbTone}
        icon={<Database size={18} />}
        insight="Database capacity compares active usage with configured pool ceilings; sustained pressure above 70% needs pool review."
        title="Database capacity"
        subtitle="DB pool pressure and configured API/worker limits."
        metrics={[
          {
            label: "API DB pool",
            value: dashboard
              ? valueOrNotConfigured(dashboard.capacity.api_db_pool)
              : "Unknown",
          },
          {
            label: "Worker DB pool",
            value: dashboard
              ? valueOrNotConfigured(dashboard.capacity.worker_db_pool)
              : "Unknown",
          },
          {
            label: "In use",
            value: dashboard
              ? String(dashboard.current.db_pool.in_use_connections)
              : "Unknown",
          },
          {
            label: "Open",
            value: dashboard
              ? String(dashboard.current.db_pool.open_connections)
              : "Unknown",
          },
        ]}
        thresholds={[
          { label: "Warn", tone: "warning", value: "70% pool pressure" },
          { label: "Critical", tone: "critical", value: "85% pool pressure" },
        ]}
        lines={chartLines(
          series,
          [
            "db_pool.in_use_connections",
            "db_pool.open_connections",
            "db_pool.idle_connections",
            "db_pool.max_connections",
          ],
          dashboard?.bucket_secs ?? 60,
        )}
        valueFormatter={(value) => formatNumber(value)}
      />
    ) : activeSubsystem === "gateway" ? (
      <SystemMetricSection
        badge={gatewayEvents?.status ?? "unavailable"}
        badgeTone={gatewayModel.tone}
        icon={<Network size={18} />}
        insight="Gateway capacity warns only when queue age, growth, queue-full failures, or live status indicate pressure."
        title="Gateway capacity"
        subtitle="Gateway-to-API forwarding backlog, retries, drops, and queue saturation."
        metrics={[
          { label: "Status", value: gatewayEvents?.status ?? "unavailable" },
          {
            label: "Queue depth",
            value: valueOrNotConfigured(gatewayEvents?.current_queue_depth),
          },
          {
            label: "Oldest event",
            value: secondsOrNotConfigured(gatewayEvents?.oldest_event_age_secs),
          },
          {
            label: "Queue growth",
            value: formatDelta(gatewayQueueGrowth.delta),
          },
          {
            label: "Target queue full",
            value: valueOrNotConfigured(
              gatewayEvents?.dropped_by_reason?.target_queue_full,
            ),
          },
          {
            label: "Rejected connects",
            value: valueOrNotConfigured(
              gatewayEvents?.rejected_agent_connections,
            ),
          },
        ]}
        thresholds={[
          {
            label: "Live",
            tone: gatewayEvents?.status === "live" ? "ok" : "warning",
            value: gatewayEvents?.status ?? "unavailable",
          },
          {
            label: "Critical",
            tone: "critical",
            value: "queue full / expired drops",
          },
          { label: "Warn", tone: "warning", value: "age or growth" },
        ]}
        lines={chartLines(
          series,
          [
            "gateway_events.current_queue_depth",
            "gateway_events.oldest_event_age_secs",
            "gateway_events.dropped_events",
            "gateway_events.retry_attempts",
          ],
          dashboard?.bucket_secs ?? 60,
        )}
        valueFormatter={(value) => formatNumber(value)}
      />
    ) : (
      <SystemMetricSection
        badge={queueDepth === null ? "Unknown" : `${queueDepth} queued`}
        badgeTone={dispatchModel.tone}
        icon={<Activity size={18} />}
        insight="Dispatch capacity uses queue growth, reported age, configured thresholds, and worker availability instead of warning on every nonzero queue."
        title="Dispatch capacity"
        subtitle="Queued, dispatching, running, retry, and active job pressure."
        metrics={[
          {
            label: "Active jobs",
            value: dashboard
              ? String(dashboard.current.dispatch.active_jobs)
              : "Unknown",
          },
          {
            label: "Dispatch queue",
            value: queueDepth === null ? "Unknown" : String(queueDepth),
          },
          {
            label: "Queue growth",
            value: formatDelta(dispatchQueueGrowth.delta),
          },
          {
            label: "Warning threshold",
            value: dispatchModel.warningThreshold,
          },
          {
            label: "Active targets",
            value: dashboard
              ? String(dashboard.current.targets.active)
              : "Unknown",
          },
          {
            label: "Retried targets",
            value: dashboard
              ? String(dashboard.current.dispatch.retried_targets)
              : "Unknown",
          },
        ]}
        thresholds={[
          {
            label: "Capacity",
            tone: "info",
            value: dashboard
              ? `${valueOrNotConfigured(dispatcherInFlight)} in-flight`
              : "Unknown",
          },
          {
            label: "Batch",
            tone: "info",
            value: dashboard
              ? `${valueOrNotConfigured(dispatcherBatch)} max`
              : "Unknown",
          },
          {
            label: "Health",
            tone: dispatchModel.tone,
            value: dispatchModel.reason,
          },
        ]}
        lines={chartLines(
          series,
          [
            "dispatch.queue_depth",
            "targets.dispatching",
            "targets.running",
            "dispatch.retried_targets",
          ],
          dashboard?.bucket_secs ?? 60,
        )}
        valueFormatter={(value) => formatNumber(value)}
      />
    );
  const capacityTabs: Array<{
    id: CapacitySubsystem;
    label: string;
    tone: SystemHealthTone;
    value: string;
  }> = [
    {
      id: "database",
      label: "Database",
      tone: dbTone,
      value: dbPressurePercent === null ? "Unknown" : `${dbPressurePercent}%`,
    },
    {
      id: "dispatch",
      label: "Dispatch",
      tone: dispatchModel.tone,
      value: queueDepth === null ? "Unknown" : `${queueDepth} queued`,
    },
    {
      id: "gateway",
      label: "Gateway",
      tone: gatewayModel.tone,
      value: gatewayEvents?.status ?? "unavailable",
    },
  ];
  const selectedFactors: CapacityFactorItem[] =
    activeSubsystem === "database"
      ? [
          {
            detail:
              "Current database usage compared with the dashboard pool ceiling.",
            label: "Pool pressure",
            tone: dbTone,
            value:
              dbPressurePercent === null ? "Unknown" : `${dbPressurePercent}%`,
          },
          {
            detail: "Warning threshold is 70% of max connections.",
            label: "Warn threshold",
            tone: "warning",
            value: dashboard
              ? `${Math.ceil(dashboard.current.db_pool.max_connections * 0.7)} connections`
              : "Unknown",
          },
          {
            detail: "Critical threshold is 85% of max connections.",
            label: "Critical threshold",
            tone: "critical",
            value: dashboard
              ? `${Math.ceil(dashboard.current.db_pool.max_connections * 0.85)} connections`
              : "Unknown",
          },
          {
            detail: "Suite Config owns the API and worker DB pool limits.",
            label: "Config owner",
            tone: "info",
            value: "System / Suite config",
          },
        ]
      : activeSubsystem === "gateway"
        ? [
            {
              detail: "Current gateway forwarding queue depth.",
              label: "Queue depth",
              tone: gatewayModel.tone,
              value: valueOrNotConfigured(gatewayEvents?.current_queue_depth),
            },
            {
              detail:
                "Oldest queued event age; missing age is not treated as pressure.",
              label: "Oldest event age",
              tone: !dashboard
                ? "neutral"
                : gatewayEvents?.oldest_event_age_secs
                  ? "warning"
                  : "ok",
              value: secondsOrNotConfigured(
                gatewayEvents?.oldest_event_age_secs,
              ),
            },
            {
              detail:
                gatewayQueueGrowth.delta && gatewayQueueGrowth.delta > 0
                  ? "gateway queue is growing across the available samples."
                  : "Change in gateway queue depth across the available samples.",
              label: "Queue growth",
              tone:
                gatewayQueueGrowth.delta === null
                  ? "neutral"
                  : gatewayQueueGrowth.delta > 0
                    ? "warning"
                    : "ok",
              value: formatDelta(gatewayQueueGrowth.delta),
            },
            {
              detail: gatewayModel.reason,
              label: "Capacity state",
              tone: gatewayModel.tone,
              value: systemToneLabel(gatewayModel.tone),
            },
          ]
        : [
            {
              detail:
                "Queue age is unavailable; use queue growth and configured limits to assess pressure.",
              label: "Queue age",
              tone: "info",
              value: "Unavailable",
            },
            {
              detail:
                dispatchQueueGrowth.delta && dispatchQueueGrowth.delta > 0
                  ? "Queue is growing across the available samples."
                  : "Change in dispatch queue depth across the available samples.",
              label: "Queue growth",
              tone:
                dispatchQueueGrowth.delta === null
                  ? "neutral"
                  : dispatchQueueGrowth.delta > 0
                    ? "warning"
                    : "ok",
              value: formatDelta(dispatchQueueGrowth.delta),
            },
            {
              detail:
                "Queue warning threshold from configured dispatcher in-flight capacity.",
              label: "Warning threshold",
              tone: "warning",
              value: dispatchModel.warningThreshold,
            },
            {
              detail:
                "Dispatcher capacity is considered available when in-flight capacity is configured.",
              label: "Worker availability",
              tone: !dashboard
                ? "neutral"
                : dispatcherInFlight
                  ? "ok"
                  : "critical",
              value: !dashboard
                ? "Unknown"
                : dispatcherInFlight
                  ? `${dispatcherInFlight} in-flight configured`
                  : "Not configured",
            },
          ];
  const selectedConfigLinks =
    activeSubsystem === "database"
      ? [
          ["API DB pool", "capacity.api_db_pool"],
          ["Worker DB pool", "capacity.worker_db_pool"],
        ]
      : activeSubsystem === "dispatch"
        ? [
            ["Dispatcher in-flight", "capacity.dispatcher_in_flight"],
            ["Dispatcher batch", "capacity.dispatcher_batch"],
          ]
        : [
            ["Event post seconds", "timeouts.event_post_secs"],
            ["Internal HTTP read", "timeouts.internal_http_read_secs"],
          ];
  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <div className="dashboardToolbar">
          <div>
            <h2>Capacity telemetry</h2>
            <span>
              {dashboard
                ? `${formatDurationLabel(dashboard.effective_resolution_secs)} coarsest source resolution; ${formatDurationLabel(dashboard.bucket_secs)} chart buckets / selected ${dashboardWindowLabel(window)}; ${sampleCoverage} / generated ${formatFullTime(dashboard.generated_at)}`
                : "Capacity telemetry loading"}
            </span>
          </div>
          <div className="dashboardToolbarActions">
            <label className="dashboardToolbarSelect">
              <span>Points</span>
              <select
                aria-label="System capacity point density"
                onChange={(event) =>
                  onPointDensityChange(
                    event.target.value as SystemDashboardPointDensity,
                  )
                }
                value={pointDensity}
              >
                {pointDensityOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
            <div
              className="timeRangeTabs"
              aria-label="System capacity time range"
            >
              {dashboardWindowOptions.map((option) => (
                <button
                  aria-label={dashboardWindowAccessibleLabel(option)}
                  aria-pressed={window === option}
                  className={window === option ? "active" : ""}
                  key={option}
                  onClick={() => onWindowChange(option)}
                  title={dashboardWindowAccessibleLabel(option)}
                  type="button"
                >
                  {dashboardWindowLabel(option)}
                </button>
              ))}
            </div>
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason="System capacity data is already refreshing."
              disabled={loading}
              onClick={onRefresh}
              type="button"
            >
              <RefreshCw size={16} />
              <span>{loading ? "Refreshing" : "Refresh"}</span>
            </button>
          </div>
        </div>
        <ActionFeedback
          className="localActionFeedback dashboardActionFeedback systemDashboardActionFeedback"
          message={error}
          tone="danger"
        />
        {dashboard &&
        dashboard.effective_resolution_secs > dashboard.requested_step_secs ? (
          <p className="observabilitySparseNotice" role="status">
            {`Older system evidence is retained at ${formatDurationLabel(dashboard.effective_resolution_secs)} resolution; missing detail is not interpolated.`}
          </p>
        ) : null}

        <section
          className="dashboardSection systemDashboardOverview"
          aria-label="System capacity posture overview"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2>Subsystem capacity</h2>
              <span>
                Database, dispatch, and gateway capacity. Select one subsystem
                to inspect its thresholds and chart.
              </span>
            </div>
            <ConsoleStatusBadge tone={capacityTone}>
              {systemToneLabel(capacityTone)}
            </ConsoleStatusBadge>
          </div>
          <div
            className="systemCapacityTabs"
            role="tablist"
            aria-label="System capacity subsystems"
            onKeyDown={handleTabListKeyDown}
          >
            {capacityTabs.map((tab) => (
              <button
                aria-controls="system-capacity-tabpanel"
                aria-selected={activeSubsystem === tab.id}
                className={activeSubsystem === tab.id ? "active" : ""}
                id={tabId("system-capacity", tab.id)}
                key={tab.id}
                onClick={() => setActiveSubsystem(tab.id)}
                role="tab"
                tabIndex={activeSubsystem === tab.id ? 0 : -1}
                type="button"
              >
                <span>{tab.label}</span>
                <strong>{tab.value}</strong>
                <ConsoleStatusBadge tone={tab.tone}>
                  {systemToneLabel(tab.tone)}
                </ConsoleStatusBadge>
              </button>
            ))}
          </div>
          <div
            aria-labelledby={tabId("system-capacity", activeSubsystem)}
            id="system-capacity-tabpanel"
            role="tabpanel"
          >
            <div
              className="systemCapacityStrip"
              aria-label="System capacity configured limits"
            >
              <div>
                <strong>Selected range</strong>
                <span>{`${window}; ${sampleCoverage}`}</span>
              </div>
              <div>
                <strong>Capacity profile</strong>
                <span>{capacityForecast}</span>
              </div>
              <div>
                <strong>Dispatch limit</strong>
                <span>{configuredLimit}</span>
              </div>
            </div>
            <CapacityFactorGrid
              ariaLabel={`${activeSubsystem} capacity health factors`}
              items={selectedFactors}
            />
            <div
              className="systemCapacityConfigLinks"
              aria-label="Capacity Suite Config links"
            >
              <strong>Suite Config fields</strong>
              <span>
                Limits shown here are edited in System / Suite config; each
                action keeps the field key visible.
              </span>
              <div>
                {selectedConfigLinks.map(([label, key]) => (
                  <button
                    className="secondaryAction compactAction"
                    key={key}
                    onClick={() => onSelectView("System", "suite_config")}
                    title={`Open System / Suite config for ${key}`}
                    type="button"
                  >
                    <SlidersHorizontal size={16} />
                    <span>{label}</span>
                    <small>{key}</small>
                  </button>
                ))}
              </div>
            </div>
          </div>
        </section>

        {activeChart}
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
            <span className="systemSectionTitle">
              {icon}
              {title} curves
            </span>
            {thresholds.length ? (
              <div
                className="systemThresholdChips"
                aria-label={`${title} thresholds`}
              >
                {thresholds.map((threshold) => (
                  <span
                    className={threshold.tone ?? "neutral"}
                    key={threshold.label}
                  >
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

function CapacityFactorGrid({
  ariaLabel,
  items,
}: {
  ariaLabel: string;
  items: CapacityFactorItem[];
}) {
  return (
    <div className="systemCapacityFactorGrid" aria-label={ariaLabel}>
      {items.map((item) => (
        <div
          className={`systemCapacityFactor ${item.tone ?? "neutral"}`}
          key={item.label}
        >
          <small>{item.label}</small>
          <strong>{item.value}</strong>
          <p>{item.detail}</p>
        </div>
      ))}
    </div>
  );
}

function SystemDiagnosticsRow({
  detail,
  label,
  value,
}: {
  detail: string;
  label: string;
  value: string;
}) {
  return (
    <div className="systemDiagnosticsRow">
      <strong>{label}</strong>
      <b>{value}</b>
      <p>{detail}</p>
    </div>
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
  onUpdate: (
    toml: string,
    privilegeAssertion: unknown,
  ) => Promise<SuiteConfigUpdateResponse>;
  onValidate: (toml: string) => Promise<SuiteConfigValidateResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
}) {
  const [draftToml, setDraftToml] = useState("");
  const [validation, setValidation] =
    useState<SuiteConfigValidateResponse | null>(null);
  const [configMessage, setConfigMessage] = useState<string | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [validationPending, setValidationPending] = useState(false);
  const [savePending, setSavePending] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [editorMode, setEditorMode] = useState<"form" | "toml">("form");
  const [advancedTomlEdited, setAdvancedTomlEdited] = useState(false);
  const [configSearch, setConfigSearch] = useState("");
  const validationRequestId = useRef(0);
  const [activeConfigSection, setActiveConfigSection] = useState(
    suiteConfigSections[0].id,
  );
  const parsedDraft = useMemo(() => parseTomlDraft(draftToml), [draftToml]);
  const currentConfigDraft = useMemo(
    () => (config ? parseTomlDraft(config.toml) : null),
    [config],
  );
  const normalizedConfigSearch = configSearch.trim().toLowerCase();
  const filteredConfigSections = useMemo(
    () =>
      suiteConfigSections
        .map((section) => ({
          ...section,
          fields: section.fields.filter((field) =>
            configFieldMatchesSearch(section, field, normalizedConfigSearch),
          ),
        }))
        .filter((section) => section.fields.length > 0),
    [normalizedConfigSearch],
  );
  const visibleConfigFieldCount = filteredConfigSections.reduce(
    (total, section) => total + section.fields.length,
    0,
  );
  const displayedConfigSections = normalizedConfigSearch
    ? filteredConfigSections
    : filteredConfigSections.filter(
        (section) => section.id === activeConfigSection,
      );
  const draftTextChanged = Boolean(config && draftToml !== config.toml);
  const changedKeys = validation?.changed_keys ?? [];
  const textOnlyDraftChange =
    draftTextChanged &&
    advancedTomlEdited &&
    Boolean(validation) &&
    changedKeys.length === 0;
  const dirty =
    draftTextChanged &&
    (!validation || changedKeys.length > 0 || advancedTomlEdited);
  const activeValidation = validation?.validation ?? config?.validation ?? null;
  const hotReloadFields = activeValidation?.hot_reload_fields ?? [];
  const restartRequiredFields = activeValidation?.restart_required_fields ?? [];
  const hotReloadCount = suiteConfigSections
    .flatMap((section) => section.fields)
    .filter(
      (field) => configFieldImpact(field.path, activeValidation) === "hot",
    ).length;
  const restartRequiredCount = suiteConfigSections
    .flatMap((section) => section.fields)
    .filter(
      (field) => configFieldImpact(field.path, activeValidation) === "restart",
    ).length;
  const changedHotReloadFields = changedKeys.filter(
    (key) => configFieldImpact(key, activeValidation) === "hot",
  );
  const changedRestartRequiredFields = changedKeys.filter(
    (key) => configFieldImpact(key, activeValidation) === "restart",
  );
  const changedUnknownImpactFields = changedKeys.filter(
    (key) => configFieldImpact(key, activeValidation) === "unknown",
  );
  const impactSummaryLabel = validation
    ? "Draft impact"
    : "Configuration inventory";
  const impactSummaryDetail = validation
    ? textOnlyDraftChange
      ? "Exact Advanced TOML text changed · no runtime value changes"
      : `${changedKeys.length} changed · ${changedHotReloadFields.length} hot reload · ${changedRestartRequiredFields.length} restart · ${changedUnknownImpactFields.length} not reported`
    : `${hotReloadCount} hot-reload fields · ${restartRequiredCount} restart-required fields`;
  const hotReloadSummaryLabel = validation
    ? "Draft hot reload"
    : "Inventory hot reload";
  const hotReloadSummaryValue = validation
    ? `${changedHotReloadFields.length} changed`
    : `${hotReloadCount} fields`;
  const restartSummaryLabel = validation
    ? "Draft restart"
    : "Inventory restart";
  const restartSummaryValue = validation
    ? `${changedRestartRequiredFields.length} changed`
    : `${restartRequiredCount} fields`;
  const validationState = validation
    ? validation.validation.valid
      ? "validated"
      : "invalid"
    : config?.validation.valid
      ? "loaded"
      : "invalid";
  const pending = validationPending || savePending;
  const reviewDisabled =
    pending ||
    !dirty ||
    !privilegeMaterial ||
    (validation ? !validation.validation.valid : false);
  const nextAction = systemConfigNextAction({
    dirty,
    pending,
    privilegeReady: Boolean(privilegeMaterial),
    reviewDisabled,
    validation,
  });
  const systemConfigFeedbackMessage =
    configError ??
    error ??
    (configMessage ||
      (validationPending
        ? "Validating suite config"
        : savePending
          ? "Saving suite config"
          : null));
  const systemConfigFeedbackTone =
    configError || error ? "danger" : configMessage ? "success" : "progress";

  useEffect(() => {
    if (config) {
      validationRequestId.current += 1;
      setDraftToml(config.toml);
      setAdvancedTomlEdited(false);
      setValidation(null);
      setValidationPending(false);
      setSavePending(false);
      setConfigMessage(null);
      setConfigError(null);
      setConfirmOpen(false);
    }
  }, [config]);

  useEffect(() => {
    if (
      filteredConfigSections.length > 0 &&
      !filteredConfigSections.some(
        (section) => section.id === activeConfigSection,
      )
    ) {
      setActiveConfigSection(filteredConfigSections[0].id);
    }
  }, [activeConfigSection, filteredConfigSections]);

  useEffect(() => {
    if (!config || !dirty || !draftToml.trim() || !parsedDraft.ok) {
      return;
    }
    const timeout = window.setTimeout(() => {
      void validateDraft({ silent: true, toml: draftToml });
    }, 450);
    return () => window.clearTimeout(timeout);
  }, [config, dirty, draftToml, parsedDraft.ok]);

  async function validateDraft({
    silent = false,
    toml = draftToml,
  }: {
    silent?: boolean;
    toml?: string;
  } = {}) {
    const requestId = ++validationRequestId.current;
    setValidationPending(true);
    if (!silent) {
      setConfigError(null);
      setConfigMessage(null);
    }
    try {
      const result = await onValidate(toml);
      if (requestId !== validationRequestId.current) {
        return null;
      }
      setValidation(result);
      setConfigError(null);
      if (!silent) {
        setConfigMessage(
          `Validation passed; ${result.changed_keys.length} changed key${result.changed_keys.length === 1 ? "" : "s"}.`,
        );
      }
      return result;
    } catch (validateError) {
      if (requestId === validationRequestId.current) {
        setValidation(null);
        setConfigError(
          validateError instanceof Error
            ? validateError.message
            : "Suite config validation failed",
        );
      }
      return null;
    } finally {
      if (requestId === validationRequestId.current) {
        setValidationPending(false);
      }
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
    setSavePending(true);
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
      const auditNote =
        response.audit_status === "applied_recorded"
          ? ""
          : ` Audit status: ${response.audit_status}.`;
      setConfigMessage(
        `Saved suite config; changed keys: ${response.changed_keys.join(", ") || "none"}.${auditNote}`,
      );
      setConfirmOpen(false);
      onLoad();
    } catch (saveError) {
      setConfigError(
        saveError instanceof Error
          ? saveError.message
          : "Suite config save failed",
      );
    } finally {
      setSavePending(false);
    }
  }

  async function reviewChanges() {
    if (!dirty) {
      setConfigError("Edit a field or Advanced TOML before reviewing changes");
      return;
    }
    if (!privilegeMaterial) {
      setConfigError("Unlock privilege before reviewing suite config save");
      return;
    }
    let activeResult = validation;
    if (!activeResult) {
      activeResult = await validateDraft({ silent: false });
    }
    if (!activeResult) {
      return;
    }
    if (!activeResult.validation.valid) {
      setConfigError(
        "Fix validation errors before reviewing suite config save",
      );
      return;
    }
    setConfigError(null);
    setConfirmOpen(true);
  }

  function updateField(path: string, value: unknown) {
    if (!parsedDraft.ok) {
      setConfigError(parsedDraft.error);
      return;
    }
    validationRequestId.current += 1;
    const next = cloneTable(parsedDraft.table);
    setTomlPath(next, path.split("."), value);
    setConfigError(null);
    setConfigMessage(null);
    setDraftToml(stringify(next));
    setValidation(null);
    setValidationPending(false);
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
              <span>
                {config?.path ?? "Suite TOML path"} /{" "}
                {config?.exists ? "file exists" : "new file"}
              </span>
            </div>
            <div className="buttonCluster">
              <button
                className="secondaryAction compactAction"
                disabled={loading || pending}
                onClick={onLoad}
                type="button"
              >
                <RefreshCw size={16} />
                <span>{loading ? "Loading" : "Reload"}</span>
              </button>
              <button
                className="primaryAction compactAction"
                disabled={reviewDisabled}
                onClick={() => void reviewChanges()}
                title={
                  reviewDisabled
                    ? nextAction.detail
                    : "Review changed suite config keys, reload impact, and privilege state."
                }
                type="button"
              >
                <Save size={16} />
                <span>Review changes</span>
              </button>
            </div>
          </div>
          <ActionFeedback
            className="localActionFeedback systemConfigActionFeedback"
            message={systemConfigFeedbackMessage}
            tone={systemConfigFeedbackTone}
          />
          {config && (
            <div className="systemConfigSummary">
              <div
                className="systemConfigImpactSummary"
                aria-label="Suite config impact summary"
              >
                <strong>{impactSummaryLabel}</strong>
                <span>{impactSummaryDetail}</span>
              </div>
              <SystemConfigStatusItem
                icon={<SlidersHorizontal size={17} />}
                label="State"
                value={dirty ? "draft" : validationState}
                tone={
                  dirty
                    ? "warning"
                    : validationState === "invalid"
                      ? "critical"
                      : "ok"
                }
              />
              <SystemConfigStatusItem
                icon={<CheckCircle2 size={17} />}
                label="Changed keys"
                value={
                  validation
                    ? String(changedKeys.length)
                    : validationPending
                      ? "validating"
                      : "not validated"
                }
                tone={
                  validation
                    ? "info"
                    : validationPending
                      ? "warning"
                      : "neutral"
                }
              />
              <SystemConfigStatusItem
                icon={<RefreshCw size={17} />}
                label={hotReloadSummaryLabel}
                value={hotReloadSummaryValue}
                tone="info"
              />
              <SystemConfigStatusItem
                icon={<AlertTriangle size={17} />}
                label={restartSummaryLabel}
                value={restartSummaryValue}
                tone={
                  validation
                    ? changedRestartRequiredFields.length
                      ? "warning"
                      : "ok"
                    : restartRequiredCount
                      ? "warning"
                      : "ok"
                }
              />
              <SystemConfigStatusItem
                icon={<LockKeyhole size={17} />}
                label="Privilege"
                value={privilegeMaterial ? "unlocked" : "locked"}
                tone={privilegeMaterial ? "ok" : "warning"}
              />
            </div>
          )}
        </section>

        <section
          className="systemConfigOwnership"
          aria-label="Suite config ownership boundary"
        >
          <div className="systemConfigOwnershipItem">
            <span>
              <ServerCog size={17} />
            </span>
            <div>
              <strong>System scope</strong>
              <p>
                Suite TOML controls API, gateway, network, worker, capacity,
                storage, secrets, and control-plane timeouts.
              </p>
            </div>
          </div>
          <div className="systemConfigOwnershipItem">
            <span>
              <SlidersHorizontal size={17} />
            </span>
            <div>
              <strong>Runtime config scope</strong>
              <p>
                Per-VPS runtime reads, overrides, patches, configuration
                presets, and rules stay in Config workflows.
              </p>
            </div>
            <div className="systemConfigOwnershipActions">
              <button
                className="secondaryAction compactAction"
                onClick={() => onSelectView("Config", "per_vps")}
                type="button"
              >
                Open Config / Per-VPS
              </button>
              <button
                className="secondaryAction compactAction"
                onClick={() => onSelectView("Config", "bulk_patch")}
                type="button"
              >
                Open Config / VPS override patch
              </button>
            </div>
          </div>
          <div className="systemConfigOwnershipItem">
            <span>
              <ShieldCheck size={17} />
            </span>
            <div>
              <strong>Save contract</strong>
              <p>
                Every suite change follows structured edit, automatic
                validation, redacted diff, privilege unlock, save, reload or
                restart, and audit evidence.
              </p>
            </div>
          </div>
        </section>

        <section
          className={`systemConfigStickyBar ${dirty ? "dirty" : ""}`}
          aria-label="Suite config sticky save bar"
        >
          <div>
            <strong>
              {validation
                ? textOnlyDraftChange
                  ? "Advanced TOML text changed"
                  : changedKeyCountLabel(changedKeys.length)
                : dirty
                  ? validationPending
                    ? "Auto-validating draft"
                    : "Draft changes pending"
                  : "No draft changes"}
            </strong>
            <span>{nextAction.detail}</span>
          </div>
          <div className="buttonCluster">
            {!privilegeMaterial ? (
              <button
                className="secondaryAction compactAction"
                onClick={onOpenPrivilegeUnlock}
                type="button"
              >
                <LockKeyhole size={16} />
                <span>Unlock</span>
              </button>
            ) : null}
            <button
              className="primaryAction compactAction"
              disabled={reviewDisabled}
              onClick={() => void reviewChanges()}
              title={
                reviewDisabled
                  ? nextAction.detail
                  : "Review changed suite config keys, reload impact, and privilege state."
              }
              type="button"
            >
              <Save size={16} />
              <span>Review changes</span>
            </button>
          </div>
        </section>

        <div className="systemConfigBody">
          <aside
            className="systemConfigSideNav"
            aria-label="Suite config sections"
          >
            <strong>Sections</strong>
            {filteredConfigSections.map((section) => (
              <button
                aria-current={
                  activeConfigSection === section.id ? "true" : undefined
                }
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
              aria-current={
                activeConfigSection === "review" ? "true" : undefined
              }
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
                  <span>
                    {editorMode === "form"
                      ? "Structured sections with help, defaults, current value, validation rule, and reload impact."
                      : "Advanced TOML editor for settings not covered by structured controls."}
                  </span>
                </div>
                <div className="editorModeGroup">
                  <label className="systemConfigSearch">
                    <Search size={15} />
                    <span>
                      {normalizedConfigSearch
                        ? `${visibleConfigFieldCount} matching settings`
                        : "Search settings"}
                    </span>
                    <input
                      aria-label="Search suite config settings"
                      onChange={(event) => setConfigSearch(event.target.value)}
                      placeholder="Search key, label, help"
                      type="search"
                      value={configSearch}
                    />
                  </label>
                  <ConsoleStatusBadge tone={parsedDraft.ok ? "ok" : "warning"}>
                    {parsedDraft.ok ? "TOML parsed" : "TOML invalid"}
                  </ConsoleStatusBadge>
                  <div
                    className="segmented"
                    role="group"
                    aria-label="Suite config editor mode"
                  >
                    <button
                      aria-pressed={editorMode === "form"}
                      className={editorMode === "form" ? "selected" : ""}
                      onClick={() => setEditorMode("form")}
                      type="button"
                    >
                      Fields
                    </button>
                    <button
                      aria-pressed={editorMode === "toml"}
                      className={editorMode === "toml" ? "selected" : ""}
                      onClick={() => setEditorMode("toml")}
                      type="button"
                    >
                      Advanced TOML
                    </button>
                  </div>
                </div>
              </div>
              {!parsedDraft.ok && (
                <div className="panelWarning systemConfigNotice">
                  Structured controls are paused until the TOML parses. Use the
                  advanced TOML editor to repair the document.
                </div>
              )}
              {editorMode === "form" ? (
                <div className="systemConfigSectionStack">
                  {filteredConfigSections.length === 0 ? (
                    <div className="systemConfigEmptySearch">
                      <strong>No settings match “{configSearch.trim()}”</strong>
                      <span>
                        Search labels, config paths, validation rules, defaults,
                        and help text.
                      </span>
                      <button
                        className="secondaryAction compactAction"
                        onClick={() => setConfigSearch("")}
                        type="button"
                      >
                        Clear search
                      </button>
                    </div>
                  ) : null}
                  {filteredConfigSections.length > 0 &&
                  displayedConfigSections.length === 0 ? (
                    <div className="systemConfigEmptySearch">
                      <strong>Review workflow selected</strong>
                      <span>
                        Use the review panel below for validation, privilege,
                        save, reload, restart, and audit evidence.
                      </span>
                    </div>
                  ) : null}
                  {displayedConfigSections.map((section) => (
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
                <div
                  className="systemTomlEditor"
                  title="Complete Suite Config TOML editor; changes are validated before the review and save workflow."
                >
                  <div className="systemTomlNotes">
                    <span>
                      {config?.hot_reload_note ??
                        "Hot-reload notes unavailable"}
                    </span>
                    <span>
                      {config?.restart_required_note ??
                        "Restart notes unavailable"}
                    </span>
                  </div>
                  <textarea
                    aria-label="Suite config TOML"
                    className="systemConfigToml"
                    onChange={(event) => {
                      validationRequestId.current += 1;
                      setConfigError(null);
                      setConfigMessage(null);
                      setDraftToml(event.target.value);
                      setAdvancedTomlEdited(
                        event.target.value !== config?.toml,
                      );
                      setValidation(null);
                      setValidationPending(false);
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
                  <span>
                    Edit, auto-validate, unlock, review diff, save, then follow
                    reload/restart and audit evidence.
                  </span>
                </div>
                <ConsoleStatusBadge
                  tone={
                    validation?.validation.valid
                      ? "ok"
                      : dirty
                        ? "warning"
                        : "neutral"
                  }
                >
                  {validation
                    ? textOnlyDraftChange
                      ? "Text changed"
                      : `${changedKeys.length} changed`
                    : dirty
                      ? "Draft"
                      : "No draft"}
                </ConsoleStatusBadge>
              </div>

              <div className="systemReviewStack">
                <ConfigSaveStepper
                  auditRecorded={
                    configMessage?.includes("Saved suite config") ?? false
                  }
                  dirty={dirty}
                  pending={pending}
                  privilegeReady={Boolean(privilegeMaterial)}
                  validated={Boolean(validation?.validation.valid)}
                />

                <div className={`systemNextAction ${nextAction.tone}`}>
                  <strong>{nextAction.label}</strong>
                  <span>{nextAction.detail}</span>
                  {nextAction.action === "validate" ? (
                    <button
                      className="secondaryAction compactAction"
                      disabled={pending || !draftToml.trim()}
                      onClick={() => void validateDraft({ silent: false })}
                      type="button"
                    >
                      <CheckCircle2 size={16} />
                      Validate now
                    </button>
                  ) : nextAction.action === "unlock" ? (
                    <button
                      className="secondaryAction compactAction"
                      onClick={onOpenPrivilegeUnlock}
                      type="button"
                    >
                      <LockKeyhole size={16} />
                      Unlock privilege
                    </button>
                  ) : nextAction.action === "review" ? (
                    <button
                      className="primaryAction compactAction"
                      disabled={reviewDisabled}
                      onClick={() => void reviewChanges()}
                      title={
                        reviewDisabled
                          ? nextAction.detail
                          : "Review changed suite config keys, reload impact, and privilege state."
                      }
                      type="button"
                    >
                      <Save size={16} />
                      Review changes
                    </button>
                  ) : null}
                </div>

                <div className="systemReviewBlock">
                  <h3>Changed keys</h3>
                  <div className="chipList compactChipList">
                    {changedKeys.map((key) => (
                      <span
                        className={configFieldImpact(key, activeValidation)}
                        key={key}
                      >
                        {key}
                      </span>
                    ))}
                    {validation && changedKeys.length === 0 ? (
                      <span>
                        {textOnlyDraftChange
                          ? "Formatting or comments only"
                          : "No changes"}
                      </span>
                    ) : null}
                    {!validation ? (
                      <span>
                        {validationPending
                          ? "Auto-validating draft"
                          : "Draft will validate before review"}
                      </span>
                    ) : null}
                  </div>
                </div>

                <div
                  className="systemImpactGrid"
                  aria-label="Suite config reload and restart plan"
                >
                  <ImpactList
                    title="Hot reload after save"
                    fields={changedHotReloadFields}
                    emptyLabel="No changed hot-reload fields"
                  />
                  <ImpactList
                    title="Restart required after save"
                    fields={changedRestartRequiredFields}
                    emptyLabel="No changed restart-only fields"
                  />
                  <ImpactList
                    title="Impact not reported"
                    fields={changedUnknownImpactFields}
                    emptyLabel="All changed keys have a reported impact"
                  />
                </div>

                <div className="systemReviewBlock">
                  <h3>Privilege</h3>
                  <div
                    className={`privilegeGateBox ${privilegeMaterial ? "ready" : ""}`}
                  >
                    <LockKeyhole size={18} />
                    <span>
                      {privilegeMaterial
                        ? "Privilege unlocked for this browser session"
                        : "Unlock privilege before saving suite config"}
                    </span>
                    {!privilegeMaterial && (
                      <button
                        className="secondaryAction compactAction"
                        onClick={onOpenPrivilegeUnlock}
                        type="button"
                      >
                        Unlock privilege
                      </button>
                    )}
                  </div>
                </div>

                <div className="systemReviewBlock">
                  <h3>Save</h3>
                  <button
                    className="primaryAction wideAction"
                    disabled={reviewDisabled}
                    onClick={() => void reviewChanges()}
                    title={
                      reviewDisabled
                        ? nextAction.detail
                        : "Review changed suite config keys, reload impact, and privilege state."
                    }
                    type="button"
                  >
                    <Save size={16} />
                    <span>{savePending ? "Saving" : "Review changes"}</span>
                  </button>
                </div>

                <details
                  className="systemAdvancedDiff"
                  title="Read-only comparison of the persisted and proposed redacted configuration."
                >
                  <summary>Advanced redacted JSON diff</summary>
                  <div className="systemDiffPreview">
                    <div>
                      <h3>Current redacted</h3>
                      <pre className="jsonPreview compactJsonPreview">
                        {formatJson(
                          config?.redacted ?? validation?.old_redacted ?? null,
                        )}
                      </pre>
                    </div>
                    <div>
                      <h3>Draft redacted</h3>
                      <pre className="jsonPreview compactJsonPreview">
                        {formatJson(validation?.redacted ?? null)}
                      </pre>
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
                  ...(textOnlyDraftChange
                    ? [{ label: "Advanced TOML text", value: "Changed" }]
                    : []),
                  {
                    label: "Hot reload changed",
                    value: String(changedHotReloadFields.length),
                  },
                  {
                    label: "Restart required changed",
                    value: String(changedRestartRequiredFields.length),
                  },
                  {
                    label: "Privilege",
                    value: privilegeMaterial ? "Unlocked locally" : "Locked",
                  },
                ]}
                onCancel={() => setConfirmOpen(false)}
                onConfirm={() => void saveDraft()}
                open={confirmOpen}
                pending={savePending}
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

type ParsedTomlDraft =
  | { ok: true; table: TomlTable }
  | { ok: false; error: string };

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

function ImpactList({
  emptyLabel,
  fields,
  title,
}: {
  emptyLabel: string;
  fields: string[];
  title: string;
}) {
  return (
    <div className="systemImpactList">
      <h3>{title}</h3>
      <ul>
        {fields.slice(0, 8).map((field) => (
          <li key={field}>{field}</li>
        ))}
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
  const draftValue = parsedDraft.ok
    ? getTomlPath(parsedDraft.table, pathParts)
    : undefined;
  const currentValue = currentDraft?.ok
    ? getTomlPath(currentDraft.table, pathParts)
    : undefined;
  const currentLabel = formatConfigValue(currentValue);
  const defaultValue = configFieldDefaultValue(field);
  const defaultLabel = formatConfigValue(defaultValue);
  const changed = !configValuesEqual(draftValue, currentValue);
  const impact = configFieldImpact(field.path, activeValidation);
  const controlId = `suite-config-field-${field.path.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  return (
    <div className={`systemConfigFieldRow ${changed ? "changed" : ""}`}>
      <div className="systemConfigFieldText">
        <label htmlFor={controlId}>
          {field.label}
          {field.required ? <span>required</span> : null}
          {changed ? <span className="changed">changed</span> : null}
        </label>
        <p>{field.help}</p>
        <details className="systemConfigFieldMeta" open={changed}>
          <summary>
            <strong>{changed ? "Changed metadata" : "Field metadata"}</strong>
            <span>
              Current {currentLabel} · Default {defaultLabel} ·{" "}
              {configImpactLabel(impact)}
            </span>
          </summary>
          <dl>
            <div>
              <dt>Current</dt>
              <dd>{currentLabel}</dd>
            </div>
            <div>
              <dt>Default</dt>
              <dd>{defaultLabel}</dd>
            </div>
            <div>
              <dt>Validation</dt>
              <dd>{field.rule}</dd>
            </div>
            <div>
              <dt>Impact</dt>
              <dd>
                <ConfigImpactBadge impact={impact} />
              </dd>
            </div>
          </dl>
        </details>
      </div>
      <div
        className="systemConfigFieldControl"
        title={
          draftValue === undefined
            ? `${field.label} has no explicit value and inherits its configured default.`
            : undefined
        }
      >
        {field.kind === "checkbox" ? (
          <label className="checkLine inlineCheck">
            <input
              aria-label={field.label}
              checked={draftValue === true}
              data-tooltip-disabled-reason="Form fields are unavailable until the Suite Config TOML parses successfully."
              disabled={!parsedDraft.ok}
              id={controlId}
              onChange={(event) => onChange(field.path, event.target.checked)}
              type="checkbox"
            />
            <span>{draftValue === true ? "Enabled" : "Disabled"}</span>
          </label>
        ) : (
          <input
            aria-label={field.label}
            aria-describedby={`${controlId}-meta`}
            data-tooltip-disabled-reason="Form fields are unavailable until the Suite Config TOML parses successfully."
            data-tooltip-sensitive={
              configFieldValueIsSensitive(field) ? "true" : undefined
            }
            disabled={!parsedDraft.ok}
            id={controlId}
            min={field.kind === "number" ? 0 : undefined}
            onChange={(event) => {
              const next = event.target.value.trim();
              onChange(
                field.path,
                next
                  ? field.kind === "number"
                    ? Number(next)
                    : event.target.value
                  : undefined,
              );
            }}
            type={field.kind === "number" ? "number" : "text"}
            value={
              field.kind === "number"
                ? formatNumberInputValue(draftValue)
                : formatTextInputValue(draftValue)
            }
          />
        )}
        <span id={`${controlId}-meta`} className="systemConfigPath">
          {field.path}
        </span>
        <div className="systemConfigFieldActions">
          <button
            className="secondaryAction compactAction"
            data-tooltip-disabled-reason={
              !parsedDraft.ok
                ? "Reset is unavailable until the Suite Config TOML parses successfully."
                : "This field already matches the loaded value."
            }
            disabled={
              !parsedDraft.ok || configValuesEqual(draftValue, currentValue)
            }
            onClick={() => onChange(field.path, currentValue)}
            title={
              !parsedDraft.ok
                ? "Reset is unavailable until the Suite Config TOML parses successfully."
                : configValuesEqual(draftValue, currentValue)
                  ? "This field already matches the loaded value."
                  : `Reset ${field.label} to the loaded value.`
            }
            type="button"
          >
            <TimerReset size={14} />
            <span>Reset current</span>
          </button>
          <button
            className="secondaryAction compactAction"
            data-tooltip-disabled-reason={
              !parsedDraft.ok
                ? "Default inheritance is unavailable until the Suite Config TOML parses successfully."
                : "This field already inherits its default value."
            }
            disabled={!parsedDraft.ok || draftValue === undefined}
            onClick={() => onChange(field.path, undefined)}
            title={
              !parsedDraft.ok
                ? "Default inheritance is unavailable until the Suite Config TOML parses successfully."
                : draftValue === undefined
                  ? "This field already inherits its default value."
                  : `Use the inherited default for ${field.label}; removes the explicit value.`
            }
            type="button"
          >
            <SlidersHorizontal size={14} />
            <span>Use default</span>
          </button>
        </div>
      </div>
    </div>
  );
}

function operatorFieldDisabledReason(
  canManageUsers: boolean,
  editingDeleted: boolean,
) {
  if (!canManageUsers) {
    return "Operator fields can be changed only by an admin operator.";
  }
  if (editingDeleted) {
    return "Deleted operator records are read-only.";
  }
  return "This operator field is available.";
}

function configFieldValueIsSensitive(field: ConfigFieldSpec) {
  return (
    !field.path.endsWith("_file") &&
    /password|passphrase|secret|token|private|verifier|salt|api[_-]?key/i.test(
      `${field.path} ${field.label}`,
    )
  );
}

function ConfigImpactBadge({ impact }: { impact: ConfigImpact }) {
  const label = configImpactLabel(impact);
  return <span className={`configImpactBadge ${impact}`}>{label}</span>;
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
        <li
          className={`${step.complete ? "complete" : ""} ${step.active ? "active" : ""}`}
          key={step.label}
        >
          <span>
            {step.complete ? (
              <CheckCircle2 size={14} />
            ) : (
              <span aria-hidden="true" />
            )}
          </span>
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
  if (
    validation.restart_required_fields.some((pattern) =>
      configPathMatches(path, pattern),
    )
  ) {
    return "restart";
  }
  if (
    validation.hot_reload_fields.some((pattern) =>
      configPathMatches(path, pattern),
    )
  ) {
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
    return "Unset (uses default)";
  }
  if (typeof value === "boolean") {
    return value ? "On" : "Off";
  }
  if (typeof value === "number") {
    return String(value);
  }
  if (typeof value === "string") {
    return value;
  }
  return "configured";
}

function changedKeyCountLabel(count: number): string {
  return `${count} changed key${count === 1 ? "" : "s"}`;
}

function configFieldDefaultValue(field: ConfigFieldSpec): unknown {
  if (field.defaultValue === "unset") {
    return undefined;
  }
  if (field.kind === "checkbox") {
    return field.defaultValue === "true";
  }
  if (field.kind === "number") {
    const numeric = Number(field.defaultValue);
    return Number.isFinite(numeric) ? numeric : undefined;
  }
  return field.defaultValue;
}

function configValuesEqual(left: unknown, right: unknown): boolean {
  return JSON.stringify(left ?? null) === JSON.stringify(right ?? null);
}

function configImpactLabel(impact: ConfigImpact): string {
  return impact === "restart"
    ? "Restart required"
    : impact === "hot"
      ? "Hot reload"
      : "Impact not reported";
}

function configFieldMatchesSearch(
  section: ConfigSectionSpec,
  field: ConfigFieldSpec,
  query: string,
): boolean {
  if (!query) {
    return true;
  }
  return [
    section.title,
    section.description,
    field.label,
    field.path,
    field.help,
    field.rule,
    field.defaultValue,
  ]
    .join(" ")
    .toLowerCase()
    .includes(query);
}

function formatNumberInputValue(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value)
    ? String(value)
    : "";
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
      detail:
        "No draft changes are waiting. Edit a field or use Advanced TOML to start a save flow.",
      label: "No draft changes",
      tone: "neutral",
    };
  }
  if (!validation) {
    if (!privilegeReady) {
      return {
        action: "unlock",
        detail:
          "Draft validation runs automatically. Unlock privilege before reviewing changed keys and reload impact.",
        label: "Next: unlock privilege",
        tone: "warning",
      };
    }
    return {
      action: "review",
      detail:
        "Review changes will validate this draft before opening the save confirmation.",
      label: "Next: review changes",
      tone: "warning",
    };
  }
  if (!validation.validation.valid) {
    return {
      action: "validate",
      detail:
        "Validation failed. Fix TOML or field values; the draft will validate again automatically.",
      label: "Fix validation errors",
      tone: "warning",
    };
  }
  if (!privilegeReady) {
    return {
      action: "unlock",
      detail:
        "Suite config writes require local privilege unlock before review can proceed.",
      label: "Next: unlock privilege",
      tone: "warning",
    };
  }
  if (!reviewDisabled) {
    return {
      action: "review",
      detail:
        "Changed keys, reload plan, and privilege state are ready for final confirmation.",
      label: "Next: review changes",
      tone: "ok",
    };
  }
  return {
    action: "none",
    detail:
      "The save flow is waiting for a valid draft, privilege, and review state.",
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
  const warningSignals =
    (gatewayEvents.dropped_events ?? 0) +
    (gatewayEvents.telemetry_dropped_events ?? 0) +
    (gatewayEvents.retained_output_truncated_events ?? 0) +
    (gatewayEvents.rejected_agent_connections ?? 0) +
    (gatewayEvents.retry_attempts ?? 0);
  if (warningSignals > 0) {
    return "warning";
  }
  const oldestAgeSecs = gatewayEvents.oldest_event_age_secs;
  if (oldestAgeSecs !== null && oldestAgeSecs >= 300) {
    return "critical";
  }
  if (oldestAgeSecs !== null && oldestAgeSecs >= 60) {
    return "warning";
  }
  return (gatewayEvents.current_queue_depth ?? 0) > 0 ? "info" : "ok";
}

function dispatchCapacityHealth({
  dispatcherBatch,
  dispatcherInFlight,
  oldestAgeSecs,
  queueDepth,
  queueGrowth,
}: {
  dispatcherBatch: number | null;
  dispatcherInFlight: number | null;
  oldestAgeSecs: number | null;
  queueDepth: number | null;
  queueGrowth: number | null;
}): { reason: string; tone: SystemHealthTone; warningThreshold: string } {
  const warningThreshold = dispatcherInFlight
    ? Math.ceil(dispatcherInFlight * 0.5)
    : null;
  const hardThreshold = dispatcherBatch ?? dispatcherInFlight;
  const thresholdLabel = warningThreshold
    ? `${warningThreshold} queued`
    : "Not configured";
  if (queueDepth === null) {
    return {
      reason: "queue evidence unavailable",
      tone: "neutral",
      warningThreshold: "Unknown",
    };
  }
  if (queueDepth <= 0) {
    return {
      reason: "queue empty",
      tone: "ok",
      warningThreshold: thresholdLabel,
    };
  }
  if (!dispatcherInFlight) {
    return {
      reason: "worker capacity not configured",
      tone: "critical",
      warningThreshold: thresholdLabel,
    };
  }
  if (hardThreshold && queueDepth >= hardThreshold) {
    return {
      reason: "queue reached configured batch capacity",
      tone: "critical",
      warningThreshold: thresholdLabel,
    };
  }
  if (oldestAgeSecs !== null && oldestAgeSecs >= 300) {
    return {
      reason: "oldest queued item exceeds 5m",
      tone: "critical",
      warningThreshold: thresholdLabel,
    };
  }
  if (oldestAgeSecs !== null && oldestAgeSecs >= 60) {
    return {
      reason: "oldest queued item exceeds 60s",
      tone: "warning",
      warningThreshold: thresholdLabel,
    };
  }
  if (warningThreshold && queueDepth >= warningThreshold) {
    return {
      reason: "queue crossed half of in-flight capacity",
      tone: "warning",
      warningThreshold: thresholdLabel,
    };
  }
  if (queueGrowth !== null && queueGrowth > 0) {
    return {
      reason: "queue is growing in selected samples",
      tone: "warning",
      warningThreshold: thresholdLabel,
    };
  }
  return {
    reason: "queue below threshold",
    tone: "info",
    warningThreshold: thresholdLabel,
  };
}

function gatewayCapacityHealth({
  gatewayEvents,
  queueGrowth,
}: {
  gatewayEvents: SystemDashboardRecord["current"]["gateway_events"] | undefined;
  queueGrowth: number | null;
}): { reason: string; tone: SystemHealthTone } {
  if (!gatewayEvents || gatewayEvents.status !== "live") {
    return { reason: "gateway status unavailable", tone: "warning" };
  }
  const hardFailures =
    (gatewayEvents.critical_failures ?? 0) +
    gatewayEvents.critical_failures_by_reason.global_queue_full +
    gatewayEvents.critical_failures_by_reason.target_queue_full +
    gatewayEvents.critical_failures_by_reason.expired +
    gatewayEvents.dropped_by_reason.global_queue_full +
    gatewayEvents.dropped_by_reason.target_queue_full +
    (gatewayEvents.expired_events ?? 0);
  if (hardFailures > 0) {
    return {
      reason: "queue-full or expired gateway failures",
      tone: "critical",
    };
  }
  const queueDepth = gatewayEvents.current_queue_depth ?? 0;
  const oldestAgeSecs = gatewayEvents.oldest_event_age_secs;
  if (oldestAgeSecs !== null && oldestAgeSecs >= 300) {
    return { reason: "oldest gateway event exceeds 5m", tone: "critical" };
  }
  if (oldestAgeSecs !== null && oldestAgeSecs >= 60) {
    return { reason: "oldest gateway event exceeds 60s", tone: "warning" };
  }
  if (queueGrowth !== null && queueGrowth > 0) {
    return { reason: "gateway queue is growing", tone: "warning" };
  }
  if (queueDepth > 0) {
    return { reason: "queue present below age threshold", tone: "info" };
  }
  return { reason: "queue clear", tone: "ok" };
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
  gatewayOldestAgeLabel,
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
  gatewayOldestAgeLabel: string;
  gatewayQueueDepth: number;
  gatewayRejected: number;
  gatewayRetries: number;
  gatewayTone: SystemHealthTone;
  lifecycleFailures: number;
  queueDepth: number;
}): SystemAttentionItem[] {
  const items: SystemAttentionItem[] = [];
  if (dbTone !== "ok" && dbTone !== "neutral") {
    items.push({
      detail:
        "DB pool pressure crossed the dashboard threshold; validate pool sizing before sustained operator traffic.",
      label: "DB pool pressure",
      tone: dbTone,
      value: `${dbPressurePercent}%`,
    });
  }
  if (dispatchTone === "warning" || dispatchTone === "critical") {
    items.push({
      detail:
        "Queued dispatch work exists in the current sample; confirm the queue is draining and retries are not climbing.",
      label: "Dispatch queue",
      tone: dispatchTone,
      value: `${queueDepth} queued`,
    });
  }
  if (lifecycleFailures > 0) {
    items.push({
      detail:
        "Control, agent timeout, or lost-agent outcomes were recorded in the last 24 hours.",
      label: "Deadline timeouts",
      tone: "warning",
      value: `${lifecycleFailures} event${lifecycleFailures === 1 ? "" : "s"}`,
    });
  }
  if (gatewayTone === "warning" || gatewayTone === "critical") {
    items.push({
      detail: `${gatewayQueueDepth} queued, ${gatewayOldestAgeLabel}, ${gatewayDropped} dropped, ${gatewayRetries} retries, ${gatewayRejected} rejected connects.`,
      label: "Gateway delivery",
      tone: gatewayTone,
      value:
        gatewayCriticalFailures > 0
          ? `${gatewayCriticalFailures} critical`
          : `${gatewayDropped} dropped`,
    });
  }
  return items;
}

function seriesDelta(
  series: SystemMetricSeriesRecord[],
  metric: string,
): { delta: number | null; latest: number | null } {
  const entry = series.find((candidate) => candidate.metric === metric);
  const values =
    entry?.points
      .map((point) => point.latest_value)
      .filter((value) => Number.isFinite(value)) ?? [];
  if (!values.length) {
    return { delta: null, latest: null };
  }
  return {
    delta: values[values.length - 1] - values[0],
    latest: values[values.length - 1],
  };
}

function formatDelta(value: number | null): string {
  if (value === null) {
    return "Not reported";
  }
  if (value > 0) {
    return `+${formatNumber(value)}`;
  }
  return formatNumber(value);
}

function systemSeriesCoverageSummary(
  series: SystemMetricSeriesRecord[],
): string {
  const times = Array.from(
    new Set(
      series.flatMap((entry) =>
        entry.points.map((point) => point.bucket_start),
      ),
    ),
  ).sort((left, right) => Date.parse(left) - Date.parse(right));
  if (!times.length) {
    return "no samples in selected range";
  }
  const first = Date.parse(times[0]);
  const last = Date.parse(times[times.length - 1]);
  if (!Number.isFinite(first) || !Number.isFinite(last)) {
    return `${times.length} sample buckets`;
  }
  const firstLabel = new Date(first).toLocaleTimeString([], {
    hour: "numeric",
    minute: "2-digit",
  });
  const lastLabel = new Date(last).toLocaleTimeString([], {
    hour: "numeric",
    minute: "2-digit",
  });
  const duration = formatDurationLabel(
    Math.max(0, Math.round((last - first) / 1000)),
  );
  return `${times.length} sample buckets from ${firstLabel} to ${lastLabel} (${duration})`;
}

function formatDurationLabel(totalSeconds: number): string {
  if (totalSeconds < 60) {
    return `${totalSeconds}s`;
  }
  const minutes = Math.round(totalSeconds / 60);
  if (minutes < 60) {
    return `${minutes}m`;
  }
  const hours = Math.round(minutes / 60);
  return `${hours}h`;
}

function chartLines(
  series: SystemMetricSeriesRecord[],
  metrics: string[],
  bucketSecs: number,
): { lines: TimeSeriesChartLine[]; times: string[] } {
  const selected = metrics
    .map((metric) => series.find((entry) => entry.metric === metric))
    .filter((entry): entry is SystemMetricSeriesRecord => Boolean(entry));
  const observedTimes = Array.from(
    new Set(
      selected.flatMap((entry) =>
        entry.points.map((point) => point.bucket_start),
      ),
    ),
  ).sort((left, right) => Date.parse(left) - Date.parse(right));
  const observedEpochs = observedTimes
    .map((time) => Math.floor(Date.parse(time) / 1_000))
    .filter(Number.isFinite);
  const step = Math.max(60, Math.floor(bucketSecs));
  const first = observedEpochs[0];
  const last = observedEpochs[observedEpochs.length - 1];
  const epochs: number[] = [];
  if (first !== undefined && last !== undefined) {
    for (
      let epoch = first;
      epoch <= last && epochs.length <= 1_442;
      epoch += step
    ) {
      epochs.push(epoch);
    }
  }
  const times = epochs.map((epoch) => new Date(epoch * 1_000).toISOString());
  const lines = selected.map((entry, index) => {
    const points = new Map(
      entry.points.map((point) => [
        Math.floor(Date.parse(point.bucket_start) / 1_000),
        point.latest_value,
      ]),
    );
    return {
      color: dashboardChartColors[index % dashboardChartColors.length],
      label: entry.label,
      values: epochs.map((epoch) => points.get(epoch) ?? null),
    };
  });
  return { lines, times };
}

function parseTomlDraft(toml: string): ParsedTomlDraft {
  try {
    return { ok: true, table: parse(toml) as TomlTable };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : "Invalid TOML",
    };
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
  if (value === undefined || value === null || value === "") {
    let current = table as Record<string, unknown>;
    const ancestors: Array<{
      child: Record<string, unknown>;
      key: string;
      parent: Record<string, unknown>;
    }> = [];
    for (const part of path.slice(0, -1)) {
      const child = current[part];
      if (!child || typeof child !== "object" || Array.isArray(child)) {
        return;
      }
      ancestors.push({
        child: child as Record<string, unknown>,
        key: part,
        parent: current,
      });
      current = child as Record<string, unknown>;
    }
    delete current[path[path.length - 1]];
    for (const { child, key, parent } of ancestors.reverse()) {
      if (Object.keys(child).length > 0) {
        break;
      }
      delete parent[key];
    }
    return;
  }
  let current = table as Record<string, unknown>;
  for (const part of path.slice(0, -1)) {
    if (
      !current[part] ||
      typeof current[part] !== "object" ||
      Array.isArray(current[part])
    ) {
      current[part] = {};
    }
    current = current[part] as Record<string, unknown>;
  }
  const key = path[path.length - 1];
  current[key] = value;
}

function formatNumber(value: number | null | undefined): string {
  return value === null || value === undefined
    ? "No data"
    : String(Math.round(value));
}

function valueOrUnset(value: number | null | undefined): string {
  return value === null || value === undefined ? "unset" : String(value);
}

function valueOrNotConfigured(value: number | null | undefined): string {
  return value === null || value === undefined
    ? "Not configured"
    : String(value);
}

function secondsOrUnset(value: number | null | undefined): string {
  return value === null || value === undefined ? "unset" : `${value}s`;
}

function secondsOrNotConfigured(value: number | null | undefined): string {
  return value === null || value === undefined ? "Not configured" : `${value}s`;
}

function formatJson(value: JsonValue | null): string {
  return value === null
    ? "Validate draft to preview redacted JSON."
    : JSON.stringify(value, null, 2);
}
