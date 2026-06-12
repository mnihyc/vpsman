import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Database,
  LockKeyhole,
  Network,
  RefreshCw,
  Save,
  ServerCog,
  SlidersHorizontal,
  TimerReset,
} from "lucide-react";
import { parse, stringify, type TomlTable } from "smol-toml";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { TimeSeriesChart, type TimeSeriesChartLine } from "../components/TimeSeriesChart";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
} from "../privilege";
import type {
  JsonValue,
  OperatorView,
  SuiteConfigResponse,
  SuiteConfigUpdateResponse,
  SuiteConfigValidateResponse,
  SystemDashboardRecord,
  SystemMetricSeriesRecord,
} from "../types";
import type { SystemDashboardPointDensity, SystemDashboardWindow } from "../hooks/useSystemData";
import { PreferencesPanel } from "./PreferencesPanel";

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
  onLoadSuiteConfig: () => void;
  onPrivilegeMaterialChange: (material: PrivilegeMaterial | null) => void;
  onUpdateSuiteConfig: (
    toml: string,
    privilegeAssertion: unknown,
  ) => Promise<SuiteConfigUpdateResponse>;
  onValidateSuiteConfig: (toml: string) => Promise<SuiteConfigValidateResponse>;
  operator: OperatorView | null;
  privilegeMaterial: PrivilegeMaterial | null;
  suiteConfig: SuiteConfigResponse | null;
  suiteConfigError: string | null;
  suiteConfigLoading: boolean;
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
  onLoadSuiteConfig,
  onPrivilegeMaterialChange,
  onUpdateSuiteConfig,
  onValidateSuiteConfig,
  operator,
  privilegeMaterial,
  suiteConfig,
  suiteConfigError,
  suiteConfigLoading,
}: SystemPanelProps) {
  if (activeSubpage === "config") {
    return (
      <SystemConfigPanel
        config={suiteConfig}
        error={suiteConfigError}
        loading={suiteConfigLoading}
        onLoad={onLoadSuiteConfig}
        onPrivilegeMaterialChange={onPrivilegeMaterialChange}
        onUpdate={onUpdateSuiteConfig}
        onValidate={onValidateSuiteConfig}
        privilegeMaterial={privilegeMaterial}
      />
    );
  }
  if (activeSubpage === "operator") {
    return <PreferencesPanel operator={operator} />;
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
          subtitle="Pending, delivering, running, retry, and active job pressure."
          metrics={[
            { label: "Active jobs", value: String(dashboard?.current.dispatch.active_jobs ?? 0) },
            { label: "Queue depth", value: String(dashboard?.current.dispatch.queue_depth ?? 0) },
            { label: "Active targets", value: String(dashboard?.current.targets.active ?? 0) },
            { label: "Retried targets", value: String(dashboard?.current.dispatch.retried_targets ?? 0) },
          ]}
          lines={chartLines(series, [
            "dispatch.queue_depth",
            "targets.delivering",
            "targets.running",
            "dispatch.retried_targets",
          ])}
          valueFormatter={(value) => formatNumber(value)}
        />

        <SystemMetricSection
          badge={`${dashboard?.current.targets.deadline_expired_active ?? 0} expired`}
          icon={<AlertTriangle size={18} />}
          title="Deadlines"
          subtitle="Control deadline expiry, agent timeouts, and canceled outcomes."
          metrics={[
            { label: "Expired active", value: String(dashboard?.current.targets.deadline_expired_active ?? 0) },
            { label: "Control timed out", value: String(dashboard?.current.targets.control_timed_out_last_24h ?? 0) },
            { label: "Agent timed out", value: String(dashboard?.current.targets.agent_timed_out_last_24h ?? 0) },
            { label: "Agent offline timeout", value: secondsOrUnset(dashboard?.capacity.agent_offline_secs) },
          ]}
          lines={chartLines(series, [
            "targets.deadline_expired_active",
            "targets.control_timed_out_last_24h",
            "targets.agent_timed_out_last_24h",
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
            { label: "Queued", value: valueOrUnset(dashboard?.current.gateway_events.queued_events) },
            { label: "Delivered", value: valueOrUnset(dashboard?.current.gateway_events.delivered_events) },
            { label: "Retries", value: valueOrUnset(dashboard?.current.gateway_events.retry_attempts) },
          ]}
          lines={chartLines(series, [
            "gateway_events.queued_events",
            "gateway_events.delivered_events",
            "gateway_events.retry_attempts",
            "gateway_events.active_queues",
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
            { label: "Acked", value: String(dashboard?.current.cancellations.acked ?? 0) },
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
            <SystemStatusTile icon={<Activity size={18} />} label="Schedule command" value={secondsOrUnset(dashboard?.capacity.worker_schedule_command_secs)} />
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
  onPrivilegeMaterialChange,
  onUpdate,
  onValidate,
  privilegeMaterial,
}: {
  config: SuiteConfigResponse | null;
  error: string | null;
  loading: boolean;
  onLoad: () => void;
  onPrivilegeMaterialChange: (material: PrivilegeMaterial | null) => void;
  onUpdate: (toml: string, privilegeAssertion: unknown) => Promise<SuiteConfigUpdateResponse>;
  onValidate: (toml: string) => Promise<SuiteConfigValidateResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
}) {
  const [draftToml, setDraftToml] = useState("");
  const [validation, setValidation] = useState<SuiteConfigValidateResponse | null>(null);
  const [configMessage, setConfigMessage] = useState<string | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const parsedDraft = useMemo(() => parseTomlDraft(draftToml), [draftToml]);
  const dirty = Boolean(config && draftToml !== config.toml);

  useEffect(() => {
    if (config) {
      setDraftToml(config.toml);
      setValidation(null);
      setConfigMessage(null);
      setConfigError(null);
      setConfirmed(false);
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
    if (!confirmed) {
      setConfigError("Review and confirm the redacted diff before saving");
      return;
    }
    setPending(true);
    setConfigError(null);
    setConfigMessage(null);
    try {
      const intent = canonicalDbPrivilegeIntent({
        action: "suite_config.update",
        confirmed: true,
        target: "suite_config",
      });
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent,
        privilegeMaterial,
      });
      const response = await onUpdate(draftToml, privilegeAssertion);
      setConfigMessage(`Saved suite config; changed keys: ${response.changed_keys.join(", ") || "none"}.`);
      setConfirmed(false);
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
    setConfirmed(false);
  }

  return (
    <div className="workspace singleColumn systemWorkspace">
      <div className="workspaceStack">
        <section className="fleetPanel">
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
              <button className="primaryAction compactAction" disabled={pending || !dirty || !validation || !confirmed || !privilegeMaterial} onClick={saveDraft} type="button">
                <Save size={16} />
                <span>Save</span>
              </button>
            </div>
          </div>
          {error && <div className="panelError">{error}</div>}
          {configError && <div className="panelError">{configError}</div>}
          {configMessage && <div className="panelSuccess">{configMessage}</div>}
          {config && (
            <div className="dashboardCardGrid operationalGrid">
              <SystemStatusTile icon={<SlidersHorizontal size={18} />} label="Validation" value={config.validation.valid ? "valid" : "invalid"} />
              <SystemStatusTile icon={<RefreshCw size={18} />} label="Hot reload" value={`${config.validation.hot_reload_fields.length} fields`} />
              <SystemStatusTile icon={<AlertTriangle size={18} />} label="Restart required" value={`${config.validation.restart_required_fields.length} fields`} />
              <SystemStatusTile icon={<LockKeyhole size={18} />} label="Privilege" value={privilegeMaterial ? "unlocked" : "locked"} />
            </div>
          )}
        </section>

        {!privilegeMaterial && (
          <PrivilegeVaultBox
            lastPayloadHash={null}
            onPrivilegeMaterialChange={onPrivilegeMaterialChange}
            privilegeMaterial={privilegeMaterial}
          />
        )}

        <section className="dashboardSection">
          <div className="dashboardSectionHeader">
            <div>
              <h2>Structured Controls</h2>
              <span>Common suite settings rendered from TOML. Empty string fields remove optional keys.</span>
            </div>
            <ConsoleStatusBadge tone={parsedDraft.ok ? "ok" : "warning"}>
              {parsedDraft.ok ? "TOML parsed" : "TOML invalid"}
            </ConsoleStatusBadge>
          </div>
          <div className="systemConfigGrid">
            <ConfigGroup title="API">
              <ConfigText path="api.bind" label="Bind address" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="api.gateway_control_url" label="Gateway control URL" parsed={parsedDraft} onChange={updateField} />
              <ConfigNumber path="api.job_output_artifact_min_bytes" label="Output artifact threshold" parsed={parsedDraft} onChange={updateField} />
              <ConfigCheckbox path="api.require_registered_agent_updates" label="Require registered agent updates" parsed={parsedDraft} onChange={updateField} />
            </ConfigGroup>
            <ConfigGroup title="Gateway">
              <ConfigText path="gateway.bind" label="Agent bind" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="gateway.control_bind" label="Control bind" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="gateway.api_url" label="API URL" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="gateway.gateway_id" label="Gateway ID" parsed={parsedDraft} onChange={updateField} />
              <ConfigNumber path="gateway.reconnect_grace_secs" label="Reconnect grace seconds" parsed={parsedDraft} onChange={updateField} />
            </ConfigGroup>
            <ConfigGroup title="Worker">
              <ConfigNumber path="worker.tick_secs" label="Tick seconds" parsed={parsedDraft} onChange={updateField} />
              <ConfigNumber path="worker.worker_lease_secs" label="Worker lease seconds" parsed={parsedDraft} onChange={updateField} />
              <ConfigNumber path="worker.agent_offline_timeout_secs" label="Offline timeout seconds" parsed={parsedDraft} onChange={updateField} />
              <ConfigNumber path="worker.schedule_command_timeout_secs" label="Schedule command timeout" parsed={parsedDraft} onChange={updateField} />
            </ConfigGroup>
            <ConfigGroup title="Capacity">
              <ConfigNumber path="capacity.api_db_pool" label="API DB pool" parsed={parsedDraft} onChange={updateField} />
              <ConfigNumber path="capacity.worker_db_pool" label="Worker DB pool" parsed={parsedDraft} onChange={updateField} />
              <ConfigNumber path="capacity.dispatcher_batch" label="Dispatcher batch" parsed={parsedDraft} onChange={updateField} />
              <ConfigNumber path="capacity.dispatcher_in_flight" label="Dispatcher in-flight" parsed={parsedDraft} onChange={updateField} />
            </ConfigGroup>
            <ConfigGroup title="Storage">
              <ConfigText path="storage.object_store_dir" label="Object store dir" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="storage.object_endpoint" label="Object endpoint" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="storage.object_bucket" label="Object bucket" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="storage.update_object_bucket" label="Update object bucket" parsed={parsedDraft} onChange={updateField} />
            </ConfigGroup>
            <ConfigGroup title="Secrets">
              <ConfigText path="secrets.internal_token_file" label="Internal token file" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="secrets.gateway_private_key_file" label="Gateway key file" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="secrets.privilege_verifier_key_file" label="Privilege verifier file" parsed={parsedDraft} onChange={updateField} />
              <ConfigText path="secrets.object_secret_key_file" label="Object secret key file" parsed={parsedDraft} onChange={updateField} />
            </ConfigGroup>
          </div>
        </section>

        <section className="dashboardSection">
          <div className="dashboardSectionHeader">
            <div>
              <h2>Raw TOML</h2>
              <span>{config?.hot_reload_note ?? "Hot-reload notes unavailable"} / {config?.restart_required_note ?? "Restart notes unavailable"}</span>
            </div>
          </div>
          <textarea
            aria-label="Suite config TOML"
            className="systemConfigToml"
            onChange={(event) => {
              setDraftToml(event.target.value);
              setValidation(null);
              setConfirmed(false);
            }}
            spellCheck={false}
            value={draftToml}
          />
        </section>

        <section className="dashboardSection">
          <div className="dashboardSectionHeader">
            <div>
              <h2>Validation And Redacted Diff</h2>
              <span>Secret file references are shown; secret file contents are never displayed.</span>
            </div>
            <ConsoleStatusBadge tone={validation?.validation.valid ? "ok" : "neutral"}>
              {validation ? `${validation.changed_keys.length} changed` : "Not validated"}
            </ConsoleStatusBadge>
          </div>
          <div className="systemDiffGrid">
            <div>
              <h3>Changed keys</h3>
              <div className="chipList">
                {(validation?.changed_keys ?? []).map((key) => <span key={key}>{key}</span>)}
                {validation && validation.changed_keys.length === 0 ? <span>No changes</span> : null}
              </div>
              <label className="checkLine inlineCheck">
                <input checked={confirmed} disabled={!validation || !dirty} onChange={(event) => setConfirmed(event.target.checked)} type="checkbox" />
                <span>Reviewed redacted diff and restart/hot-reload impact</span>
              </label>
            </div>
            <div>
              <h3>Current redacted</h3>
              <pre className="jsonPreview">{formatJson(config?.redacted ?? validation?.old_redacted ?? null)}</pre>
            </div>
            <div>
              <h3>Draft redacted</h3>
              <pre className="jsonPreview">{formatJson(validation?.redacted ?? null)}</pre>
            </div>
          </div>
        </section>
      </div>
    </div>
  );
}

type ParsedTomlDraft = { ok: true; table: TomlTable } | { ok: false; error: string };

function ConfigGroup({ children, title }: { children: ReactNode; title: string }) {
  return (
    <div className="systemConfigGroup">
      <h3>{title}</h3>
      {children}
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
