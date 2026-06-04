import { useState, type FormEvent } from "react";
import { Layers3, ShieldCheck } from "lucide-react";
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
  ResourcePoolView,
  TagView,
  UpdateDataSourcePresetRequest,
  UpdateDataSourcePresetResponse,
} from "../types";
import { runPanelAction, toggleValue } from "../utils";
import { CrudPager } from "../components/CrudPager";
import { DataSourcePresetPanel } from "./DataSourcePresetPanel";

export function PoolsTagsPanel({
  agents,
  error,
  loading,
  onAssignPool,
  onAssignDataSourcePreset,
  onAssignTag,
  onCloneDataSourcePreset,
  onCreateJob,
  onCreateDataSourcePreset,
  onCreatePool,
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
  pools,
  tags,
}: {
  agents: AgentView[];
  dataSourceAssignments: DataSourcePresetAssignmentRecord[];
  dataSourcePresets: DataSourcePresetRecord[];
  dataSourceStatus: DataSourceStatusRecord[];
  error: string | null;
  loading: boolean;
  onAssignDataSourcePreset: (request: AssignDataSourcePresetRequest) => Promise<AssignDataSourcePresetResponse>;
  onAssignPool: (clientId: string, poolId: string) => Promise<void>;
  onAssignTag: (clientId: string, tag: string) => Promise<void>;
  onCloneDataSourcePreset: (presetId: string, request: CloneDataSourcePresetRequest) => Promise<void>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateDataSourcePreset: (request: CreateDataSourcePresetRequest) => Promise<void>;
  onCreatePool: (name: string, provider: string, region: string) => Promise<void>;
  onCreateTag: (name: string) => Promise<void>;
  onDiffDataSourcePreset: (presetId: string, request: DataSourcePresetDiffRequest) => Promise<DataSourcePresetDiffResponse>;
  onRefresh: () => void;
  onRenderDataSourceHotConfig: (clientId: string) => Promise<DataSourceHotConfigResponse>;
  onResolveBulk: (
    poolIds: string[],
    tagNames: string[],
    destructive: boolean,
    tagMode: "any" | "all",
  ) => Promise<BulkResolveResponse>;
  onTestDataSourcePreset: (presetId: string, request: DataSourcePresetTestRequest) => Promise<DataSourcePresetTestResponse>;
  onUpdateDataSourcePreset: (presetId: string, request: UpdateDataSourcePresetRequest) => Promise<UpdateDataSourcePresetResponse>;
  pools: ResourcePoolView[];
  tags: TagView[];
}) {
  const [poolName, setPoolName] = useState("");
  const [poolProvider, setPoolProvider] = useState("");
  const [poolRegion, setPoolRegion] = useState("");
  const [tagName, setTagName] = useState("");
  const [targetClient, setTargetClient] = useState("");
  const [targetPool, setTargetPool] = useState("");
  const [targetTag, setTargetTag] = useState("");
  const [selectedPools, setSelectedPools] = useState<string[]>([]);
  const [selectedTags, setSelectedTags] = useState<string[]>([]);
  const [tagMode, setTagMode] = useState<"any" | "all">("any");
  const [destructive, setDestructive] = useState(false);
  const [bulkPreview, setBulkPreview] = useState<BulkResolveResponse | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  async function submitPool(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      await onCreatePool(poolName.trim(), poolProvider.trim(), poolRegion.trim());
      setPoolName("");
      setPoolProvider("");
      setPoolRegion("");
    });
  }

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
      if (targetClient && targetPool) {
        await onAssignPool(targetClient, targetPool);
      }
      if (targetClient && targetTag) {
        await onAssignTag(targetClient, targetTag);
      }
      setTargetTag("");
    });
  }

  async function previewBulk() {
    await runPanelAction(setPending, setActionError, async () => {
      setBulkPreview(await onResolveBulk(selectedPools, selectedTags, destructive, tagMode));
    });
  }

  const status = actionError ?? error ?? (loading ? "Refreshing pool and tag state" : "Live hierarchy and tag targets");

  return (
    <section className="workspace">
      <div className="workspaceStack">
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Pools and tags</h2>
              <span>{status}</span>
            </div>
            <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
              Refresh
            </button>
          </div>

          <div className="managementGrid">
            <form className="compactForm" onSubmit={submitPool}>
              <strong>Resource pool</strong>
              <div className="formRow">
                <input
                  aria-label="Pool name"
                  onChange={(event) => setPoolName(event.target.value)}
                  placeholder="pool name"
                  value={poolName}
                />
                <input
                  aria-label="Provider"
                  onChange={(event) => setPoolProvider(event.target.value)}
                  placeholder="provider"
                  value={poolProvider}
                />
                <input
                  aria-label="Region"
                  onChange={(event) => setPoolRegion(event.target.value)}
                  placeholder="region"
                  value={poolRegion}
                />
                <button className="secondaryAction" disabled={pending || !poolName.trim()} type="submit">
                  Create
                </button>
              </div>
            </form>

            <form className="compactForm" onSubmit={submitTag}>
              <strong>Custom tag</strong>
              <div className="formRow">
                <input
                  aria-label="Tag name"
                  onChange={(event) => setTagName(event.target.value)}
                  placeholder="tag name"
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
              { label: "Pool", value: (pool) => pool.name },
              { label: "Provider", value: (pool) => pool.provider },
              { label: "Region", value: (pool) => pool.region },
              { label: "Clients", value: (pool) => pool.clients.length },
            ]}
            itemLabel="pools"
            items={pools}
            pageSize={8}
            title="Pool records"
            empty={
              <div className="emptyState">
                <Layers3 size={22} />
                <strong>No resource pools</strong>
                <span>{error ?? "Create a provider or resource-pool parent node to group VPSs."}</span>
              </div>
            }
          >
            {(poolRows) => (
              <div className="table hierarchyTable">
                <div className="historyRow heading poolGrid">
                  <span>Pool</span>
                  <span>Provider</span>
                  <span>Region</span>
                  <span>Clients</span>
                </div>
                {poolRows.map((pool) => (
                  <div className="historyRow poolGrid" key={pool.id}>
                    <span className="historyPrimary">
                      <strong>{pool.name}</strong>
                      <small>{pool.provider || pool.region || "resource pool"}</small>
                    </span>
                    <span>{pool.provider || "-"}</span>
                    <span>{pool.region || "-"}</span>
                    <span>{pool.clients.length}</span>
                  </div>
                ))}
              </div>
            )}
          </CrudPager>

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
                <strong>No custom tags</strong>
                <span>Create tags to target recurring VPS groups.</span>
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
          pools={pools}
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
                {agent.display_name || agent.id}
              </option>
            ))}
          </select>
          <select aria-label="Pool" onChange={(event) => setTargetPool(event.target.value)} value={targetPool}>
            <option value="">Pool</option>
            {pools.map((pool) => (
              <option key={pool.id} value={pool.id}>
                {pool.name}
              </option>
            ))}
          </select>
          <input
            aria-label="Tag to assign"
            onChange={(event) => setTargetTag(event.target.value)}
            placeholder="tag"
            value={targetTag}
          />
          <button className="wideAction" disabled={pending || !targetClient || (!targetPool && !targetTag)} type="submit">
            Apply
          </button>
        </form>

        <div className="sideForm">
          <strong>Bulk preview</strong>
          <div className="chipList">
            {pools.map((pool) => (
              <label className="checkChip" key={pool.id}>
                <input
                  checked={selectedPools.includes(pool.id)}
                  onChange={() => setSelectedPools(toggleValue(selectedPools, pool.id))}
                  type="checkbox"
                />
                <span>{pool.name}</span>
              </label>
            ))}
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
            disabled={pending || (selectedPools.length === 0 && selectedTags.length === 0)}
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
