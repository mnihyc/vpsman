import { AlertTriangle, Bell, ExternalLink, RadioTower } from "lucide-react";
import { useState } from "react";
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
  policyAlerts,
}: AlertsPanelProps) {
  const [activeTab, setActiveTab] = useState<AlertConfigTab>("policies");
  const [policyEditorOpen, setPolicyEditorOpen] = useState(false);
  const [previewRows, setPreviewRows] = useState<FleetAlertNotificationDeliveryRecord[]>([]);
  const failedDeliveries = fleetAlertNotifications.filter((delivery) =>
    ["failed", "permanently_failed"].includes(delivery.status),
  ).length;
  const urgentPolicyAlerts = policyAlerts.filter((alert) => ["critical", "warning"].includes(alert.severity)).length;

  function openDeliveryEvidence() {
    setActiveTab("deliveries");
    const target = document.getElementById("observability-alert-deliveries");
    window.requestAnimationFrame(() => {
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
                Active triage queue
              </button>
            </div>
          </div>
        ) : null}

        {apiError ? (
          <div className="panelError observabilityMetricsError" role="alert">
            {apiError}
          </div>
        ) : null}

        {!policyEditorOpen ? (
          <>
            <div className="metricGrid observabilityMetricsSummary" aria-label="Alert routing summary">
              <MetricTile actionLabel="Open triage" detail="Operational alert triage records live in Fleet / Alerts" label="Active fleet alerts" onAction={onOpenFleetAlerts} value={String(fleetAlerts.length)} />
              <MetricTile actionLabel="Policies" detail={`${urgentPolicyAlerts} warning or critical policy-issued alerts`} label="Policy alerts" onAction={() => setActiveTab("policies")} value={String(policyAlerts.length)} />
              <MetricTile actionLabel="Destinations" detail="Reviewed notification destinations, separate from event webhooks" label="Destinations" onAction={() => setActiveTab("destinations")} value={String(fleetAlertNotificationChannels.length)} />
              <MetricTile actionLabel="Failed deliveries" detail={`${failedDeliveries} failed retained notification deliveries`} label="Delivery history" onAction={openDeliveryEvidence} value={String(fleetAlertNotifications.length)} />
            </div>

            <div className="observabilityWorkflowTabs" role="tablist" aria-label="Alert configuration sections">
              {[
                ["policies", "Policies", "Threshold rules and matched policy alerts"],
                ["destinations", "Destinations", "Alert notification channels"],
                ["deliveries", "Deliveries", "Previewed, failed, and retained notifications"],
              ].map(([id, label, detail]) => (
                <button
                  aria-selected={activeTab === id}
                  className={activeTab === id ? "active" : ""}
                  key={id}
                  onClick={() => setActiveTab(id as AlertConfigTab)}
                  role="tab"
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
          <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-alert-policies-title">
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
              policyFocusId={null}
            />
          </section>
        ) : null}

        {activeTab === "destinations" ? (
          <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-alert-channels-title">
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
          <section className="dashboardSection observabilityGroupSection" id="observability-alert-deliveries" aria-labelledby="observability-alert-deliveries-title">
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="observability-alert-deliveries-title">Notification deliveries</h2>
                <span>Previewed, failed, retried, and retained alert notification delivery rows. Failed evidence is searchable here.</span>
              </div>
              <RadioTower size={18} />
            </div>
            {previewRows.length > 0 ? (
              <DeliveryPreviewSection count={previewRows.length} onClear={() => setPreviewRows([])} title="Notification delivery preview">
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
