import { useEffect, useState, type FormEvent } from "react";
import { ShieldCheck } from "lucide-react";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { usePanelDisplaySettings } from "../panelDisplay";
import { parseSearchExpression } from "../searchExpression";
import type {
  AgentView,
  AssignDataSourcePresetRequest,
  AssignDataSourcePresetResponse,
  BulkResolveResponse,
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
  TagView,
  UpdateDataSourcePresetRequest,
  UpdateDataSourcePresetResponse,
} from "../types";
import { formatVpsName, runPanelAction } from "../utils";
import { CrudPager } from "../components/CrudPager";
import { DataSourcePresetPanel } from "./DataSourcePresetPanel";
import type { ProofMaterial } from "../proof";

const TAG_TARGET_SELECTOR_STORAGE_KEY = "vpsman.tagsTargeting.selectorExpression";

function readLocalString(key: string): string {
  if (typeof window === "undefined") {
    return "";
  }
  try {
    return window.localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function writeLocalString(key: string, value: string) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    if (value.trim()) {
      window.localStorage.setItem(key, value);
    } else {
      window.localStorage.removeItem(key);
    }
  } catch {
    // Browser-local targeting persistence must not block tag management.
  }
}

export function TagsPanel({
  activeSubpage,
  agents,
  error,
  loading,
  onAssignDataSourcePreset,
  onAssignTag,
  onCloneDataSourcePreset,
  onCreateJob,
  onCreateDataSourcePreset,
  onCreateTag,
  onDiffDataSourcePreset,
  onOpenProofUnlock,
  onRefresh,
  onRenderDataSourceHotConfig,
  onResolveBulk,
  onTestDataSourcePreset,
  onUpdateDataSourcePreset,
  proofMaterial,
  setProofMaterial,
  dataSourceAssignments,
  dataSourcePresets,
  dataSourceStatus,
  tags,
}: {
  activeSubpage: string;
  agents: AgentView[];
  dataSourceAssignments: DataSourcePresetAssignmentRecord[];
  dataSourcePresets: DataSourcePresetRecord[];
  dataSourceStatus: DataSourceStatusRecord[];
  error: string | null;
  loading: boolean;
  onAssignDataSourcePreset: (request: AssignDataSourcePresetRequest) => Promise<AssignDataSourcePresetResponse>;
  onAssignTag: (clientId: string, tag: string) => Promise<void>;
  onCloneDataSourcePreset: (presetId: string, request: CloneDataSourcePresetRequest) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateDataSourcePreset: (request: CreateDataSourcePresetRequest) => Promise<void>;
  onCreateTag: (name: string) => Promise<void>;
  onDiffDataSourcePreset: (presetId: string, request: DataSourcePresetDiffRequest) => Promise<DataSourcePresetDiffResponse>;
  onOpenProofUnlock: () => void;
  onRefresh: () => void;
  onRenderDataSourceHotConfig: (clientId: string) => Promise<DataSourceHotConfigResponse>;
  onResolveBulk: (selectorExpression: string, destructive: boolean, confirmed?: boolean) => Promise<BulkResolveResponse>;
  onTestDataSourcePreset: (presetId: string, request: DataSourcePresetTestRequest) => Promise<DataSourcePresetTestResponse>;
  onUpdateDataSourcePreset: (presetId: string, request: UpdateDataSourcePresetRequest) => Promise<UpdateDataSourcePresetResponse>;
  proofMaterial: ProofMaterial | null;
  setProofMaterial: (material: ProofMaterial | null) => void;
  tags: TagView[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [tagName, setTagName] = useState("");
  const [targetClient, setTargetClient] = useState("");
  const [targetTag, setTargetTag] = useState("");
  const [selectorExpression, setSelectorExpression] = useState(() => readLocalString(TAG_TARGET_SELECTOR_STORAGE_KEY));
  const [destructive, setDestructive] = useState(false);
  const [bulkPreview, setBulkPreview] = useState<BulkResolveResponse | null>(null);
  const [selectorVerification, setSelectorVerification] = useState<"checking" | "invalid" | "neutral" | "valid">("neutral");
  const [selectorVerificationMessage, setSelectorVerificationMessage] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const tagSubpage = ["registry", "targeting", "presets", "status"].includes(activeSubpage)
    ? activeSubpage
    : "registry";

  async function submitTag(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      await onCreateTag(tagName.trim());
      setTagName("");
    });
  }

  async function submitAssignments(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (targetClient && targetTag) {
        await onAssignTag(targetClient, targetTag);
      }
      setTargetTag("");
    });
  }

  async function previewBulk() {
    const parsed = parseSearchExpression(selectorExpression);
    if (parsed.error) {
      setActionError(parsed.error);
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      setBulkPreview(await onResolveBulk(selectorExpression.trim(), destructive));
    });
  }

  useEffect(() => {
    writeLocalString(TAG_TARGET_SELECTOR_STORAGE_KEY, selectorExpression);
  }, [selectorExpression]);

  useEffect(() => {
    if (!selectorExpression.trim()) {
      setSelectorVerification("neutral");
      setSelectorVerificationMessage(null);
      setBulkPreview(null);
      return;
    }
    const parsed = parseSearchExpression(selectorExpression);
    if (parsed.error) {
      setSelectorVerification("invalid");
      setSelectorVerificationMessage("Invalid");
      setBulkPreview(null);
      return;
    }
    let canceled = false;
    setSelectorVerification("checking");
    setSelectorVerificationMessage("Checking");
    const timeout = window.setTimeout(() => {
      void onResolveBulk(selectorExpression.trim(), destructive)
        .then((response) => {
          if (canceled) {
            return;
          }
          setBulkPreview(response);
          setSelectorVerification("valid");
          setSelectorVerificationMessage(`${response.target_count}/${agents.length}`);
        })
        .catch(() => {
          if (canceled) {
            return;
          }
          setBulkPreview(null);
          setSelectorVerification("invalid");
          setSelectorVerificationMessage("Invalid");
        });
    }, 300);
    return () => {
      canceled = true;
      window.clearTimeout(timeout);
    };
  }, [agents.length, destructive, onResolveBulk, selectorExpression]);

  const status = actionError ?? error ?? (loading ? "Refreshing tag state" : "Live tag targets");

  return (
    <section className="workspace singleColumn">
      {tagSubpage === "registry" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Tags</h2>
              <span>{status}</span>
            </div>
            <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
              Refresh
            </button>
          </div>

          <div className="managementGrid">
            <form className="compactForm" onSubmit={submitTag}>
              <strong>Create tag</strong>
              <div className="formRow">
                <input
                  aria-label="Tag name"
                  onChange={(event) => setTagName(event.target.value)}
                  placeholder="provider:alpha, country:us, app:edge"
                  value={tagName}
                />
                <button className="secondaryAction" disabled={pending || !tagName.trim()} type="submit">
                  Create
                </button>
              </div>
            </form>
          </div>

          <CrudPager
            fields={[
              { label: "Tag", value: (tag) => tag.name },
              { label: "Clients", value: (tag) => tag.clients.length },
            ]}
            itemLabel="tags"
            items={tags}
            pageSize={8}
            title="Tag records"
            empty={
              <div className="emptyState">
                <ShieldCheck size={22} />
                <strong>No tags</strong>
                <span>Create provider, country, or custom tags to target recurring VPS groups.</span>
              </div>
            }
          >
            {(tagRows) => (
              <div className="table hierarchyTable">
                <div className="historyRow heading tagGrid">
                  <span>Tag</span>
                  <span>Clients</span>
                </div>
                {tagRows.map((tag) => (
                  <div className="historyRow tagGrid" key={tag.name}>
                    <span className="tags">
                      <em>{tag.name}</em>
                    </span>
                    <span>{tag.clients.length}</span>
                  </div>
                ))}
              </div>
            )}
          </CrudPager>
        </div>
      )}

      {(tagSubpage === "presets" || tagSubpage === "status") && (
        <DataSourcePresetPanel
          activeSubpage={tagSubpage}
          agents={agents}
          assignments={dataSourceAssignments}
          dataSourceStatus={dataSourceStatus}
          onAssignPreset={onAssignDataSourcePreset}
          onClonePreset={onCloneDataSourcePreset}
          onCreateJob={onCreateJob}
          onCreatePreset={onCreateDataSourcePreset}
          onDiffPreset={onDiffDataSourcePreset}
          onOpenProofUnlock={onOpenProofUnlock}
          onRenderHotConfig={onRenderDataSourceHotConfig}
          onTestPreset={onTestDataSourcePreset}
          onUpdatePreset={onUpdateDataSourcePreset}
          proofMaterial={proofMaterial}
          presets={dataSourcePresets}
          setProofMaterial={setProofMaterial}
        />
      )}

      {tagSubpage === "targeting" && (
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
          <h2>Targeting</h2>
          <span>Resolve before dispatch</span>
          </div>
        </div>

        <div className="managementGrid">
        <form className="compactForm" onSubmit={submitAssignments}>
          <strong>Assign VPS</strong>
          <select aria-label="VPS" onChange={(event) => setTargetClient(event.target.value)} value={targetClient}>
            <option value="">VPS</option>
            {agents.map((agent) => (
              <option key={agent.id} value={agent.id}>
                {formatVpsName(agent, vpsNameDisplayMode)}
              </option>
            ))}
          </select>
          <input
            aria-label="Tag to assign"
            onChange={(event) => setTargetTag(event.target.value)}
            placeholder="tag"
            value={targetTag}
          />
          <button className="wideAction" disabled={pending || !targetClient || !targetTag} type="submit">
            Apply
          </button>
        </form>

        <div className="compactForm">
          <strong>Bulk preview</strong>
          <SearchExpressionInput
            agents={agents}
            ariaLabel="Tag targeting selector expression"
            className="compact"
            onChange={(value) => {
              setSelectorExpression(value);
              setBulkPreview(null);
            }}
            placeholder="provider:* && country:US"
            showMatchCount
            value={selectorExpression}
            verification={selectorVerification}
            verificationMessage={selectorVerificationMessage}
          />
          <label className="checkLine">
            <input checked={destructive} onChange={(event) => setDestructive(event.target.checked)} type="checkbox" />
            <span>Destructive operation</span>
          </label>
          <button
            className="wideAction"
            disabled={pending || !selectorExpression.trim() || selectorVerification === "invalid"}
            onClick={previewBulk}
            type="button"
          >
            Preview targets
          </button>
        </div>
        </div>

        <div className="timeline">
          <ShieldCheck size={18} />
          <div>
            <strong>{bulkPreview ? `${bulkPreview.target_count} targets` : "No preview"}</strong>
            <span>
              {bulkPreview?.confirmation_required
                ? "Destructive dispatch requires confirmation"
                : "Resolved target set appears here"}
            </span>
          </div>
        </div>
      </div>
      )}
    </section>
  );
}
