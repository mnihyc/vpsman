import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from "react";
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type Announcements,
  type CollisionDetection,
  type DragEndEvent,
  type DragOverEvent,
  type DragStartEvent,
  type DroppableContainer,
  type KeyboardCoordinateGetter,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  ArrowDownAZ,
  ChevronRight,
  ChevronsUpDown,
  GripVertical,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  ShieldCheck,
  Tag,
  Trash2,
  X,
} from "lucide-react";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../components/ActionFeedback";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { ConsoleActionDrawer } from "../components/ConsoleLayout";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
import { scrollIntoViewWithMotion } from "../motion";
import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { usePanelDisplaySettings } from "../panelDisplay";
import { agentDisplayState } from "../agentDisplayState";
import type {
  AgentView,
  BulkResolveResponse,
  BulkTagMutationRequest,
  FleetAlertPolicyRecord,
  ScheduleRecord,
  TagMutationResponse,
  TagOrderState,
  TagView,
  UpdateTagOrderRequest,
} from "../types";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
  type PrivilegeAssertion,
} from "../privilege";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  selectorExpressionForClientIds,
  VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE,
  vpsRuleSearchUnavailable,
} from "../searchExpression";
import { useVpsRuleSearchContext } from "../vpsRuleSearchContext";
import { formatVpsName, runPanelAction } from "../utils";
import {
  buildTagOrderBlocks,
  moveTagOrderBlock,
  moveTagOrderLeaf,
  naturallySortTagOrderBlock,
  naturallySortedTagNames,
  normalizeNaturalTagOrder,
  reconcileTagOrderDraft,
  sameTagOrder,
  tagNamespaceDisplayLabel,
  tagOrderLeafId,
  type TagOrderBlock,
} from "../tagOrder";
import { LocalTargetPreview } from "./TargetImpactPreview";

const TAG_BULK_SELECTOR_STORAGE_KEY = "vpsman.tags.bulk.selectorExpression";
const TAG_ORDER_EXPANSION_STORAGE_KEY = "vpsman.tags.order.expansion";
const SILENT_TAG_ORDER_DND_ANNOUNCEMENTS: Announcements = {
  onDragCancel: () => undefined,
  onDragEnd: () => undefined,
  onDragOver: () => undefined,
  onDragStart: () => undefined,
};

type TagOrderDndData =
  | {
      expanded: boolean;
      kind: "block";
      names: string[];
    }
  | {
      kind: "leaf";
      name: string;
      topLevel: boolean;
    };

const tagOrderKeyboardCoordinates: KeyboardCoordinateGetter = (event, args) => {
  const direction =
    event.code === "ArrowDown" || event.code === "ArrowRight"
      ? 1
      : event.code === "ArrowUp" || event.code === "ArrowLeft"
        ? -1
        : 0;
  const { collisionRect } = args.context;
  const activeData = readTagOrderDndData(args.context.active?.data.current);
  if (direction === 0 || !collisionRect || !activeData) {
    return sortableKeyboardCoordinates(event, args);
  }

  event.preventDefault();
  const activeCenter = collisionRect.top + collisionRect.height / 2;
  const target = eligibleTagOrderDroppables(
    activeData,
    args.context.droppableContainers.getEnabled(),
  )
    .filter((container) => container.id !== args.active)
    .map((container) => ({
      container,
      rect: args.context.droppableRects.get(container.id),
    }))
    .filter(
      (
        candidate,
      ): candidate is {
        container: DroppableContainer;
        rect: NonNullable<typeof candidate.rect>;
      } => candidate.rect !== undefined,
    )
    .map((candidate) => ({
      ...candidate,
      distance:
        direction *
        (candidate.rect.top + candidate.rect.height / 2 - activeCenter),
    }))
    .filter((candidate) => candidate.distance > 1)
    .sort(
      (left, right) =>
        left.distance - right.distance || left.rect.left - right.rect.left,
    )[0];
  if (!target) return undefined;
  return {
    x: target.rect.left + (target.rect.width - collisionRect.width) / 2,
    y: target.rect.top + (target.rect.height - collisionRect.height) / 2,
  };
};

const tagOrderCollisionDetection: CollisionDetection = (args) => {
  const activeData = readTagOrderDndData(args.active.data.current);
  return closestCenter({
    ...args,
    droppableContainers: activeData
      ? eligibleTagOrderDroppables(activeData, args.droppableContainers)
      : args.droppableContainers,
  });
};

function eligibleTagOrderDroppables(
  activeData: TagOrderDndData,
  containers: DroppableContainer[],
): DroppableContainer[] {
  return containers.filter((container) => {
    const candidate = readTagOrderDndData(container.data.current);
    if (!candidate) return false;
    if (activeData.kind === "block") {
      return candidate.kind === "block" || candidate.topLevel;
    }
    if (candidate.kind === "leaf") return true;
    return !candidate.expanded && !candidate.names.includes(activeData.name);
  });
}

function readTagOrderDndData(data: unknown): TagOrderDndData | null {
  if (!data || typeof data !== "object" || !("tagOrder" in data)) {
    return null;
  }
  const tagOrder = (data as { tagOrder?: unknown }).tagOrder;
  if (!tagOrder || typeof tagOrder !== "object" || !("kind" in tagOrder)) {
    return null;
  }
  if (
    tagOrder.kind === "block" &&
    "expanded" in tagOrder &&
    typeof tagOrder.expanded === "boolean" &&
    "names" in tagOrder &&
    Array.isArray(tagOrder.names) &&
    tagOrder.names.every((name) => typeof name === "string")
  ) {
    return {
      expanded: tagOrder.expanded,
      kind: "block",
      names: tagOrder.names,
    };
  }
  if (
    tagOrder.kind === "leaf" &&
    "name" in tagOrder &&
    typeof tagOrder.name === "string" &&
    "topLevel" in tagOrder &&
    typeof tagOrder.topLevel === "boolean"
  ) {
    return {
      kind: "leaf",
      name: tagOrder.name,
      topLevel: tagOrder.topLevel,
    };
  }
  return null;
}

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
  namespaceNaturalSortEnabled,
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
  namespaceNaturalSortEnabled: boolean;
  onAssignTag: (
    clientId: string,
    tag: string,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<TagMutationResponse>;
  onBulkMutateTags: (
    request: BulkTagMutationRequest,
  ) => Promise<TagMutationResponse>;
  onCreateTag: (
    name: string,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
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
  onUpdateTagOrder: (request: UpdateTagOrderRequest) => Promise<TagOrderState>;
  privilegeMaterial: PrivilegeMaterial | null;
  schedules: ScheduleRecord[];
  tags: TagView[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
}) {
  const subpage = ["registry", "assignments", "bulk"].includes(activeSubpage)
    ? activeSubpage
    : "registry";
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionStatus, setActionStatus] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [lastMutation, setLastMutation] = useState<TagMutationResponse | null>(
    null,
  );
  const groupSummary = useMemo(
    () => buildGroupSummary(tags, agents),
    [agents, tags],
  );
  const activeLabelCount =
    groupSummary.providerGroupCount +
    groupSummary.countryGroupCount +
    groupSummary.customGroupCount;
  const headerDescription = `${tags.length} registry groups, ${activeLabelCount} active labels across ${agents.length} VPSs`;
  const groupsPageFeedbackMessage =
    error ?? (loading ? "Refreshing group state" : null);
  const groupsPageFeedbackTone = error ? "danger" : "progress";
  const groupsActionFeedbackMessage =
    actionError ??
    actionStatus ??
    (lastMutation
      ? `Group ${lastMutation.tag}: ${lastMutation.changed_count} changed, ${lastMutation.skipped_count} skipped`
      : null);
  const groupsActionFeedbackTone = actionError ? "danger" : "success";
  const runGroupAction = (action: () => Promise<void>) => {
    setActionStatus(null);
    setLastMutation(null);
    return runPanelAction(setPending, setActionError, action);
  };
  const clearGroupActionFeedback = useCallback(() => {
    setActionError(null);
    setActionStatus(null);
    setLastMutation(null);
  }, []);

  useEffect(() => {
    setActionError(null);
    setActionStatus(null);
    setLastMutation(null);
  }, [subpage]);

  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>
              {subpage === "bulk"
                ? "Bulk groups"
                : subpage === "assignments"
                  ? "Group assignments"
                  : "Fleet groups"}
            </h2>
            <span>{headerDescription}</span>
          </div>
          <div className="headerActionStack">
            <button
              className="secondaryAction"
              data-tooltip-disabled-reason={
                loading
                  ? "Fleet groups are still loading"
                  : pending
                    ? "Another fleet group change is in progress"
                    : undefined
              }
              disabled={loading || pending}
              onClick={onRefresh}
              type="button"
            >
              <RefreshCw size={15} />
              <span>Refresh</span>
            </button>
            <ActionFeedback
              message={groupsPageFeedbackMessage}
              tone={groupsPageFeedbackTone}
            />
          </div>
        </div>
        <GroupSummaryStrip summary={groupSummary} />
        {subpage === "registry" && (
          <TagRegistry
            actionFeedbackMessage={groupsActionFeedbackMessage}
            actionFeedbackTone={groupsActionFeedbackTone}
            onClearActionFeedback={clearGroupActionFeedback}
            onCreateTag={onCreateTag}
            onDeleteTag={onDeleteTag}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onOpenSchedules={onOpenSchedules}
            onUpdateTagOrder={onUpdateTagOrder}
            namespaceNaturalSortEnabled={namespaceNaturalSortEnabled}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={runGroupAction}
            setActionStatus={setActionStatus}
            setLastMutation={setLastMutation}
            tags={tags}
          />
        )}
        {subpage === "assignments" && (
          <TagAssignments
            actionFeedbackMessage={groupsActionFeedbackMessage}
            actionFeedbackTone={groupsActionFeedbackTone}
            agents={agents}
            onAssignTag={onAssignTag}
            onBulkMutateTags={onBulkMutateTags}
            onClearActionFeedback={clearGroupActionFeedback}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={runGroupAction}
            schedules={schedules}
            setLastMutation={setLastMutation}
            tags={tags}
            fleetAlertPolicies={fleetAlertPolicies}
          />
        )}
        {subpage === "bulk" && (
          <BulkTagPanel
            actionFeedbackMessage={groupsActionFeedbackMessage}
            actionFeedbackTone={groupsActionFeedbackTone}
            agents={agents}
            onBulkMutateTags={onBulkMutateTags}
            onDeleteTag={onDeleteTag}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onOpenSchedules={onOpenSchedules}
            onResolveBulk={onResolveBulk}
            pending={pending}
            privilegeMaterial={privilegeMaterial}
            runAction={runGroupAction}
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
  contactReviewCount: number;
  countryGroupCount: number;
  customGroupCount: number;
  offlineCount: number;
  providerGroupCount: number;
  reachableCount: number;
  totalAssignments: number;
};

function GroupSummaryStrip({ summary }: { summary: GroupSummary }) {
  return (
    <div className="groupSummaryStrip" aria-label="Fleet group counts">
      <span>
        <strong>{summary.providerGroupCount}</strong>
        <small>provider groups</small>
      </span>
      <span>
        <strong>{summary.countryGroupCount}</strong>
        <small>country groups</small>
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
        <small>assigned VPS</small>
      </span>
      <span>
        <strong>
          {summary.reachableCount}/{summary.contactReviewCount}/
          {summary.offlineCount}
        </strong>
        <small>reachable/review/offline</small>
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
  const displayStates = agents.map((agent) => agentDisplayState(agent));
  return {
    assignedVpsCount: assignedVpsIds.size,
    contactReviewCount: displayStates.filter(
      (state) =>
        state.label !== "Online" &&
        state.label !== "Offline" &&
        (state.tone === "warning" || state.tone === "critical"),
    ).length,
    countryGroupCount: groupNameList.filter((tag) => isCountryGroup(tag))
      .length,
    customGroupCount: groupNameList.filter(
      (tag) => !isProviderGroup(tag) && !isCountryGroup(tag),
    ).length,
    offlineCount: displayStates.filter((state) => state.label === "Offline")
      .length,
    providerGroupCount: groupNameList.filter((tag) => isProviderGroup(tag))
      .length,
    reachableCount: displayStates.filter((state) => state.label === "Online")
      .length,
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
  if (kind === "provider") return "Provider group";
  if (kind === "country") return "Country group";
  return "Operator group";
}

function groupKindTone(tag: string) {
  return groupKind(tag) === "custom" ? "ok" : "info";
}

function groupKindDetail(tag: string) {
  const kind = groupKind(tag);
  if (kind === "provider") {
    return "Structured provider group for scoped filters and recurring targets.";
  }
  if (kind === "country") {
    return "Structured country group for regional filters and recurring targets.";
  }
  return "Custom group for recurring VPS targeting.";
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

function groupNameValidationError(value: string): string | null {
  const name = value.trim();
  if (name.length > 128) {
    return "Group names must be 128 characters or fewer.";
  }
  if (name.includes(",")) {
    return "Use one group name; commas are not accepted.";
  }
  if (name && name.split(":").some((segment) => segment.length === 0)) {
    return "Every group segment before or after a colon must have a value.";
  }
  if (name && !/^[A-Za-z0-9._:-]+$/.test(name)) {
    return "Use letters, numbers, periods, dashes, underscores, and colons only.";
  }
  if (name.startsWith("id:") || name.startsWith("name:")) {
    return "The id: and name: prefixes are reserved for VPS selectors.";
  }
  return null;
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
      policy.enabled &&
      selectorReferencesGroup(policy.selector_expression, tag),
  ).length;
  return {
    alertPolicies: policyCount,
    schedules: scheduleCount,
    total: scheduleCount + policyCount,
  };
}

function selectorReferencesGroup(
  selector: string | null | undefined,
  tag: string,
) {
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
    new RegExp(
      `(^|[^a-z0-9_:-])${escapeRegExp(variant)}($|[^a-z0-9_:-])`,
      "i",
    ).test(haystack),
  );
}

function dependencySummaryText(summary: GroupDependencySummary) {
  if (summary.total === 0) {
    return "No automation references";
  }
  const parts = [];
  if (summary.schedules > 0) {
    parts.push(
      `${summary.schedules} schedule${summary.schedules === 1 ? "" : "s"}`,
    );
  }
  if (summary.alertPolicies > 0) {
    parts.push(
      `${summary.alertPolicies} alert polic${summary.alertPolicies === 1 ? "y" : "ies"}`,
    );
  }
  return `Used by ${parts.join(" and ")}`;
}

type TargetStatusCounts = {
  offline: number;
  ready: number;
  review: number;
  eligible: number;
  total: number;
};

function targetStatusCounts(
  targets: AgentView[],
  includeReviewTargets = false,
): TargetStatusCounts {
  const states = targets.map((target) => agentDisplayState(target));
  const ready = states.filter((state) => state.label === "Online").length;
  const review = states.filter(
    (state) =>
      state.label !== "Offline" &&
      (state.tone === "warning" || state.tone === "critical"),
  ).length;
  const offline = states.filter((state) => state.label === "Offline").length;
  return {
    eligible: ready + (includeReviewTargets ? review : 0),
    offline,
    ready,
    review,
    total: targets.length,
  };
}

function targetStatusText(
  prefix: string,
  targets: AgentView[],
  includeReviewTargets = false,
) {
  const counts = targetStatusCounts(targets, includeReviewTargets);
  const parts = [
    `${prefix} ${bulkVpsCountLabel(counts.total)}`,
    `${counts.ready} ready`,
    `${counts.review} needs review`,
  ];
  if (counts.offline > 0) {
    parts.push(`${counts.offline} offline`);
  }
  if (counts.review > 0 && !includeReviewTargets) {
    parts.push("review targets excluded");
  }
  if (includeReviewTargets && counts.review > 0) {
    parts.push(`${counts.eligible} included`);
  }
  return parts.join(" · ");
}

function tagMutationEligibleTargets(
  targets: AgentView[],
  includeReviewTargets: boolean,
) {
  return targets.filter((target) => {
    const state = agentDisplayState(target);
    if (state.label === "Online") {
      return true;
    }
    return includeReviewTargets && state.label !== "Offline";
  });
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
  actionFeedbackMessage,
  actionFeedbackTone,
  namespaceNaturalSortEnabled,
  onClearActionFeedback,
  onCreateTag,
  onDeleteTag,
  onOpenPrivilegeUnlock,
  onOpenSchedules,
  onUpdateTagOrder,
  pending,
  privilegeMaterial,
  runAction,
  setActionStatus,
  setLastMutation,
  tags,
}: {
  actionFeedbackMessage: string | null;
  actionFeedbackTone: "danger" | "success";
  namespaceNaturalSortEnabled: boolean;
  onClearActionFeedback: () => void;
  onCreateTag: (
    name: string,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<void>;
  onDeleteTag: (
    tag: string,
    confirmed: boolean,
    privilegeAssertion?: PrivilegeAssertion | null,
    previewHash?: string | null,
  ) => Promise<TagMutationResponse>;
  onOpenPrivilegeUnlock: () => void;
  onOpenSchedules?: () => void;
  onUpdateTagOrder: (request: UpdateTagOrderRequest) => Promise<TagOrderState>;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  setActionStatus: (status: string | null) => void;
  setLastMutation: (response: TagMutationResponse | null) => void;
  tags: TagView[];
}) {
  const [tagName, setTagName] = useState("");
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteCandidate, setDeleteCandidate] = useState<TagView | null>(null);
  const [deletePreview, setDeletePreview] =
    useState<TagMutationResponse | null>(null);
  const actionFeedbackRef = useRef<HTMLDivElement | null>(null);
  const trimmedGroupName = tagName.trim();
  const groupNameError = groupNameValidationError(tagName);

  useEffect(() => {
    if (!actionFeedbackMessage || createOpen) return;
    const frame = window.requestAnimationFrame(() => {
      if (actionFeedbackRef.current) {
        scrollIntoViewWithMotion(actionFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [actionFeedbackMessage, createOpen]);

  async function submitTag(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!trimmedGroupName || groupNameError) {
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
      setCreateOpen(false);
      setLastMutation(null);
      setActionStatus(`Created group ${tag}`);
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
    if (!candidate || !preview) {
      return;
    }
    await runAction(async () => {
      const targetIds = (preview?.affected ?? candidate.clients).map(
        (client) => client.id,
      );
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
      setDeleteCandidate(null);
      setDeletePreview(null);
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
        header: "Assigned VPS",
        id: "assigned",
        searchValue: (tag) => tag.clients.length,
        sortValue: (tag) => tag.clients.length,
      },
    ],
    [],
  );

  return (
    <>
      <ActionFeedback
        className="localActionFeedback"
        message={createOpen ? null : actionFeedbackMessage}
        ref={actionFeedbackRef}
        tone={actionFeedbackTone}
      />
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
            <span>
              Create operator groups to target recurring VPS workflows.
            </span>
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
            <strong>
              {tag.clients.map((client) => client.id).join(", ") || "None"}
            </strong>
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
        searchPlaceholder="Search groups or namespaces"
        storageKey="vpsman.tags.registry"
        title="Group registry"
        toolbarActions={
          <button
            className="primaryAction compactAction"
            data-tooltip-disabled-reason={
              pending
                ? "Wait for the current group change to finish"
                : undefined
            }
            disabled={pending}
            onClick={() => {
              onClearActionFeedback();
              setTagName("");
              setCreateOpen(true);
            }}
            type="button"
          >
            <Plus size={14} />
            Create group
          </button>
        }
      />
      <TagOrderManager
        disabled={pending}
        namespaceNaturalSortEnabled={namespaceNaturalSortEnabled}
        onUpdateTagOrder={onUpdateTagOrder}
        tags={tags}
      />
      <ConsoleActionDrawer
        description="Create one reusable fleet group. Assignment can also create a valid group when needed."
        onClose={() => {
          if (pending) return;
          onClearActionFeedback();
          setCreateOpen(false);
          setTagName("");
        }}
        open={createOpen}
        title="Create group"
      >
        <form className="consoleFormGrid" onSubmit={submitTag}>
          <ActionFeedback
            className="localActionFeedback fieldFull"
            message={actionFeedbackMessage}
            tone={actionFeedbackTone}
          />
          <label className="consoleField fieldFull">
            <span>Group name</span>
            <input
              aria-describedby="group-name-hint"
              aria-label="Group name"
              data-action-drawer-initial-focus="true"
              maxLength={129}
              onChange={(event) => setTagName(event.target.value)}
              placeholder="role:edge or maintenance"
              value={tagName}
            />
            <small id="group-name-hint">
              {groupNameError ??
                "Use provider: or country: for structured groups; other valid names create operator groups."}
            </small>
          </label>
          <div className="consoleFormActions fieldFull">
            <button
              className="secondaryAction"
              data-tooltip-disabled-reason={
                pending
                  ? "Wait for the current group change to finish"
                  : undefined
              }
              disabled={pending}
              onClick={() => {
                onClearActionFeedback();
                setCreateOpen(false);
                setTagName("");
              }}
              type="button"
            >
              Cancel
            </button>
            <button
              className="primaryAction"
              data-tooltip-disabled-reason={
                pending
                  ? "Wait for the current group change to finish"
                  : !trimmedGroupName
                    ? "Enter a group name before creating it"
                    : (groupNameError ?? undefined)
              }
              disabled={pending || !trimmedGroupName || groupNameError !== null}
              type="submit"
            >
              <Plus size={14} />
              Create group
            </button>
          </div>
        </form>
      </ConsoleActionDrawer>
      <ConfirmationPrompt
        confirmLabel="Delete group"
        detail="Delete this group and remove it from assigned VPSs. Recreate and reassign it to use the group again."
        error={
          actionFeedbackTone === "danger"
            ? (actionFeedbackMessage ?? undefined)
            : undefined
        }
        items={[
          { label: "Group", value: deleteCandidate?.name ?? "-" },
          {
            label: "Type",
            value: deleteCandidate ? groupKindLabel(deleteCandidate.name) : "-",
          },
          {
            label: "Assignments",
            value: String(
              deletePreview?.target_count ??
                deleteCandidate?.clients.length ??
                0,
            ),
          },
          {
            label: "Preview hash",
            title: deletePreview?.preview_hash,
            value: deletePreview?.preview_hash ?? "-",
          },
          {
            label: "Schedule target notices",
            value: (
              <ScheduleImpactTable
                impacts={deletePreview?.schedule_impacts ?? []}
                onOpenSchedules={onOpenSchedules}
              />
            ),
          },
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
  namespaceNaturalSortEnabled,
  onUpdateTagOrder,
  tags,
}: {
  disabled: boolean;
  namespaceNaturalSortEnabled: boolean;
  onUpdateTagOrder: (request: UpdateTagOrderRequest) => Promise<TagOrderState>;
  tags: TagView[];
}) {
  const incomingNames = useMemo(() => tags.map((tag) => tag.name), [tags]);
  const [editor, setEditor] = useState<TagOrderEditorState>(() => ({
    baseNames: incomingNames,
    baseNaturalSortEnabled: namespaceNaturalSortEnabled,
    names: incomingNames,
    naturalSortEnabled: namespaceNaturalSortEnabled,
  }));
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [activeDrag, setActiveDrag] = useState<TagOrderDragItem | null>(null);
  const [dropTarget, setDropTarget] = useState<TagOrderDropTarget | null>(null);
  const [dragAnnouncement, setDragAnnouncement] = useState("");
  const [expansion, setExpansion] = useState<TagOrderExpansionState>(() =>
    readTagOrderExpansionState(),
  );
  const tagByName = useMemo(
    () => new Map(tags.map((tag) => [tag.name, tag])),
    [tags],
  );
  const blocks = useMemo(
    () => buildTagOrderBlocks(editor.names),
    [editor.names],
  );
  const tagIndexes = useMemo(
    () => new Map(editor.names.map((name, index) => [name, index])),
    [editor.names],
  );
  const totalAssignments = editor.names.reduce(
    (total, name) => total + (tagByName.get(name)?.clients.length ?? 0),
    0,
  );
  const multiTagBlocks = blocks.filter(
    (block) => block.namespace !== null && block.names.length > 1,
  );
  const multiTagBlockIdsKey = multiTagBlocks
    .map((block) => block.id)
    .join("\u0000");
  const currentExpandedBlockIds = new Set(expansion.expandedBlockIds);
  const allCurrentBlocksExpanded =
    multiTagBlocks.length > 0 &&
    multiTagBlocks.every((block) => currentExpandedBlockIds.has(block.id));
  const dirty =
    editor.naturalSortEnabled !== editor.baseNaturalSortEnabled ||
    !sameTagOrder(editor.names, editor.baseNames);
  const interactionDisabled = disabled || saving;
  const interactionDisabledReason = saving
    ? "The updated tag order is being saved"
    : disabled
      ? "Tag order is locked while another group change is in progress"
      : undefined;
  const status = saving
    ? "Saving order"
    : saveError
      ? saveError
      : dirty
        ? "Unsaved changes"
        : "Saved";
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: tagOrderKeyboardCoordinates,
    }),
  );

  useEffect(() => {
    setEditor((current) => {
      const clean =
        current.naturalSortEnabled === current.baseNaturalSortEnabled &&
        sameTagOrder(current.names, current.baseNames);
      return {
        baseNames: incomingNames,
        baseNaturalSortEnabled: namespaceNaturalSortEnabled,
        names: clean
          ? incomingNames
          : reconcileTagOrderDraft(
              current.names,
              incomingNames,
              current.naturalSortEnabled,
            ),
        naturalSortEnabled: clean
          ? namespaceNaturalSortEnabled
          : current.naturalSortEnabled,
      };
    });
  }, [incomingNames, namespaceNaturalSortEnabled]);

  useEffect(() => {
    writeTagOrderExpansionState(expansion);
  }, [expansion]);

  useEffect(() => {
    setExpansion((current) => {
      const expandedBlockIds = preserveExpandedTagOrderBlocks(
        current.expandedBlockIds,
        blocks,
      );
      return sameTagOrder(expandedBlockIds, current.expandedBlockIds)
        ? current
        : { ...current, expandedBlockIds };
    });
  }, [multiTagBlockIdsKey]);

  function stageNames(nextNames: string[]) {
    setSaveError(null);
    setExpansion((current) => ({
      ...current,
      expandedBlockIds: preserveExpandedTagOrderBlocks(
        current.expandedBlockIds,
        buildTagOrderBlocks(nextNames),
      ),
    }));
    setEditor((current) => ({ ...current, names: nextNames }));
  }

  function handleDragStart(event: DragStartEvent) {
    const activeId = String(event.active.id);
    const activeBlock = blocks.find(
      (block) => block.names.length > 1 && block.id === activeId,
    );
    const activeName = tagNameForDndId(editor.names, activeId);
    const nextActive = activeBlock
      ? {
          count: activeBlock.names.length,
          id: activeId,
          kind: "block" as const,
          label: `${tagNamespaceDisplayLabel(activeBlock.names[0] ?? "tags")} group`,
        }
      : activeName
        ? {
            count: 1,
            id: activeId,
            kind: "tag" as const,
            label: activeName,
          }
        : null;
    setActiveDrag(nextActive);
    setDropTarget(null);
    if (nextActive) {
      setDragAnnouncement(`Picked up ${nextActive.label}.`);
    }
  }

  function handleDragOver(event: DragOverEvent) {
    const activeId = String(event.active.id);
    const overId = event.over ? String(event.over.id) : null;
    const activeBlockIndex = blocks.findIndex(
      (block) => block.names.length > 1 && block.id === activeId,
    );
    const overBlock = overId ? tagOrderBlockForDndId(blocks, overId) : null;
    const overBlockIndex = overBlock ? blocks.indexOf(overBlock) : -1;
    const activeName = tagNameForDndId(editor.names, activeId);
    const exactOverName = overId
      ? tagNameForDndId(editor.names, overId)
      : undefined;
    let nextDropTarget: TagOrderDropTarget | null = null;
    if (
      activeBlockIndex >= 0 &&
      overBlock &&
      activeBlockIndex !== overBlockIndex
    ) {
      nextDropTarget = {
        id: tagOrderTopLevelDndId(overBlock),
        placement: activeBlockIndex < overBlockIndex ? "after" : "before",
      };
    } else if (activeName && exactOverName && activeName !== exactOverName) {
      nextDropTarget = {
        id: tagOrderLeafId(exactOverName),
        placement:
          editor.names.indexOf(activeName) < editor.names.indexOf(exactOverName)
            ? "after"
            : "before",
      };
    } else if (activeName && overBlock && overId === overBlock.id) {
      const firstBlockName = overBlock.names[0];
      if (firstBlockName && activeName !== firstBlockName) {
        const sameBlock = overBlock.names.includes(activeName);
        nextDropTarget = {
          id: sameBlock ? tagOrderLeafId(firstBlockName) : overBlock.id,
          placement:
            sameBlock ||
            editor.names.indexOf(activeName) >
              editor.names.indexOf(firstBlockName)
              ? "before"
              : "after",
        };
      }
    }
    setDropTarget(nextDropTarget);
    if (
      nextDropTarget &&
      (nextDropTarget.id !== dropTarget?.id ||
        nextDropTarget.placement !== dropTarget.placement)
    ) {
      const targetLabel = tagOrderDndTargetLabel(blocks, nextDropTarget.id);
      if (targetLabel) {
        setDragAnnouncement(`Move ${nextDropTarget.placement} ${targetLabel}.`);
      }
    }
  }

  function handleDragCancel() {
    setActiveDrag(null);
    setDropTarget(null);
    setDragAnnouncement("Reorder canceled.");
  }

  function handleDragEnd(event: DragEndEvent) {
    const activeId = String(event.active.id);
    const overId = event.over ? String(event.over.id) : null;
    const draggedLabel = activeDrag?.label ?? "item";
    setActiveDrag(null);
    setDropTarget(null);
    if (!overId || activeId === overId || interactionDisabled) {
      setDragAnnouncement(`${draggedLabel} stayed in its current position.`);
      return;
    }
    const activeBlock = blocks.find(
      (block) => block.names.length > 1 && block.id === activeId,
    );
    const overBlock = tagOrderBlockForDndId(blocks, overId);
    if (activeBlock) {
      if (!overBlock) {
        setDragAnnouncement(`${draggedLabel} stayed in its current position.`);
        return;
      }
      const nextNames = moveTagOrderBlock(
        editor.names,
        activeBlock.id,
        overBlock.id,
        editor.naturalSortEnabled,
      );
      if (sameTagOrder(nextNames, editor.names)) {
        setDragAnnouncement(`${draggedLabel} stayed in its current position.`);
        return;
      }
      stageNames(nextNames);
      setDragAnnouncement(
        `${draggedLabel} order staged.${editor.naturalSortEnabled ? " Automatic namespace sorting was reapplied." : ""}`,
      );
      return;
    }
    const activeName = tagNameForDndId(editor.names, activeId);
    const exactOverName = tagNameForDndId(editor.names, overId);
    const activeIndex = activeName ? editor.names.indexOf(activeName) : -1;
    const overBlockStart = overBlock?.names[0];
    const overName =
      exactOverName ??
      (overBlock &&
      overBlockStart &&
      activeIndex < editor.names.indexOf(overBlockStart)
        ? overBlock.names[overBlock.names.length - 1]
        : overBlockStart);
    if (!activeName || !overName) {
      setDragAnnouncement(`${draggedLabel} stayed in its current position.`);
      return;
    }
    const nextNames = moveTagOrderLeaf(
      editor.names,
      activeName,
      overName,
      editor.naturalSortEnabled,
    );
    if (sameTagOrder(nextNames, editor.names)) {
      setDragAnnouncement(`${draggedLabel} stayed in its current position.`);
      return;
    }
    stageNames(nextNames);
    setDragAnnouncement(
      `${draggedLabel} order staged.${editor.naturalSortEnabled ? " Automatic namespace sorting was reapplied." : ""}`,
    );
  }

  async function saveOrder() {
    if (!dirty || interactionDisabled) return;
    setSaving(true);
    setSaveError(null);
    try {
      const updated = await onUpdateTagOrder({
        namespace_natural_sort_enabled: editor.naturalSortEnabled,
        ordered_tags: editor.names,
      });
      const updatedNames = updated.tags.map((tag) => tag.name);
      setEditor({
        baseNames: updatedNames,
        baseNaturalSortEnabled: updated.namespace_natural_sort_enabled,
        names: updatedNames,
        naturalSortEnabled: updated.namespace_natural_sort_enabled,
      });
    } catch (error) {
      setSaveError(
        error instanceof Error ? error.message : "Order save failed",
      );
    } finally {
      setSaving(false);
    }
  }

  function revertOrder() {
    if (!dirty || interactionDisabled) return;
    setSaveError(null);
    setEditor((current) => ({
      ...current,
      names: current.baseNames,
      naturalSortEnabled: current.baseNaturalSortEnabled,
    }));
  }

  function toggleNaturalSort(enabled: boolean) {
    if (interactionDisabled) return;
    setSaveError(null);
    setEditor((current) => ({
      ...current,
      names: enabled ? normalizeNaturalTagOrder(current.names) : current.names,
      naturalSortEnabled: enabled,
    }));
  }

  function toggleAllBlocks() {
    const currentIds = new Set(multiTagBlocks.map((block) => block.id));
    setExpansion((current) => {
      const next = new Set(
        current.expandedBlockIds.filter((id) => !currentIds.has(id)),
      );
      if (!allCurrentBlocksExpanded) {
        for (const id of currentIds) next.add(id);
      }
      return { ...current, expandedBlockIds: Array.from(next) };
    });
  }

  return (
    <section
      aria-busy={saving}
      aria-label="Manage display order"
      className="tagOrderPanel"
    >
      <div className="tagOrderPanelHeader">
        <div>
          <strong>Manage display order</strong>
          <span>Stage the shared fleet tag order, then save it once.</span>
        </div>
        <span
          aria-live="polite"
          className={`consoleStatusBadge ${saveError ? "danger" : dirty || saving ? "warning" : "ok"}`}
          role={saveError ? "alert" : "status"}
        >
          {status}
        </span>
      </div>
      <div
        aria-label="Tag order staged actions"
        className={`tagOrderSaveBar${saving ? " saving" : ""}`}
      >
        <span>
          {dirty
            ? "Review the staged hierarchy before saving it fleet-wide."
            : "The displayed hierarchy matches the saved fleet order."}
        </span>
        <div className="buttonCluster">
          <button
            className="secondaryAction compactAction"
            data-tooltip-disabled-reason={
              interactionDisabledReason ??
              (!dirty
                ? "There are no staged order changes to revert"
                : undefined)
            }
            disabled={!dirty || interactionDisabled}
            onClick={revertOrder}
            type="button"
          >
            <RotateCcw aria-hidden="true" size={15} />
            <span>Revert</span>
          </button>
          <button
            className="primaryAction compactAction"
            data-tooltip-disabled-reason={
              interactionDisabledReason ??
              (!dirty ? "There are no staged order changes to save" : undefined)
            }
            disabled={!dirty || interactionDisabled}
            onClick={() => void saveOrder()}
            type="button"
          >
            <Save aria-hidden="true" size={15} />
            <span>{saving ? "Saving" : "Save order"}</span>
          </button>
        </div>
      </div>
      <div className="tagOrderRoot">
        <div className="tagOrderRootHeader">
          <button
            aria-expanded={expansion.rootOpen}
            aria-label={`${expansion.rootOpen ? "Collapse" : "Expand"} Total tag order`}
            className="tagOrderDisclosure"
            onClick={() =>
              setExpansion((current) => ({
                ...current,
                rootOpen: !current.rootOpen,
              }))
            }
            type="button"
          >
            <ChevronRight aria-hidden="true" size={16} />
          </button>
          <div className="tagOrderRootIdentity">
            <strong>Total</strong>
            <span>
              {editor.names.length} exact tag
              {editor.names.length === 1 ? "" : "s"}
              {" · "}
              {totalAssignments} assignment
              {totalAssignments === 1 ? "" : "s"}
            </span>
          </div>
          <div className="tagOrderRootActions">
            <label className="tagOrderNaturalToggle">
              <input
                checked={editor.naturalSortEnabled}
                disabled={interactionDisabled}
                onChange={(event) => toggleNaturalSort(event.target.checked)}
                type="checkbox"
              />
              <span>Automatically sort tags within namespace groups</span>
            </label>
            <button
              aria-label={
                allCurrentBlocksExpanded
                  ? "Collapse all tag groups"
                  : "Expand all tag groups"
              }
              className="iconButton"
              disabled={multiTagBlocks.length === 0}
              onClick={toggleAllBlocks}
              title={
                allCurrentBlocksExpanded
                  ? "Collapse all precise tag lists."
                  : "Expand all precise tag lists."
              }
              type="button"
            >
              <ChevronsUpDown aria-hidden="true" size={16} />
            </button>
          </div>
        </div>
        {expansion.rootOpen && (
          <DndContext
            accessibility={{
              announcements: SILENT_TAG_ORDER_DND_ANNOUNCEMENTS,
            }}
            collisionDetection={tagOrderCollisionDetection}
            onDragCancel={handleDragCancel}
            onDragEnd={handleDragEnd}
            onDragOver={handleDragOver}
            onDragStart={handleDragStart}
            sensors={sensors}
          >
            <SortableContext
              items={blocks.map(tagOrderTopLevelDndId)}
              strategy={verticalListSortingStrategy}
            >
              {editor.names.length === 0 ? (
                <div className="emptyState compactEmptyState">
                  <ShieldCheck size={20} />
                  <strong>No groups</strong>
                  <span>Create groups before setting fleet display order.</span>
                </div>
              ) : (
                <div className="tagOrderList" role="list">
                  {blocks.map((block) => {
                    const multiTagBlock =
                      block.namespace !== null && block.names.length > 1;
                    if (multiTagBlock) {
                      return (
                        <SortableTagOrderBlock
                          activeDragId={activeDrag?.id ?? null}
                          block={block}
                          disabled={interactionDisabled}
                          disabledReason={interactionDisabledReason}
                          dropTarget={dropTarget}
                          expanded={currentExpandedBlockIds.has(block.id)}
                          key={block.id}
                          naturalSortEnabled={editor.naturalSortEnabled}
                          onSort={() =>
                            stageNames(
                              naturallySortTagOrderBlock(
                                editor.names,
                                block.id,
                              ),
                            )
                          }
                          onToggle={() =>
                            setExpansion((current) => ({
                              ...current,
                              expandedBlockIds: toggleStoredValue(
                                current.expandedBlockIds,
                                block.id,
                              ),
                            }))
                          }
                          tagByName={tagByName}
                          tagIndexes={tagIndexes}
                        />
                      );
                    }
                    const name = block.names[0];
                    const tag = name ? tagByName.get(name) : undefined;
                    if (!name || !tag) return null;
                    return (
                      <SortableTagOrderRow
                        child={false}
                        disabled={interactionDisabled}
                        disabledReason={interactionDisabledReason}
                        dropPlacement={
                          dropTarget?.id === tagOrderLeafId(name) &&
                          activeDrag?.id !== tagOrderLeafId(name)
                            ? dropTarget.placement
                            : null
                        }
                        id={tagOrderLeafId(name)}
                        index={(tagIndexes.get(name) ?? 0) + 1}
                        key={name}
                        tag={tag}
                      />
                    );
                  })}
                </div>
              )}
            </SortableContext>
            <DragOverlay dropAnimation={null}>
              {activeDrag ? (
                <div className="tagOrderDragOverlay">
                  <GripVertical aria-hidden="true" size={15} />
                  <strong>{activeDrag.label}</strong>
                  {activeDrag.kind === "block" && (
                    <span>
                      {activeDrag.count} exact tag
                      {activeDrag.count === 1 ? "" : "s"}
                    </span>
                  )}
                </div>
              ) : null}
            </DragOverlay>
          </DndContext>
        )}
      </div>
      <span aria-live="polite" className="srOnly" role="status">
        {dragAnnouncement}
      </span>
    </section>
  );
}

type TagOrderEditorState = {
  baseNames: string[];
  baseNaturalSortEnabled: boolean;
  names: string[];
  naturalSortEnabled: boolean;
};

type TagOrderExpansionState = {
  expandedBlockIds: string[];
  rootOpen: boolean;
};

type TagOrderDragItem = {
  count: number;
  id: string;
  kind: "block" | "tag";
  label: string;
};

type TagOrderDropPlacement = "after" | "before";

type TagOrderDropTarget = {
  id: string;
  placement: TagOrderDropPlacement;
};

function SortableTagOrderBlock({
  activeDragId,
  block,
  disabled,
  disabledReason,
  dropTarget,
  expanded,
  naturalSortEnabled,
  onSort,
  onToggle,
  tagByName,
  tagIndexes,
}: {
  activeDragId: string | null;
  block: TagOrderBlock;
  disabled: boolean;
  disabledReason?: string;
  dropTarget: TagOrderDropTarget | null;
  expanded: boolean;
  naturalSortEnabled: boolean;
  onSort: () => void;
  onToggle: () => void;
  tagByName: Map<string, TagView>;
  tagIndexes: Map<string, number>;
}) {
  const {
    attributes,
    isDragging,
    listeners,
    setNodeRef,
    transform,
    transition,
  } = useSortable({
    data: {
      tagOrder: {
        expanded,
        kind: "block",
        names: block.names,
      } satisfies TagOrderDndData,
    },
    disabled,
    id: block.id,
  });
  const firstName = block.names[0] ?? block.namespace ?? "tags";
  const label = tagNamespaceDisplayLabel(firstName);
  const naturallySorted = sameTagOrder(
    block.names,
    naturallySortedTagNames(block.names),
  );
  const startIndex = (tagIndexes.get(block.names[0] ?? "") ?? 0) + 1;
  const endIndex =
    (tagIndexes.get(block.names[block.names.length - 1] ?? "") ?? 0) + 1;
  const indexRange =
    startIndex === endIndex ? `${startIndex}` : `${startIndex}–${endIndex}`;
  const assignmentCount = block.names.reduce(
    (total, name) => total + (tagByName.get(name)?.clients.length ?? 0),
    0,
  );
  const sortDisabled = disabled || naturalSortEnabled || naturallySorted;
  const sortDisabledReason = disabledReason
    ? disabledReason
    : naturalSortEnabled
      ? "Automatic natural sorting is enabled for every tag group"
      : naturallySorted
        ? "This tag group is already in natural order"
        : undefined;
  return (
    <div
      className={`tagOrderBlock${isDragging ? " dragging" : ""}${dropTarget?.id === block.id && activeDragId !== block.id ? ` dropTarget drop${capitalizeDropPlacement(dropTarget.placement)}` : ""}`}
      ref={setNodeRef}
      role="listitem"
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
      }}
    >
      <div className="tagOrderBlockHeader">
        <button
          aria-label={`Reorder ${label} tag group`}
          className="tagOrderHandle"
          data-tooltip-disabled-reason={disabledReason}
          disabled={disabled}
          type="button"
          {...attributes}
          {...listeners}
        >
          <GripVertical aria-hidden="true" size={15} />
        </button>
        <button
          aria-expanded={expanded}
          aria-label={`${expanded ? "Collapse" : "Expand"} ${label} tag group`}
          className="tagOrderDisclosure"
          onClick={onToggle}
          type="button"
        >
          <ChevronRight aria-hidden="true" size={15} />
        </button>
        <div className="tagOrderBlockIdentity">
          <strong>{label}</strong>
          <span>{indexRange}</span>
          <span>
            {block.names.length} tag{block.names.length === 1 ? "" : "s"}
            {" · "}
            {assignmentCount} assignment{assignmentCount === 1 ? "" : "s"}
          </span>
        </div>
        <button
          aria-label={`Sort ${label} tag group naturally`}
          className="iconButton tagOrderNaturalSort"
          data-tooltip-disabled-reason={sortDisabledReason}
          disabled={sortDisabled}
          onClick={onSort}
          title={`Sort ${label} tag group naturally.`}
          type="button"
        >
          <ArrowDownAZ aria-hidden="true" size={15} />
        </button>
      </div>
      {expanded && (
        <SortableContext
          items={block.names.map(tagOrderLeafId)}
          strategy={verticalListSortingStrategy}
        >
          <div
            aria-label={`${label} precise tags`}
            className="tagOrderChildren"
            role="list"
          >
            {block.names.map((name) => {
              const tag = tagByName.get(name);
              if (!tag) return null;
              return (
                <SortableTagOrderRow
                  child
                  disabled={disabled}
                  disabledReason={disabledReason}
                  dropPlacement={
                    dropTarget?.id === tagOrderLeafId(name) &&
                    activeDragId !== tagOrderLeafId(name)
                      ? dropTarget.placement
                      : null
                  }
                  id={tagOrderLeafId(name)}
                  index={(tagIndexes.get(name) ?? 0) + 1}
                  key={name}
                  tag={tag}
                />
              );
            })}
          </div>
        </SortableContext>
      )}
    </div>
  );
}

function SortableTagOrderRow({
  child,
  disabled,
  disabledReason,
  dropPlacement,
  id,
  index,
  tag,
}: {
  child: boolean;
  disabled: boolean;
  disabledReason?: string;
  dropPlacement: TagOrderDropPlacement | null;
  id: string;
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
  } = useSortable({
    data: {
      tagOrder: {
        kind: "leaf",
        name: tag.name,
        topLevel: !child,
      } satisfies TagOrderDndData,
    },
    disabled,
    id,
  });
  return (
    <div
      className={`tagOrderRow${child ? " child" : ""}${isDragging ? " dragging" : ""}${dropPlacement ? ` dropTarget drop${capitalizeDropPlacement(dropPlacement)}` : ""}`}
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
        data-tooltip-disabled-reason={disabledReason}
        disabled={disabled}
        type="button"
        {...attributes}
        {...listeners}
      >
        <GripVertical aria-hidden="true" size={15} />
      </button>
      <span className="tagOrderIndex">{index}</span>
      <span className="tags" title={tag.name}>
        <em>{tag.name}</em>
      </span>
      <span className="tagOrderClients">
        {tag.clients.length} VPS{tag.clients.length === 1 ? "" : "s"}
      </span>
    </div>
  );
}

function tagOrderTopLevelDndId(block: TagOrderBlock): string {
  const name = block.names[0] ?? "";
  return block.names.length > 1 ? block.id : tagOrderLeafId(name);
}

function tagOrderBlockForDndId(
  blocks: readonly TagOrderBlock[],
  id: string,
): TagOrderBlock | undefined {
  return blocks.find(
    (block) =>
      block.id === id ||
      block.names.some((name) => tagOrderLeafId(name) === id),
  );
}

function tagNameForDndId(
  names: readonly string[],
  id: string,
): string | undefined {
  return names.find((name) => tagOrderLeafId(name) === id);
}

function tagOrderDndTargetLabel(
  blocks: readonly TagOrderBlock[],
  id: string,
): string | undefined {
  const name = blocks
    .flatMap((block) => block.names)
    .find((candidate) => tagOrderLeafId(candidate) === id);
  if (name) return name;
  const block = blocks.find((candidate) => candidate.id === id);
  const firstName = block?.names[0];
  return firstName ? `${tagNamespaceDisplayLabel(firstName)} group` : undefined;
}

function capitalizeDropPlacement(
  placement: TagOrderDropPlacement,
): "After" | "Before" {
  return placement === "after" ? "After" : "Before";
}

function toggleStoredValue(values: readonly string[], value: string): string[] {
  return values.includes(value)
    ? values.filter((candidate) => candidate !== value)
    : [...values, value];
}

function preserveExpandedTagOrderBlocks(
  expandedBlockIds: readonly string[],
  nextBlocks: readonly TagOrderBlock[],
): string[] {
  return nextBlocks
    .filter(
      (block) => block.names.length > 1 && expandedBlockIds.includes(block.id),
    )
    .map((block) => block.id);
}

function readTagOrderExpansionState(): TagOrderExpansionState {
  if (typeof window === "undefined") {
    return { expandedBlockIds: [], rootOpen: true };
  }
  try {
    const parsed = JSON.parse(
      window.localStorage.getItem(TAG_ORDER_EXPANSION_STORAGE_KEY) ?? "null",
    ) as Partial<TagOrderExpansionState> | null;
    return {
      expandedBlockIds: Array.isArray(parsed?.expandedBlockIds)
        ? parsed.expandedBlockIds.filter(
            (value): value is string => typeof value === "string",
          )
        : [],
      rootOpen: typeof parsed?.rootOpen === "boolean" ? parsed.rootOpen : true,
    };
  } catch {
    return { expandedBlockIds: [], rootOpen: true };
  }
}

function writeTagOrderExpansionState(state: TagOrderExpansionState) {
  try {
    window.localStorage.setItem(
      TAG_ORDER_EXPANSION_STORAGE_KEY,
      JSON.stringify(state),
    );
  } catch {
    // Browser-local presentation state is best effort only.
  }
}

function TagAssignments({
  actionFeedbackMessage,
  actionFeedbackTone,
  agents,
  fleetAlertPolicies,
  onAssignTag,
  onBulkMutateTags,
  onClearActionFeedback,
  onOpenPrivilegeUnlock,
  pending,
  privilegeMaterial,
  runAction,
  schedules,
  setLastMutation,
  tags,
}: {
  actionFeedbackMessage: string | null;
  actionFeedbackTone: ActionFeedbackTone;
  agents: AgentView[];
  fleetAlertPolicies: FleetAlertPolicyRecord[];
  onAssignTag: (
    clientId: string,
    tag: string,
    privilegeAssertion: PrivilegeAssertion,
  ) => Promise<TagMutationResponse>;
  onBulkMutateTags: (
    request: BulkTagMutationRequest,
  ) => Promise<TagMutationResponse>;
  onClearActionFeedback: () => void;
  onOpenPrivilegeUnlock: () => void;
  pending: boolean;
  privilegeMaterial: PrivilegeMaterial | null;
  runAction: (action: () => Promise<void>) => Promise<void>;
  schedules: ScheduleRecord[];
  setLastMutation: (response: TagMutationResponse | null) => void;
  tags: TagView[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
  const [tagByAgent, setTagByAgent] = useState<Record<string, string>>({});
  const [recentRemoval, setRecentRemoval] = useState<{
    agentId: string;
    agentLabel: string;
    scheduleImpactCount: number;
    selectorExpression: string;
    tag: string;
  } | null>(null);
  const actionFeedbackRef = useRef<HTMLDivElement | null>(null);
  const editingAgent =
    agents.find((agent) => agent.id === editingAgentId) ?? null;
  const tagNames = useMemo(() => tags.map((tag) => tag.name), [tags]);
  const tagOptions = useMemo(() => tags.map(groupOption), [tags]);
  const suggestionsText = tagOptions.length
    ? `Suggestions: ${tagOptions
        .slice(0, 4)
        .map((option) => option.label)
        .join(", ")}`
    : "No saved operator groups yet";
  const editingGroupInput = editingAgent
    ? (tagByAgent[editingAgent.id] ?? "")
    : "";
  const editingGroupError = groupNameValidationError(editingGroupInput);

  useEffect(() => {
    if (editingAgentId && !editingAgent) {
      setEditingAgentId(null);
    }
  }, [editingAgent, editingAgentId]);

  useEffect(() => {
    if (!actionFeedbackMessage) return;
    const frame = window.requestAnimationFrame(() => {
      if (actionFeedbackRef.current) {
        scrollIntoViewWithMotion(actionFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [actionFeedbackMessage]);

  const addTag = useCallback(
    async (agent: AgentView) => {
      const tag = tagByAgent[agent.id]?.trim();
      if (!tag || groupNameValidationError(tag)) {
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
    },
    [
      onAssignTag,
      onOpenPrivilegeUnlock,
      privilegeMaterial,
      runAction,
      setLastMutation,
      tagByAgent,
    ],
  );

  const removeTag = useCallback(
    async (agent: AgentView, tag: string) => {
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
        const preview = await onBulkMutateTags({
          action: "remove",
          confirmed: false,
          privilege_assertion: null,
          selector_expression: selector,
          target_client_ids: [agent.id],
          tag,
        });
        const response = await onBulkMutateTags({
          action: "remove",
          confirmed: true,
          preview_hash: preview.preview_hash,
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
    },
    [
      onBulkMutateTags,
      onOpenPrivilegeUnlock,
      privilegeMaterial,
      runAction,
      setLastMutation,
      vpsNameDisplayMode,
    ],
  );

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
      const preview = await onBulkMutateTags({
        action: "add",
        confirmed: false,
        privilege_assertion: null,
        selector_expression: removal.selectorExpression,
        target_client_ids: [removal.agentId],
        tag: removal.tag,
      });
      setLastMutation(
        await onBulkMutateTags({
          action: "add",
          confirmed: true,
          preview_hash: preview.preview_hash,
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
            <strong title={agent.id}>
              {formatVpsName(agent, vpsNameDisplayMode)}
            </strong>
            <small>{agent.id}</small>
          </span>
        ),
        header: "VPS",
        id: "vps",
        searchValue: (agent) =>
          `${formatVpsName(agent, vpsNameDisplayMode)} ${agent.id}`,
        sortValue: (agent) => formatVpsName(agent, vpsNameDisplayMode),
      },
      {
        cell: (agent) => {
          const state = agentDisplayState(agent);
          return (
            <span className="historyPrimary">
              <strong
                className={`status ${groupReachabilityToneClass(state.tone)}`}
              >
                {state.label}
              </strong>
              <small title={state.detail}>{state.detail}</small>
            </span>
          );
        },
        header: "Reachability",
        id: "status",
        searchValue: (agent) => {
          const state = agentDisplayState(agent);
          return `${state.label} ${state.detail}`;
        },
        sortValue: (agent) => agentDisplayState(agent).label,
      },
      {
        cell: (agent) => (
          <span className="tagChipList">
            {agent.tags.map((tag) => {
              const dependencies = groupDependencySummary(
                tag,
                schedules,
                fleetAlertPolicies,
              );
              const dependencyLabel = dependencySummaryText(dependencies);
              const hasDependencies = dependencies.total > 0;
              return (
                <span
                  className={`tagRemoveChip${hasDependencies ? " linked" : ""}`}
                  key={tag}
                  title={`${tag} (${groupKindLabel(tag).toLowerCase()}). ${dependencyLabel}`}
                >
                  <span>{tag}</span>
                  {hasDependencies && <small>{dependencyLabel}</small>}
                </span>
              );
            })}
          </span>
        ),
        header: "Current groups",
        id: "tags",
        searchValue: (agent) => agent.tags.join(" "),
        sortValue: (agent) => agent.tags.join(" "),
      },
    ],
    [fleetAlertPolicies, schedules, vpsNameDisplayMode],
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
            <span>Reachability</span>
            <strong>{agentDisplayState(agent).label}</strong>
            <span>Groups</span>
            <strong>{agent.tags.join(", ") || "None"}</strong>
          </div>
        )}
        rowActions={[
          {
            description: (rows) =>
              pending
                ? "Wait for the current group change to finish before editing another VPS."
                : rows[0]
                  ? `Edit groups assigned to ${formatVpsName(rows[0], vpsNameDisplayMode)}.`
                  : "Select one VPS to edit its groups.",
            disabled: (rows) => pending || rows.length !== 1,
            icon: <Tag size={14} />,
            label: "Edit groups",
            onSelect: (rows) => {
              onClearActionFeedback();
              setRecentRemoval(null);
              setEditingAgentId(rows[0]?.id ?? null);
            },
          },
        ]}
        rows={agents}
        searchPlaceholder="Search VPS assignments"
        storageKey="vpsman.tags.assignments"
        title="VPS group assignments"
      />
      <ConsoleActionDrawer
        description={
          editingAgent
            ? `${agentDisplayState(editingAgent).label} · ${editingAgent.tags.length} assigned group${editingAgent.tags.length === 1 ? "" : "s"}`
            : undefined
        }
        onClose={() => {
          onClearActionFeedback();
          setEditingAgentId(null);
          setRecentRemoval(null);
        }}
        open={editingAgent !== null}
        title={
          editingAgent
            ? `Edit groups · ${formatVpsName(editingAgent, vpsNameDisplayMode)}`
            : "Edit VPS groups"
        }
      >
        {editingAgent ? (
          <div className="compactForm">
            <div className="consoleInlineDetailGrid">
              <span>VPS</span>
              <strong>{formatVpsName(editingAgent, vpsNameDisplayMode)}</strong>
              <span>Client ID</span>
              <strong>{editingAgent.id}</strong>
              <span>Reachability</span>
              <strong>{agentDisplayState(editingAgent).label}</strong>
            </div>
            <ActionFeedback
              className="localActionFeedback"
              message={actionFeedbackMessage}
              ref={actionFeedbackRef}
              tone={actionFeedbackTone}
            />
            <section className="consoleFormGroup">
              <div className="consoleFormGroupHeader">
                <strong>Group membership</strong>
                <span>
                  Remove an assigned group, or choose an existing group name to
                  add. A new valid name is created on assignment.
                </span>
              </div>
              {editingAgent.tags.length > 0 ? (
                <span className="tagChipList">
                  {editingAgent.tags.map((tag) => {
                    const dependencies = groupDependencySummary(
                      tag,
                      schedules,
                      fleetAlertPolicies,
                    );
                    const dependencyLabel = dependencySummaryText(dependencies);
                    return (
                      <span
                        className={`tagRemoveChip${dependencies.total > 0 ? " linked" : ""}`}
                        key={tag}
                        title={`${tag} (${groupKindLabel(tag).toLowerCase()}). ${dependencyLabel}`}
                      >
                        <span>{tag}</span>
                        {dependencies.total > 0 && (
                          <small>{dependencyLabel}</small>
                        )}
                        <button
                          aria-label={`Remove ${tag} from ${formatVpsName(editingAgent, vpsNameDisplayMode)}`}
                          className="tagEditChipRemove"
                          data-tooltip-disabled-reason={
                            pending
                              ? "Wait for the current group assignment change to finish"
                              : undefined
                          }
                          disabled={pending}
                          onClick={() => void removeTag(editingAgent, tag)}
                          title={`Remove ${tag}`}
                          type="button"
                        >
                          <X aria-hidden="true" size={12} />
                        </button>
                      </span>
                    );
                  })}
                </span>
              ) : (
                <span className="formHint">No groups assigned.</span>
              )}
              <label className="consoleField fieldFull">
                <span title={suggestionsText}>Add group</span>
                <input
                  aria-describedby={`group-suggestions-${editingAgent.id}`}
                  aria-label={`Group to add to ${formatVpsName(editingAgent, vpsNameDisplayMode)}`}
                  data-action-drawer-initial-focus="true"
                  list="tag-options"
                  onChange={(event) =>
                    setTagByAgent((current) => ({
                      ...current,
                      [editingAgent.id]: event.target.value,
                    }))
                  }
                  placeholder="group name"
                  value={editingGroupInput}
                />
                <small id={`group-suggestions-${editingAgent.id}`}>
                  {editingGroupError ??
                    "Choose a saved group from the dropdown or enter one new valid group name."}
                </small>
              </label>
              <div className="consoleFormActions">
                <button
                  aria-label={`Add group to ${formatVpsName(editingAgent, vpsNameDisplayMode)}`}
                  className="primaryAction"
                  data-tooltip-disabled-reason={
                    pending
                      ? "Wait for the current group assignment change to finish"
                      : !editingGroupInput.trim()
                        ? "Enter a group name before adding it"
                        : (editingGroupError ?? undefined)
                  }
                  disabled={
                    pending ||
                    !editingGroupInput.trim() ||
                    editingGroupError !== null
                  }
                  onClick={() => void addTag(editingAgent)}
                  type="button"
                >
                  <Plus size={13} />
                  Add
                </button>
              </div>
              {recentRemoval?.agentId === editingAgent.id ? (
                <div
                  aria-live="polite"
                  className="tagAssignmentNotice"
                  role="status"
                >
                  <span>
                    Removed <strong>{recentRemoval.tag}</strong> from{" "}
                    <strong>{recentRemoval.agentLabel}</strong>.
                  </span>
                  {recentRemoval.scheduleImpactCount > 0 && (
                    <small>
                      Used by {recentRemoval.scheduleImpactCount} schedule
                      {recentRemoval.scheduleImpactCount === 1 ? "" : "s"};
                      saved targets stay fixed until updated.
                    </small>
                  )}
                  <button
                    className="secondaryAction compactAction"
                    data-tooltip-disabled-reason={
                      pending
                        ? "Wait for the current group assignment change to finish"
                        : undefined
                    }
                    disabled={pending}
                    onClick={undoRemoveTag}
                    type="button"
                  >
                    Undo
                  </button>
                </div>
              ) : null}
            </section>
          </div>
        ) : null}
      </ConsoleActionDrawer>
      <datalist id="tag-options">
        {tagNames.map((tag) => (
          <option
            key={tag}
            label={groupOptionLabel(tag, tagClientsCount(tags, tag))}
            value={tag}
          />
        ))}
      </datalist>
    </>
  );
}

function groupReachabilityToneClass(
  tone: ReturnType<typeof agentDisplayState>["tone"],
): "info" | "neutral" | "ok" | "warn" {
  if (tone === "warning" || tone === "critical") {
    return "warn";
  }
  return tone;
}

function BulkTagPanel({
  actionFeedbackMessage,
  actionFeedbackTone,
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
  actionFeedbackMessage: string | null;
  actionFeedbackTone: ActionFeedbackTone;
  agents: AgentView[];
  onBulkMutateTags: (
    request: BulkTagMutationRequest,
  ) => Promise<TagMutationResponse>;
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
  const vpsRuleSearch = useVpsRuleSearchContext();
  const [selectorExpression, setSelectorExpression] = useState(() =>
    readLocalString(TAG_BULK_SELECTOR_STORAGE_KEY),
  );
  const [action, setAction] = useState<"add" | "remove" | "delete">("add");
  const [tag, setTag] = useState("");
  const [preview, setPreview] = useState<TagMutationResponse | null>(null);
  const [resolvedTargets, setResolvedTargets] =
    useState<BulkResolveResponse | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [mutationSnapshot, setMutationSnapshot] =
    useState<BulkTagMutationSnapshot | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewStatus, setPreviewStatus] = useState<string | null>(null);
  const [includeReviewTargets, setIncludeReviewTargets] = useState(false);
  const actionFeedbackRef = useRef<HTMLDivElement | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectorParse = useMemo(
    () => parseSearchExpression(selectorExpression),
    [selectorExpression],
  );
  const trimmedTag = tag.trim();
  const bulkGroupNameError = groupNameValidationError(tag);
  const trimmedSelector = selectorExpression.trim();
  const selectorEvidenceUnavailable = vpsRuleSearchUnavailable(
    trimmedSelector,
    vpsRuleSearch,
  );
  const localTargets = useMemo(
    () =>
      trimmedSelector && !selectorParse.error && !selectorEvidenceUnavailable
        ? agentsMatchingExpression(agents, trimmedSelector, vpsRuleSearch)
        : [],
    [
      agents,
      selectorEvidenceUnavailable,
      selectorParse.error,
      trimmedSelector,
      vpsRuleSearch,
    ],
  );
  const eligibleLocalTargets = useMemo(
    () => tagMutationEligibleTargets(localTargets, includeReviewTargets),
    [includeReviewTargets, localTargets],
  );
  const eligibleResolvedTargets = useMemo(
    () =>
      resolvedTargets
        ? tagMutationEligibleTargets(
            resolvedTargets.targets,
            includeReviewTargets,
          )
        : null,
    [includeReviewTargets, resolvedTargets],
  );
  const targetCountForAction =
    action === "delete"
      ? (preview?.target_count ?? 0)
      : (eligibleResolvedTargets?.length ?? eligibleLocalTargets.length);
  const canReviewMutation = Boolean(
    trimmedTag &&
    !bulkGroupNameError &&
    (action === "delete" ||
      (trimmedSelector &&
        !selectorParse.error &&
        !selectorEvidenceUnavailable &&
        eligibleLocalTargets.length > 0)),
  );

  const localFeedbackMessage = previewError
    ? `Preview failed. ${previewError}. Retry review; final apply stays locked until a fresh server preview succeeds.`
    : (previewStatus ?? actionFeedbackMessage);
  const localFeedbackTone: ActionFeedbackTone = previewError
    ? "danger"
    : previewStatus
      ? "progress"
      : actionFeedbackTone;

  useEffect(() => {
    if (!localFeedbackMessage) return;
    const frame = window.requestAnimationFrame(() => {
      if (actionFeedbackRef.current) {
        scrollIntoViewWithMotion(actionFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [localFeedbackMessage]);

  useEffect(
    () => writeLocalString(TAG_BULK_SELECTOR_STORAGE_KEY, selectorExpression),
    [selectorExpression],
  );

  function clearMutationPreview() {
    invalidateReviewGeneration();
    setPreview(null);
    setResolvedTargets(null);
    setMutationSnapshot(null);
    setConfirmOpen(false);
    setPreviewError(null);
    setPreviewStatus(null);
  }

  async function reviewMutation() {
    const reviewGeneration = captureReviewGeneration();
    const frozenAction = action;
    const frozenIncludeReviewTargets = includeReviewTargets;
    const frozenTag = trimmedTag;
    const frozenSelector = trimmedSelector;
    setPreviewError(null);
    setPreviewStatus(
      frozenAction === "delete"
        ? "Preparing delete preview"
        : "Resolving targets and preparing preview",
    );
    try {
      await runAction(async () => {
        try {
          await waitForReviewRender();
          if (frozenAction !== "delete" && selectorParse.error) {
            throw new Error(selectorParse.error);
          }
          if (bulkGroupNameError) {
            throw new Error(bulkGroupNameError);
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
          const targetClientIds = tagMutationEligibleTargets(
            resolved.targets,
            frozenIncludeReviewTargets,
          ).map((target) => target.id);
          if (!targetClientIds.length) {
            throw new Error(
              frozenIncludeReviewTargets
                ? "Bulk group action resolved no eligible VPSs"
                : "Bulk group action has no ready VPSs; include review targets to apply anyway",
            );
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
        } catch (error) {
          if (isReviewGenerationCurrent(reviewGeneration)) {
            setMutationSnapshot(null);
            setConfirmOpen(false);
            setPreviewError(previewFailureMessage(error));
          }
          throw error;
        }
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
        throw new Error(
          "Group mutation confirmation snapshot is missing; preview the mutation again",
        );
      }
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error(
          "Privilege unlock is required before bulk group mutation",
        );
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
        setLastMutation(
          await onDeleteTag(
            snapshot.tag,
            true,
            privilegeAssertion,
            snapshot.preview.preview_hash,
          ),
        );
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
    clearMutationPreview();
  }

  const previewAgents = preview?.affected ?? [];
  const confirmationSnapshot = confirmOpen ? mutationSnapshot : null;
  const confirmationPreview = confirmationSnapshot?.preview ?? preview;
  const reviewButtonLabel =
    action !== "delete" &&
    trimmedTag &&
    localTargets.length > 0 &&
    eligibleLocalTargets.length === 0 &&
    !includeReviewTargets
      ? `Include review targets to apply ${trimmedTag}`
      : bulkMutationPrimaryLabel(action, trimmedTag, targetCountForAction);

  return (
    <div className="configApplyGrid bulkTagApplyGrid">
      <div className="compactForm bulkTagMutationForm">
        <strong>Bulk group mutation</strong>
        <label>
          <span>Mutation</span>
          <select
            aria-label="Bulk group action"
            onChange={(event) => {
              setAction(event.target.value as "add" | "remove" | "delete");
              clearMutationPreview();
            }}
            value={action}
          >
            <option value="add">Add group by selector</option>
            <option value="remove">Remove group by selector</option>
            <option value="delete">Delete group globally</option>
          </select>
        </label>
        <label>
          <span>Group</span>
          <input
            aria-label="Bulk group"
            list="bulk-tag-options"
            onChange={(event) => {
              setTag(event.target.value);
              clearMutationPreview();
            }}
            placeholder="provider:aws or role:edge"
            value={tag}
          />
          <small className={bulkGroupNameError ? "errorText" : "formHint"}>
            {bulkGroupNameError ??
              "Choose an existing group or enter one valid new group name."}
          </small>
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
              ariaLabel="Bulk group selector expression"
              className="targetExpressionBar"
              metaDescription={
                selectorExpression.trim() && !selectorParse.error
                  ? targetStatusText(
                      "Local match",
                      localTargets,
                      includeReviewTargets,
                    )
                  : undefined
              }
              onChange={(value) => {
                setSelectorExpression(value);
                clearMutationPreview();
              }}
              placeholder="provider:* && country:US"
              showMatchCount
              value={selectorExpression}
              verification={
                selectorParse.error
                  ? "invalid"
                  : selectorExpression.trim()
                    ? "valid"
                    : "neutral"
              }
              verificationMessage={selectorParse.error ?? undefined}
            />
            {selectorEvidenceUnavailable ? (
              <small className="errorText">
                {VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE}
              </small>
            ) : (
              <LocalTargetPreview
                agents={localTargets}
                ariaLabel="Bulk group local VPS preview"
              />
            )}
            <label
              className="inlineCheck tightCheck compactReviewCheck"
              title="Default excludes contact-unknown, stale, and degraded targets from the final mutation. Enable only when you intentionally want those targets included."
            >
              <input
                checked={includeReviewTargets}
                onChange={(event) => {
                  setIncludeReviewTargets(event.target.checked);
                  clearMutationPreview();
                }}
                type="checkbox"
              />
              <span>Include targets needing review</span>
            </label>
          </>
        )}
        <div className={`privilegeGateBox ${privilegeMaterial ? "ready" : ""}`}>
          <ShieldCheck size={16} />
          <span>
            {privilegeMaterial
              ? "Privilege unlocked for final apply"
              : "Preview works now; unlock only when applying."}
          </span>
          {!privilegeMaterial && (
            <button
              className="secondaryAction compactAction"
              onClick={onOpenPrivilegeUnlock}
              type="button"
            >
              Unlock privilege
            </button>
          )}
        </div>
        <button
          className="primaryAction"
          data-tooltip-disabled-reason={
            pending
              ? "A bulk group operation is already in progress"
              : !canReviewMutation
                ? reviewButtonLabel
                : undefined
          }
          disabled={pending || !canReviewMutation}
          onClick={() => void reviewMutation()}
          title={
            canReviewMutation
              ? "Review the server-resolved target snapshot before final apply."
              : reviewButtonLabel
          }
          type="button"
        >
          <Tag size={16} />
          {reviewButtonLabel}
        </button>
        <ActionFeedback
          className="localActionFeedback bulkTagPreviewActionFeedback"
          message={localFeedbackMessage}
          ref={actionFeedbackRef}
          tone={localFeedbackTone}
        />
      </div>
      {preview && (
        <section
          className="bulkTagPreviewPanel"
          aria-label="Bulk group target preview"
        >
          <div className="bulkTagPreviewHeader">
            <div>
              <strong>Server preview</strong>
              <span>{`${preview.target_count} resolved / ${preview.changed_count} changes`}</span>
            </div>
          </div>
          <div
            className="bulkTagPreviewStats"
            aria-label="Bulk group preview evidence"
          >
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
              <strong title={preview.preview_hash}>
                {preview.preview_hash}
              </strong>
              <small>preview hash</small>
            </span>
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
              <span>No VPSs would change for this mutation.</span>
            </div>
          )}
        </section>
      )}
      <ConfirmationPrompt
        confirmLabel="Apply tag mutation"
        detail={
          confirmationSnapshot?.action === "delete"
            ? "Delete this tag and all assignments."
            : "Apply this selector-based tag mutation."
        }
        items={[
          { label: "Action", value: confirmationSnapshot?.action ?? action },
          { label: "Group", value: confirmationSnapshot?.tag || tag || "-" },
          {
            label: "Selector",
            value:
              confirmationSnapshot?.action === "delete"
                ? "all assignments"
                : confirmationSnapshot?.selectorExpression ||
                  selectorExpression ||
                  "-",
          },
          {
            label: "Targets",
            value: String(confirmationPreview?.target_count ?? 0),
          },
          {
            label: "Changed",
            value: String(confirmationPreview?.changed_count ?? 0),
          },
          {
            label: "Excluded / no-change",
            value: String(confirmationPreview?.skipped_count ?? 0),
          },
          {
            label: "Membership after apply",
            value: membershipOutcomeText(
              confirmationSnapshot?.action ?? action,
              confirmationPreview,
            ),
          },
          {
            label: "Preview hash",
            title: confirmationPreview?.preview_hash,
            value: confirmationPreview?.preview_hash ?? "-",
          },
          {
            label: "Schedule target notices",
            value: (
              <ScheduleImpactTable
                impacts={confirmationPreview?.schedule_impacts ?? []}
                onOpenSchedules={onOpenSchedules}
              />
            ),
          },
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

function previewFailureMessage(error: unknown): string {
  return error instanceof Error ? error.message : "Bulk group preview failed";
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
    <div className="tagScheduleImpactBlock">
      <div className="bulkTagPreviewHeader">
        <div>
          <strong>Affected schedules</strong>
          <span>
            {impacts.length} saved target snapshot
            {impacts.length === 1 ? "" : "s"} need review
          </span>
        </div>
        {onOpenSchedules && (
          <button
            className="secondaryAction compactAction"
            type="button"
            onClick={onOpenSchedules}
          >
            Open schedules
          </button>
        )}
      </div>
      <div
        aria-label="Affected schedules"
        className="tagScheduleImpactTable"
        role="table"
      >
        <div className="tagScheduleImpactRow heading" role="row">
          <span role="columnheader">Schedule</span>
          <span role="columnheader">Command</span>
          <span role="columnheader">Selector result</span>
          <span role="columnheader">Impact</span>
          <span role="columnheader">Added</span>
          <span role="columnheader">Removed</span>
        </div>
        {impacts.map((impact) => (
          <div
            className="tagScheduleImpactRow"
            key={impact.schedule_id}
            role="row"
          >
            <span className="historyPrimary" data-label="Schedule" role="cell">
              <strong>{impact.name}</strong>
              <small>{impact.selector_expression}</small>
            </span>
            <span data-label="Command" role="cell">
              {impact.command_type}
            </span>
            <span data-label="Selector result" role="cell">
              {impact.before_target_count} -&gt; {impact.after_target_count}
            </span>
            <span data-label="Impact" role="cell">
              {impact.summary}; saved targets stay fixed until you update them.
            </span>
            <span data-label="Added" role="cell">
              <VpsChipList agents={impact.added_targets} />
            </span>
            <span data-label="Removed" role="cell">
              <VpsChipList agents={impact.removed_targets} />
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function VpsChipList({ agents }: { agents: AgentView[] }) {
  if (agents.length === 0) {
    return (
      <span
        className="mutedText"
        data-tooltip-empty-reason="No VPS is present in this impact set"
      >
        -
      </span>
    );
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
