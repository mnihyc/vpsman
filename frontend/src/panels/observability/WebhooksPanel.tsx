import { useState } from "react";
import {
  DeliveryPreviewSection,
  WebhookDeliveryHistoryGrid,
  WebhookDeliveryMaintenancePanel,
  WebhookDryRunNotice,
  WebhookRuleManager,
} from "../FleetWorkspace";
import type {
  AgentView,
  WebhookDeliveryRotationRequest,
  WebhookDeliveryRotationResponse,
  WebhookRuleDeliveryRecord,
  WebhookRuleDispatchRequest,
  WebhookRuleDryRunRecord,
  WebhookRuleDryRunRequest,
  WebhookRuleProcessRequest,
  WebhookRuleRecord,
  WebhookRuleRequest,
} from "../../types";

type WebhookConfigTab = "rules" | "deliveries" | "maintenance";

type WebhooksPanelProps = {
  agents: AgentView[];
  apiError: string | null;
  onDeleteWebhookRule: (ruleId: string, reviewedName: string) => Promise<void>;
  onDispatchWebhookRules: (request: WebhookRuleDispatchRequest) => Promise<WebhookRuleDeliveryRecord[]>;
  onDryRunWebhookRule: (request: WebhookRuleDryRunRequest) => Promise<WebhookRuleDryRunRecord>;
  onProcessWebhookRuleDeliveries: (request: WebhookRuleProcessRequest) => Promise<WebhookRuleDeliveryRecord[]>;
  onRotateWebhookDeliveryHistory: (request: WebhookDeliveryRotationRequest) => Promise<WebhookDeliveryRotationResponse>;
  onUpsertWebhookRule: (request: WebhookRuleRequest) => Promise<WebhookRuleRecord>;
  webhookRuleDeliveries: WebhookRuleDeliveryRecord[];
  webhookRules: WebhookRuleRecord[];
};

export function WebhooksPanel({
  agents,
  apiError,
  onDeleteWebhookRule,
  onDispatchWebhookRules,
  onDryRunWebhookRule,
  onProcessWebhookRuleDeliveries,
  onRotateWebhookDeliveryHistory,
  onUpsertWebhookRule,
  webhookRuleDeliveries,
  webhookRules,
}: WebhooksPanelProps) {
  const [activeTab, setActiveTab] = useState<WebhookConfigTab>("rules");
  const [ruleEditorOpen, setRuleEditorOpen] = useState(false);
  const [previewRows, setPreviewRows] = useState<WebhookRuleDeliveryRecord[]>([]);
  const [dryRunPreview, setDryRunPreview] = useState<WebhookRuleDryRunRecord | null>(null);
  const disabledRules = webhookRules.filter((rule) => !rule.enabled).length;
  const failedDeliveries = webhookRuleDeliveries.filter((delivery) =>
    ["failed", "permanently_failed"].includes(delivery.status),
  ).length;
  const queuedDeliveries = webhookRuleDeliveries.filter((delivery) => delivery.status === "queued").length;

  function openDeliveryEvidence() {
    setActiveTab("deliveries");
    const target = document.getElementById("observability-webhook-deliveries");
    window.requestAnimationFrame(() => {
      target?.scrollIntoView({ block: "start", behavior: "smooth" });
    });
  }

  function previewDeliveries(rows: WebhookRuleDeliveryRecord[]) {
    setPreviewRows(rows);
    if (!ruleEditorOpen) {
      openDeliveryEvidence();
    }
  }

  function previewDryRun(preview: WebhookRuleDryRunRecord | null) {
    setDryRunPreview(preview);
    if (preview && !ruleEditorOpen) {
      openDeliveryEvidence();
    }
  }

  function clearPreview() {
    setPreviewRows([]);
    setDryRunPreview(null);
  }

  return (
    <section className="workspace singleColumn observabilityWebhooksWorkspace">
      <div className="fleetPanel observabilityWebhooksPanel">
        {!ruleEditorOpen ? (
          <div className="sectionHeader">
            <div>
              <h2>Event webhooks</h2>
              <span>Event webhooks are independent from alert notification destinations.</span>
            </div>
          </div>
        ) : null}

        {apiError ? (
          <div className="panelError observabilityMetricsError" role="alert">
            {apiError}
          </div>
        ) : null}

        {!ruleEditorOpen ? (
          <>
            <div className="metricGrid observabilityMetricsSummary" aria-label="Webhook routing summary">
              <MetricTile actionLabel="Rules" detail={`${disabledRules} disabled rules`} label="Event webhook rules" onAction={() => setActiveTab("rules")} value={String(webhookRules.length)} />
              <MetricTile actionLabel="Deliveries" detail="Queued event webhook rows awaiting processing" label="Queued" onAction={openDeliveryEvidence} value={String(queuedDeliveries)} />
              <MetricTile actionLabel="Retry failed" detail="Failed event webhook deliveries, separate from alert notification failures" label="Failures" onAction={() => setActiveTab("rules")} value={String(failedDeliveries)} />
              <MetricTile actionLabel="History" detail="Retained event webhook delivery rows" label="Deliveries" onAction={openDeliveryEvidence} value={String(webhookRuleDeliveries.length)} />
            </div>

            <div className="observabilityWorkflowTabs" role="tablist" aria-label="Event webhook sections">
              {[
                ["rules", "Rules", "Create rules, send tests, and retry failed deliveries"],
                ["deliveries", "Deliveries", "Previewed, queued, failed, and retained event webhooks"],
                ["maintenance", "Maintenance", "Reviewed retention cleanup"],
              ].map(([id, label, detail]) => (
                <button
                  aria-selected={activeTab === id}
                  className={activeTab === id ? "active" : ""}
                  key={id}
                  onClick={() => setActiveTab(id as WebhookConfigTab)}
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

        {activeTab === "rules" ? (
          <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-webhook-rules-title">
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="observability-webhook-rules-title">Event webhook rules</h2>
                <span>Create event webhook rules, preview matching events, send reviewed tests, and retry failed deliveries without mixing alert notification channels into this workflow.</span>
              </div>
            </div>
            <WebhookRuleManager
              agents={agents}
              onDelete={onDeleteWebhookRule}
              onDispatch={onDispatchWebhookRules}
              onDryRun={onDryRunWebhookRule}
              onOpenDeliveries={openDeliveryEvidence}
              onPreviewDryRun={previewDryRun}
              onPreviewRows={previewDeliveries}
              onProcess={onProcessWebhookRuleDeliveries}
              onUpsert={onUpsertWebhookRule}
              editorMode="focused"
              onEditorOpenChange={setRuleEditorOpen}
              queueMode="configuration"
              rules={webhookRules}
            />
          </section>
        ) : null}

        {activeTab === "deliveries" ? (
          <section className="dashboardSection observabilityGroupSection" id="observability-webhook-deliveries" aria-labelledby="observability-webhook-deliveries-title">
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="observability-webhook-deliveries-title">Event webhook deliveries</h2>
                <span>Dry-run previews, queued tests, retained status, target, attempts, and event webhook errors. Alert notification deliveries stay on Alerts.</span>
              </div>
            </div>
            {dryRunPreview || previewRows.length > 0 ? (
              <DeliveryPreviewSection count={previewRows.length} onClear={clearPreview} title="Event webhook delivery preview">
                {dryRunPreview ? <WebhookDryRunNotice agents={agents} preview={dryRunPreview} /> : null}
                <WebhookDeliveryHistoryGrid deliveries={previewRows} preview />
              </DeliveryPreviewSection>
            ) : null}
            <WebhookDeliveryHistoryGrid deliveries={webhookRuleDeliveries} preview={false} />
          </section>
        ) : null}

        {activeTab === "maintenance" ? (
          <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-webhook-maintenance-title">
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="observability-webhook-maintenance-title">Event webhook maintenance</h2>
                <span>Review retained event webhook cleanup by age, status, and rule before deleting delivery history rows.</span>
              </div>
            </div>
            <WebhookDeliveryMaintenancePanel onRotate={onRotateWebhookDeliveryHistory} rules={webhookRules} />
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
