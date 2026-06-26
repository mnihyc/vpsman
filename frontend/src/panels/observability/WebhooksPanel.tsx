import { RadioTower, RefreshCw, Webhook } from "lucide-react";
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
  const [previewRows, setPreviewRows] = useState<WebhookRuleDeliveryRecord[]>([]);
  const [dryRunPreview, setDryRunPreview] = useState<WebhookRuleDryRunRecord | null>(null);
  const disabledRules = webhookRules.filter((rule) => !rule.enabled).length;
  const failedDeliveries = webhookRuleDeliveries.filter((delivery) =>
    ["failed", "permanently_failed"].includes(delivery.status),
  ).length;
  const queuedDeliveries = webhookRuleDeliveries.filter((delivery) => delivery.status === "queued").length;

  function openDeliveryEvidence() {
    const target = document.getElementById("observability-webhook-deliveries");
    target?.scrollIntoView({ block: "start", behavior: "smooth" });
  }

  function previewDeliveries(rows: WebhookRuleDeliveryRecord[]) {
    setPreviewRows(rows);
    window.requestAnimationFrame(openDeliveryEvidence);
  }

  function previewDryRun(preview: WebhookRuleDryRunRecord | null) {
    setDryRunPreview(preview);
    if (preview) {
      window.requestAnimationFrame(openDeliveryEvidence);
    }
  }

  function clearPreview() {
    setPreviewRows([]);
    setDryRunPreview(null);
  }

  return (
    <section className="workspace singleColumn observabilityWebhooksWorkspace">
      <div className="fleetPanel observabilityWebhooksPanel">
        <div className="sectionHeader">
          <div>
            <h2>Webhooks</h2>
            <span>Expression webhook rules, dispatch previews, delivery evidence, and retention maintenance.</span>
          </div>
        </div>

        {apiError ? (
          <div className="panelError observabilityMetricsError" role="alert">
            {apiError}
          </div>
        ) : null}

        <div className="metricGrid observabilityMetricsSummary" aria-label="Webhook routing summary">
          <MetricTile detail={`${disabledRules} disabled rules`} label="Webhook rules" value={String(webhookRules.length)} />
          <MetricTile detail="Queued delivery rows awaiting processing" label="Queued" value={String(queuedDeliveries)} />
          <MetricTile detail="Failed or permanently failed retained rows" label="Failures" value={String(failedDeliveries)} />
          <MetricTile detail="Retained webhook delivery history rows" label="Deliveries" value={String(webhookRuleDeliveries.length)} />
        </div>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-webhook-rules-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-webhook-rules-title">Webhook rules</h2>
              <span>Author expressions, templates, preview events, and reviewed queue/delivery actions from the Webhooks page.</span>
            </div>
            <Webhook size={18} />
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
            rules={webhookRules}
          />
        </section>

        <section className="dashboardSection observabilityGroupSection" id="observability-webhook-deliveries" aria-labelledby="observability-webhook-deliveries-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-webhook-deliveries-title">Webhook deliveries</h2>
              <span>Dry-run previews, queue previews, retained status, target, attempts, and delivery errors.</span>
            </div>
            <RadioTower size={18} />
          </div>
          {dryRunPreview || previewRows.length > 0 ? (
            <DeliveryPreviewSection count={previewRows.length} onClear={clearPreview} title="Webhook delivery preview">
              {dryRunPreview ? <WebhookDryRunNotice agents={agents} preview={dryRunPreview} /> : null}
              <WebhookDeliveryHistoryGrid deliveries={previewRows} preview />
            </DeliveryPreviewSection>
          ) : null}
          <WebhookDeliveryHistoryGrid deliveries={webhookRuleDeliveries} preview={false} />
        </section>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-webhook-maintenance-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-webhook-maintenance-title">Webhook delivery maintenance</h2>
              <span>Review retained delivery cleanup by age, status, and rule before deleting history rows.</span>
            </div>
            <RefreshCw size={18} />
          </div>
          <WebhookDeliveryMaintenancePanel onRotate={onRotateWebhookDeliveryHistory} rules={webhookRules} />
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
