import { useEffect, useMemo, useState, type FormEvent } from "react";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical, Plus, RefreshCw, ShieldCheck, Tag, Trash2, X } from "lucide-react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { useReviewGenerationGuard, waitForReviewRender } from "../hooks/useReviewGenerationGuard";
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

type BulkTagMutationSnapshot = {
  action: "add" | "remove" | "delete";
  preview: TagMutationResponse;
  selectorExpression: string;
  tag: string;
};

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
  onUpdateTagOrder,
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
  onUpdateTagOrder: (orderedTags: string[]) => Promise<TagView[]>;
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
            onUpdateTagOrder={onUpdateTagOrder}
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
  onUpdateTagOrder,
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
  onUpdateTagOrder: (orderedTags: string[]) => Promise<TagView[]>;
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

  const tagColumns = useMemo<ConsoleDataGridColumn<TagView>[]>(
    () => [
      {
        cell: (tag) => (
          <span className="tags">
            <em>{tag.name}</em>
          </span>
        ),
        header: "Tag",
        id: "tag",
        searchValue: (tag) => tag.name,
        sortValue: (tag) => tag.name,
      },
      {
        cell: (tag) => tag.clients.length,
        header: "Clients",
        id: "clients",
        searchValue: (tag) => tag.clients.length,
        sortValue: (tag) => tag.clients.length,
      },
      {
        cell: (tag) => (
          <button
            className="secondaryAction compactAction dangerAction"
            disabled={pending}
            onClick={(event) => {
              event.stopPropagation();
              void previewDelete(tag);
            }}
            type="button"
          >
            <Trash2 size={13} />
            <span>Review deletion</span>
          </button>
        ),
        enableHiding: false,
        header: "Action",
        id: "action",
      },
    ],
    [pending],
  );

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
      <ConsoleDataGrid
        columns={tagColumns}
        defaultPageSize={12}
        expandOnRowClick
        getRowId={(tag) => tag.name}
        itemLabel="tags"
        empty={
          <div className="emptyState">
            <ShieldCheck size={22} />
            <strong>No tags</strong>
            <span>Create provider, country, or custom tags to target recurring VPS groups.</span>
          </div>
        }
        renderExpandedRow={(tag) => (
          <div className="consoleInlineDetailGrid">
            <span>Tag</span>
            <strong>{tag.name}</strong>
            <span>Assigned VPSs</span>
            <strong>{tag.clients.length}</strong>
            <span>Clients</span>
            <strong>{tag.clients.map((client) => client.id).join(", ") || "None"}</strong>
          </div>
        )}
        rows={tags}
        searchPlaceholder="Search tags"
        selectable={false}
        storageKey="vpsman.tags.registry"
        title="Tag registry"
      />
      <TagOrderManager
        disabled={pending}
        onUpdateTagOrder={onUpdateTagOrder}
        tags={tags}
      />
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

function TagOrderManager({
  disabled,
  onUpdateTagOrder,
  tags,
}: {
  disabled: boolean;
  onUpdateTagOrder: (orderedTags: string[]) => Promise<TagView[]>;
  tags: TagView[];
}) {
  const [orderedNames, setOrderedNames] = useState(() => tags.map((tag) => tag.name));
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const tagByName = useMemo(() => new Map(tags.map((tag) => [tag.name, tag])), [tags]);
  const orderedTags = useMemo(
    () => orderedNames.map((name) => tagByName.get(name)).filter((tag): tag is TagView => Boolean(tag)),
    [orderedNames, tagByName],
  );
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  useEffect(() => {
    setOrderedNames(tags.map((tag) => tag.name));
  }, [tags]);

  async function handleDragEnd(event: DragEndEvent) {
    const activeId = String(event.active.id);
    const overId = event.over ? String(event.over.id) : null;
    if (!overId || activeId === overId || saving || disabled) {
      return;
    }
    const oldIndex = orderedNames.indexOf(activeId);
    const newIndex = orderedNames.indexOf(overId);
    if (oldIndex < 0 || newIndex < 0) {
      return;
    }
    const nextOrder = arrayMove(orderedNames, oldIndex, newIndex);
    setOrderedNames(nextOrder);
    setSaving(true);
    setStatus("Saving order");
    try {
      const updated = await onUpdateTagOrder(nextOrder);
      setOrderedNames(updated.map((tag) => tag.name));
      setStatus("Order saved");
    } catch (error) {
      setOrderedNames(tags.map((tag) => tag.name));
      setStatus(error instanceof Error ? error.message : "Order save failed");
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="tagOrderPanel">
      <div className="tagOrderHeader">
        <div>
          <strong>Fleet tag order</strong>
          <span>{tags.length} tags</span>
        </div>
        {status && (
          <span className={`consoleStatusBadge ${saving || status !== "Order saved" ? "warning" : "ok"}`}>
            {status}
          </span>
        )}
      </div>
      {orderedTags.length === 0 ? (
        <div className="emptyState compactEmptyState">
          <ShieldCheck size={20} />
          <strong>No tags</strong>
          <span>Create tags before setting Fleet display order.</span>
        </div>
      ) : (
        <DndContext collisionDetection={closestCenter} onDragEnd={(event) => void handleDragEnd(event)} sensors={sensors}>
          <SortableContext items={orderedTags.map((tag) => tag.name)} strategy={verticalListSortingStrategy}>
            <div className="tagOrderList" role="list">
              {orderedTags.map((tag, index) => (
                <SortableTagOrderRow
                  disabled={disabled || saving}
                  index={index}
                  key={tag.name}
                  tag={tag}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
      )}
    </section>
  );
}

function SortableTagOrderRow({
  disabled,
  index,
  tag,
}: {
  disabled: boolean;
  index: number;
  tag: TagView;
}) {
  const {
    attributes,
    isDragging,
    listeners,
    setNodeRef,
    transform,
    transition,
  } = useSortable({ disabled, id: tag.name });
  return (
    <div
      className={`tagOrderRow${isDragging ? " dragging" : ""}`}
      ref={setNodeRef}
      role="listitem"
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
      }}
    >
      <button
        aria-label={`Reorder ${tag.name}`}
        className="tagOrderHandle"
        disabled={disabled}
        type="button"
        {...attributes}
        {...listeners}
      >
        <GripVertical size={15} />
      </button>
      <span className="tagOrderIndex">{index + 1}</span>
      <span className="tags">
        <em>{tag.name}</em>
      </span>
      <span className="tagOrderClients">{tag.clients.length} VPS{tag.clients.length === 1 ? "" : "s"}</span>
    </div>
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
  const tagNames = useMemo(() => tags.map((tag) => tag.name), [tags]);

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
          target_client_ids: [agent.id],
          tag,
        }),
      );
    });
  }

  const assignmentColumns = useMemo<ConsoleDataGridColumn<AgentView>[]>(
    () => [
      {
        cell: (agent) => (
          <span className="historyPrimary">
            <strong title={agent.id}>{formatVpsName(agent, vpsNameDisplayMode)}</strong>
            <small>{agent.id}</small>
          </span>
        ),
        header: "VPS",
        id: "vps",
        searchValue: (agent) => `${formatVpsName(agent, vpsNameDisplayMode)} ${agent.id}`,
        sortValue: (agent) => formatVpsName(agent, vpsNameDisplayMode),
      },
      {
        cell: (agent) => agent.status,
        header: "Status",
        id: "status",
        searchValue: (agent) => agent.status,
        sortValue: (agent) => agent.status,
      },
      {
        cell: (agent) => (
          <span className="tagChipList">
            {agent.tags.map((tag) => (
              <button
                className="tagRemoveChip"
                disabled={pending}
                key={tag}
                onClick={(event) => {
                  event.stopPropagation();
                  void removeTag(agent, tag);
                }}
                title={`Remove ${tag}`}
                type="button"
              >
                <span>{tag}</span>
                <X size={12} />
              </button>
            ))}
          </span>
        ),
        header: "Current tags",
        id: "tags",
        searchValue: (agent) => agent.tags.join(" "),
        sortValue: (agent) => agent.tags.join(" "),
      },
      {
        cell: (agent) => (
          <span className="formRow inlineTagAdd">
            <input
              aria-label={`Tag to add to ${agent.display_name}`}
              list="tag-options"
              onChange={(event) => setTagByAgent((current) => ({ ...current, [agent.id]: event.target.value }))}
              onClick={(event) => event.stopPropagation()}
              placeholder="tag"
              value={tagByAgent[agent.id] ?? ""}
            />
            <button
              className="secondaryAction compactAction"
              disabled={pending || !(tagByAgent[agent.id] ?? "").trim()}
              onClick={(event) => {
                event.stopPropagation();
                void addTag(agent);
              }}
              type="button"
            >
              <Plus size={13} />
            </button>
          </span>
        ),
        enableHiding: false,
        header: "Add tag",
        id: "addTag",
      },
    ],
    [pending, tagByAgent, vpsNameDisplayMode],
  );

  return (
    <>
      <ConsoleDataGrid
        columns={assignmentColumns}
        defaultPageSize={10}
        expandOnRowClick
        getRowId={(agent) => agent.id}
        itemLabel="VPSs"
        renderExpandedRow={(agent) => (
          <div className="consoleInlineDetailGrid">
            <span>VPS</span>
            <strong>{formatVpsName(agent, vpsNameDisplayMode)}</strong>
            <span>Client ID</span>
            <strong>{agent.id}</strong>
            <span>Status</span>
            <strong>{agent.status}</strong>
            <span>Tags</span>
            <strong>{agent.tags.join(", ") || "None"}</strong>
          </div>
        )}
        rows={agents}
        searchPlaceholder="Search VPS assignments"
        selectable={false}
        storageKey="vpsman.tags.assignments"
        title="VPS tag assignments"
      />
      <datalist id="tag-options">
        {tagNames.map((tag) => (
          <option key={tag} value={tag} />
        ))}
      </datalist>
    </>
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
  const [mutationSnapshot, setMutationSnapshot] = useState<BulkTagMutationSnapshot | null>(null);
  const [previewStatus, setPreviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);

  useEffect(() => writeLocalString(TAG_BULK_SELECTOR_STORAGE_KEY, selectorExpression), [selectorExpression]);

  function clearMutationPreview() {
    invalidateReviewGeneration();
    setPreview(null);
    setMutationSnapshot(null);
    setConfirmOpen(false);
    setPreviewStatus(null);
  }

  async function previewTargets() {
    const reviewGeneration = captureReviewGeneration();
    const frozenAction = action;
    const frozenTag = tag.trim();
    const frozenSelector = selectorExpression.trim();
    setPreviewStatus("Preparing tag preview");
    try {
      await runAction(async () => {
        await waitForReviewRender();
        if (frozenAction !== "delete" && selectorParse.error) {
          throw new Error(selectorParse.error);
        }
        if (frozenAction === "delete") {
          const nextPreview = await onDeleteTag(frozenTag, false, null);
          if (!isReviewGenerationCurrent(reviewGeneration)) {
            return;
          }
          setPreview(nextPreview);
          setMutationSnapshot(null);
          return;
        }
        const resolved = await onResolveBulk(frozenSelector);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const targetClientIds = resolved.targets.map((target) => target.id);
        if (!targetClientIds.length) {
          throw new Error("Bulk tag preview resolved no VPSs");
        }
        const nextPreview = await onBulkMutateTags({
          action: frozenAction,
          confirmed: false,
          privilege_assertion: null,
          selector_expression: frozenSelector,
          target_client_ids: targetClientIds,
          tag: frozenTag,
        });
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setPreview(nextPreview);
        setMutationSnapshot(null);
      });
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setPreviewStatus(null);
      }
    }
  }

  async function submitMutation() {
    const snapshot = mutationSnapshot;
    setConfirmOpen(false);
    await runAction(async () => {
      if (!snapshot) {
        throw new Error("Tag mutation confirmation snapshot is missing; preview the mutation again");
      }
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is required before bulk tag mutation");
      }
      if (snapshot.action === "delete") {
        const targetIds = snapshot.preview.affected.map((client) => client.id);
        const privilegeAssertion = await dbPrivilegeAssertion(
          privilegeMaterial,
          onOpenPrivilegeUnlock,
          "tag.delete",
          snapshot.tag,
          null,
          targetIds,
        );
        setLastMutation(await onDeleteTag(snapshot.tag, true, privilegeAssertion));
        setMutationSnapshot(null);
        return;
      }
      const targetIds = snapshot.preview.affected.map((agent) => agent.id);
      if (!targetIds.length) {
        throw new Error("Review targets before applying the tag mutation");
      }
      const privilegeAssertion = await dbPrivilegeAssertion(
        privilegeMaterial,
        onOpenPrivilegeUnlock,
        snapshot.action === "add" ? "tag.bulk_add" : "tag.bulk_remove",
        snapshot.tag,
        snapshot.selectorExpression,
        targetIds,
      );
      setLastMutation(
        await onBulkMutateTags({
          action: snapshot.action,
          confirmed: true,
          privilege_assertion: privilegeAssertion,
          selector_expression: snapshot.selectorExpression,
          target_client_ids: targetIds,
          tag: snapshot.tag,
        }),
      );
      setMutationSnapshot(null);
    });
  }

  const previewAgents = preview?.affected ?? [];
  const confirmationSnapshot = confirmOpen ? mutationSnapshot : null;
  const confirmationPreview = confirmationSnapshot?.preview ?? preview;

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
              clearMutationPreview();
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
              clearMutationPreview();
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
                clearMutationPreview();
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
          onClick={() => {
            if (!preview) {
              return;
            }
            setMutationSnapshot({
              action,
              preview,
              selectorExpression: action === "delete" ? "" : selectorExpression.trim(),
              tag: tag.trim(),
            });
            setConfirmOpen(true);
          }}
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
            <span>{previewStatus ?? (preview ? `${preview.target_count} resolved / ${preview.changed_count} changes` : "Review before mutation")}</span>
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
        detail={confirmationSnapshot?.action === "delete" ? "Delete this tag and all assignments." : "Apply this selector-based tag mutation."}
        items={[
          { label: "Action", value: confirmationSnapshot?.action ?? action },
          { label: "Tag", value: confirmationSnapshot?.tag || tag || "-" },
          {
            label: "Selector",
            value:
              confirmationSnapshot?.action === "delete"
                ? "all assignments"
                : confirmationSnapshot?.selectorExpression || selectorExpression || "-",
          },
          { label: "Targets", value: String(confirmationPreview?.target_count ?? 0) },
          { label: "Changed", value: String(confirmationPreview?.changed_count ?? 0) },
          { label: "Schedule target notices", value: <ScheduleImpactTable impacts={confirmationPreview?.schedule_impacts ?? []} onOpenSchedules={onOpenSchedules} /> },
        ]}
        onCancel={() => {
          setConfirmOpen(false);
          setMutationSnapshot(null);
        }}
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
