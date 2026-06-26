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
import { formatCompactTime, formatVpsName } from "../utils";

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
        header: "Alert",
        size: 360,
        minSize: 240,
        sortValue: (alert) => alert.title,
        searchValue: (alert) => `${alert.title} ${alert.detail}`,
        cell: (alert) => (
          <span className="historyPrimary">
            <strong>{alert.title}</strong>
            <small>{alert.detail}</small>
          </span>
        ),
      },
      {
        id: "target",
        header: "Target",
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
              <small>{alert.target_kind}</small>
            </span>
          );
        },
      },
      {
        id: "category",
        header: "Category",
        size: 140,
        minSize: 110,
        sortValue: (alert) => alert.category,
        searchValue: (alert) => alert.category,
        cell: (alert) => <span className="monoValue">{alert.category}</span>,
      },
      {
        id: "state",
        header: "Operator state",
        size: 170,
        minSize: 150,
        sortValue: alertOperatorState,
        searchValue: (alert) =>
          `${alertOperatorState(alert)} ${alert.state_reason ?? ""}`,
        cell: (alert) => {
          const operatorState = alertOperatorState(alert);
          return (
            <span className="historyPrimary">
              <ConsoleStatusBadge
                tone={operatorState === "open" ? "warning" : "info"}
              >
                {operatorState}
              </ConsoleStatusBadge>
              {alert.state_reason && <small>{alert.state_reason}</small>}
            </span>
          );
        },
      },
      {
        id: "observed",
        header: "Observed",
        size: 140,
        minSize: 110,
        sortValue: (alert) => alert.observed_at,
        cell: (alert) => formatCompactTime(alert.observed_at),
      },
    ],
    [nameById],
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
        getRowId={(alert) => alert.id}
        itemLabel="alerts"
        renderExpandedRow={(alert) => (
          <div className="consoleGridDetails">
            <span>
              <strong>Status:</strong> {alert.status}
            </span>
            <span>
              <strong>Target:</strong> {alert.target_kind}:{alert.target_id}
            </span>
            {alert.muted_until_unix && (
              <span>
                <strong>Muted until:</strong> {formatUnixTime(alert.muted_until_unix)}
              </span>
            )}
            <span>
              <strong>Escalation:</strong> {alert.escalation_level ?? 0}
            </span>
            <div className="configOverrideActions">
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
                type="button"
              >
                <Bell size={14} />
                <span>Open alert policies</span>
              </button>
            </div>
            <pre>{JSON.stringify(alert.evidence, null, 2)}</pre>
          </div>
        )}
        rowActions={[
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
            label: "Alert policies",
            description: (rows) =>
              actionTargetDescription(
                "Open",
                "alert policy context for",
                rows[0]?.title,
              ),
            icon: <Bell size={14} />,
            onSelect: () => onOpenAlertPolicies(),
          },
          {
            label: "Ack",
            description: (rows) =>
              actionTargetDescription(
                "Acknowledge",
                "fleet alert",
                rows[0]?.title,
                "Marks the open alert as acknowledged.",
              ),
            disabled: (rows) =>
              pending != null || !rows[0] || alertOperatorState(rows[0]) !== "open",
            icon: <Check size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "acknowledge"),
            separatorBefore: true,
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
            icon: <VolumeX size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "mute"),
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
            icon: <ArrowUpCircle size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "escalate"),
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
            icon: <CircleCheck size={14} />,
            onSelect: (rows) => reviewAlertUpdate(rows, "clear"),
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
        searchPlaceholder="Search alerts by VPS, category, state, or detail"
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
