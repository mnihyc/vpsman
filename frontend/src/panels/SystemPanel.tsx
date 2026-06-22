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
  JsonValue,
  OperatorAuthEventRecord,
  OperatorView,
  OperatorSessionRecord,
  SuiteConfigResponse,
  SuiteConfigUpdateResponse,
  SuiteConfigValidateResponse,
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

const chartColors = ["#1a73e8", "#188038", "#f29900", "#9334e6", "#d93025", "#129eaf", "#5f6368", "#b06000"];
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
    "Refresh-token/session lifetime in days for newly issued operator sessions. This is not the short access-token expiry shown in System > Sessions.",
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
        onUpdate={onUpdateSuiteConfig}
        onValidate={onValidateSuiteConfig}
        privilegeMaterial={privilegeMaterial}
      />
    );
  }
  if (activeSubpage === "users") {
    return (
      <SystemUsersPanel
        currentOperator={operator}
        onClearOperatorTotp={onClearOperatorTotp}
        onCreateOperator={onCreateOperator}
        onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
        onResetOperatorPassword={onResetOperatorPassword}
        onSetOperatorStatus={onSetOperatorStatus}
        onUpdateOperator={onUpdateOperator}
        operators={operators}
        privilegeMaterial={privilegeMaterial}
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
    };

type OperatorPrivilegeSnapshot = {
  payloadHashHex: string;
  privilegeAssertion: PrivilegeAssertion;
};

type PendingSessionRevoke = {
  sessions: OperatorSessionRecord[];
  adminRisk: boolean;
  privileges: Record<string, OperatorPrivilegeSnapshot>;
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
  currentOperator,
  onClearOperatorTotp,
  onCreateOperator,
  onOpenPrivilegeUnlock,
  onResetOperatorPassword,
  onSetOperatorStatus,
  onUpdateOperator,
  operators,
  privilegeMaterial,
}: {
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
        id: "ttl",
        header: "Session TTL",
        cell: (row) => (
          <span title={operatorHelpText.sessionRefreshTtl}>
            {secondsToDays(row.session_refresh_ttl_secs)}d
          </span>
        ),
        sortValue: (row) => row.session_refresh_ttl_secs,
      },
      {
        id: "totp",
        header: "TOTP",
        cell: (row) => (row.totp_enabled ? "enabled" : "off"),
        searchValue: (row) => row.totp_enabled,
        sortValue: (row) => row.totp_enabled,
      },
      {
        id: "created",
        header: "Created",
        cell: (row) => formatTime(row.created_at),
        sortValue: (row) => row.created_at,
      },
    ],
    [],
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
      } else {
        for (const operator of pendingAction.operators) {
          await onClearOperatorTotp(
            operator.id,
            pendingAction.adminRisk,
            pendingAction.privileges[operator.id].privilegeAssertion,
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
          renderExpandedRow={(row) => <OperatorDetailGrid operator={row} />}
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
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const sessionColumns = useMemo<ConsoleDataGridColumn<OperatorSessionRecord>[]>(
    () => [
      { id: "operator", header: "User", cell: (row) => row.operator_username, searchValue: (row) => row.operator_username },
      { id: "role", header: "Role", cell: (row) => row.operator_role, searchValue: (row) => row.operator_role },
      { id: "created", header: "Created", cell: (row) => formatTime(row.created_at), sortValue: (row) => row.created_at },
      {
        id: "access",
        header: "Access expires",
        cell: (row) => (
          <span title={operatorHelpText.sessionAccessExpires}>
            {formatTime(row.expires_at)}
          </span>
        ),
        sortValue: (row) => row.expires_at,
      },
      {
        id: "refresh",
        header: "Refresh expires",
        cell: (row) => (
          <span title={operatorHelpText.sessionRefreshExpires}>
            {formatTime(row.refresh_expires_at)}
          </span>
        ),
        sortValue: (row) => row.refresh_expires_at,
      },
      {
        id: "state",
        header: "State",
        cell: (row) => row.current ? "current" : row.revoked ? "revoked" : "active",
        searchValue: (row) => row.current ? "current" : row.revoked ? "revoked" : "active",
      },
    ],
    [],
  );
  const eventColumns = useMemo<ConsoleDataGridColumn<OperatorAuthEventRecord>[]>(
    () => [
      { id: "time", header: "Time", cell: (row) => formatTime(row.created_at), sortValue: (row) => row.created_at },
      { id: "username", header: "Username", cell: (row) => row.username, searchValue: (row) => row.username },
      {
        id: "result",
        header: "Result",
        cell: (row) => <span className={`statusPill ${statusClass(row.result)}`}>{row.result}</span>,
        searchValue: (row) => row.result,
      },
      { id: "reason", header: "Reason", cell: (row) => row.reason ?? "-", searchValue: (row) => row.reason },
      { id: "remote", header: "Remote IP", cell: (row) => row.remote_ip ?? "-", searchValue: (row) => row.remote_ip },
      { id: "session", header: "Session", cell: (row) => shortId(row.session_id), searchValue: (row) => row.session_id },
    ],
    [],
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
            renderExpandedRow={(row) => <SessionDetailGrid session={row} />}
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
            searchPlaceholder="Search user, role, or state"
            singleExpandedRow
            storageKey="vpsman.system.sessions"
            title="Sessions"
          />
        </section>
        <section className="controlPanel">
          <div className="sectionHeader compact">
            <h2>Authentication history</h2>
            <span>{authEvents.length} login results</span>
          </div>
          <ConsoleDataGrid
            columns={eventColumns}
            defaultPageSize={12}
            empty="No authentication events"
            expandOnRowClick
            getRowId={(row) => row.id}
            itemLabel="events"
            renderExpandedRow={(row) => <AuthEventDetailGrid event={row} />}
            rows={authEvents}
            searchPlaceholder="Search username, result, reason, remote IP, or session"
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

function OperatorDetailGrid({ operator }: { operator: OperatorView }) {
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

function SessionDetailGrid({ session }: { session: OperatorSessionRecord }) {
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
  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <div className="dashboardToolbar">
          <div>
            <h2>System Dashboard</h2>
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
                aria-label="System dashboard point density"
                onChange={(event) => onPointDensityChange(event.target.value as SystemDashboardPointDensity)}
                value={pointDensity}
              >
                {pointDensityOptions.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </label>
            <div className="timeRangeTabs" aria-label="System dashboard time range">
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

        <SystemMetricSection
          badge={`${Math.round(dbPressure * 100)}% in use`}
          icon={<Database size={18} />}
          title="Capacity"
          subtitle="Database pool pressure and configured control-plane limits."
          metrics={[
            { label: "API DB pool", value: valueOrUnset(dashboard?.capacity.api_db_pool) },
            { label: "Worker DB pool", value: valueOrUnset(dashboard?.capacity.worker_db_pool) },
            { label: "Dispatcher in-flight", value: valueOrUnset(dashboard?.capacity.dispatcher_in_flight) },
            { label: "Dispatcher batch", value: valueOrUnset(dashboard?.capacity.dispatcher_batch) },
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
          badge={`${dashboard?.current.dispatch.queue_depth ?? 0} queued`}
          icon={<Activity size={18} />}
          title="Dispatch Lifecycle"
          subtitle="Queued, dispatching, running, retry, and active job pressure."
          metrics={[
            { label: "Active jobs", value: String(dashboard?.current.dispatch.active_jobs ?? 0) },
            { label: "Dispatch queue", value: String(dashboard?.current.dispatch.queue_depth ?? 0) },
            { label: "Active targets", value: String(dashboard?.current.targets.active ?? 0) },
            { label: "Retried targets", value: String(dashboard?.current.dispatch.retried_targets ?? 0) },
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
          icon={<AlertTriangle size={18} />}
          title="Deadlines"
          subtitle="Control deadline expiry, agent timeouts, agent loss, and canceled outcomes."
          metrics={[
            { label: "Lifecycle failures", value: String(lifecycleFailures) },
            { label: "Control timed out", value: String(dashboard?.current.targets.control_timeout_last_24h ?? 0) },
            { label: "Agent timed out", value: String(dashboard?.current.targets.agent_timeout_last_24h ?? 0) },
            { label: "Agent lost", value: String(dashboard?.current.targets.agent_lost_last_24h ?? 0) },
            { label: "Agent offline timeout", value: secondsOrUnset(dashboard?.capacity.agent_offline_secs) },
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
          icon={<Network size={18} />}
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
          lines={chartLines(series, [
            "gateway_events.current_queue_depth",
            "gateway_events.oldest_event_age_secs",
            "gateway_events.dropped_events",
            "gateway_events.critical_failures",
            "gateway_events.dropped_by_reason.coalesced",
            "gateway_events.dropped_by_reason.protocol_conflict",
            "gateway_events.dropped_by_reason.target_queue_full",
            "gateway_events.retained_output_truncated_events",
            "gateway_events.rejected_agent_connections",
            "gateway_events.delivered_events",
            "gateway_events.retry_attempts",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <SystemMetricSection
          badge={`${dashboard?.current.cancellations.awaiting_ack ?? 0} waiting`}
          icon={<TimerReset size={18} />}
          title="Cancellations"
          subtitle="Operator cancel delivery and explicit agent acknowledgement state."
          metrics={[
            { label: "Requested", value: String(dashboard?.current.cancellations.requested ?? 0) },
            { label: "Sent", value: String(dashboard?.current.cancellations.sent ?? 0) },
            { label: "Cancel acks", value: String(dashboard?.current.cancellations.acked ?? 0) },
            { label: "Awaiting ack", value: String(dashboard?.current.cancellations.awaiting_ack ?? 0) },
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

function SystemMetricSection({
  badge,
  icon,
  lines,
  metrics,
  subtitle,
  title,
  valueFormatter,
}: {
  badge: string;
  icon: ReactNode;
  lines: { lines: TimeSeriesChartLine[]; times: string[] };
  metrics: Array<{ label: string; value: string }>;
  subtitle: string;
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
        <ConsoleStatusBadge tone="info">{badge}</ConsoleStatusBadge>
      </div>
      <div className="dashboardNetworkPanel systemMetricPanel">
        <div className="dashboardCurveCard">
          <div className="dashboardChartHeader">
            <span className="systemSectionTitle">{icon}{title} curves</span>
          </div>
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
  onUpdate,
  onValidate,
  privilegeMaterial,
}: {
  config: SuiteConfigResponse | null;
  error: string | null;
  loading: boolean;
  onLoad: () => void;
  onOpenPrivilegeUnlock: () => void;
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
  const parsedDraft = useMemo(() => parseTomlDraft(draftToml), [draftToml]);
  const dirty = Boolean(config && draftToml !== config.toml);
  const changedKeys = validation?.changed_keys ?? [];
  const activeValidation = validation?.validation ?? config?.validation ?? null;
  const hotReloadFields = activeValidation?.hot_reload_fields ?? [];
  const restartRequiredFields = activeValidation?.restart_required_fields ?? [];
  const validationState = validation
    ? validation.validation.valid
      ? "validated"
      : "invalid"
    : config?.validation.valid
      ? "loaded"
      : "invalid";
  const reviewDisabled = pending || !dirty || !validation || !privilegeMaterial || !validation.validation.valid;

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

  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <section className="fleetPanel systemConfigOverview">
          <div className="sectionHeader">
            <div>
              <h2>System Config</h2>
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
              <button className="primaryAction compactAction" disabled={reviewDisabled} onClick={() => setConfirmOpen(true)} type="button">
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
              <SystemConfigStatusItem icon={<RefreshCw size={17} />} label="Hot reload" value={`${hotReloadFields.length} fields`} tone="info" />
              <SystemConfigStatusItem icon={<AlertTriangle size={17} />} label="Restart required" value={`${restartRequiredFields.length} fields`} tone={restartRequiredFields.length ? "warning" : "ok"} />
              <SystemConfigStatusItem icon={<LockKeyhole size={17} />} label="Privilege" value={privilegeMaterial ? "unlocked" : "locked"} tone={privilegeMaterial ? "ok" : "warning"} />
            </div>
          )}
        </section>

        <div className="systemConfigBody">
          <section className="dashboardSection systemConfigEditor">
            <div className="dashboardSectionHeader">
              <div>
                <h2>Suite editor</h2>
                <span>{editorMode === "form" ? "Structured controls for common runtime settings." : "Full TOML editor for advanced settings."}</span>
              </div>
              <div className="editorModeGroup">
                <ConsoleStatusBadge tone={parsedDraft.ok ? "ok" : "warning"}>
                  {parsedDraft.ok ? "TOML parsed" : "TOML invalid"}
                </ConsoleStatusBadge>
                <div className="segmented" role="group" aria-label="Suite config editor mode">
                  <button aria-pressed={editorMode === "form"} className={editorMode === "form" ? "selected" : ""} onClick={() => setEditorMode("form")} type="button">
                    Form
                  </button>
                  <button aria-pressed={editorMode === "toml"} className={editorMode === "toml" ? "selected" : ""} onClick={() => setEditorMode("toml")} type="button">
                    TOML
                  </button>
                </div>
              </div>
            </div>
            {!parsedDraft.ok && (
              <div className="panelWarning systemConfigNotice">
                Structured controls are paused until the TOML parses. Use the raw TOML editor to repair the document.
              </div>
            )}
            {editorMode === "form" ? (
              <div className="systemConfigGrid compactForm">
                <ConfigGroup title="API" description="Private operator API bind and gateway control settings.">
                  <ConfigText path="api.bind" label="Private API bind" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="api.gateway_control_url" label="Gateway control URL" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="api.job_output_artifact_min_bytes" label="Output artifact threshold" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="api.artifact_max_bytes" label="Artifact max bytes" parsed={parsedDraft} onChange={updateField} />
                  <ConfigCheckbox path="api.require_registered_agent_updates" label="Require registered agent updates" parsed={parsedDraft} onChange={updateField} />
                </ConfigGroup>
                <ConfigGroup title="Gateway" description="Agent listener, control listener, and API forwarding identity.">
                  <ConfigText path="gateway.bind" label="Agent bind" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="gateway.control_bind" label="Control bind" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="gateway.api_url" label="API URL" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="gateway.gateway_id" label="Gateway ID" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="gateway.reconnect_grace_secs" label="Reconnect grace seconds" parsed={parsedDraft} onChange={updateField} />
                </ConfigGroup>
                <ConfigGroup title="Worker" description="Schedule cadence, leases, and offline reconciliation.">
                  <ConfigNumber path="worker.tick_secs" label="Tick seconds" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="worker.worker_lease_secs" label="Worker lease seconds" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="worker.agent_offline_timeout_secs" label="Offline timeout seconds" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="worker.schedule_job_max_timeout_secs" label="Schedule job max timeout" parsed={parsedDraft} onChange={updateField} />
                </ConfigGroup>
                <ConfigGroup title="Capacity" description="Fleet defaults sized for 20-50 VPS operation.">
                  <ConfigNumber path="capacity.api_db_pool" label="API DB pool" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="capacity.worker_db_pool" label="Worker DB pool" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="capacity.dispatcher_batch" label="Dispatcher batch" parsed={parsedDraft} onChange={updateField} />
                  <ConfigNumber path="capacity.dispatcher_in_flight" label="Dispatcher in-flight" parsed={parsedDraft} onChange={updateField} />
                </ConfigGroup>
                <ConfigGroup title="Storage" description="Object-store locations and optional S3 buckets.">
                  <ConfigText path="storage.backup_object_store_dir" label="Backup object store dir" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="storage.object_endpoint" label="Object endpoint" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="storage.object_bucket" label="Object bucket" parsed={parsedDraft} onChange={updateField} />
                </ConfigGroup>
                <ConfigGroup title="Secrets" description="Mounted secret-file references only; contents stay hidden.">
                  <ConfigText path="secrets.internal_token_file" label="Internal token file" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="secrets.gateway_private_key_file" label="Gateway key file" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="secrets.privilege_verifier_key_file" label="Privilege verifier file" parsed={parsedDraft} onChange={updateField} />
                  <ConfigText path="secrets.object_secret_key_file" label="Object secret key file" parsed={parsedDraft} onChange={updateField} />
                </ConfigGroup>
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

          <section className="dashboardSection systemConfigReview" aria-label="Suite config validation and save review">
            <div className="dashboardSectionHeader">
              <div>
                <h2>Review and save</h2>
                <span>Validate, review impact, use global privilege unlock, then confirm save.</span>
              </div>
              <ConsoleStatusBadge tone={validation?.validation.valid ? "ok" : dirty ? "warning" : "neutral"}>
                {validation ? `${changedKeys.length} changed` : dirty ? "Draft" : "No draft"}
              </ConsoleStatusBadge>
            </div>

            <div className="systemReviewStack">
              <div className="systemReviewBlock">
                <h3>Changed keys</h3>
                <div className="chipList compactChipList">
                  {changedKeys.map((key) => <span key={key}>{key}</span>)}
                  {validation && changedKeys.length === 0 ? <span>No changes</span> : null}
                  {!validation ? <span>Validate draft first</span> : null}
                </div>
              </div>

              <div className="systemImpactGrid">
                <ImpactList title="Hot reload" fields={hotReloadFields} emptyLabel="No hot-reload fields reported" />
                <ImpactList title="Restart required" fields={restartRequiredFields} emptyLabel="No restart-only fields reported" />
              </div>

              <div className="systemReviewBlock">
                <h3>Privilege</h3>
                <div className={`privilegeGateBox ${privilegeMaterial ? "ready" : ""}`}>
                  <LockKeyhole size={18} />
                  <span>{privilegeMaterial ? "Privilege unlocked for this browser session" : "Unlock privilege from Access before saving suite config"}</span>
                  {!privilegeMaterial && (
                    <button className="secondaryAction compactAction" onClick={onOpenPrivilegeUnlock} type="button">
                      Unlock in Access
                    </button>
                  )}
                </div>
              </div>

              <div className="systemReviewBlock">
                <h3>Save</h3>
                <button className="primaryAction wideAction" disabled={reviewDisabled} onClick={() => setConfirmOpen(true)} type="button">
                  <Save size={16} />
                  <span>{pending ? "Saving" : "Review save"}</span>
                </button>
              </div>

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
            </div>
            <ConfirmationPrompt
              confirmLabel="Save suite config"
              detail="This writes the suite TOML, may hot-reload runtime settings, and may require service restarts for restart-only keys."
              error={configError}
              items={[
                { label: "Changed keys", value: String(changedKeys.length) },
                { label: "Hot reload fields", value: String(hotReloadFields.length) },
                { label: "Restart required fields", value: String(restartRequiredFields.length) },
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

function ConfigGroup({ children, description, title }: { children: ReactNode; description: string; title: string }) {
  return (
    <div className="systemConfigGroup">
      <h3>{title}</h3>
      <p>{description}</p>
      {children}
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

function ConfigText({
  label,
  onChange,
  parsed,
  path,
}: {
  label: string;
  onChange: (path: string, value: unknown) => void;
  parsed: ParsedTomlDraft;
  path: string;
}) {
  return (
    <label>
      <span>{label}</span>
      <input
        disabled={!parsed.ok}
        onChange={(event) => onChange(path, event.target.value.trim() ? event.target.value : undefined)}
        value={parsed.ok ? String(getTomlPath(parsed.table, path.split(".")) ?? "") : ""}
      />
    </label>
  );
}

function ConfigNumber({
  label,
  onChange,
  parsed,
  path,
}: {
  label: string;
  onChange: (path: string, value: unknown) => void;
  parsed: ParsedTomlDraft;
  path: string;
}) {
  const value = parsed.ok ? getTomlPath(parsed.table, path.split(".")) : "";
  return (
    <label>
      <span>{label}</span>
      <input
        disabled={!parsed.ok}
        min={0}
        onChange={(event) => {
          const next = event.target.value.trim();
          onChange(path, next ? Number(next) : undefined);
        }}
        type="number"
        value={typeof value === "number" ? String(value) : ""}
      />
    </label>
  );
}

function ConfigCheckbox({
  label,
  onChange,
  parsed,
  path,
}: {
  label: string;
  onChange: (path: string, value: unknown) => void;
  parsed: ParsedTomlDraft;
  path: string;
}) {
  const value = parsed.ok ? getTomlPath(parsed.table, path.split(".")) : false;
  return (
    <label className="checkLine inlineCheck">
      <input
        checked={value === true}
        disabled={!parsed.ok}
        onChange={(event) => onChange(path, event.target.checked)}
        type="checkbox"
      />
      <span>{label}</span>
    </label>
  );
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
      color: chartColors[index % chartColors.length],
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
