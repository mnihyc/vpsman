import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  AlertTriangle,
  ArrowUpCircle,
  Bell,
  Check,
  CircleCheck,
  Server,
  VolumeX,
} from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { usePanelDisplaySettings } from "../panelDisplay";
import type {
  AgentView,
  FleetAlertRecord,
  FleetAlertStateRecord,
  FleetAlertStateRequest,
} from "../types";
import { formatCompactTime, formatFullTime, formatVpsName } from "../utils";

type FleetAlertsPanelProps = {
  agents: AgentView[];
  apiError: string | null;
  alerts: FleetAlertRecord[];
  stateCount: number;
  onOpenAlertPolicies: () => void;
  onOpenVpsDetail: (agent: AgentView) => void;
  onUpdate: (request: FleetAlertStateRequest) => Promise<FleetAlertStateRecord>;
};

export function FleetAlertsPanel({
  agents,
  apiError,
  alerts,
  stateCount,
  onOpenAlertPolicies,
  onOpenVpsDetail,
  onUpdate,
}: FleetAlertsPanelProps) {
  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Fleet alerts</h2>
            <span>{`${alerts.length} active fleet alerts`}</span>
          </div>
          <div className="sectionActions">
            <span className="sectionContext">{stateCount} triaged states</span>
            <button
              className="secondaryAction compactAction"
              onClick={onOpenAlertPolicies}
              title="Open alert policy configuration for fleet alert triage."
              type="button"
            >
              <Bell size={14} />
              <span>Open alert policies</span>
            </button>
          </div>
        </div>
        <ConsoleFreshnessBanner error={apiError} />
        <FleetAlertList
          agents={agents}
          alerts={alerts}
          onOpenAlertPolicies={onOpenAlertPolicies}
          onOpenVpsDetail={onOpenVpsDetail}
          onUpdate={onUpdate}
          stateCount={stateCount}
        />
      </div>
    </section>
  );
}

function ConsoleFreshnessBanner({ error }: { error: string | null }) {
  if (!error) {
    return null;
  }
  return (
    <div className="consoleFreshnessBanner">
      <span>Using cached data. Last refresh failed: {error}</span>
    </div>
  );
}

function FleetAlertList({
  agents,
  alerts,
  stateCount,
  onOpenAlertPolicies,
  onOpenVpsDetail,
  onUpdate,
}: {
  agents: AgentView[];
  alerts: FleetAlertRecord[];
  stateCount: number;
  onOpenAlertPolicies: () => void;
  onOpenVpsDetail: (agent: AgentView) => void;
  onUpdate: (request: FleetAlertStateRequest) => Promise<FleetAlertStateRecord>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [pending, setPending] = useState<string | null>(null);
  const [reviewSnapshot, setReviewSnapshot] = useState<{
    action: FleetAlertStateRequest["action"];
    requests: FleetAlertStateRequest[];
    rows: FleetAlertRecord[];
  } | null>(null);
  const agentById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent])),
    [agents],
  );
  const nameById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, formatVpsName(agent, vpsNameDisplayMode)])),
    [agents, vpsNameDisplayMode],
  );
  const criticalCount = alerts.filter((alert) => alert.severity === "critical").length;
  const warningCount = alerts.filter((alert) => alert.severity === "warning").length;

  const alertColumns = useMemo<ConsoleDataGridColumn<FleetAlertRecord>[]>(
    () => [
      {
        id: "severity",
        header: "Severity",
        size: 115,
        minSize: 95,
        sortValue: (alert) => alert.severity,
        searchValue: (alert) => alert.severity,
        cell: (alert) => (
          <ConsoleStatusBadge tone={alertTone(alert.severity)}>
            {alert.severity}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "alert",
        header: "Summary",
        size: 390,
        minSize: 240,
        sortValue: (alert) => alert.title,
        searchValue: (alert) => `${alert.title} ${alert.detail} ${alert.category}`,
        cell: (alert) => (
          <span className="historyPrimary fleetAlertSummary">
            <strong>{alert.title}</strong>
            <small>{alert.detail}</small>
            <small>{alertCategoryLabel(alert)} · {alertStatusLabel(alert.status)}</small>
          </span>
        ),
      },
      {
        id: "target",
        header: "VPS",
        size: 210,
        minSize: 150,
        sortValue: (alert) =>
          alert.client_id
            ? (nameById.get(alert.client_id) ?? alert.client_id)
            : alertTargetLabel(alert),
        searchValue: (alert) =>
          `${alert.target_kind} ${alert.target_id} ${alert.client_id ?? ""} ${
            alert.client_id ? (nameById.get(alert.client_id) ?? "") : ""
          }`,
        cell: (alert) => {
          const label = alert.client_id
            ? (nameById.get(alert.client_id) ?? "Unnamed VPS")
            : alertTargetLabel(alert);
          return (
            <span
              className="historyPrimary"
              title={`${alert.target_kind}:${alert.target_id}`}
            >
              <strong>{label}</strong>
              <small>{alertTargetScopeLabel(alert)}</small>
            </span>
          );
        },
      },
      {
        id: "state",
        header: "State",
        size: 190,
        minSize: 160,
        sortValue: alertOperatorState,
        searchValue: (alert) =>
          `${alertOperatorState(alert)} ${alert.status} ${alert.state_reason ?? ""}`,
        cell: (alert) => {
          const operatorState = alertOperatorState(alert);
          return (
            <span className="fleetAlertStateStack">
              <ConsoleStatusBadge
                tone={operatorState === "open" ? "warning" : "info"}
              >
                {operatorStateLabel(operatorState)}
              </ConsoleStatusBadge>
              <small>{alertStatusLabel(alert.status)}</small>
              {alert.state_reason && <small>{alert.state_reason}</small>}
            </span>
          );
        },
      },
      {
        id: "observed",
        header: "Age",
        size: 140,
        minSize: 110,
        sortValue: (alert) => alert.observed_at,
        cell: (alert) => (
          <time dateTime={alert.observed_at} title={formatFullTime(alert.observed_at)}>
            {formatCompactTime(alert.observed_at)}
          </time>
        ),
      },
      {
        id: "action",
        header: "Action",
        size: 180,
        minSize: 160,
        enableHiding: false,
        cell: (alert) => {
          const operatorState = alertOperatorState(alert);
          const agent = alert.client_id ? agentById.get(alert.client_id) : null;
          return (
            <span className="fleetAlertInlineActions">
              {operatorState === "open" ? (
                <button
                  className="secondaryAction compactAction"
                  disabled={pending != null}
                  onClick={(event) => {
                    event.stopPropagation();
                    reviewAlertUpdate([alert], "acknowledge");
                  }}
                  type="button"
                >
                  <Check size={13} />
                  <span>Acknowledge</span>
                </button>
              ) : (
                <button
                  className="secondaryAction compactAction"
                  disabled={pending != null}
                  onClick={(event) => {
                    event.stopPropagation();
                    reviewAlertUpdate([alert], "clear");
                  }}
                  type="button"
                >
                  <CircleCheck size={13} />
                  <span>Clear</span>
                </button>
              )}
              <button
                className="secondaryAction compactAction"
                disabled={!agent}
                onClick={(event) => {
                  event.stopPropagation();
                  if (agent) {
                    onOpenVpsDetail(agent);
                  }
                }}
                type="button"
              >
                <Server size={13} />
                <span>Open</span>
              </button>
            </span>
          );
        },
      },
    ],
    [agentById, nameById, onOpenVpsDetail, pending],
  );

  useEffect(() => {
    setReviewSnapshot(null);
  }, [alerts]);

  function reviewAlertUpdate(
    rows: FleetAlertRecord[],
    action: FleetAlertStateRequest["action"],
  ) {
    if (rows.length === 0 || pending) {
      return;
    }
    setReviewSnapshot({
      action,
      rows,
      requests: rows.map((alert) => ({
        alert_id: alert.id,
        action,
        muted_for_secs: action === "mute" ? 4 * 60 * 60 : null,
        reason:
          action === "mute"
            ? "panel mute"
            : action === "acknowledge"
              ? "panel acknowledgement"
              : action === "escalate"
                ? "panel escalation"
                : "panel clear",
        confirmed: true,
      })),
    });
  }

  async function updateReviewedAlerts() {
    const snapshot = reviewSnapshot;
    if (!snapshot || pending) {
      return;
    }
    setPending(`${snapshot.action}:${snapshot.rows.map((alert) => alert.id).join(",")}`);
    try {
      for (const request of snapshot.requests) {
        await onUpdate(request);
      }
      setReviewSnapshot(null);
    } finally {
      setPending(null);
    }
  }

  const openRows = (rows: FleetAlertRecord[]) =>
    rows.filter((alert) => alertOperatorState(alert) === "open");
  const triagedRows = (rows: FleetAlertRecord[]) =>
    rows.filter((alert) => alertOperatorState(alert) !== "open");

  return (
    <div className="fleetAlertList" aria-label="Fleet alerts">
      <div className="fleetAlertHeader">
        <span>
          <AlertTriangle size={17} />
          <strong>Fleet alerts</strong>
        </span>
        <small>
          {alerts.length === 0
            ? "clear"
            : `${criticalCount} critical / ${warningCount} warning / ${stateCount} triaged`}
        </small>
      </div>
      <ConsoleDataGrid
        actions={[
          {
            label: "Acknowledge open",
            description: (rows) =>
              `Acknowledge ${openRows(rows).length} selected open fleet alerts.`,
            disabled: (rows) => pending != null || openRows(rows).length === 0,
            icon: <Check size={14} />,
            onSelect: (rows) => reviewAlertUpdate(openRows(rows), "acknowledge"),
          },
          {
            label: "Mute open 4h",
            description: (rows) =>
              `Mute ${openRows(rows).length} selected open fleet alerts for four hours.`,
            disabled: (rows) => pending != null || openRows(rows).length === 0,
            icon: <VolumeX size={14} />,
            onSelect: (rows) => reviewAlertUpdate(openRows(rows), "mute"),
          },
          {
            label: "Escalate open",
            description: (rows) =>
              `Escalate ${openRows(rows).length} selected open fleet alerts.`,
            disabled: (rows) => pending != null || openRows(rows).length === 0,
            icon: <ArrowUpCircle size={14} />,
            onSelect: (rows) => reviewAlertUpdate(openRows(rows), "escalate"),
          },
          {
            label: "Clear triaged",
            description: (rows) =>
              `Clear ${triagedRows(rows).length} selected triaged fleet alerts.`,
            disabled: (rows) => pending != null || triagedRows(rows).length === 0,
            icon: <CircleCheck size={14} />,
            onSelect: (rows) => reviewAlertUpdate(triagedRows(rows), "clear"),
          },
        ]}
        columns={alertColumns}
        defaultPageSize={10}
        empty="No active fleet alerts."
        expandOnRowClick
        getRowId={(alert) => alert.id}
        itemLabel="alerts"
        renderExpandedRow={(alert) => (
          <div className="consoleGridDetails fleetAlertDetail">
            <div className="consoleInlineDetailGrid">
              <span>Operator state</span>
              <strong>{operatorStateLabel(alertOperatorState(alert))}</strong>
              <span>Alert status</span>
              <strong>{alertStatusLabel(alert.status)}</strong>
              <span>Category</span>
              <strong>{alertCategoryLabel(alert)}</strong>
              <span>Target</span>
              <strong>{alert.target_kind}:{alert.target_id}</strong>
              <span>Observed</span>
              <strong>{formatFullTime(alert.observed_at)}</strong>
              {alert.muted_until_unix && (
                <>
                  <span>Muted until</span>
                  <strong>{formatUnixTime(alert.muted_until_unix)}</strong>
                </>
              )}
              <span>Escalation</span>
              <strong>{alert.escalation_level ?? 0}</strong>
            </div>
            <div className="configOverrideActions">
              {alertOperatorState(alert) === "open" ? (
                <>
                  <button
                    className="secondaryAction compactAction"
                    disabled={pending != null}
                    onClick={() => reviewAlertUpdate([alert], "acknowledge")}
                    type="button"
                  >
                    <Check size={14} />
                    <span>Acknowledge</span>
                  </button>
                  <button
                    className="secondaryAction compactAction"
                    disabled={pending != null}
                    onClick={() => reviewAlertUpdate([alert], "mute")}
                    type="button"
                  >
                    <VolumeX size={14} />
                    <span>Silence 4h</span>
                  </button>
                </>
              ) : (
                <button
                  className="secondaryAction compactAction"
                  disabled={pending != null}
                  onClick={() => reviewAlertUpdate([alert], "clear")}
                  type="button"
                >
                  <CircleCheck size={14} />
                  <span>Clear triage</span>
                </button>
              )}
              <button
                className="secondaryAction compactAction"
                disabled={!alert.client_id || !agentById.has(alert.client_id)}
                onClick={() => {
                  const agent = alert.client_id ? agentById.get(alert.client_id) : null;
                  if (agent) {
                    onOpenVpsDetail(agent);
                  }
                }}
                type="button"
              >
                <Server size={14} />
                <span>Open VPS detail</span>
              </button>
              <button
                className="secondaryAction compactAction"
                onClick={onOpenAlertPolicies}
                title="Open alert policy configuration for this fleet alert."
                type="button"
              >
                <Bell size={14} />
                <span>Open alert policies</span>
              </button>
            </div>
            {policyNameFromAlert(alert) && (
              <span className="fleetAlertPolicyHint">
                Policy: <strong>{policyNameFromAlert(alert)}</strong>
              </span>
            )}
            <pre>{JSON.stringify(alert.evidence, null, 2)}</pre>
          </div>
        )}
        rowActions={[
          {
            label: "Acknowledge",
            description: (rows) =>
              actionTargetDescription(
                "Acknowledge",
                "fleet alert",
                rows[0]?.title,
                "Marks the open alert as acknowledged.",
              ),
            disabled: (rows) =>
              pending != null || !rows[0] || alertOperatorState(rows[0]) !== "open",
            hidden: (rows) => !rows[0] || alertOperatorState(rows[0]) !== "open",
            icon: <Check size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "acknowledge"),
          },
          {
            label: "Open VPS",
            description: (rows) =>
              actionTargetDescription(
                "Open",
                "VPS detail for alert",
                rows[0]?.title,
              ),
            disabled: (rows) => !rows[0]?.client_id || !agentById.has(rows[0].client_id),
            icon: <Server size={14} />,
            onSelect: (rows) => {
              const clientId = rows[0]?.client_id;
              const agent = clientId ? agentById.get(clientId) : null;
              if (agent) {
                onOpenVpsDetail(agent);
              }
            },
          },
          {
            label: "Clear",
            description: (rows) =>
              actionTargetDescription(
                "Clear",
                "fleet alert",
                rows[0]?.title,
                "Clears a triaged alert.",
              ),
            disabled: (rows) =>
              pending != null || !rows[0] || alertOperatorState(rows[0]) === "open",
            hidden: (rows) => !rows[0] || alertOperatorState(rows[0]) === "open",
            icon: <CircleCheck size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "clear"),
          },
          {
            label: "Mute",
            description: (rows) =>
              actionTargetDescription(
                "Mute",
                "fleet alert",
                rows[0]?.title,
                "Suppresses the open alert for four hours.",
              ),
            disabled: (rows) =>
              pending != null || !rows[0] || alertOperatorState(rows[0]) !== "open",
            hidden: (rows) => !rows[0] || alertOperatorState(rows[0]) !== "open",
            icon: <VolumeX size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "mute"),
            separatorBefore: true,
          },
          {
            label: "Escalate",
            description: (rows) =>
              actionTargetDescription(
                "Escalate",
                "fleet alert",
                rows[0]?.title,
                "Raises the open alert escalation level.",
              ),
            disabled: (rows) =>
              pending != null || !rows[0] || alertOperatorState(rows[0]) !== "open",
            hidden: (rows) => !rows[0] || alertOperatorState(rows[0]) !== "open",
            icon: <ArrowUpCircle size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "escalate"),
          },
          {
            label: "Policies",
            description: (rows) =>
              actionTargetDescription(
                "Open",
                "alert policy context for",
                rows[0]?.title,
              ),
            icon: <Bell size={14} />,
            onSelect: () => onOpenAlertPolicies(),
          },
        ]}
        renderSelectionPanel={(rows) => {
          const selectedOpen = openRows(rows).length;
          const selectedTriaged = triagedRows(rows).length;
          return (
            <span>
              {rows.length} selected · {selectedOpen} open · {selectedTriaged} triaged
            </span>
          );
        }}
        rows={alerts}
        searchPlaceholder="Search alerts"
        storageKey="vpsman.grid.fleet.alerts.v1"
        title="Fleet alerts"
      />
      <ConfirmationPrompt
        confirmLabel={fleetAlertActionLabel(reviewSnapshot?.action)}
        detail="Applies the reviewed operator state update to the selected fleet alerts."
        items={[
          {
            label: "Action",
            value: fleetAlertActionLabel(reviewSnapshot?.action),
          },
          {
            label: "Alerts",
            value: selectedRecordSummary(
              reviewSnapshot?.rows ?? null,
              "alert",
              "alerts",
              (row) => row.title,
              (row) => row.id,
            ),
          },
        ]}
        onCancel={() => setReviewSnapshot(null)}
        onConfirm={() => void updateReviewedAlerts()}
        open={reviewSnapshot !== null}
        pending={pending !== null}
        title="Confirm fleet alert triage"
        tone={reviewSnapshot?.action === "clear" ? "normal" : "danger"}
      />
    </div>
  );
}

function formatUnixTime(value: number): string {
  return formatCompactTime(new Date(value * 1000).toISOString());
}

function fleetAlertActionLabel(action: FleetAlertStateRequest["action"] | undefined): string {
  switch (action) {
    case "acknowledge":
      return "Acknowledge";
    case "mute":
      return "Mute";
    case "escalate":
      return "Escalate";
    case "clear":
      return "Clear";
    default:
      return "Confirm";
  }
}

function alertTone(severity: string): "critical" | "warning" | "info" {
  if (severity === "critical") {
    return "critical";
  }
  if (severity === "warning") {
    return "warning";
  }
  return "info";
}

function alertTargetLabel(alert: FleetAlertRecord) {
  return alert.target_kind === "client" ? "Unknown VPS" : alert.target_id;
}

function alertOperatorState(alert: FleetAlertRecord): string {
  return alert.operator_state?.trim() || "open";
}

function operatorStateLabel(state: string): string {
  switch (state) {
    case "open":
      return "Open";
    case "acknowledged":
      return "Acknowledged";
    case "muted":
      return "Muted";
    case "escalated":
      return "Escalated";
    case "cleared":
      return "Cleared";
    default:
      return readableAlertToken(state);
  }
}

function alertStatusLabel(status: string): string {
  switch (status) {
    case "tunnel_adapter_degraded":
      return "Tunnel adapter degraded";
    case "stale":
      return "Agent stale";
    case "selected_no_store":
      return "Source not configured";
    case "policy_reached":
      return "Policy threshold reached";
    default:
      return readableAlertToken(status);
  }
}

function alertCategoryLabel(alert: FleetAlertRecord): string {
  switch (alert.category) {
    case "network":
      return "Network";
    case "agent_status":
      return "Agent status";
    case "source_readiness":
      return "Source readiness";
    case "traffic":
      return "Traffic policy";
    default:
      return readableAlertToken(alert.category);
  }
}

function alertTargetScopeLabel(alert: FleetAlertRecord): string {
  switch (alert.target_kind) {
    case "agent":
    case "client":
      return "VPS";
    case "tunnel":
      return "Tunnel";
    case "source_template":
      return "Source template";
    case "policy_alert":
      return "Policy alert";
    default:
      return readableAlertToken(alert.target_kind);
  }
}

function policyNameFromAlert(alert: FleetAlertRecord): string | null {
  const evidence = alert.evidence;
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) {
    return null;
  }
  const policy = (evidence as { policy?: unknown }).policy;
  if (!policy || typeof policy !== "object" || Array.isArray(policy)) {
    return null;
  }
  const name = (policy as { name?: unknown }).name;
  return typeof name === "string" && name.trim() ? name : null;
}

function readableAlertToken(value: string): string {
  const label = value
    .split(/[_:\-.]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
  return label || "Unknown";
}

function actionTargetDescription(
  action: string,
  kind: string,
  name: string | undefined,
  detail?: string,
): string {
  const target = name ? `${kind} ${name}` : kind;
  return detail ? `${action} ${target}. ${detail}` : `${action} ${target}.`;
}

function selectedRecordSummary<T>(
  rows: T[] | null,
  singularLabel: string,
  pluralLabel: string,
  getName: (row: T) => string,
  getId: (row: T) => string,
): ReactNode {
  const selectedRows = rows ?? [];
  if (selectedRows.length === 0) {
    return `0 ${pluralLabel}`;
  }
  const names = selectedRows.map(getName).join(", ");
  const ids = selectedRows.map(getId).join(", ");
  return (
    <span title={ids}>
      {selectedRows.length} {selectedRows.length === 1 ? singularLabel : pluralLabel}: {names}
    </span>
  );
}
