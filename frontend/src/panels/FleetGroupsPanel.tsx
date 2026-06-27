import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
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
  FleetAlertPolicyRecord,
  ScheduleRecord,
  TagMutationResponse,
  TagView,
} from "../types";
import { buildPrivilegeAssertion, canonicalDbPrivilegeIntent, type PrivilegeMaterial, type PrivilegeAssertion } from "../privilege";
import { agentsMatchingExpression, parseSearchExpression, selectorExpressionForClientIds } from "../searchExpression";
import { formatVpsName, runPanelAction } from "../utils";

const TAG_BULK_SELECTOR_STORAGE_KEY = "vpsman.tags.bulk.selectorExpression";

type BulkTagMutationSnapshot = {
  action: "add" | "remove" | "delete";
  preview: TagMutationResponse;
  selectorExpression: string;
  tag: string;
};

export function FleetGroupsPanel({
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
  schedules,
  tags,
  fleetAlertPolicies,
}: {
  activeSubpage: string;
  agents: AgentView[];
  error: string | null;
  loading: boolean;
  onAssignTag: (clientId: string, tag: string, privilegeAssertion: PrivilegeAssertion) => Promise<TagMutationResponse>;
  onBulkMutateTags: (request: BulkTagMutationRequest) => Promise<TagMutationResponse>;
  onCreateTag: (name: string, privilegeAssertion: PrivilegeAssertion) => Promise<void>;
  onDeleteTag: (
    tag: string,
    confirmed: boolean,
    privilegeAssertion?: PrivilegeAssertion | null,
    previewHash?: string | null,
  ) => Promise<TagMutationResponse>;
  onOpenPrivilegeUnlock: () => void;
  onOpenSchedules?: () => void;
  onRefresh: () => void;
  onResolveBulk: (selectorExpression: string) => Promise<BulkResolveResponse>;
  onUpdateTagOrder: (orderedTags: string[]) => Promise<TagView[]>;
  privilegeMaterial: PrivilegeMaterial | null;
  schedules: ScheduleRecord[];
  tags: TagView[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
}) {
  const subpage = ["registry", "assignments", "bulk"].includes(activeSubpage) ? activeSubpage : "registry";
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [lastMutation, setLastMutation] = useState<TagMutationResponse | null>(null);
  const groupSummary = useMemo(() => buildGroupSummary(tags, agents), [agents, tags]);
  const activeLabelCount =
    groupSummary.providerGroupCount + groupSummary.countryGroupCount + groupSummary.customGroupCount;
  const status =
    actionError ??
    error ??
    (lastMutation
      ? `Group ${lastMutation.tag}: ${lastMutation.changed_count} changed, ${lastMutation.skipped_count} skipped`
      : loading
        ? "Refreshing group state"
        : `${tags.length} registry groups, ${activeLabelCount} active labels across ${agents.length} VPSs`);

  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>{subpage === "bulk" ? "Bulk groups" : subpage === "assignments" ? "Group assignments" : "Fleet groups"}</h2>
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
            summary={groupSummary}
            tags={tags}
          />
        )}
        {subpage !== "registry" && <GroupSummaryStrip summary={groupSummary} />}
        {subpage === "assignments" && (
          <TagAssignments
            agents={agents}
            onAssignTag={onAssignTag}
            onBulkMutateTags={onBulkMutateTags}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={(action) => runPanelAction(setPending, setActionError, action)}
            schedules={schedules}
            setLastMutation={setLastMutation}
            tags={tags}
            fleetAlertPolicies={fleetAlertPolicies}
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

type GroupSummary = {
  assignedVpsCount: number;
  countryGroupCount: number;
  customGroupCount: number;
  offlineCount: number;
  onlineCount: number;
  providerGroupCount: number;
  staleCount: number;
  totalAssignments: number;
};

function GroupSummaryStrip({ summary }: { summary: GroupSummary }) {
  return (
    <div className="groupSummaryStrip" aria-label="Fleet group counts">
      <span>
        <strong>{summary.providerGroupCount}</strong>
        <small>provider metadata</small>
      </span>
      <span>
        <strong>{summary.countryGroupCount}</strong>
        <small>country metadata</small>
      </span>
      <span>
        <strong>{summary.customGroupCount}</strong>
        <small>operator groups</small>
      </span>
      <span>
        <strong>{summary.totalAssignments}</strong>
        <small>group assignments</small>
      </span>
      <span>
        <strong>{summary.assignedVpsCount}</strong>
        <small>assigned VPSs</small>
      </span>
      <span>
        <strong>{summary.onlineCount}/{summary.staleCount}/{summary.offlineCount}</strong>
        <small>online/stale/offline</small>
      </span>
    </div>
  );
}

function buildGroupSummary(tags: TagView[], agents: AgentView[]): GroupSummary {
  const assignedVpsIds = new Set<string>();
  const assignments = new Set<string>();
  const groupNames = new Set<string>();
  for (const tag of tags) {
    groupNames.add(tag.name);
    for (const client of tag.clients) {
      assignedVpsIds.add(client.id);
      assignments.add(`${tag.name}\u0000${client.id}`);
    }
  }
  for (const agent of agents) {
    for (const tag of agent.tags) {
      groupNames.add(tag);
      assignedVpsIds.add(agent.id);
      assignments.add(`${tag}\u0000${agent.id}`);
    }
  }
  const groupNameList = Array.from(groupNames);
  return {
    assignedVpsCount: assignedVpsIds.size,
    countryGroupCount: groupNameList.filter((tag) => isCountryGroup(tag)).length,
    customGroupCount: groupNameList.filter((tag) => !isProviderGroup(tag) && !isCountryGroup(tag)).length,
    offlineCount: agents.filter((agent) => agent.status === "offline").length,
    onlineCount: agents.filter((agent) => agent.status === "online").length,
    providerGroupCount: groupNameList.filter((tag) => isProviderGroup(tag)).length,
    staleCount: agents.filter((agent) => agent.status === "stale").length,
    totalAssignments: assignments.size,
  };
}

function isCountryGroup(tag: string) {
  return tag.toLowerCase().startsWith("country:");
}

function isProviderGroup(tag: string) {
  return tag.toLowerCase().startsWith("provider:");
}

function groupKind(tag: string): "country" | "custom" | "provider" {
  if (isProviderGroup(tag)) return "provider";
  if (isCountryGroup(tag)) return "country";
  return "custom";
}

function groupKindLabel(tag: string) {
  const kind = groupKind(tag);
  if (kind === "provider") return "Provider metadata";
  if (kind === "country") return "Country metadata";
  return "Operator group";
}

function groupKindTone(tag: string) {
  return groupKind(tag) === "custom" ? "ok" : "info";
}

function groupKindDetail(tag: string) {
  const kind = groupKind(tag);
  if (kind === "provider") {
    return "Managed from VPS provider metadata; useful for scoped filters.";
  }
  if (kind === "country") {
    return "Managed from VPS location metadata; useful for regional targeting.";
  }
  return "Created by operators for recurring VPS targeting.";
}

function groupDisplayName(tag: string) {
  const [prefix, ...rest] = tag.split(":");
  const value = rest.join(":");
  if (!value) return tag;
  if (prefix.toLowerCase() === "provider") return `Provider: ${value}`;
  if (prefix.toLowerCase() === "country") return `Country: ${value}`;
  return tag;
}

function groupOption(tag: TagView) {
  return {
    label: groupOptionLabel(tag.name, tag.clients.length),
    value: tag.name,
  };
}

function groupOptionLabel(tag: string, clientCount: number) {
  return `${tag} (${clientCount} VPS${clientCount === 1 ? "" : "s"})`;
}

function tagClientsCount(tags: TagView[], tagName: string) {
  return tags.find((tag) => tag.name === tagName)?.clients.length ?? 0;
}

type GroupDependencySummary = {
  alertPolicies: number;
  schedules: number;
  total: number;
};

function groupDependencySummary(
  tag: string,
  schedules: ScheduleRecord[],
  fleetAlertPolicies: FleetAlertPolicyRecord[],
): GroupDependencySummary {
  const scheduleCount = schedules.filter(
    (schedule) =>
      !schedule.deleted_at &&
      selectorReferencesGroup(schedule.selector_expression, tag),
  ).length;
  const policyCount = fleetAlertPolicies.filter(
    (policy) =>
      policy.enabled && selectorReferencesGroup(policy.selector_expression, tag),
  ).length;
  return {
    alertPolicies: policyCount,
    schedules: scheduleCount,
    total: scheduleCount + policyCount,
  };
}

function selectorReferencesGroup(selector: string | null | undefined, tag: string) {
  if (!selector || !tag) {
    return false;
  }
  const haystack = selector.toLowerCase();
  const needle = tag.toLowerCase();
  const variants = new Set([
    needle,
    `tag:${needle}`,
    `tags:${needle}`,
    `vps.tag:${needle}`,
    `vps.tags:${needle}`,
  ]);
  return Array.from(variants).some((variant) =>
    new RegExp(`(^|[^a-z0-9_:-])${escapeRegExp(variant)}($|[^a-z0-9_:-])`, "i").test(haystack),
  );
}

function dependencySummaryText(summary: GroupDependencySummary) {
  if (summary.total === 0) {
    return "No automation references";
  }
  const parts = [];
  if (summary.schedules > 0) {
    parts.push(`${summary.schedules} schedule${summary.schedules === 1 ? "" : "s"}`);
  }
  if (summary.alertPolicies > 0) {
    parts.push(`${summary.alertPolicies} alert polic${summary.alertPolicies === 1 ? "y" : "ies"}`);
  }
  return `Used by ${parts.join(" and ")}`;
}

type TargetStatusCounts = {
  offline: number;
  ready: number;
  stale: number;
  total: number;
};

function targetStatusCounts(targets: AgentView[]): TargetStatusCounts {
  const ready = targets.filter((target) => target.status === "online").length;
  const stale = targets.filter((target) => target.status === "stale").length;
  const offline = targets.filter((target) => target.status === "offline").length;
  return {
    offline,
    ready,
    stale,
    total: targets.length,
  };
}

function targetStatusText(prefix: string, targets: AgentView[]) {
  const counts = targetStatusCounts(targets);
  const parts = [
    `${prefix} ${bulkVpsCountLabel(counts.total)}`,
    `${counts.ready} ready`,
    `${counts.stale} stale`,
  ];
  if (counts.offline > 0) {
    parts.push(`${counts.offline} offline`);
  }
  return parts.join(" · ");
}

function bulkVpsCountLabel(count: number) {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function bulkMutationPrimaryLabel(
  action: "add" | "delete" | "remove",
  tag: string,
  targetCount: number,
) {
  if (!tag && action === "delete") {
    return "Choose group to delete";
  }
  if (!tag && targetCount === 0) {
    return "Choose group and targets";
  }
  if (!tag) {
    return `Choose group for ${bulkVpsCountLabel(targetCount)}`;
  }
  if (action !== "delete" && targetCount === 0) {
    return "Select target VPSs";
  }
  if (action === "delete") {
    return tag ? `Delete ${tag} globally` : "Delete group globally";
  }
  if (action === "remove") {
    return `Remove ${tag} from ${bulkVpsCountLabel(targetCount)}`;
  }
  return `Add ${tag} to ${bulkVpsCountLabel(targetCount)}`;
}

function membershipOutcomeText(
  action: "add" | "delete" | "remove",
  preview: TagMutationResponse | null | undefined,
) {
  if (!preview) {
    return "Server preview required before apply.";
  }
  if (action === "delete") {
    return `${preview.changed_count} assignment${preview.changed_count === 1 ? "" : "s"} removed; ${preview.skipped_count} skipped.`;
  }
  if (action === "remove") {
    return `${preview.changed_count} VPS${preview.changed_count === 1 ? "" : "s"} will lose the group; ${preview.skipped_count} already lacked it.`;
  }
  return `${preview.changed_count} VPS${preview.changed_count === 1 ? "" : "s"} will gain the group; ${preview.skipped_count} already had it.`;
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
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
  summary,
  tags,
}: {
  onCreateTag: (name: string, privilegeAssertion: PrivilegeAssertion) => Promise<void>;
  onDeleteTag: (
    tag: string,
    confirmed: boolean,
    privilegeAssertion?: PrivilegeAssertion | null,
    previewHash?: string | null,
  ) => Promise<TagMutationResponse>;
  onOpenPrivilegeUnlock: () => void;
  onOpenSchedules?: () => void;
  onUpdateTagOrder: (orderedTags: string[]) => Promise<TagView[]>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setLastMutation: (response: TagMutationResponse | null) => void;
  summary: GroupSummary;
  tags: TagView[];
}) {
  const [tagName, setTagName] = useState("");
  const [deleteCandidate, setDeleteCandidate] = useState<TagView | null>(null);
  const [deletePreview, setDeletePreview] = useState<TagMutationResponse | null>(null);
  const trimmedGroupName = tagName.trim();
  const groupNameHasComma = trimmedGroupName.includes(",");

  async function submitTag(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!trimmedGroupName || groupNameHasComma) {
      return;
    }
    await runAction(async () => {
      const tag = trimmedGroupName;
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
      setLastMutation(
        await onDeleteTag(
          candidate.name,
          true,
          privilegeAssertion,
          preview?.preview_hash ?? null,
        ),
      );
    });
  }

  const tagColumns = useMemo<ConsoleDataGridColumn<TagView>[]>(
    () => [
      {
        cell: (tag) => (
          <span className="historyPrimary">
            <strong>{groupDisplayName(tag.name)}</strong>
            <small>{tag.name}</small>
          </span>
        ),
        header: "Group",
        id: "group",
        searchValue: (tag) => tag.name,
        sortValue: (tag) => tag.name,
      },
      {
        cell: (tag) => (
          <span className={`consoleStatusBadge ${groupKindTone(tag.name)}`}>
            {groupKindLabel(tag.name)}
          </span>
        ),
        header: "Type",
        id: "type",
        searchValue: (tag) => groupKindLabel(tag.name),
        sortValue: (tag) => groupKindLabel(tag.name),
      },
      {
        cell: (tag) => tag.clients.length,
        header: "Assigned VPSs",
        id: "assigned",
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
            <span>Delete</span>
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
        <strong>Create group</strong>
        <span className="formHint">
          Add one operator-managed group per submission. Provider and country
          metadata are read from VPS records.
        </span>
        <div className="formRow">
          <input
            aria-describedby="group-name-hint"
            aria-label="Group name"
            onChange={(event) => setTagName(event.target.value)}
            placeholder="role:edge or maintenance"
            value={tagName}
          />
          <button
            className="secondaryAction"
            disabled={pending || !trimmedGroupName || groupNameHasComma}
            type="submit"
          >
            <Plus size={14} />
            <span>Create group</span>
          </button>
        </div>
        <small id="group-name-hint">
          {groupNameHasComma
            ? "Use one group name per submission; commas are not accepted here."
            : "Use the group later in selectors, schedules, alerts, and bulk operations."}
        </small>
      </form>
      <ConsoleDataGrid
        columns={tagColumns}
        defaultPageSize={12}
        expandOnRowClick
        getRowId={(tag) => tag.name}
        itemLabel="groups"
        empty={
          <div className="emptyState">
            <ShieldCheck size={22} />
            <strong>No groups</strong>
            <span>Create operator groups to target recurring VPS workflows.</span>
          </div>
        }
        renderExpandedRow={(tag) => (
          <div className="consoleInlineDetailGrid">
            <span>Group</span>
            <strong>{tag.name}</strong>
            <span>Type</span>
            <strong>{groupKindLabel(tag.name)}</strong>
            <span>Model</span>
            <strong>{groupKindDetail(tag.name)}</strong>
            <span>Assigned VPSs</span>
            <strong>{tag.clients.length}</strong>
            <span>VPS IDs</span>
            <strong>{tag.clients.map((client) => client.id).join(", ") || "None"}</strong>
          </div>
        )}
        rowActions={[
          {
            icon: <Trash2 size={13} />,
            label: "Delete",
            onSelect: ([tag]) => {
              if (tag) {
                void previewDelete(tag);
              }
            },
            tone: "danger",
          },
        ]}
        rows={tags}
        searchPlaceholder="Search groups or metadata"
        selectable={false}
        storageKey="vpsman.tags.registry"
        title="Group registry"
      />
      <GroupSummaryStrip summary={summary} />
      <TagOrderManager
        disabled={pending}
        onUpdateTagOrder={onUpdateTagOrder}
        tags={tags}
      />
      <ConfirmationPrompt
        confirmLabel="Delete group"
        detail="Delete this group and remove it from assigned VPSs. Managed metadata can reappear when VPS records report it again."
        items={[
          { label: "Group", value: deleteCandidate?.name ?? "-" },
          {
            label: "Type",
            value: deleteCandidate ? groupKindLabel(deleteCandidate.name) : "-",
          },
          { label: "Assignments", value: String(deletePreview?.target_count ?? deleteCandidate?.clients.length ?? 0) },
          {
            label: "Preview hash",
            title: deletePreview?.preview_hash,
            value: deletePreview?.preview_hash ?? "-",
          },
          { label: "Schedule target notices", value: <ScheduleImpactTable impacts={deletePreview?.schedule_impacts ?? []} onOpenSchedules={onOpenSchedules} /> },
        ]}
        onCancel={() => {
          setDeleteCandidate(null);
          setDeletePreview(null);
        }}
        onConfirm={() => void deleteSelected()}
        open={deleteCandidate !== null}
        pending={pending}
        title="Confirm group delete"
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
    <details className="tagOrderPanel">
      <summary className="tagOrderSummary">
        <div>
          <strong>Manage display order</strong>
          <span>{tags.length} groups</span>
        </div>
        {status && (
          <span className={`consoleStatusBadge ${saving || status !== "Order saved" ? "warning" : "ok"}`}>
            {status}
          </span>
        )}
      </summary>
      {orderedTags.length === 0 ? (
        <div className="emptyState compactEmptyState">
          <ShieldCheck size={20} />
          <strong>No groups</strong>
          <span>Create groups before setting fleet display order.</span>
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
    </details>
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
  fleetAlertPolicies,
  onAssignTag,
  onBulkMutateTags,
  onOpenPrivilegeUnlock,
  pending,
  privilegeMaterial,
  runAction,
  schedules,
  setLastMutation,
  tags,
}: {
  agents: AgentView[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  onAssignTag: (clientId: string, tag: string, privilegeAssertion: PrivilegeAssertion) => Promise<TagMutationResponse>;
  onBulkMutateTags: (request: BulkTagMutationRequest) => Promise<TagMutationResponse>;
  onOpenPrivilegeUnlock: () => void;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  schedules: ScheduleRecord[];
  setLastMutation: (response: TagMutationResponse | null) => void;
  tags: TagView[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [tagByAgent, setTagByAgent] = useState<Record<string, string>>({});
  const [recentRemoval, setRecentRemoval] = useState<{
    agentId: string;
    agentLabel: string;
    scheduleImpactCount: number;
    selectorExpression: string;
    tag: string;
  } | null>(null);
  const tagNames = useMemo(() => tags.map((tag) => tag.name), [tags]);
  const tagOptions = useMemo(() => tags.map(groupOption), [tags]);
  const suggestionsText = tagOptions.length
    ? `Suggestions: ${tagOptions.slice(0, 4).map((option) => option.label).join(", ")}`
    : "No saved operator groups yet";

  const addTag = useCallback(async (agent: AgentView) => {
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
      setRecentRemoval(null);
    });
  }, [onAssignTag, onOpenPrivilegeUnlock, privilegeMaterial, runAction, setLastMutation, tagByAgent]);

  const removeTag = useCallback(async (agent: AgentView, tag: string) => {
    const agentLabel = formatVpsName(agent, vpsNameDisplayMode);
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
      const response = await onBulkMutateTags({
        action: "remove",
        confirmed: true,
        privilege_assertion: privilegeAssertion,
        selector_expression: selector,
        target_client_ids: [agent.id],
        tag,
      });
      setLastMutation(response);
      if (response.changed_count > 0) {
        setRecentRemoval({
          agentId: agent.id,
          agentLabel,
          scheduleImpactCount: response.schedule_impacts.length,
          selectorExpression: selector,
          tag,
        });
      } else {
        setRecentRemoval(null);
      }
    });
  }, [onBulkMutateTags, onOpenPrivilegeUnlock, privilegeMaterial, runAction, setLastMutation, vpsNameDisplayMode]);

  async function undoRemoveTag() {
    if (!recentRemoval) {
      return;
    }
    const removal = recentRemoval;
    await runAction(async () => {
      const privilegeAssertion = await dbPrivilegeAssertion(
        privilegeMaterial,
        onOpenPrivilegeUnlock,
        "tag.bulk_add",
        removal.tag,
        removal.selectorExpression,
        [removal.agentId],
      );
      setLastMutation(
        await onBulkMutateTags({
          action: "add",
          confirmed: true,
          privilege_assertion: privilegeAssertion,
          selector_expression: removal.selectorExpression,
          target_client_ids: [removal.agentId],
          tag: removal.tag,
        }),
      );
      setRecentRemoval(null);
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
            {agent.tags.map((tag) => {
              const dependencies = groupDependencySummary(tag, schedules, fleetAlertPolicies);
              const dependencyLabel = dependencySummaryText(dependencies);
              const hasDependencies = dependencies.total > 0;
              if (groupKind(tag) !== "custom") {
                return (
                  <span
                    className="tagRemoveChip managed"
                    key={tag}
                    title={`${groupKindLabel(tag)}. ${dependencyLabel}`}
                  >
                    <ShieldCheck size={12} />
                    <span>{tag}</span>
                    {hasDependencies && <small>{dependencyLabel}</small>}
                  </span>
                );
              }
              return (
                <button
                  aria-label={`Remove ${tag} from ${formatVpsName(agent, vpsNameDisplayMode)}`}
                  className={`tagRemoveChip${hasDependencies ? " linked" : ""}`}
                  disabled={pending}
                  key={tag}
                  onClick={(event) => {
                    event.stopPropagation();
                    void removeTag(agent, tag);
                  }}
                  title={`Remove ${tag}. ${dependencyLabel}`}
                  type="button"
                >
                  <span>{tag}</span>
                  {hasDependencies && <small>{dependencyLabel}</small>}
                  <X size={12} />
                </button>
              );
            })}
          </span>
        ),
        header: "Current groups",
        id: "tags",
        searchValue: (agent) => agent.tags.join(" "),
        sortValue: (agent) => agent.tags.join(" "),
      },
      {
        cell: (agent) => (
          <span className="inlineTagAddStack">
            <span className="formRow inlineTagAdd">
              <input
                aria-describedby={`group-suggestions-${agent.id}`}
                aria-label={`Group to add to ${agent.display_name}`}
                list="tag-options"
                onChange={(event) => setTagByAgent((current) => ({ ...current, [agent.id]: event.target.value }))}
                onClick={(event) => event.stopPropagation()}
                placeholder="group name"
                value={tagByAgent[agent.id] ?? ""}
              />
              <button
                aria-label={`Add group to ${formatVpsName(agent, vpsNameDisplayMode)}`}
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
            <small id={`group-suggestions-${agent.id}`}>{suggestionsText}</small>
          </span>
        ),
        enableHiding: false,
        header: "Add group",
        id: "addTag",
      },
    ],
    [addTag, fleetAlertPolicies, pending, removeTag, schedules, suggestionsText, tagByAgent, vpsNameDisplayMode],
  );

  return (
    <>
      {recentRemoval && (
        <div className="tagAssignmentNotice" role="status" aria-live="polite">
          <span>
            Removed <strong>{recentRemoval.tag}</strong> from <strong>{recentRemoval.agentLabel}</strong>.
          </span>
          {recentRemoval.scheduleImpactCount > 0 && (
            <small>
              Used by {recentRemoval.scheduleImpactCount} schedule{recentRemoval.scheduleImpactCount === 1 ? "" : "s"}; saved targets stay fixed until updated.
            </small>
          )}
          <button className="secondaryAction compactAction" disabled={pending} onClick={undoRemoveTag} type="button">
            Undo
          </button>
        </div>
      )}
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
            <span>Groups</span>
            <strong>{agent.tags.join(", ") || "None"}</strong>
          </div>
        )}
        rows={agents}
        searchPlaceholder="Search VPS assignments"
        selectable={false}
        storageKey="vpsman.tags.assignments"
        title="VPS group assignments"
      />
      <datalist id="tag-options">
        {tagNames.map((tag) => (
          <option key={tag} label={groupOptionLabel(tag, tagClientsCount(tags, tag))} value={tag} />
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
  onDeleteTag: (
    tag: string,
    confirmed: boolean,
    privilegeAssertion?: PrivilegeAssertion | null,
    previewHash?: string | null,
  ) => Promise<TagMutationResponse>;
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
  const [resolvedTargets, setResolvedTargets] = useState<BulkResolveResponse | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [mutationSnapshot, setMutationSnapshot] = useState<BulkTagMutationSnapshot | null>(null);
  const [previewStatus, setPreviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);
  const trimmedTag = tag.trim();
  const trimmedSelector = selectorExpression.trim();
  const localTargets = useMemo(
    () =>
      trimmedSelector && !selectorParse.error
        ? agentsMatchingExpression(agents, trimmedSelector)
        : [],
    [agents, selectorParse.error, trimmedSelector],
  );
  const targetCountForAction =
    action === "delete"
      ? (preview?.target_count ?? 0)
      : (resolvedTargets?.target_count ?? localTargets.length);
  const canReviewMutation = Boolean(
    trimmedTag &&
      (action === "delete" ||
        (trimmedSelector && !selectorParse.error && localTargets.length > 0)),
  );

  useEffect(() => writeLocalString(TAG_BULK_SELECTOR_STORAGE_KEY, selectorExpression), [selectorExpression]);

  function clearMutationPreview() {
    invalidateReviewGeneration();
    setPreview(null);
    setResolvedTargets(null);
    setMutationSnapshot(null);
    setConfirmOpen(false);
    setPreviewStatus(null);
  }

  async function reviewMutation() {
    const reviewGeneration = captureReviewGeneration();
    const frozenAction = action;
    const frozenTag = trimmedTag;
    const frozenSelector = trimmedSelector;
    setPreviewStatus(frozenAction === "delete" ? "Preparing delete preview" : "Resolving targets and preparing preview");
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
          setResolvedTargets(null);
          setMutationSnapshot({
            action: frozenAction,
            preview: nextPreview,
            selectorExpression: "",
            tag: frozenTag,
          });
          setConfirmOpen(true);
          return;
        }
        const resolved = await onResolveBulk(frozenSelector);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setResolvedTargets(resolved);
        const targetClientIds = resolved.targets.map((target) => target.id);
        if (!targetClientIds.length) {
          throw new Error("Bulk group action resolved no VPSs");
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
        setMutationSnapshot({
          action: frozenAction,
          preview: nextPreview,
          selectorExpression: frozenSelector,
          tag: frozenTag,
        });
        setConfirmOpen(true);
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
        setLastMutation(await onDeleteTag(snapshot.tag, true, privilegeAssertion, snapshot.preview.preview_hash));
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
          preview_hash: snapshot.preview.preview_hash,
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
        <strong>Bulk tag mutation</strong>
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
              verificationMessage={selectorParse.error ?? (selectorExpression.trim() ? targetStatusText("Local match", localTargets) : undefined)}
            />
            <div className="bulkTargetResolution" aria-label="Bulk group target resolution">
              <span>
                {selectorExpression.trim()
                  ? selectorParse.error
                    ? selectorParse.error
                    : targetStatusText("Local match", localTargets)
                  : "Enter a selector to estimate local matches."}
              </span>
              <span>
                {resolvedTargets
                  ? targetStatusText("Server resolved", resolvedTargets.targets)
                  : "Server resolution runs before confirmation."}
              </span>
            </div>
          </>
        )}
        <div className={`privilegeGateBox ${privilegeMaterial ? "ready" : ""}`}>
          <ShieldCheck size={16} />
          <span>{privilegeMaterial ? "Privilege unlocked for final apply" : "Preview works now; unlock only when applying."}</span>
          {!privilegeMaterial && (
            <button className="secondaryAction compactAction" onClick={onOpenPrivilegeUnlock} type="button">
              Open Privilege Vault
            </button>
          )}
        </div>
        <button
          className="primaryAction"
          disabled={pending || !canReviewMutation}
          onClick={() => void reviewMutation()}
          type="button"
        >
          <Tag size={16} />
          {previewStatus ?? bulkMutationPrimaryLabel(action, trimmedTag, targetCountForAction)}
        </button>
      </div>
      {(preview || previewStatus) && (
      <section className="bulkTagPreviewPanel" aria-label="Bulk tag target preview">
        <div className="bulkTagPreviewHeader">
          <div>
            <strong>Server preview</strong>
            <span>{previewStatus ?? (preview ? `${preview.target_count} resolved / ${preview.changed_count} changes` : "Resolving target snapshot")}</span>
          </div>
        </div>
        {preview && (
          <div className="bulkTagPreviewStats" aria-label="Bulk group preview evidence">
            <span>
              <strong>{preview.target_count}</strong>
              <small>selected</small>
            </span>
            <span>
              <strong>{preview.changed_count}</strong>
              <small>changed</small>
            </span>
            <span>
              <strong>{preview.skipped_count}</strong>
              <small>no-change</small>
            </span>
            <span>
              <strong>{preview.schedule_impacts.length}</strong>
              <small>schedule impacts</small>
            </span>
            <span>
              <strong title={preview.preview_hash}>{preview.preview_hash}</strong>
              <small>preview hash</small>
            </span>
          </div>
        )}
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
      )}
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
          { label: "Excluded / no-change", value: String(confirmationPreview?.skipped_count ?? 0) },
          { label: "Membership after apply", value: membershipOutcomeText(confirmationSnapshot?.action ?? action, confirmationPreview) },
          {
            label: "Preview hash",
            title: confirmationPreview?.preview_hash,
            value: confirmationPreview?.preview_hash ?? "-",
          },
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
