import { useState, type FormEvent } from "react";
import { ShieldCheck } from "lucide-react";
import { usePanelDisplaySettings } from "../panelDisplay";
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
import { formatVpsName, runPanelAction, toggleValue } from "../utils";
import { CrudPager } from "../components/CrudPager";
import { DataSourcePresetPanel } from "./DataSourcePresetPanel";

export function TagsPanel({
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
  onRefresh,
  onRenderDataSourceHotConfig,
  onResolveBulk,
  onTestDataSourcePreset,
  onUpdateDataSourcePreset,
  dataSourceAssignments,
  dataSourcePresets,
  dataSourceStatus,
  tags,
}: {
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
  onRefresh: () => void;
  onRenderDataSourceHotConfig: (clientId: string) => Promise<DataSourceHotConfigResponse>;
  onResolveBulk: (tagNames: string[], destructive: boolean, tagMode: "any" | "all") => Promise<BulkResolveResponse>;
  onTestDataSourcePreset: (presetId: string, request: DataSourcePresetTestRequest) => Promise<DataSourcePresetTestResponse>;
  onUpdateDataSourcePreset: (presetId: string, request: UpdateDataSourcePresetRequest) => Promise<UpdateDataSourcePresetResponse>;
  tags: TagView[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [tagName, setTagName] = useState("");
  const [targetClient, setTargetClient] = useState("");
  const [targetTag, setTargetTag] = useState("");
  const [selectedTags, setSelectedTags] = useState<string[]>([]);
  const [tagMode, setTagMode] = useState<"any" | "all">("any");
  const [destructive, setDestructive] = useState(false);
  const [bulkPreview, setBulkPreview] = useState<BulkResolveResponse | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

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
    await runPanelAction(setPending, setActionError, async () => {
      setBulkPreview(await onResolveBulk(selectedTags, destructive, tagMode));
    });
  }

  const status = actionError ?? error ?? (loading ? "Refreshing tag state" : "Live tag targets");

  return (
    <section className="workspace">
      <div className="workspaceStack">
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

        <DataSourcePresetPanel
          agents={agents}
          assignments={dataSourceAssignments}
          dataSourceStatus={dataSourceStatus}
          onAssignPreset={onAssignDataSourcePreset}
          onClonePreset={onCloneDataSourcePreset}
          onCreateJob={onCreateJob}
          onCreatePreset={onCreateDataSourcePreset}
          onDiffPreset={onDiffDataSourcePreset}
          onRenderHotConfig={onRenderDataSourceHotConfig}
          onTestPreset={onTestDataSourcePreset}
          onUpdatePreset={onUpdateDataSourcePreset}
          presets={dataSourcePresets}
          tags={tags}
        />
      </div>

      <aside className="inspector">
        <div className="sectionHeader compact">
          <h2>Targeting</h2>
          <span>Resolve before dispatch</span>
        </div>

        <form className="sideForm" onSubmit={submitAssignments}>
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

        <div className="sideForm">
          <strong>Bulk preview</strong>
          <div className="chipList">
            {tags.map((tag) => (
              <label className="checkChip" key={tag.name}>
                <input
                  checked={selectedTags.includes(tag.name)}
                  onChange={() => setSelectedTags(toggleValue(selectedTags, tag.name))}
                  type="checkbox"
                />
                <span>{tag.name}</span>
              </label>
            ))}
          </div>
          <label className="checkLine">
            <input checked={destructive} onChange={(event) => setDestructive(event.target.checked)} type="checkbox" />
            <span>Destructive operation</span>
          </label>
          <div className="targetModeControls" role="group" aria-label="Bulk tag match mode">
            <span>Tags</span>
            <button className={tagMode === "any" ? "selected" : ""} onClick={() => setTagMode("any")} type="button">
              Any
            </button>
            <button className={tagMode === "all" ? "selected" : ""} onClick={() => setTagMode("all")} type="button">
              All
            </button>
          </div>
          <button
            className="wideAction"
            disabled={pending || selectedTags.length === 0}
            onClick={previewBulk}
            type="button"
          >
            Preview targets
          </button>
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
      </aside>
    </section>
  );
}
