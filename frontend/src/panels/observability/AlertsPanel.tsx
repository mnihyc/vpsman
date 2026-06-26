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
  const [previewRows, setPreviewRows] = useState<FleetAlertNotificationDeliveryRecord[]>([]);
  const failedDeliveries = fleetAlertNotifications.filter((delivery) =>
    ["failed", "permanently_failed"].includes(delivery.status),
  ).length;
  const urgentPolicyAlerts = policyAlerts.filter((alert) => ["critical", "warning"].includes(alert.severity)).length;

  function openDeliveryEvidence() {
    const target = document.getElementById("observability-alert-deliveries");
    target?.scrollIntoView({ block: "start", behavior: "smooth" });
  }

  function previewDeliveries(rows: FleetAlertNotificationDeliveryRecord[]) {
    setPreviewRows(rows);
    window.requestAnimationFrame(openDeliveryEvidence);
  }

  return (
    <section className="workspace singleColumn observabilityAlertsWorkspace">
      <div className="fleetPanel observabilityAlertsPanel">
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

        {apiError ? (
          <div className="panelError observabilityMetricsError" role="alert">
            {apiError}
          </div>
        ) : null}

        <div className="metricGrid observabilityMetricsSummary" aria-label="Alert routing summary">
          <MetricTile detail="Open operational triage records in Fleet / Alerts" label="Active fleet alerts" value={String(fleetAlerts.length)} />
          <MetricTile detail={`${urgentPolicyAlerts} warning or critical policy-issued alerts`} label="Policy alerts" value={String(policyAlerts.length)} />
          <MetricTile detail="Threshold groups evaluated by the control plane" label="Policy groups" value={String(fleetAlertPolicies.length)} />
          <MetricTile detail={`${failedDeliveries} failed retained notification deliveries`} label="Channels" value={String(fleetAlertNotificationChannels.length)} />
        </div>

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
            onUpsert={onUpsertFleetAlertPolicy}
            policies={fleetAlertPolicies}
            policyAlerts={policyAlerts}
            policyFilterClientId={null}
            policyFocusId={null}
          />
        </section>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-alert-channels-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-alert-channels-title">Notification channels</h2>
              <span>Route fleet alerts to reviewed webhook destinations, preview queue actions, and inspect delivery evidence.</span>
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
          />
        </section>

        <section className="dashboardSection observabilityGroupSection" id="observability-alert-deliveries" aria-labelledby="observability-alert-deliveries-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-alert-deliveries-title">Notification deliveries</h2>
              <span>Previewed and retained alert notification delivery rows stay attached to the Alerts page.</span>
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
      </div>
    </section>
  );
}

function MetricTile({ detail, label, value }: { detail: string; label: string; value: string }) {
  return (
    <div className="metricCard">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}
