import { AlertTriangle, Bell, ExternalLink, RadioTower } from "lucide-react";
import { useState } from "react";
import { ActionFeedback } from "../../components/ActionFeedback";
import { formatBoundedCount } from "../../constants";
import { handleTabListKeyDown, tabId } from "../../components/AccessibleTabs";
import {
  isActivePolicyAlert,
  presentFleetAlert,
} from "../../alertPresentation";
import {
  DeliveryPreviewSection,
  FleetAlertNotificationManager,
  FleetAlertPolicyManager,
  NotificationDeliveryHistoryGrid,
} from "../FleetWorkspace";
import type {
  AgentView,
  AlertConfigurationBulkRequest,
  AlertConfigurationBulkResponse,
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
  canManageAlertPolicies: boolean;
  currentPolicyAlerts: PolicyAlertRecord[];
  currentPolicyAlertsEvidenceAvailable: boolean;
  currentPolicyAlertsTruncated: boolean;
  fleetAlertNotificationChannels: FleetAlertNotificationChannelRecord[];
  fleetAlertNotifications: FleetAlertNotificationDeliveryRecord[];
  fleetAlertNotificationsTruncated: boolean;
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  fleetAlerts: FleetAlertRecord[];
  fleetAlertsEvidenceAvailable: boolean;
  fleetAlertsTruncated: boolean;
  fleetAlertHistory: FleetAlertRecord[];
  fleetAlertHistoryEvidenceAvailable: boolean;
  fleetAlertHistoryTruncated: boolean;
  onBulkMutateFleetAlertNotificationChannels: (
    request: AlertConfigurationBulkRequest,
  ) => Promise<AlertConfigurationBulkResponse>;
  onBulkMutateFleetAlertPolicies: (
    request: AlertConfigurationBulkRequest,
  ) => Promise<AlertConfigurationBulkResponse>;
  onDispatchFleetAlertNotifications: (
    request: FleetAlertNotificationDispatchRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onDryRunFleetAlertPolicy: (
    request: PolicyDryRunRequest,
  ) => Promise<PolicyDryRunResponse>;
  onOpenFleetAlerts: () => void;
  onPolicyFocusChange: (policyId: string | null) => void;
  onProcessFleetAlertNotifications: (
    request: FleetAlertNotificationProcessRequest,
  ) => Promise<FleetAlertNotificationDeliveryRecord[]>;
  onUpsertFleetAlertNotificationChannel: (
    request: FleetAlertNotificationChannelRequest,
  ) => Promise<FleetAlertNotificationChannelRecord>;
  onUpsertFleetAlertPolicy: (
    request: FleetAlertPolicyRequest,
  ) => Promise<FleetAlertPolicyRecord>;
  policyFocusId: string | null;
  policyAlerts: PolicyAlertRecord[];
  policyAlertsEvidenceAvailable: boolean;
  policyAlertsTruncated: boolean;
};

export function AlertsPanel({
  agents,
  apiError,
  canManageAlertPolicies,
  currentPolicyAlerts,
  currentPolicyAlertsEvidenceAvailable,
  currentPolicyAlertsTruncated,
  fleetAlertNotificationChannels,
  fleetAlertNotifications,
  fleetAlertNotificationsTruncated,
  fleetAlertPolicies,
  fleetAlerts,
  fleetAlertsEvidenceAvailable,
  fleetAlertsTruncated,
  fleetAlertHistory,
  fleetAlertHistoryEvidenceAvailable,
  fleetAlertHistoryTruncated,
  onBulkMutateFleetAlertNotificationChannels,
  onBulkMutateFleetAlertPolicies,
  onDispatchFleetAlertNotifications,
  onDryRunFleetAlertPolicy,
  onOpenFleetAlerts,
  onPolicyFocusChange,
  onProcessFleetAlertNotifications,
  onUpsertFleetAlertNotificationChannel,
  onUpsertFleetAlertPolicy,
  policyFocusId,
  policyAlerts,
  policyAlertsEvidenceAvailable,
  policyAlertsTruncated,
}: AlertsPanelProps) {
  const [activeTab, setActiveTab] = useState<AlertConfigTab>("policies");
  const [policyEditorOpen, setPolicyEditorOpen] = useState(false);
  const [previewRows, setPreviewRows] = useState<
    FleetAlertNotificationDeliveryRecord[] | null
  >(null);
  const failedDeliveries = fleetAlertNotifications.filter((delivery) =>
    ["failed", "permanently_failed"].includes(delivery.status),
  ).length;
  const currentPresentations = fleetAlerts.map(presentFleetAlert);
  const activeFleetAlerts = currentPresentations.filter(
    (presentation) => presentation.active,
  ).length;
  const unknownFleetAlerts = currentPresentations.filter(
    (presentation) => presentation.lifecycleState === "unknown",
  ).length;
  const actionableFleetAlerts = currentPresentations.filter(
    (presentation) => presentation.actionable,
  ).length;
  const malformedFleetAlerts = currentPresentations.filter(
    (presentation) => presentation.malformed,
  ).length;
  const activePolicyAlerts = currentPolicyAlertsEvidenceAvailable
    ? currentPolicyAlerts.filter(isActivePolicyAlert)
    : [];
  const urgentPolicyAlerts = activePolicyAlerts.filter((alert) =>
    ["critical", "warning"].includes(alert.severity),
  ).length;
  const policyAlertHistoryTruncated =
    policyAlertsEvidenceAvailable && policyAlertsTruncated;

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
              <span>
                Alert Policies own every alert's trigger, confirmation, and
                automatic resolution. Live lifecycle and triage stay in Fleet /
                Alerts.
              </span>
            </div>
            <div className="sectionActions" aria-label="Alert action links">
              <button
                className="secondaryAction compactAction"
                onClick={onOpenFleetAlerts}
                title="Open live Fleet alert triage"
                type="button"
              >
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
            <div
              className="metricGrid observabilityMetricsSummary"
              aria-label="Alert routing summary"
            >
              <MetricTile
                actionLabel="Open triage"
                detail={
                  !fleetAlertsEvidenceAvailable
                    ? "Current alert evidence is unavailable; cached rows are not presented as current"
                    : `${activeFleetAlerts} active · ${unknownFleetAlerts} Unknown · ${actionableFleetAlerts} actionable${malformedFleetAlerts ? ` · ${malformedFleetAlerts} malformed` : ""}${fleetAlertsTruncated ? " in capped current evidence; open triage to search current occurrences or narrow scope" : ""}`
                }
                label="Current alert episodes"
                onAction={onOpenFleetAlerts}
                value={
                  fleetAlertsEvidenceAvailable
                    ? formatBoundedCount(
                        fleetAlerts.length,
                        fleetAlertsTruncated,
                      )
                    : "Unknown"
                }
              />
              <MetricTile
                actionLabel="Open history"
                detail={
                  !fleetAlertHistoryEvidenceAvailable
                    ? "Alert episode history is unavailable; retained cached rows are not treated as fresh evidence"
                    : fleetAlertHistoryTruncated
                      ? "Only the newest retained-history page is loaded; open history and use an explicit export window for older evidence"
                      : "Condition and occurrence episodes across every lifecycle state"
                }
                label="Alert episode history"
                onAction={onOpenFleetAlerts}
                value={
                  fleetAlertHistoryEvidenceAvailable
                    ? formatBoundedCount(
                        fleetAlertHistory.length,
                        fleetAlertHistoryTruncated,
                      )
                    : "Unknown"
                }
              />
              <MetricTile
                actionLabel="Policies"
                detail={
                  !currentPolicyAlertsEvidenceAvailable
                    ? "Current policy alert evidence is unavailable"
                    : `${currentPolicyAlertsTruncated ? `At least ${urgentPolicyAlerts} active warning or critical alerts in capped current evidence; open Policies and narrow the selector or policy` : `${urgentPolicyAlerts} active warning or critical alerts`}; ${policyAlertsEvidenceAvailable ? `${policyAlerts.length} issued record${policyAlerts.length === 1 ? "" : "s"}${policyAlertHistoryTruncated ? " in the newest history page; use an explicit alert export window for older evidence" : ""}` : "policy alert history unavailable"}`
                }
                label="Policy alert history"
                onAction={() => setActiveTab("policies")}
                value={
                  policyAlertsEvidenceAvailable
                    ? formatBoundedCount(
                        policyAlerts.length,
                        policyAlertHistoryTruncated,
                      )
                    : "Unknown"
                }
              />
              <MetricTile
                actionLabel="Destinations"
                detail="Reviewed notification destinations, separate from event webhooks"
                label="Destinations"
                onAction={() => setActiveTab("destinations")}
                value={formatBoundedCount(
                  fleetAlertNotificationChannels.length,
                  false,
                )}
              />
              <MetricTile
                actionLabel="Open failed deliveries"
                detail={`${failedDeliveries} failed retained notification deliveries${fleetAlertNotificationsTruncated ? " in the newest loaded page; open failed deliveries and narrow the search" : ""}`}
                label="Delivery history"
                onAction={openDeliveryEvidence}
                value={formatBoundedCount(
                  fleetAlertNotifications.length,
                  fleetAlertNotificationsTruncated,
                )}
              />
            </div>

            <div
              className="observabilityWorkflowTabs"
              role="tablist"
              aria-label="Alert configuration sections"
              onKeyDown={handleTabListKeyDown}
            >
              {[
                [
                  "policies",
                  "Policies",
                  "Typed evidence rules and matched alert history",
                ],
                ["destinations", "Destinations", "Alert notification channels"],
                [
                  "deliveries",
                  "Deliveries",
                  "Previewed, failed, and retained notifications",
                ],
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
                  title={`${label}: ${detail}`}
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
                <span>
                  Author state, metric, and occurrence rules with selectors,
                  dry-run previews, and reviewed saves without mixing live
                  triage into this workflow.
                </span>
              </div>
              <AlertTriangle size={18} />
            </div>
            <div
              aria-label="Policy alert lifecycle"
              className="consoleInlineNotice policyLifecycleNotice"
            >
              <strong>Policy alert lifecycle</strong>
              <small>
                Every raw metric, status, and occurrence is typed policy
                evidence. A rule emits Triggered only after its Trigger
                condition and optional Trigger meta condition pass; further
                confirming evidence is Persisting without another edge.
                Incomplete evidence is Unknown. Conditions resolve through the
                inverse Trigger or a separate hysteresis expression plus their
                Resolve meta condition. Occurrences resolve after their
                configured elapsed duration and may be reviewed earlier. Only
                Triggered and Resolved are durable automation edges. Operator
                triage is independent: resetting triage to Open never resolves
                the lifecycle.
              </small>
            </div>
            <FleetAlertPolicyManager
              agents={agents}
              alertsEvidenceAvailable={policyAlertsEvidenceAvailable}
              alertsTruncated={policyAlertHistoryTruncated}
              canManagePolicies={canManageAlertPolicies}
              onBulkMutate={onBulkMutateFleetAlertPolicies}
              onDryRun={onDryRunFleetAlertPolicy}
              onEditorOpenChange={setPolicyEditorOpen}
              onPolicyFocusChange={onPolicyFocusChange}
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
                <h2 id="observability-alert-channels-title">
                  Notification channels
                </h2>
                <span>
                  Route fleet alerts to reviewed webhook destinations. Event
                  webhooks stay on the separate Event webhooks page.
                </span>
              </div>
              <Bell size={18} />
            </div>
            <FleetAlertNotificationManager
              agents={agents}
              channels={fleetAlertNotificationChannels}
              deliveries={fleetAlertNotifications}
              onBulkMutate={onBulkMutateFleetAlertNotificationChannels}
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
                <h2 id="observability-alert-deliveries-title">
                  Notification deliveries
                </h2>
                <span>
                  Previewed, failed, retried, and retained alert notification
                  delivery rows. Failed evidence is searchable here.
                </span>
              </div>
              <RadioTower size={18} />
            </div>
            {previewRows !== null ? (
              <DeliveryPreviewSection
                count={previewRows.length}
                onClear={() => setPreviewRows(null)}
                title="Notification delivery preview"
              >
                <NotificationDeliveryHistoryGrid
                  deliveries={previewRows}
                  preview
                />
              </DeliveryPreviewSection>
            ) : null}
            <NotificationDeliveryHistoryGrid
              deliveries={fleetAlertNotifications}
              preview={false}
              rowsTruncated={fleetAlertNotificationsTruncated}
            />
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
      <button
        className="linkButton metricCardAction"
        onClick={onAction}
        type="button"
      >
        {actionLabel}
      </button>
    </div>
  );
}
