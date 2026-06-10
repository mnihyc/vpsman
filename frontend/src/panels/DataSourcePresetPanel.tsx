import { useEffect, useMemo, useState, type FormEvent } from "react";
import { DatabaseZap, SlidersHorizontal } from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { CrudPager } from "../components/CrudPager";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { usePanelDisplaySettings } from "../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "../privilege";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  selectorExpressionForClientIds,
} from "../searchExpression";
import type {
  AgentView,
  AssignDataSourcePresetRequest,
  AssignDataSourcePresetResponse,
  CloneDataSourcePresetRequest,
  CreateJobRequest,
  CreateJobResponse,
  CreateDataSourcePresetRequest,
  DataSourceHotConfigResponse,
  DataSourcePresetAssignmentRecord,
  DataSourcePresetDiffRequest,
  DataSourcePresetDiffResponse,
  DataSourcePresetRecord,
  DataSourcePresetTestRequest,
  DataSourcePresetTestResponse,
  DataSourceStatusRecord,
  JobOperation,
  JsonValue,
  UpdateDataSourcePresetRequest,
  UpdateDataSourcePresetResponse,
} from "../types";
import {
  formatTime,
  formatVpsName,
  runPanelAction,
  shortId,
  statusClass,
} from "../utils";

const DATA_SOURCE_DOMAINS = [
  "telemetry_metrics_source",
  "runtime_traffic_accounting_source",
  "latency_probe_source",
  "speed_test_provider",
  "process_inventory_source",
  "user_session_inventory_source",
  "command_execution_policy",
  "process_supervisor_policy",
  "runtime_tunnel_adapter",
  "traffic_limit_status_source",
  "routing_daemon_adapter",
  "backup_object_store",
  "restore_path_mapping",
  "update_artifact_source",
  "update_restart_policy",
  "update_rollback_heartbeat_source",
];

const DEFAULT_DEFINITION = "{\n  \"source\": \"custom\"\n}";
const DATA_SOURCE_SELECTOR_STORAGE_KEY = "vpsman.dataSources.assignmentSelectorExpression";
type DataSourceConfirmationAction = "assignment" | "apply" | "lifecycle-update";

export function DataSourcePresetPanel({
  activeSubpage,
  agents,
  assignments,
  dataSourceStatus,
  onAssignPreset,
  onClonePreset,
  onCreateJob,
  onCreatePreset,
  onDiffPreset,
  onOpenPrivilegeUnlock,
  onRenderHotConfig,
  onTestPreset,
  onUpdatePreset,
  privilegeMaterial,
  presets,
  setPrivilegeMaterial,
}: {
  activeSubpage: "presets" | "status";
  agents: AgentView[];
  assignments: DataSourcePresetAssignmentRecord[];
  dataSourceStatus: DataSourceStatusRecord[];
  onAssignPreset: (request: AssignDataSourcePresetRequest) => Promise<AssignDataSourcePresetResponse>;
  onClonePreset: (presetId: string, request: CloneDataSourcePresetRequest) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreatePreset: (request: CreateDataSourcePresetRequest) => Promise<void>;
  onDiffPreset: (presetId: string, request: DataSourcePresetDiffRequest) => Promise<DataSourcePresetDiffResponse>;
  onOpenPrivilegeUnlock: () => void;
  onRenderHotConfig: (clientId: string) => Promise<DataSourceHotConfigResponse>;
  onTestPreset: (presetId: string, request: DataSourcePresetTestRequest) => Promise<DataSourcePresetTestResponse>;
  onUpdatePreset: (presetId: string, request: UpdateDataSourcePresetRequest) => Promise<UpdateDataSourcePresetResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
  presets: DataSourcePresetRecord[];
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [createDomain, setCreateDomain] = useState(DATA_SOURCE_DOMAINS[1]);
  const [createName, setCreateName] = useState("");
  const [createScope, setCreateScope] = useState("shared");
  const [ownerClientId, setOwnerClientId] = useState("");
  const [description, setDescription] = useState("");
  const [definitionText, setDefinitionText] = useState(DEFAULT_DEFINITION);
  const [assignDomain, setAssignDomain] = useState(DATA_SOURCE_DOMAINS[1]);
  const [assignPresetId, setAssignPresetId] = useState("");
  const [assignmentSelectorExpression, setAssignmentSelectorExpression] = useState(() =>
    readLocalString(DATA_SOURCE_SELECTOR_STORAGE_KEY, ""),
  );
  const [renderClientId, setRenderClientId] = useState("");
  const [renderedHotConfig, setRenderedHotConfig] = useState<DataSourceHotConfigResponse | null>(null);
  const [applyTimeoutSecs, setApplyTimeoutSecs] = useState(30);
  const [lastApplyJob, setLastApplyJob] = useState<CreateJobResponse | null>(null);
  const [lastApplyPayloadHash, setLastApplyPayloadHash] = useState<string | null>(null);
  const [lifecyclePresetId, setLifecyclePresetId] = useState("");
  const [lifecycleDescription, setLifecycleDescription] = useState("");
  const [lifecycleDefinitionText, setLifecycleDefinitionText] = useState(DEFAULT_DEFINITION);
  const [lifecycleCloneName, setLifecycleCloneName] = useState("");
  const [lastDiff, setLastDiff] = useState<DataSourcePresetDiffResponse | null>(null);
  const [lastTest, setLastTest] = useState<DataSourcePresetTestResponse | null>(null);
  const [lastUpdate, setLastUpdate] = useState<UpdateDataSourcePresetResponse | null>(null);
  const [lastAssignment, setLastAssignment] = useState<AssignDataSourcePresetResponse | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [pendingConfirmation, setPendingConfirmation] = useState<DataSourceConfirmationAction | null>(null);

  const assignablePresets = useMemo(
    () => presets.filter((preset) => preset.domain === assignDomain),
    [assignDomain, presets],
  );
  const sourceStatusSummary = useMemo(() => {
    const degraded = dataSourceStatus.filter((row) => ["degraded", "agent_offline", "needs_promotion"].includes(row.status)).length;
    const ready = dataSourceStatus.filter((row) => ["ok", "selected", "ready", "ready_on_demand"].includes(row.status)).length;
    return `${ready} ready source checks, ${degraded} need attention`;
  }, [dataSourceStatus]);
  const effectivePresetId = assignPresetId || assignablePresets[0]?.id || "";
  const effectiveLifecyclePresetId = lifecyclePresetId || presets[0]?.id || "";
  const showPresetManagement = activeSubpage === "presets";
  const showSourceStatus = activeSubpage === "status";
  const lifecyclePreset = useMemo(
    () => presets.find((preset) => preset.id === effectiveLifecyclePresetId) ?? null,
    [effectiveLifecyclePresetId, presets],
  );
  const assignmentSelectorParse = useMemo(
    () => parseSearchExpression(assignmentSelectorExpression),
    [assignmentSelectorExpression],
  );
  const assignmentTargetCount = useMemo(
    () =>
      assignmentSelectorParse.error
        ? 0
        : agentsMatchingExpression(agents, assignmentSelectorExpression).length,
    [agents, assignmentSelectorExpression, assignmentSelectorParse.error],
  );
  const lifecycleStatus =
    lastUpdate?.confirmation_required
      ? `${lastUpdate.affected_client_count} VPSs inherit this preset; confirmation required`
      : lastUpdate
        ? `${lastUpdate.affected_client_count} VPSs inherited the preset update`
        : lastTest
          ? lastTest.valid
            ? `${lastTest.renderable ? "Renderable" : "Workflow"} preset test passed for ${lastTest.domain}`
            : `Preset test failed: ${lastTest.error ?? "invalid definition"}`
          : lastDiff
            ? `${lastDiff.changed_keys.length} keys changed; ${lastDiff.affected_client_count} VPSs affected`
            : null;
  const status =
    actionError ??
    lifecycleStatus ??
    (dataSourceStatus.length > 0 ? sourceStatusSummary : null) ??
    (lastAssignment
      ? `${lastAssignment.target_count} VPS preset assignments evaluated`
      : lastApplyJob
        ? `Data-source patch job ${lastApplyJob.job_id} queued ${lastApplyJob.target_count} target`
      : `${presets.length} presets across ${new Set(presets.map((preset) => preset.domain)).size} domains`);

  useEffect(() => {
    if (!lifecyclePreset) {
      return;
    }
    setLifecycleDescription(lifecyclePreset.description ?? "");
    setLifecycleDefinitionText(JSON.stringify(lifecyclePreset.definition, null, 2));
    setLifecycleCloneName(defaultCloneName(lifecyclePreset.name));
    setLastDiff(null);
    setLastTest(null);
    setLastUpdate(null);
  }, [lifecyclePreset?.id]);

  useEffect(() => {
    writeLocalString(DATA_SOURCE_SELECTOR_STORAGE_KEY, assignmentSelectorExpression);
  }, [assignmentSelectorExpression]);

  async function submitCreate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      await onCreatePreset({
        definition: parseDefinition(definitionText),
        description: description.trim() || null,
        domain: createDomain,
        name: createName.trim(),
        owner_client_id: createScope === "vps_local" ? ownerClientId || null : null,
        scope: createScope,
      });
      setCreateName("");
      setDescription("");
      setDefinitionText(DEFAULT_DEFINITION);
    });
  }

  function submitAssignment(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPendingConfirmation("assignment");
  }

  async function executeAssignment() {
    await runPanelAction(setPending, setActionError, async () => {
      if (assignmentSelectorParse.error) {
        throw new Error(`Invalid target expression: ${assignmentSelectorParse.error}`);
      }
      if (assignmentTargetCount === 0) {
        throw new Error("Add at least one matching VPS target");
      }
      const response = await onAssignPreset({
        confirmed: true,
        domain: assignDomain,
        preset_id: effectivePresetId,
        selector_expression: assignmentSelectorExpression.trim(),
      });
      setLastAssignment(response);
    });
  }

  async function previewHotConfig(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      setRenderedHotConfig(await onRenderHotConfig(renderClientId));
      setLastApplyJob(null);
    });
  }

  async function applyRenderedHotConfig() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!renderClientId) {
        throw new Error("Select a VPS before applying a data-source patch");
      }
      if (!privilegeMaterial) {
        throw new Error("Unlock privilege before applying a data-source patch");
      }
      const rendered =
        renderedHotConfig?.client_id === renderClientId ? renderedHotConfig : await onRenderHotConfig(renderClientId);
      const operation: JobOperation = { type: "data_source_config_patch", toml: rendered.toml };
      const selectorExpression = selectorExpressionForClientIds([renderClientId]);
      const timeoutSecs = clampInteger(applyTimeoutSecs, 1, 3600);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [renderClientId],
        commandType: "data_source_config_patch",
        operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs,
      });
      const response = await onCreateJob({
        argv: [],
            selector_expression: selectorExpression,
        target_client_ids: [renderClientId],
        command: "data_source_config_patch",
        confirmed: true,
        destructive: false,
        force_unprivileged: false,
        operation,
        privileged: true,
        privilege_assertion: built.privilegeAssertion,
        timeout_secs: timeoutSecs,
      });
      setRenderedHotConfig(rendered);
      setLastApplyJob(response);
      setLastApplyPayloadHash(built.payloadHashHex);
    });
  }

  function confirmApplyRenderedHotConfig() {
    setPendingConfirmation("apply");
  }

  async function diffLifecyclePreset() {
    if (!lifecyclePreset) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      setLastDiff(
        await onDiffPreset(lifecyclePreset.id, {
          definition: parseDefinition(lifecycleDefinitionText),
          description: lifecycleDescription.trim() || null,
        }),
      );
      setLastTest(null);
      setLastUpdate(null);
    });
  }

  async function testLifecyclePreset() {
    if (!lifecyclePreset) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      setLastTest(
        await onTestPreset(lifecyclePreset.id, {
          definition: parseDefinition(lifecycleDefinitionText),
        }),
      );
      setLastDiff(null);
      setLastUpdate(null);
    });
  }

  async function cloneLifecyclePreset() {
    if (!lifecyclePreset || !lifecycleCloneName.trim()) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      await onClonePreset(lifecyclePreset.id, {
        description: lifecycleDescription.trim() || lifecyclePreset.description,
        name: lifecycleCloneName.trim(),
        owner_client_id: null,
        scope: "shared",
      });
      setLastDiff(null);
      setLastTest(null);
      setLastUpdate(null);
    });
  }

  function updateLifecyclePreset() {
    setPendingConfirmation("lifecycle-update");
  }

  async function executeLifecyclePresetUpdate() {
    if (!lifecyclePreset || lifecyclePreset.built_in) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      const response = await onUpdatePreset(lifecyclePreset.id, {
        confirmed: true,
        definition: parseDefinition(lifecycleDefinitionText),
        description: lifecycleDescription.trim() || null,
      });
      setLastUpdate(response);
      setLastDiff(response.diff);
      setLastTest(null);
    });
  }

  async function confirmDataSourceAction() {
    const action = pendingConfirmation;
    if (!action) {
      return;
    }
    setPendingConfirmation(null);
    if (action === "assignment") {
      await executeAssignment();
    } else if (action === "apply") {
      await applyRenderedHotConfig();
    } else {
      await executeLifecyclePresetUpdate();
    }
  }

  const dataSourceConfirmationTitle =
    pendingConfirmation === "assignment"
      ? "Assign data-source preset"
      : pendingConfirmation === "apply"
        ? "Apply data-source patch"
        : "Update data-source preset";
  const dataSourceConfirmationDetail =
    pendingConfirmation === "assignment"
      ? "Confirm the selected preset and resolved VPS assignment set."
      : pendingConfirmation === "apply"
        ? "Confirm the rendered hot-config patch for the selected VPS."
        : "Confirm updating this preset for assigned VPSs.";
  const dataSourceConfirmationItems =
    pendingConfirmation === "assignment"
      ? [
          { label: "Domain", value: assignDomain },
          { label: "Preset", value: effectivePresetId ? shortId(effectivePresetId) : "none" },
          { label: "Targets", value: `${assignmentTargetCount}/${agents.length}` },
        ]
      : pendingConfirmation === "apply"
        ? [
            {
              label: "VPS",
              value: agents.find((agent) => agent.id === renderClientId)
                ? formatVpsName(agents.find((agent) => agent.id === renderClientId) as AgentView, vpsNameDisplayMode)
                : renderClientId || "none",
            },
            { label: "Timeout", value: `${clampInteger(applyTimeoutSecs, 1, 3600)}s` },
            { label: "Privilege", value: privilegeMaterial ? "Unlocked locally" : "Locked" },
          ]
        : [
            { label: "Preset", value: lifecyclePreset?.name ?? "none" },
            { label: "Assigned", value: `${lifecyclePreset?.assigned_client_count ?? 0} VPSs` },
          ];

  function changeAssignDomain(domain: string) {
    setAssignDomain(domain);
    setAssignPresetId("");
    setLastAssignment(null);
  }

  return (
    <section className="fleetPanel dataSourcePresetPanel">
      <div className="sectionHeader">
        <div>
          <h2>{showSourceStatus ? "Data-source status" : "Data-source presets"}</h2>
          <span>{status}</span>
        </div>
      </div>

      {showPresetManagement && (
      <div className="managementGrid presetManagementGrid">
        <ConfirmationPrompt
          confirmLabel="Confirm"
          detail={dataSourceConfirmationDetail}
          items={dataSourceConfirmationItems}
          onCancel={() => setPendingConfirmation(null)}
          onConfirm={() => void confirmDataSourceAction()}
          open={pendingConfirmation !== null}
          pending={pending}
          title={dataSourceConfirmationTitle}
          tone={pendingConfirmation === "apply" ? "danger" : "normal"}
        />
        <form className="compactForm presetForm" onSubmit={submitCreate}>
          <strong>Preset definition</strong>
          <span className="formHint">
            Create one reusable data-source preset. Scope decides whether it is shared or owned by one VPS.
          </span>
          <div className="formRow presetFormRow">
            <label>
              <span>Domain</span>
              <select aria-label="Preset domain" onChange={(event) => setCreateDomain(event.target.value)} value={createDomain}>
                {DATA_SOURCE_DOMAINS.map((domain) => (
                  <option key={domain} value={domain}>
                    {domain}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Name</span>
              <input
                aria-label="Preset name"
                onChange={(event) => setCreateName(event.target.value)}
                placeholder="shared:vnstat-json"
                value={createName}
              />
            </label>
            <label>
              <span>Scope</span>
              <select aria-label="Preset scope" onChange={(event) => setCreateScope(event.target.value)} value={createScope}>
                <option value="shared">shared</option>
                <option value="vps_local">vps_local</option>
              </select>
            </label>
          </div>
          {createScope === "vps_local" && (
            <label>
              <span>Owner VPS</span>
              <select aria-label="VPS-local owner" onChange={(event) => setOwnerClientId(event.target.value)} value={ownerClientId}>
                <option value="">Owner VPS</option>
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {formatVpsName(agent, vpsNameDisplayMode)}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label>
            <span>Description</span>
            <input
              aria-label="Preset description"
              onChange={(event) => setDescription(event.target.value)}
              placeholder="description"
              value={description}
            />
          </label>
          <label>
            <span>Definition JSON</span>
            <textarea
              aria-label="Preset definition JSON"
              onChange={(event) => setDefinitionText(event.target.value)}
              value={definitionText}
            />
          </label>
          <button
            className="secondaryAction"
            disabled={pending || !createName.trim() || (createScope === "vps_local" && !ownerClientId)}
            type="submit"
          >
            Save preset
          </button>
        </form>

        <form className="compactForm presetForm" onSubmit={submitAssignment}>
          <strong>Assign selected preset</strong>
          <span className="formHint">
            Apply one domain preset to a selector-resolved VPS set; preview target count before confirmation.
          </span>
          <div className="formRow presetFormRow">
            <label>
              <span>Domain</span>
              <select aria-label="Assignment domain" onChange={(event) => changeAssignDomain(event.target.value)} value={assignDomain}>
                {DATA_SOURCE_DOMAINS.map((domain) => (
                  <option key={domain} value={domain}>
                    {domain}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Preset</span>
              <select aria-label="Preset" onChange={(event) => setAssignPresetId(event.target.value)} value={effectivePresetId}>
                {assignablePresets.map((preset) => (
                  <option key={preset.id} value={preset.id}>
                    {preset.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <div className="targetSelector presetTargetSelector">
            <div className="targetSelectorHeader">
              <strong>Targets</strong>
              <span>
                {assignmentSelectorParse.error ??
                  `${assignmentTargetCount}/${agents.length} matching VPSs`}
              </span>
            </div>
            <SearchExpressionInput
              agents={agents}
              ariaLabel="Data-source assignment target expression"
              className="targetExpressionBar"
              onChange={setAssignmentSelectorExpression}
              placeholder="id:edge-a || provider:alpha && country:us"
              showMatchCount
              value={assignmentSelectorExpression}
              verification={
                assignmentSelectorParse.error
                  ? "invalid"
                  : assignmentSelectorExpression.trim()
                    ? "valid"
                    : "neutral"
              }
              verificationMessage={
                assignmentSelectorParse.error ??
                `${assignmentTargetCount}/${agents.length}`
              }
            />
          </div>
          {pendingConfirmation !== "assignment" && (
            <button
              className="secondaryAction"
              disabled={
                pending ||
                !effectivePresetId ||
                assignmentTargetCount === 0 ||
                Boolean(assignmentSelectorParse.error)
              }
              type="submit"
            >
              Assign preset
            </button>
          )}
        </form>

        <form className="compactForm presetForm" onSubmit={previewHotConfig}>
          <strong>Render selected config</strong>
          <span className="formHint">
            Preview the generated redacted hot-config patch for one VPS before applying it.
          </span>
          <label>
            <span>Preview VPS</span>
            <select
              aria-label="Hot-config preview VPS"
              onChange={(event) => setRenderClientId(event.target.value)}
              value={renderClientId}
            >
              <option value="">VPS</option>
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {formatVpsName(agent, vpsNameDisplayMode)}
                  </option>
                ))}
            </select>
          </label>
          <button className="secondaryAction" disabled={pending || !renderClientId} type="submit">
            Render config
          </button>
          {renderedHotConfig && (
            <div className="configPreview">
              <div className="previewMeta">
                <span>{renderedHotConfig.assignments.length} selected presets</span>
                <span>{renderedHotConfig.unsupported_domains.length} notes</span>
              </div>
              <textarea aria-label="Rendered data-source hot-config TOML" readOnly value={renderedHotConfig.toml} />
            </div>
          )}
          <div className="inlinePrivilege">
            <label>
              <span>Apply timeout seconds</span>
              <input
                aria-label="Data-source apply timeout"
                min={1}
                max={3600}
                onChange={(event) => setApplyTimeoutSecs(Number(event.target.value))}
                type="number"
                value={applyTimeoutSecs}
              />
            </label>
          </div>
          <PrivilegeVaultBox
            labelPrefix="Data-source"
            lastPayloadHash={lastApplyPayloadHash}
            onOpenUnlock={onOpenPrivilegeUnlock}
            onPrivilegeMaterialChange={setPrivilegeMaterial}
            privilegeMaterial={privilegeMaterial}
            unlockRedirectLabel="Unlock data-source privilege"
          />
          {pendingConfirmation !== "apply" && (
            <button
              className="secondaryAction"
              disabled={pending || !renderClientId || !privilegeMaterial}
              onClick={confirmApplyRenderedHotConfig}
              type="button"
            >
              Apply selected patch
            </button>
          )}
          {lastApplyJob && <span>Job {shortId(lastApplyJob.job_id)} accepted</span>}
        </form>

        <form className="compactForm presetForm" onSubmit={(event) => event.preventDefault()}>
          <strong>Preset lifecycle</strong>
          <span className="formHint">
            Diff, test, clone, or update a saved preset. Updates report affected VPS count before commit.
          </span>
          <div className="formRow presetFormRow">
            <label>
              <span>Preset</span>
              <select
                aria-label="Lifecycle preset"
                onChange={(event) => setLifecyclePresetId(event.target.value)}
                value={effectiveLifecyclePresetId}
              >
                {presets.map((preset) => (
                  <option key={preset.id} value={preset.id}>
                    {preset.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Clone name</span>
              <input
                aria-label="Clone preset name"
                onChange={(event) => setLifecycleCloneName(event.target.value)}
                placeholder="shared:copy"
                value={lifecycleCloneName}
              />
            </label>
          </div>
          <label>
            <span>Description</span>
            <input
              aria-label="Lifecycle preset description"
              onChange={(event) => setLifecycleDescription(event.target.value)}
              placeholder="description"
              value={lifecycleDescription}
            />
          </label>
          <label>
            <span>Definition JSON</span>
            <textarea
              aria-label="Lifecycle preset definition JSON"
              onChange={(event) => setLifecycleDefinitionText(event.target.value)}
              value={lifecycleDefinitionText}
            />
          </label>
          <div className="formRow presetLifecycleActions">
            <button className="secondaryAction" disabled={pending || !lifecyclePreset} onClick={diffLifecyclePreset} type="button">
              Diff
            </button>
            <button className="secondaryAction" disabled={pending || !lifecyclePreset} onClick={testLifecyclePreset} type="button">
              Test
            </button>
            <button
              className="secondaryAction"
              disabled={pending || !lifecyclePreset || !lifecycleCloneName.trim()}
              onClick={cloneLifecyclePreset}
              type="button"
            >
              Clone
            </button>
            {pendingConfirmation !== "lifecycle-update" && (
              <button
                className="secondaryAction"
                disabled={pending || !lifecyclePreset || lifecyclePreset.built_in}
                onClick={updateLifecyclePreset}
                type="button"
              >
                Update
              </button>
            )}
          </div>
          {(lastDiff || lastTest) && (
            <div className="configPreview lifecyclePreview">
              {lastDiff && (
                <div className="previewMeta">
                  <span>{lastDiff.affected_client_count} assigned VPSs</span>
                  <span>{lastDiff.changed_keys.length ? lastDiff.changed_keys.join(", ") : "no definition changes"}</span>
                </div>
              )}
              {lastTest && (
                <>
                  <div className="previewMeta">
                    <span>{lastTest.valid ? "valid" : "invalid"}</span>
                    <span>{lastTest.renderable ? "hot-config renderable" : "workflow-managed"}</span>
                  </div>
                  {lastTest.toml && <textarea aria-label="Tested preset TOML" readOnly value={lastTest.toml} />}
                  {lastTest.error && <span>{lastTest.error}</span>}
                </>
              )}
            </div>
          )}
        </form>
      </div>
      )}

      {showSourceStatus && (
      <div className="sourceStatusSection">
        <div className="sectionHeader compact">
          <h2>Active source status</h2>
          <span>{sourceStatusSummary}</span>
        </div>
        <CrudPager
          fields={[
            { label: "VPS", value: (row) => formatVpsName(row, vpsNameDisplayMode) },
            { label: "Module", value: (row) => `${row.module} ${row.domain}` },
            { label: "Preset", value: (row) => row.preset_name },
            { label: "Source", value: (row) => row.source_kind },
            { label: "Status", value: (row) => `${row.status} ${row.status_reason}` },
          ]}
          itemLabel="sources"
          items={dataSourceStatus}
          pageSize={10}
          title="Active sources"
          empty={
            <div className="emptyState">
              <DatabaseZap size={22} />
              <strong>Active source status</strong>
              <span>No selected source records match the current search.</span>
            </div>
          }
        >
          {(sourceStatusRows) => (
            <div className="table hierarchyTable">
              <div className="historyRow heading dataSourceStatusGrid">
                <span>VPS</span>
                <span>Module</span>
                <span>Preset</span>
                <span>Source</span>
                <span>Status</span>
                <span>Evidence</span>
              </div>
              {sourceStatusRows.map((row) => (
                <div className="historyRow dataSourceStatusGrid" key={`${row.client_id}:${row.domain}`}>
                  <span className="historyPrimary">
                    <strong>{formatVpsName(row, vpsNameDisplayMode)}</strong>
                    <small>{row.client_status}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{row.module}</strong>
                    <small>{row.domain}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{row.preset_name}</strong>
                    <small>{row.preset_scope}</small>
                  </span>
                  <span>{row.source_kind}</span>
                  <span className={`status ${statusClass(row.status)}`} title={row.status_reason}>
                    {row.status}
                  </span>
                  <span>{sourceEvidenceSummary(row)}</span>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
      </div>
      )}

      {showPresetManagement && (
      <CrudPager
        fields={[
          { label: "Preset", value: (preset) => preset.name },
          { label: "Domain", value: (preset) => preset.domain },
          { label: "Scope", value: (preset) => preset.scope },
          { label: "Assigned", value: (preset) => preset.assigned_client_count },
        ]}
        itemLabel="presets"
        items={presets}
        pageSize={10}
        title="Preset registry"
        empty={
          <div className="emptyState">
            <DatabaseZap size={22} />
            <strong>No data-source presets</strong>
            <span>{actionError ?? "No preset records match the current search."}</span>
          </div>
        }
      >
        {(presetRows) => (
          <div className="table hierarchyTable">
            <div className="historyRow heading dataSourcePresetGrid">
              <span>Preset</span>
              <span>Domain</span>
              <span>Scope</span>
              <span>Assigned</span>
              <span>Updated</span>
            </div>
            {presetRows.map((preset) => (
              <div className="historyRow dataSourcePresetGrid" key={preset.id}>
                <span className="historyPrimary">
                  <strong>{preset.name}</strong>
                  <small>{preset.description ?? (preset.built_in ? "built-in" : "custom")}</small>
                </span>
                <span>{preset.domain}</span>
                <span className={`status ${preset.is_default ? "info" : preset.built_in ? "neutral" : "ok"}`}>
                  {preset.is_default ? "default" : preset.scope}
                </span>
                <span>{preset.assigned_client_count}</span>
                <span>{formatTime(preset.updated_at)}</span>
              </div>
            ))}
          </div>
        )}
      </CrudPager>
      )}

      {showPresetManagement && (
      <div className="timeline presetAssignmentSummary">
        <SlidersHorizontal size={18} />
        <div>
          <strong>{assignments.length} selected preset records</strong>
          <span>{assignmentSummary(assignments, lastAssignment)}</span>
        </div>
      </div>
      )}
    </section>
  );
}

function defaultCloneName(name: string): string {
  if (name.startsWith("builtin:")) {
    return `shared:${name.slice("builtin:".length)}`;
  }
  return `${name}.copy`;
}

function parseDefinition(value: string): JsonValue {
  const parsed = JSON.parse(value) as JsonValue;
  if (parsed === null || Array.isArray(parsed) || typeof parsed !== "object") {
    throw new Error("Preset definition must be a JSON object");
  }
  return parsed;
}

function assignmentSummary(
  assignments: DataSourcePresetAssignmentRecord[],
  lastAssignment: AssignDataSourcePresetResponse | null,
): string {
  if (lastAssignment?.confirmation_required) {
    return "Confirmation required before changing multiple VPS preset selections";
  }
  const domains = new Set(assignments.map((assignment) => assignment.domain));
  return domains.size === 0 ? "No VPS preset assignments loaded" : `${domains.size} domains with explicit VPS selections`;
}

function sourceEvidenceSummary(row: DataSourceStatusRecord): string {
  const evidence = row.evidence;
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) {
    return row.status_reason;
  }
  const sampleCount = typeof evidence.sample_count === "number" ? evidence.sample_count : null;
  const promotionRequired = typeof evidence.promotion_required === "number" ? evidence.promotion_required : null;
  const degradedCount = typeof evidence.degraded_count === "number" ? evidence.degraded_count : null;
  const objectStoreConfigured =
    typeof evidence.server_object_store_configured === "boolean" ? evidence.server_object_store_configured : null;
  const objectStoreKind =
    typeof evidence.server_object_store_kind === "string" ? evidence.server_object_store_kind : null;
  const artifactCount = typeof evidence.artifact_count === "number" ? evidence.artifact_count : null;
  const releaseCount = typeof evidence.release_count === "number" ? evidence.release_count : null;
  const hostedReleaseCount = typeof evidence.hosted_release_count === "number" ? evidence.hosted_release_count : null;
  const externalReleaseCount =
    typeof evidence.external_release_count === "number" ? evidence.external_release_count : null;
  const backupRequestCount = typeof evidence.backup_request_count === "number" ? evidence.backup_request_count : null;
  const restoreSourceCount = typeof evidence.restore_source_count === "number" ? evidence.restore_source_count : null;
  const restoreTargetCount = typeof evidence.restore_target_count === "number" ? evidence.restore_target_count : null;
  const migrationSourceCount =
    typeof evidence.migration_source_count === "number" ? evidence.migration_source_count : null;
  const migrationTargetCount =
    typeof evidence.migration_target_count === "number" ? evidence.migration_target_count : null;
  const probeSampleCount = typeof evidence.probe_sample_count === "number" ? evidence.probe_sample_count : null;
  const speedSampleCount = typeof evidence.speed_sample_count === "number" ? evidence.speed_sample_count : null;
  const routingRecommendationCount =
    typeof evidence.routing_recommendation_count === "number" ? evidence.routing_recommendation_count : null;
  const ospfUpdateCandidateCount =
    typeof evidence.ospf_update_candidate_count === "number" ? evidence.ospf_update_candidate_count : null;
  const trafficLimitPlanCount =
    typeof evidence.traffic_limit_plan_count === "number" ? evidence.traffic_limit_plan_count : null;
  const workflow = typeof evidence.workflow === "string" ? evidence.workflow : null;
  const privilegeGated = typeof evidence.privilege_gated === "boolean" ? evidence.privilege_gated : null;
  const environmentPolicy = typeof evidence.environment_policy === "string" ? evidence.environment_policy : null;
  const ptyPolicy = typeof evidence.pty_policy === "string" ? evidence.pty_policy : null;
  const processCleanup = typeof evidence.process_cleanup === "string" ? evidence.process_cleanup : null;
  const configuredPing = typeof evidence.configured_ping_argv === "boolean" ? evidence.configured_ping_argv : null;
  const customCommand = typeof evidence.custom_command_configured === "boolean" ? evidence.custom_command_configured : null;
  const requiresTwoEndpoints = typeof evidence.requires_two_endpoints === "boolean" ? evidence.requires_two_endpoints : null;
  const privilegeMode = typeof evidence.privilege_mode === "string" ? evidence.privilege_mode : null;
  const processLimitsStatus =
    typeof evidence.process_limits_status === "string" ? evidence.process_limits_status : null;
  const canApplyProcessLimits =
    typeof evidence.can_apply_process_limits === "boolean" ? evidence.can_apply_process_limits : null;
  const parts = [];
  if (workflow) {
    parts.push(formatSourceToken(workflow));
  }
  if (privilegeGated) {
    parts.push("privilege-unlocked");
  }
  if (environmentPolicy) {
    parts.push(`${environmentPolicy} env`);
  }
  if (ptyPolicy) {
    parts.push(`${formatSourceToken(ptyPolicy)} PTY`);
  }
  if (processCleanup) {
    parts.push(`${formatSourceToken(processCleanup)} cleanup`);
  }
  if (configuredPing) {
    parts.push("configured ping");
  }
  if (customCommand) {
    parts.push("custom command");
  }
  if (requiresTwoEndpoints) {
    parts.push("paired endpoints");
  }
  if (privilegeMode) {
    parts.push(formatSourceToken(privilegeMode));
  }
  if (processLimitsStatus) {
    parts.push(
      canApplyProcessLimits === true
        ? "process limits available"
        : `${formatSourceToken(processLimitsStatus)} process limits`,
    );
  }
  if (objectStoreConfigured !== null) {
    parts.push(objectStoreConfigured ? `${objectStoreKind ?? "configured"} store` : "no server store");
  }
  if (artifactCount !== null) {
    parts.push(`${artifactCount} artifacts`);
  }
  if (releaseCount !== null) {
    parts.push(`${releaseCount} releases`);
  }
  if (hostedReleaseCount !== null && hostedReleaseCount > 0) {
    parts.push(`${hostedReleaseCount} hosted`);
  }
  if (externalReleaseCount !== null && externalReleaseCount > 0) {
    parts.push(`${externalReleaseCount} external`);
  }
  if (backupRequestCount !== null && backupRequestCount > 0) {
    parts.push(`${backupRequestCount} backup requests`);
  }
  if (restoreSourceCount !== null && restoreSourceCount > 0) {
    parts.push(`${restoreSourceCount} source restores`);
  }
  if (restoreTargetCount !== null && restoreTargetCount > 0) {
    parts.push(`${restoreTargetCount} target restores`);
  }
  if (migrationSourceCount !== null && migrationSourceCount > 0) {
    parts.push(`${migrationSourceCount} source migrations`);
  }
  if (migrationTargetCount !== null && migrationTargetCount > 0) {
    parts.push(`${migrationTargetCount} target migrations`);
  }
  if (probeSampleCount !== null && probeSampleCount > 0) {
    parts.push(`${probeSampleCount} probe samples`);
  }
  if (speedSampleCount !== null && speedSampleCount > 0) {
    parts.push(`${speedSampleCount} speed samples`);
  }
  if (routingRecommendationCount !== null && routingRecommendationCount > 0) {
    parts.push(`${routingRecommendationCount} routing recommendations`);
  }
  if (ospfUpdateCandidateCount !== null && ospfUpdateCandidateCount > 0) {
    parts.push(`${ospfUpdateCandidateCount} OSPF updates`);
  }
  if (trafficLimitPlanCount !== null && trafficLimitPlanCount > 0) {
    parts.push(`${trafficLimitPlanCount} traffic limit plans`);
  }
  if (sampleCount !== null) {
    parts.push(`${sampleCount} samples`);
  }
  if (promotionRequired !== null && promotionRequired > 0) {
    parts.push(`${promotionRequired} promotion`);
  }
  if (degradedCount !== null && degradedCount > 0) {
    parts.push(`${degradedCount} degraded`);
  }
  return parts.length > 0 ? parts.join(", ") : row.status_reason;
}

function formatSourceToken(value: string): string {
  return value.replace(/_/g, " ");
}

function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.min(max, Math.max(min, Math.trunc(value)));
}

function readLocalString(key: string, fallback: string): string {
  if (typeof window === "undefined") {
    return fallback;
  }
  return window.localStorage.getItem(key) ?? fallback;
}

function writeLocalString(key: string, value: string) {
  if (typeof window === "undefined") {
    return;
  }
  if (value.trim()) {
    window.localStorage.setItem(key, value);
  } else {
    window.localStorage.removeItem(key);
  }
}
