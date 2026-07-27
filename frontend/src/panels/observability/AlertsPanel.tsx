import { AlertTriangle, Bell, ExternalLink, RadioTower } from "lucide-react";
import { useState } from "react";
import { ActionFeedback } from "../../components/ActionFeedback";
import { FLEET_DETAIL_LIMIT, formatBoundedCount } from "../../constants";
import {
  handleTabListKeyDown,
  tabId,
} from "../../components/AccessibleTabs";
import {
  DeliveryPreviewSection,
  FleetAlertNotificationManager,
  FleetAlertPolicyManager,
  NotificationDeliveryHistoryGrid,
} from "../FleetWorkspace";
import type {
  AgentView,
  FleetAlertNotificationChannelRecord,
  FleetAlertNotificationChannelRequest,
  FleetAlertNotificationDeliveryRecord,
  FleetAlertNotificationDispatchRequest,
  FleetAlertNotificationProcessRequest,
  FleetAlertPolicyRecord,
  FleetAlertPolicyRequest,
  FleetAlertRecord,
  PolicyAlertRecord,
  PolicyDryRunRequest,
  PolicyDryRunResponse,
} from "../../types";

type AlertConfigTab = "policies" | "destinations" | "deliveries";

type AlertsPanelProps = {
  agents: AgentView[];
  apiError: string | null;
  fleetAlertNotificationChannels: FleetAlertNotificationChannelRecord[];
  fleetAlertNotifications: FleetAlertNotificationDeliveryRecord[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  fleetAlerts: FleetAlertRecord[];
  onDeleteFleetAlertNotificationChannel: (channelId: string, reviewedName: string) => Promise<void>;
  onDeleteFleetAlertPolicy: (policyId: string, reviewedName: string) => Promise<void>;
  onDispatchFleetAlertNotifications: (
    request: FleetAlertNotificationDispatchRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onDryRunFleetAlertPolicy: (request: PolicyDryRunRequest) => Promise<PolicyDryRunResponse>;
  onOpenFleetAlerts: () => void;
  onProcessFleetAlertNotifications: (
    request: FleetAlertNotificationProcessRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onUpsertFleetAlertNotificationChannel: (
    request: FleetAlertNotificationChannelRequest,
  ) => Promise<FleetAlertNotificationChannelRecord>;
  onUpsertFleetAlertPolicy: (request: FleetAlertPolicyRequest) => Promise<FleetAlertPolicyRecord>;
  policyFocusId: string | null;
  policyAlerts: PolicyAlertRecord[];
};

export function AlertsPanel({
  agents,
  apiError,
  fleetAlertNotificationChannels,
  fleetAlertNotifications,
  fleetAlertPolicies,
  fleetAlerts,
  onDeleteFleetAlertNotificationChannel,
  onDeleteFleetAlertPolicy,
  onDispatchFleetAlertNotifications,
  onDryRunFleetAlertPolicy,
  onOpenFleetAlerts,
  onProcessFleetAlertNotifications,
  onUpsertFleetAlertNotificationChannel,
  onUpsertFleetAlertPolicy,
  policyFocusId,
  policyAlerts,
}: AlertsPanelProps) {
  const [activeTab, setActiveTab] = useState<AlertConfigTab>("policies");
  const [policyEditorOpen, setPolicyEditorOpen] = useState(false);
  const [previewRows, setPreviewRows] = useState<FleetAlertNotificationDeliveryRecord[] | null>(null);
  const failedDeliveries = fleetAlertNotifications.filter((delivery) =>
    ["failed", "permanently_failed"].includes(delivery.status),
  ).length;
  const urgentPolicyAlerts = policyAlerts.filter((alert) => ["critical", "warning"].includes(alert.severity)).length;
  const policyAlertsTruncated = policyAlerts.length >= FLEET_DETAIL_LIMIT;
  const deliveriesTruncated =
    fleetAlertNotifications.length >= FLEET_DETAIL_LIMIT;

  function openDeliveryEvidence() {
    setActiveTab("deliveries");
    window.requestAnimationFrame(() => {
      const target = document.getElementById("observability-alert-deliveries");
      target?.scrollIntoView({ block: "start", behavior: "smooth" });
    });
  }

  function previewDeliveries(rows: FleetAlertNotificationDeliveryRecord[]) {
    setPreviewRows(rows);
    openDeliveryEvidence();
  }

  return (
    <section className="workspace singleColumn observabilityAlertsWorkspace">
      <div className="fleetPanel observabilityAlertsPanel">
        {!policyEditorOpen ? (
          <div className="sectionHeader">
            <div>
              <h2>Alerts</h2>
              <span>Policy groups, issued policy alerts, and notification channels. Live triage stays in Fleet / Alerts.</span>
            </div>
            <div className="sectionActions" aria-label="Alert action links">
              <button className="secondaryAction compactAction" onClick={onOpenFleetAlerts} type="button">
                <ExternalLink size={14} />
                Open triage
              </button>
            </div>
          </div>
        ) : null}

        <ActionFeedback
          className="localActionFeedback dashboardActionFeedback alertsActionFeedback"
          message={apiError}
          tone="danger"
        />

        {!policyEditorOpen ? (
          <>
            <div className="metricGrid observabilityMetricsSummary" aria-label="Alert routing summary">
              <MetricTile
                actionLabel="Open triage"
                detail={fleetAlerts.length >= FLEET_DETAIL_LIMIT
                  ? `At least ${fleetAlerts.length} active alerts; open triage to review the loaded page`
                  : "Operational alert triage records live in Fleet / Alerts"}
                label="Active fleet alerts"
                onAction={onOpenFleetAlerts}
                value={formatBoundedCount(
                  fleetAlerts.length,
                  fleetAlerts.length >= FLEET_DETAIL_LIMIT,
                )}
              />
              <MetricTile actionLabel="Policies" detail={`${urgentPolicyAlerts} warning or critical policy-issued alerts${policyAlertsTruncated ? " in the loaded page" : ""}`} label="Policy alerts" onAction={() => setActiveTab("policies")} value={formatBoundedCount(policyAlerts.length, policyAlertsTruncated)} />
              <MetricTile actionLabel="Destinations" detail="Reviewed notification destinations, separate from event webhooks" label="Destinations" onAction={() => setActiveTab("destinations")} value={formatBoundedCount(fleetAlertNotificationChannels.length, fleetAlertNotificationChannels.length >= FLEET_DETAIL_LIMIT)} />
              <MetricTile actionLabel="Open failed deliveries" detail={`${failedDeliveries} failed retained notification deliveries${deliveriesTruncated ? " in the loaded page" : ""}`} label="Delivery history" onAction={openDeliveryEvidence} value={formatBoundedCount(fleetAlertNotifications.length, deliveriesTruncated)} />
            </div>

            <div
              className="observabilityWorkflowTabs"
              role="tablist"
              aria-label="Alert configuration sections"
              onKeyDown={handleTabListKeyDown}
            >
              {[
                ["policies", "Policies", "Threshold rules and matched policy alerts"],
                ["destinations", "Destinations", "Alert notification channels"],
                ["deliveries", "Deliveries", "Previewed, failed, and retained notifications"],
              ].map(([id, label, detail]) => (
                <button
                  aria-controls={`observability-alert-${id}`}
                  aria-selected={activeTab === id}
                  className={activeTab === id ? "active" : ""}
                  id={tabId("observability-alert", id)}
                  key={id}
                  onClick={() => setActiveTab(id as AlertConfigTab)}
                  role="tab"
                  tabIndex={activeTab === id ? 0 : -1}
                  type="button"
                >
                  <strong>{label}</strong>
                  <span>{detail}</span>
                </button>
              ))}
            </div>
          </>
        ) : null}

        {activeTab === "policies" ? (
          <section
            aria-labelledby={tabId("observability-alert", "policies")}
            className="dashboardSection observabilityGroupSection"
            id="observability-alert-policies"
            role="tabpanel"
          >
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="observability-alert-policies-title">Alert policies</h2>
                <span>Author thresholds, selectors, dry-run previews, and reviewed saves without mixing live triage into this workflow.</span>
              </div>
              <AlertTriangle size={18} />
            </div>
            <FleetAlertPolicyManager
              agents={agents}
              onDelete={onDeleteFleetAlertPolicy}
              onDryRun={onDryRunFleetAlertPolicy}
              onEditorOpenChange={setPolicyEditorOpen}
              onUpsert={onUpsertFleetAlertPolicy}
              editorMode="focused"
              policies={fleetAlertPolicies}
              policyAlerts={policyAlerts}
              policyFilterClientId={null}
              policyFocusId={policyFocusId}
            />
          </section>
        ) : null}

        {activeTab === "destinations" ? (
          <section
            aria-labelledby={tabId("observability-alert", "destinations")}
            className="dashboardSection observabilityGroupSection"
            id="observability-alert-destinations"
            role="tabpanel"
          >
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="observability-alert-channels-title">Notification channels</h2>
                <span>Route fleet alerts to reviewed webhook destinations. Event webhooks stay on the separate Event webhooks page.</span>
              </div>
              <Bell size={18} />
            </div>
            <FleetAlertNotificationManager
              agents={agents}
              channels={fleetAlertNotificationChannels}
              deliveries={fleetAlertNotifications}
              onDelete={onDeleteFleetAlertNotificationChannel}
              onDispatch={onDispatchFleetAlertNotifications}
              onOpenDeliveries={openDeliveryEvidence}
              onPreviewRows={previewDeliveries}
              onProcess={onProcessFleetAlertNotifications}
              onUpsert={onUpsertFleetAlertNotificationChannel}
              queueMode="configuration"
            />
          </section>
        ) : null}

        {activeTab === "deliveries" ? (
          <section
            aria-labelledby={tabId("observability-alert", "deliveries")}
            className="dashboardSection observabilityGroupSection"
            id="observability-alert-deliveries"
            role="tabpanel"
          >
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="observability-alert-deliveries-title">Notification deliveries</h2>
                <span>Previewed, failed, retried, and retained alert notification delivery rows. Failed evidence is searchable here.</span>
              </div>
              <RadioTower size={18} />
            </div>
            {previewRows !== null ? (
              <DeliveryPreviewSection count={previewRows.length} onClear={() => setPreviewRows(null)} title="Notification delivery preview">
                <NotificationDeliveryHistoryGrid deliveries={previewRows} preview />
              </DeliveryPreviewSection>
            ) : null}
            <NotificationDeliveryHistoryGrid deliveries={fleetAlertNotifications} preview={false} />
          </section>
        ) : null}
      </div>
    </section>
  );
}

function MetricTile({
  actionLabel,
  detail,
  label,
  onAction,
  value,
}: {
  actionLabel: string;
  detail: string;
  label: string;
  onAction: () => void;
  value: string;
}) {
  return (
    <div className="metricCard">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
      <button className="linkButton metricCardAction" onClick={onAction} type="button">
        {actionLabel}
      </button>
    </div>
  );
}
