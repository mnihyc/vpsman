import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Plus, RefreshCw, ShieldCheck, Tag, Trash2, X } from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { CrudPager } from "../components/CrudPager";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { usePanelDisplaySettings } from "../panelDisplay";
import type {
  AgentView,
  BulkResolveResponse,
  BulkTagMutationRequest,
  TagMutationResponse,
  TagView,
} from "../types";
import { buildPrivilegeAssertion, canonicalDbPrivilegeIntent, type PrivilegeMaterial, type PrivilegeAssertion } from "../privilege";
import { parseSearchExpression, selectorExpressionForClientIds } from "../searchExpression";
import { formatVpsName, runPanelAction } from "../utils";

const TAG_BULK_SELECTOR_STORAGE_KEY = "vpsman.tags.bulk.selectorExpression";

export function TagsPanel({
  activeSubpage,
  agents,
  error,
  loading,
  onAssignTag,
  onBulkMutateTags,
  onCreateTag,
  onDeleteTag,
  onOpenPrivilegeUnlock,
  onOpenSchedules,
  onRefresh,
  onResolveBulk,
  privilegeMaterial,
  tags,
}: {
  activeSubpage: string;
  agents: AgentView[];
  error: string | null;
  loading: boolean;
  onAssignTag: (clientId: string, tag: string, privilegeAssertion: PrivilegeAssertion) => Promise<TagMutationResponse>;
  onBulkMutateTags: (request: BulkTagMutationRequest) => Promise<TagMutationResponse>;
  onCreateTag: (name: string, privilegeAssertion: PrivilegeAssertion) => Promise<void>;
  onDeleteTag: (tag: string, confirmed: boolean, privilegeAssertion?: PrivilegeAssertion | null) => Promise<TagMutationResponse>;
  onOpenPrivilegeUnlock: () => void;
  onOpenSchedules?: () => void;
  onRefresh: () => void;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
  tags: TagView[];
}) {
  const subpage = ["registry", "assignments", "bulk"].includes(activeSubpage) ? activeSubpage : "registry";
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [lastMutation, setLastMutation] = useState<TagMutationResponse | null>(null);
  const status =
    actionError ??
    error ??
    (lastMutation
      ? `${lastMutation.action} ${lastMutation.tag}: ${lastMutation.changed_count} changed, ${lastMutation.skipped_count} skipped`
      : loading
        ? "Refreshing tag state"
        : `${tags.length} tags across ${agents.length} VPSs`);

  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>{subpage === "bulk" ? "Bulk tags" : subpage === "assignments" ? "Tag assignments" : "Tags"}</h2>
            <span>{status}</span>
          </div>
          <button className="secondaryAction" disabled={loading || pending} onClick={onRefresh} type="button">
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
        {subpage === "registry" && (
          <TagRegistry
            onCreateTag={onCreateTag}
            onDeleteTag={onDeleteTag}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onOpenSchedules={onOpenSchedules}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
            setLastMutation={setLastMutation}
            tags={tags}
          />
        )}
        {subpage === "assignments" && (
          <TagAssignments
            agents={agents}
            onAssignTag={onAssignTag}
            onBulkMutateTags={onBulkMutateTags}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
            setLastMutation={setLastMutation}
            tags={tags}
          />
        )}
        {subpage === "bulk" && (
          <BulkTagPanel
            agents={agents}
            onBulkMutateTags={onBulkMutateTags}
            onDeleteTag={onDeleteTag}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onOpenSchedules={onOpenSchedules}
            onResolveBulk={onResolveBulk}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
            setLastMutation={setLastMutation}
            tags={tags}
          />
        )}
      </div>
    </section>
  );
}

function TagRegistry({
  onCreateTag,
  onDeleteTag,
  onOpenPrivilegeUnlock,
  onOpenSchedules,
  pending,
  privilegeMaterial,
  runAction,
  setLastMutation,
  tags,
}: {
  onCreateTag: (name: string, privilegeAssertion: PrivilegeAssertion) => Promise<void>;
  onDeleteTag: (tag: string, confirmed: boolean, privilegeAssertion?: PrivilegeAssertion | null) => Promise<TagMutationResponse>;
  onOpenPrivilegeUnlock: () => void;
  onOpenSchedules?: () => void;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setLastMutation: (response: TagMutationResponse | null) => void;
  tags: TagView[];
}) {
  const [tagName, setTagName] = useState("");
  const [deleteCandidate, setDeleteCandidate] = useState<TagView | null>(null);
  const [deletePreview, setDeletePreview] = useState<TagMutationResponse | null>(null);

  async function submitTag(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runAction(async () => {
      const tag = tagName.trim();
      const privilegeAssertion = await dbPrivilegeAssertion(
        privilegeMaterial,
        onOpenPrivilegeUnlock,
        "tag.create",
        tag,
        null,
        [],
      );
      await onCreateTag(tag, privilegeAssertion);
      setTagName("");
      setLastMutation(null);
    });
  }

  async function previewDelete(candidate: TagView) {
    await runAction(async () => {
      setDeleteCandidate(candidate);
      setDeletePreview(await onDeleteTag(candidate.name, false, null));
    });
  }

  async function deleteSelected() {
    const candidate = deleteCandidate;
    const preview = deletePreview;
    setDeleteCandidate(null);
    setDeletePreview(null);
    if (!candidate) {
      return;
    }
    await runAction(async () => {
      const targetIds = (preview?.affected ?? candidate.clients).map((client) => client.id);
      const privilegeAssertion = await dbPrivilegeAssertion(
        privilegeMaterial,
        onOpenPrivilegeUnlock,
        "tag.delete",
        candidate.name,
        null,
        targetIds,
      );
      setLastMutation(await onDeleteTag(candidate.name, true, privilegeAssertion));
    });
  }

  return (
    <>
      <form className="compactForm tagCreateForm" onSubmit={submitTag}>
        <strong>Create tag</strong>
        <div className="formRow">
          <input aria-label="Tag name" onChange={(event) => setTagName(event.target.value)} placeholder="provider:alpha, country:us, app:edge" value={tagName} />
          <button className="secondaryAction" disabled={pending || !tagName.trim()} type="submit">
            <Plus size={14} />
            <span>Create</span>
          </button>
        </div>
      </form>
      <CrudPager
        fields={[
          { label: "Tag", value: (tag) => tag.name },
          { label: "Clients", value: (tag) => tag.clients.length },
        ]}
        itemLabel="tags"
        items={tags}
        pageSize={12}
        title="Tag registry"
        empty={
          <div className="emptyState">
            <ShieldCheck size={22} />
            <strong>No tags</strong>
            <span>Create provider, country, or custom tags to target recurring VPS groups.</span>
          </div>
        }
      >
        {(rows) => (
          <div className="table hierarchyTable">
            <div className="historyRow heading tagRegistryGrid">
              <span>Tag</span>
              <span>Clients</span>
              <span>Action</span>
            </div>
            {rows.map((tag) => (
              <div className="historyRow tagRegistryGrid" key={tag.name}>
                <span className="tags">
                  <em>{tag.name}</em>
                </span>
                <span>{tag.clients.length}</span>
                <span>
                  <button className="secondaryAction compactAction dangerAction" disabled={pending} onClick={() => void previewDelete(tag)} type="button">
                    <Trash2 size={13} />
                    <span>Review deletion</span>
                  </button>
                </span>
              </div>
            ))}
          </div>
        )}
      </CrudPager>
      <ConfirmationPrompt
        confirmLabel="Delete tag"
        detail="Delete this tag and all assignments."
        items={[
          { label: "Tag", value: deleteCandidate?.name ?? "-" },
          { label: "Assignments", value: String(deletePreview?.target_count ?? deleteCandidate?.clients.length ?? 0) },
          { label: "Schedule target notices", value: <ScheduleImpactTable impacts={deletePreview?.schedule_impacts ?? []} onOpenSchedules={onOpenSchedules} /> },
        ]}
        onCancel={() => {
          setDeleteCandidate(null);
          setDeletePreview(null);
        }}
        onConfirm={() => void deleteSelected()}
        open={deleteCandidate !== null}
        pending={pending}
        title="Confirm tag delete"
      />
    </>
  );
}

function TagAssignments({
  agents,
  onAssignTag,
  onBulkMutateTags,
  onOpenPrivilegeUnlock,
  pending,
  privilegeMaterial,
  runAction,
  setLastMutation,
  tags,
}: {
  agents: AgentView[];
  onAssignTag: (clientId: string, tag: string, privilegeAssertion: PrivilegeAssertion) => Promise<TagMutationResponse>;
  onBulkMutateTags: (request: BulkTagMutationRequest) => Promise<TagMutationResponse>;
  onOpenPrivilegeUnlock: () => void;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setLastMutation: (response: TagMutationResponse | null) => void;
  tags: TagView[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [tagByAgent, setTagByAgent] = useState<Record<string, string>>({});
  const tagNames = useMemo(() => tags.map((tag) => tag.name).sort(), [tags]);

  async function addTag(agent: AgentView) {
    const tag = tagByAgent[agent.id]?.trim();
    if (!tag) {
      return;
    }
    await runAction(async () => {
      const privilegeAssertion = await dbPrivilegeAssertion(
        privilegeMaterial,
        onOpenPrivilegeUnlock,
        "tag.assign",
        tag,
        null,
        [agent.id],
      );
      setLastMutation(await onAssignTag(agent.id, tag, privilegeAssertion));
      setTagByAgent((current) => ({ ...current, [agent.id]: "" }));
    });
  }

  async function removeTag(agent: AgentView, tag: string) {
    await runAction(async () => {
      const selector = selectorExpressionForClientIds([agent.id]);
      const privilegeAssertion = await dbPrivilegeAssertion(
        privilegeMaterial,
        onOpenPrivilegeUnlock,
        "tag.bulk_remove",
        tag,
        selector,
        [agent.id],
      );
      setLastMutation(
        await onBulkMutateTags({
          action: "remove",
          confirmed: true,
          privilege_assertion: privilegeAssertion,
          selector_expression: selector,
          tag,
        }),
      );
    });
  }

  return (
    <CrudPager
      fields={[
        { label: "VPS", value: (agent) => formatVpsName(agent, vpsNameDisplayMode) },
        { label: "Status", value: (agent) => agent.status },
        { label: "Tags", value: (agent) => agent.tags.join(" ") },
      ]}
      itemLabel="VPSs"
      items={agents}
      pageSize={10}
      title="VPS tag assignments"
    >
      {(rows) => (
        <div className="table hierarchyTable">
          <div className="historyRow heading tagAssignmentGrid">
            <span>VPS</span>
            <span>Status</span>
            <span>Current tags</span>
            <span>Add tag</span>
          </div>
          {rows.map((agent) => (
            <div className="historyRow tagAssignmentGrid" key={agent.id}>
              <span className="historyPrimary">
                <strong title={agent.id}>{formatVpsName(agent, vpsNameDisplayMode)}</strong>
                <small>{agent.id}</small>
              </span>
              <span>{agent.status}</span>
              <span className="tagChipList">
                {agent.tags.map((tag) => (
                  <button className="tagRemoveChip" disabled={pending} key={tag} onClick={() => void removeTag(agent, tag)} title={`Remove ${tag}`} type="button">
                    <span>{tag}</span>
                    <X size={12} />
                  </button>
                ))}
              </span>
              <span className="formRow inlineTagAdd">
                <input
                  aria-label={`Tag to add to ${agent.display_name}`}
                  list="tag-options"
                  onChange={(event) => setTagByAgent((current) => ({ ...current, [agent.id]: event.target.value }))}
                  placeholder="tag"
                  value={tagByAgent[agent.id] ?? ""}
                />
                <button className="secondaryAction compactAction" disabled={pending || !(tagByAgent[agent.id] ?? "").trim()} onClick={() => void addTag(agent)} type="button">
                  <Plus size={13} />
                </button>
              </span>
            </div>
          ))}
          <datalist id="tag-options">
            {tagNames.map((tag) => (
              <option key={tag} value={tag} />
            ))}
          </datalist>
        </div>
      )}
    </CrudPager>
  );
}

function BulkTagPanel({
  agents,
  onBulkMutateTags,
  onDeleteTag,
  onOpenPrivilegeUnlock,
  onOpenSchedules,
  onResolveBulk,
  pending,
  privilegeMaterial,
  runAction,
  setLastMutation,
  tags,
}: {
  agents: AgentView[];
  onBulkMutateTags: (request: BulkTagMutationRequest) => Promise<TagMutationResponse>;
  onDeleteTag: (tag: string, confirmed: boolean, privilegeAssertion?: PrivilegeAssertion | null) => Promise<TagMutationResponse>;
  onOpenPrivilegeUnlock: () => void;
  onOpenSchedules?: () => void;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setLastMutation: (response: TagMutationResponse | null) => void;
  tags: TagView[];
}) {
  const [selectorExpression, setSelectorExpression] = useState(() => readLocalString(TAG_BULK_SELECTOR_STORAGE_KEY));
  const [action, setAction] = useState<"add" | "remove" | "delete">("add");
  const [tag, setTag] = useState("");
  const [preview, setPreview] = useState<TagMutationResponse | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);

  useEffect(() => writeLocalString(TAG_BULK_SELECTOR_STORAGE_KEY, selectorExpression), [selectorExpression]);

  async function previewTargets() {
    await runAction(async () => {
      if (action !== "delete" && selectorParse.error) {
        throw new Error(selectorParse.error);
      }
      if (action === "delete") {
        setPreview(await onDeleteTag(tag.trim(), false, null));
        return;
      }
      setPreview(
        await onBulkMutateTags({
          action,
          confirmed: false,
          privilege_assertion: null,
          selector_expression: selectorExpression.trim(),
          tag: tag.trim(),
        }),
      );
    });
  }

  async function submitMutation() {
    setConfirmOpen(false);
    await runAction(async () => {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is required before bulk tag mutation");
      }
      if (action === "delete") {
        const targetIds = (preview?.affected ?? tags.find((item) => item.name === tag.trim())?.clients ?? []).map((client) => client.id);
        const privilegeAssertion = await dbPrivilegeAssertion(
          privilegeMaterial,
          onOpenPrivilegeUnlock,
          "tag.delete",
          tag.trim(),
          null,
          targetIds,
        );
        setLastMutation(await onDeleteTag(tag.trim(), true, privilegeAssertion));
        return;
      }
      const targetIds = preview?.affected.map((agent) => agent.id) ?? [];
      const privilegeAssertion = await dbPrivilegeAssertion(
        privilegeMaterial,
        onOpenPrivilegeUnlock,
        action === "add" ? "tag.bulk_add" : "tag.bulk_remove",
        tag.trim(),
        selectorExpression.trim(),
        targetIds,
      );
      setLastMutation(
        await onBulkMutateTags({
          action,
          confirmed: true,
          privilege_assertion: privilegeAssertion,
          selector_expression: selectorExpression.trim(),
          tag: tag.trim(),
        }),
      );
    });
  }

  const previewAgents = preview?.affected ?? [];

  return (
    <div className="configApplyGrid bulkTagApplyGrid">
      <div className="compactForm bulkTagMutationForm">
        <strong>Bulk mutation</strong>
        <label>
          <span>Mutation</span>
          <select
            aria-label="Bulk tag action"
            onChange={(event) => {
              setAction(event.target.value as "add" | "remove" | "delete");
              setPreview(null);
            }}
            value={action}
          >
            <option value="add">Add tag by selector</option>
            <option value="remove">Remove tag by selector</option>
            <option value="delete">Delete tag globally</option>
          </select>
        </label>
        <label>
          <span>Tag</span>
          <input
            aria-label="Bulk tag"
            list="bulk-tag-options"
            onChange={(event) => {
              setTag(event.target.value);
              setPreview(null);
            }}
            placeholder="provider:aws or role:edge"
            value={tag}
          />
        </label>
        <datalist id="bulk-tag-options">
          {tags.map((item) => (
            <option key={item.name} value={item.name} />
          ))}
        </datalist>
        {action !== "delete" && (
          <>
            <SearchExpressionInput
              agents={agents}
              ariaLabel="Bulk tag selector expression"
              className="targetExpressionBar"
              onChange={(value) => {
                setSelectorExpression(value);
                setPreview(null);
              }}
              placeholder="provider:* && country:US"
              showMatchCount
              value={selectorExpression}
              verification={selectorParse.error ? "invalid" : selectorExpression.trim() ? "valid" : "neutral"}
              verificationMessage={selectorParse.error ?? (preview ? `${preview.target_count}/${agents.length}` : selectorExpression.trim() ? undefined : "no selector")}
            />
            <button className="secondaryAction" disabled={pending || !tag.trim() || !selectorExpression.trim()} onClick={previewTargets} type="button">
              Preview targets
            </button>
          </>
        )}
        {action === "delete" && (
          <button className="secondaryAction" disabled={pending || !tag.trim()} onClick={previewTargets} type="button">
            Preview targets
          </button>
        )}
        <div className="privilegeGateBox">
          <ShieldCheck size={16} />
          <span>{privilegeMaterial ? "Privilege unlocked" : "Unlock privilege to enable bulk tag mutation"}</span>
          {!privilegeMaterial && (
            <button className="secondaryAction compactAction" onClick={onOpenPrivilegeUnlock} type="button">
              Unlock
            </button>
          )}
        </div>
        <button
          className="primaryAction"
          disabled={pending || !privilegeMaterial || !tag.trim() || !preview || (action !== "delete" && Boolean(selectorParse.error))}
          onClick={() => setConfirmOpen(true)}
          type="button"
        >
          <Tag size={16} />
          Review mutation
        </button>
      </div>
      <section className="bulkTagPreviewPanel" aria-label="Bulk tag target preview">
        <div className="bulkTagPreviewHeader">
          <div>
            <strong>Target preview</strong>
            <span>{preview ? `${preview.target_count} resolved / ${preview.changed_count} changes` : "Review before mutation"}</span>
          </div>
        </div>
        {previewAgents.length > 0 ? (
          <div className="targetChipList bulkTagPreview">
            {previewAgents.map((agent) => (
              <span className="targetChip" key={agent.id} title={agent.id}>
                {agent.display_name}
              </span>
            ))}
          </div>
        ) : (
          <div className="bulkTagPreviewEmpty">
            <ShieldCheck size={18} />
            <span>{preview ? "No VPSs would change for this mutation." : "Review targets to show selected VPSs and schedule target-update notices."}</span>
          </div>
        )}
      </section>
      <ConfirmationPrompt
        confirmLabel="Apply tag mutation"
        detail={action === "delete" ? "Delete this tag and all assignments." : "Apply this selector-based tag mutation."}
        items={[
          { label: "Action", value: action },
          { label: "Tag", value: tag || "-" },
          { label: "Selector", value: action === "delete" ? "all assignments" : selectorExpression || "-" },
          { label: "Targets", value: String(preview?.target_count ?? 0) },
          { label: "Changed", value: String(preview?.changed_count ?? 0) },
          { label: "Schedule target notices", value: <ScheduleImpactTable impacts={preview?.schedule_impacts ?? []} onOpenSchedules={onOpenSchedules} /> },
        ]}
        onCancel={() => setConfirmOpen(false)}
        onConfirm={() => void submitMutation()}
        open={confirmOpen}
        pending={pending}
        title="Confirm tag mutation"
      />
    </div>
  );
}

function ScheduleImpactTable({
  impacts,
  onOpenSchedules,
}: {
  impacts: TagMutationResponse["schedule_impacts"];
  onOpenSchedules?: () => void;
}) {
  if (impacts.length === 0) {
    return <span>No saved schedule target snapshots need review</span>;
  }
  return (
    <div className="tagScheduleImpactTable">
      <div className="tagScheduleImpactRow heading">
        <span>Schedule</span>
        <span>Command</span>
        <span>Selector result</span>
        <span>Manual action</span>
        <span>Added</span>
        <span>Removed</span>
      </div>
      {impacts.map((impact) => (
        <div className="tagScheduleImpactRow" key={impact.schedule_id}>
          <span className="historyPrimary">
            <strong>{impact.name}</strong>
            <small>{impact.selector_expression}</small>
          </span>
          <span>{impact.command_type}</span>
          <span>
            {impact.before_target_count} -&gt; {impact.after_target_count}
          </span>
          <span className="tagScheduleManualAction">
            <span>{impact.summary}; saved targets stay fixed until you update them.</span>
            {onOpenSchedules && (
              <button className="secondaryAction compactAction" type="button" onClick={onOpenSchedules}>
                Open schedules
              </button>
            )}
          </span>
          <VpsChipList agents={impact.added_targets} />
          <VpsChipList agents={impact.removed_targets} />
        </div>
      ))}
    </div>
  );
}

function VpsChipList({ agents }: { agents: AgentView[] }) {
  if (agents.length === 0) {
    return <span className="mutedText">-</span>;
  }
  return (
    <span className="targetChipList impactTargetChips">
      {agents.map((agent) => (
        <span className="targetChip" key={agent.id} title={agent.id}>
          {agent.display_name}
        </span>
      ))}
    </span>
  );
}

function readLocalString(key: string): string {
  try {
    return window.localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function writeLocalString(key: string, value: string) {
  try {
    if (value.trim()) {
      window.localStorage.setItem(key, value);
    } else {
      window.localStorage.removeItem(key);
    }
  } catch {
    // Browser-local selector persistence must not block tag workflows.
  }
}

async function dbPrivilegeAssertion(
  privilegeMaterial: PrivilegeMaterial | null,
  onOpenPrivilegeUnlock: () => void,
  action: string,
  target: string,
  selectorExpression: string | null,
  resolvedTargets: string[],
): Promise<PrivilegeAssertion> {
  if (!privilegeMaterial) {
    onOpenPrivilegeUnlock();
    throw new Error("Privilege unlock is required");
  }
  return buildPrivilegeAssertion({
    intent: canonicalDbPrivilegeIntent({
      action,
      confirmed: true,
      resolvedTargets,
      selectorExpression,
      target,
    }),
    privilegeMaterial,
  });
}
