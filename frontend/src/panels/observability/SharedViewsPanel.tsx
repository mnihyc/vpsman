import { Ban, Clock3, Copy, Link2, Plus, RefreshCw } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../../components/ActionFeedback";
import { handleTabListKeyDown, tabId } from "../../components/AccessibleTabs";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  ConsoleActionDrawer,
  ConsoleStatusBadge,
} from "../../components/ConsoleLayout";
import { SearchExpressionInput } from "../../components/SearchExpressionInput";
import { apiGet, apiPost } from "../../api";
import { useHistoryEntryState } from "../../historyEntryState";
import { scrollIntoViewWithMotion } from "../../motion";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE,
  vpsRuleSearchUnavailable,
} from "../../searchExpression";
import { useVpsRuleSearchContext } from "../../vpsRuleSearchContext";
import { LocalTargetPreview } from "../TargetImpactPreview";
import type {
  AgentView,
  BulkUpdateMonitoringShareTargetsRequest,
  BulkUpdateMonitoringShareTargetsResponse,
  BulkResolveResponse,
  CreateMonitoringShareRequest,
  CreateMonitoringShareResponse,
  ExtendMonitoringSharesRequest,
  MonitoringSharesMutationResponse,
  MonitoringShareUrlResponse,
  MonitoringShareView,
  MonitoringShareVisibilityRequest,
  RevokeMonitoringSharesRequest,
} from "../../types";
import {
  formatCompactTime,
  formatFullTime,
  timestampMillis,
} from "../../utils";

type ShareStatus = "active" | "expired" | "revoked";
type DurationUnit = "minutes" | "hours" | "days";

type ShareDraft = {
  durationUnit: DurationUnit;
  durationValue: number;
  name: string;
  selectorExpression: string;
  visibility: Required<MonitoringShareVisibilityRequest>;
};

type ReviewedShare = {
  request: CreateMonitoringShareRequest;
  targets: AgentView[];
};

type PendingLifecycleAction =
  | { kind: "extend"; shares: MonitoringShareView[] }
  | { kind: "revoke"; shares: MonitoringShareView[] }
  | null;

type TargetUpdateReview = {
  response: BulkUpdateMonitoringShareTargetsResponse;
  shares: MonitoringShareView[];
};

type SharedViewUrl = {
  createdAt: string;
  name: string;
  shareId: string;
  url: string;
};

type LocalFeedback = {
  message: string;
  tone: ActionFeedbackTone;
};

const SHARE_PAGE_SIZE = 1_000;
const MIN_EXPIRY_SECS = 60;
const MAX_EXPIRY_SECS = 365 * 24 * 60 * 60;
const STATUS_OPTIONS: Array<{
  detail: string;
  label: string;
  status: ShareStatus;
}> = [
  {
    detail: "Available links that can still be extended or revoked",
    label: "Active",
    status: "active",
  },
  {
    detail: "Read-only lifecycle evidence; create a replacement to share again",
    label: "Expired",
    status: "expired",
  },
  {
    detail: "Irreversibly disabled links retained as evidence",
    label: "Revoked",
    status: "revoked",
  },
];

const DEFAULT_VISIBILITY: Required<MonitoringShareVisibilityRequest> = {
  billing: false,
  detail_history: true,
  identity_context: false,
  network: true,
  ping: true,
  resources: true,
  system_information: false,
  traffic: true,
};

export function SharedViewsPanel({
  agents,
  apiToken,
  initialSelectorExpression = "*",
  onInitialSelectorConsumed,
  onResolveTargets,
}: {
  agents: AgentView[];
  apiToken: string;
  initialSelectorExpression?: string;
  onInitialSelectorConsumed?: () => void;
  onResolveTargets: (
    selectorExpression: string,
  ) => Promise<BulkResolveResponse>;
}) {
  const vpsRuleSearch = useVpsRuleSearchContext();
  const [statusFilter, setStatusFilter] = useHistoryEntryState<ShareStatus>(
    "observability.shared-views.status",
    "active",
  );
  const [sharedViewUrl, setSharedViewUrl] =
    useHistoryEntryState<SharedViewUrl | null>(
      "observability.shared-views.public-url",
      null,
    );
  const [shares, setShares] = useState<MonitoringShareView[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<LocalFeedback | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [draft, setDraft] = useState<ShareDraft>(() =>
    defaultShareDraft(initialSelectorExpression),
  );
  const [review, setReview] = useState<ReviewedShare | null>(null);
  const [pendingAction, setPendingAction] =
    useState<PendingLifecycleAction>(null);
  const [targetUpdateReview, setTargetUpdateReview] =
    useState<TargetUpdateReview | null>(null);
  const [extensionValue, setExtensionValue] = useState(24);
  const [extensionUnit, setExtensionUnit] = useState<DurationUnit>("hours");
  const [pending, setPending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const loadGeneration = useRef(0);
  const initialSelectorSnapshot = useRef(initialSelectorExpression);
  const initialSelectorConsumed = useRef(onInitialSelectorConsumed);
  const feedbackRef = useRef<HTMLDivElement | null>(null);
  const createFeedbackRef = useRef<HTMLDivElement | null>(null);
  const sharedViewUrlRef = useRef<HTMLElement | null>(null);
  const createdUrlFocusShareIdRef = useRef<string | null>(null);

  useEffect(() => {
    initialSelectorConsumed.current?.();
  }, []);

  const loadShares = useCallback(async () => {
    const generation = ++loadGeneration.current;
    if (!apiToken) {
      setShares([]);
      setLoading(false);
      setLoadError(null);
      return;
    }
    setLoading(true);
    setLoadError(null);
    try {
      const loaded: MonitoringShareView[] = [];
      for (let offset = 0; ; offset += SHARE_PAGE_SIZE) {
        const page = await apiGet<MonitoringShareView[]>(
          `/api/v1/monitoring-shares?limit=${SHARE_PAGE_SIZE}&offset=${offset}`,
          apiToken,
        );
        loaded.push(...page);
        if (page.length < SHARE_PAGE_SIZE) {
          break;
        }
      }
      if (loadGeneration.current !== generation) {
        return;
      }
      setShares(deduplicateShares(loaded));
    } catch (error) {
      if (loadGeneration.current !== generation) {
        return;
      }
      setLoadError(actionErrorMessage(error));
    } finally {
      if (loadGeneration.current === generation) {
        setLoading(false);
      }
    }
  }, [apiToken]);

  useEffect(() => {
    void loadShares();
    return () => {
      loadGeneration.current += 1;
    };
  }, [loadShares]);

  useEffect(() => {
    const message = loadError ?? feedback?.message;
    if (
      !message ||
      drawerOpen ||
      review !== null ||
      pendingAction !== null ||
      targetUpdateReview !== null ||
      createdUrlFocusShareIdRef.current !== null
    ) {
      return undefined;
    }
    const frame = window.requestAnimationFrame(() => {
      const outcome = feedbackRef.current;
      if (!outcome) {
        return;
      }
      outcome.tabIndex = -1;
      scrollIntoViewWithMotion(outcome, { block: "nearest" });
      outcome.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [
    drawerOpen,
    feedback?.message,
    feedback?.tone,
    loadError,
    pendingAction,
    review,
    targetUpdateReview,
  ]);

  useEffect(() => {
    if (!actionError || !drawerOpen || review !== null) {
      return undefined;
    }
    const frame = window.requestAnimationFrame(() => {
      const outcome = createFeedbackRef.current;
      if (!outcome) {
        return;
      }
      outcome.tabIndex = -1;
      scrollIntoViewWithMotion(outcome, { block: "nearest" });
      outcome.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [actionError, drawerOpen, review]);

  useEffect(() => {
    if (
      !sharedViewUrl ||
      drawerOpen ||
      review !== null ||
      targetUpdateReview !== null ||
      createdUrlFocusShareIdRef.current !== sharedViewUrl.shareId
    ) {
      return undefined;
    }
    const frame = window.requestAnimationFrame(() => {
      const outcome = sharedViewUrlRef.current;
      if (!outcome) {
        return;
      }
      createdUrlFocusShareIdRef.current = null;
      scrollIntoViewWithMotion(outcome, { block: "start" });
      outcome.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [drawerOpen, review, sharedViewUrl, targetUpdateReview]);

  const sharesByStatus = useMemo(() => {
    const grouped: Record<ShareStatus, MonitoringShareView[]> = {
      active: [],
      expired: [],
      revoked: [],
    };
    for (const share of shares) {
      grouped[effectiveShareStatus(share)].push(share);
    }
    return grouped;
  }, [shares]);
  const visibleShares = sharesByStatus[statusFilter];
  const selectorParse = useMemo(
    () => parseSearchExpression(draft.selectorExpression),
    [draft.selectorExpression],
  );
  const selectorEvidenceUnavailable = vpsRuleSearchUnavailable(
    draft.selectorExpression,
    vpsRuleSearch,
  );
  const localTargets = useMemo(
    () =>
      draft.selectorExpression.trim() &&
      !selectorParse.error &&
      !selectorEvidenceUnavailable
        ? agentsMatchingExpression(
            agents,
            draft.selectorExpression,
            vpsRuleSearch,
          )
        : [],
    [
      agents,
      draft.selectorExpression,
      selectorEvidenceUnavailable,
      selectorParse.error,
      vpsRuleSearch,
    ],
  );
  const draftExpirySecs = durationSeconds(
    draft.durationValue,
    draft.durationUnit,
  );
  const extensionSecs = durationSeconds(extensionValue, extensionUnit);
  const visibleMetricCount = visibleMetricGroupCount(draft.visibility);
  const draftReady =
    draft.name.trim().length > 0 &&
    draft.name.trim().length <= 128 &&
    draft.selectorExpression.trim().length > 0 &&
    !selectorParse.error &&
    !selectorEvidenceUnavailable &&
    draftExpirySecs !== null &&
    (!draft.visibility.detail_history || visibleMetricCount > 0);

  function updateDraft(update: Partial<ShareDraft>) {
    setDraft((current) => ({ ...current, ...update }));
    setReview(null);
    setActionError(null);
    setFeedback(null);
  }

  function updateVisibility(
    field: keyof MonitoringShareVisibilityRequest,
    checked: boolean,
  ) {
    setDraft((current) => {
      const visibility = { ...current.visibility, [field]: checked };
      if (
        field !== "detail_history" &&
        visibleMetricGroupCount(visibility) === 0
      ) {
        visibility.detail_history = false;
      }
      return { ...current, visibility };
    });
    setReview(null);
    setActionError(null);
    setFeedback(null);
  }

  function enterReviewWorkflow(
    workflow: "create" | "targets" | "lifecycle" | "copy",
  ) {
    setReview(null);
    setPendingAction(null);
    setTargetUpdateReview(null);
    setActionError(null);
    setFeedback(null);
    if (workflow !== "create") {
      setDrawerOpen(false);
    }
  }

  function openCreate() {
    enterReviewWorkflow("create");
    setDraft(defaultShareDraft(initialSelectorSnapshot.current));
    setDrawerOpen(true);
  }

  function closeCreate() {
    if (pending) return;
    setReview(null);
    setActionError(null);
    setDrawerOpen(false);
  }

  async function reviewCreate() {
    if (!draftReady || draftExpirySecs === null) {
      setActionError(createDraftError(draft, selectorParse.error));
      return;
    }
    setPending(true);
    setActionError(null);
    try {
      const selectorExpression = draft.selectorExpression.trim();
      const resolved = await onResolveTargets(selectorExpression);
      if (resolved.target_count === 0 || resolved.targets.length === 0) {
        setActionError(
          "The selector currently resolves to no VPSs. Adjust it before creating a public view.",
        );
        return;
      }
      setReview({
        request: {
          confirmed: true,
          expires_in_secs: draftExpirySecs,
          name: draft.name.trim(),
          selector_expression: selectorExpression,
          target_client_ids: resolved.targets.map((agent) => agent.id),
          visibility: { ...draft.visibility },
        },
        targets: resolved.targets,
      });
    } catch (error) {
      const message = actionErrorMessage(error);
      setActionError(message);
      setFeedback({ message, tone: "danger" });
    } finally {
      setPending(false);
    }
  }

  async function createShare() {
    if (!review) return;
    setPending(true);
    setActionError(null);
    try {
      const response = await apiPost<CreateMonitoringShareResponse>(
        "/api/v1/monitoring-shares",
        apiToken,
        review.request,
      );
      const createdShare = {
        ...response.share,
        target_update_available: false,
        target_update_evidence_available: true,
      };
      setShares((current) => mergeShares(current, [createdShare]));
      createdUrlFocusShareIdRef.current = response.share.id;
      setSharedViewUrl({
        createdAt: response.share.created_at,
        name: response.share.name,
        shareId: response.share.id,
        url: absoluteShareUrl(response.fragment_path),
      });
      setFeedback({
        message: `Created ${response.share.name} for ${response.share.target_count} ${response.share.target_count === 1 ? "VPS" : "VPSs"}. The public URL is ready to copy and remains recoverable while active.`,
        tone: "success",
      });
      setStatusFilter("active");
      setReview(null);
      setDrawerOpen(false);
    } catch (error) {
      const message = actionErrorMessage(error);
      setActionError(message);
      setFeedback({ message, tone: "danger" });
    } finally {
      setPending(false);
    }
  }

  async function applyLifecycleAction() {
    if (!pendingAction) return;
    const ids = pendingAction.shares.map((share) => share.id);
    setActionError(null);
    if (pendingAction.kind === "extend" && extensionSecs === null) {
      setActionError("Choose an extension from one minute through 365 days.");
      return;
    }
    setPending(true);
    try {
      let response: MonitoringSharesMutationResponse;
      if (pendingAction.kind === "extend") {
        if (extensionSecs === null) {
          return;
        }
        response = await apiPost<MonitoringSharesMutationResponse>(
          "/api/v1/monitoring-shares/extend",
          apiToken,
          {
            extend_by_secs: extensionSecs,
            share_ids: ids,
          } satisfies ExtendMonitoringSharesRequest,
        );
      } else {
        response = await apiPost<MonitoringSharesMutationResponse>(
          "/api/v1/monitoring-shares/revoke",
          apiToken,
          { share_ids: ids } satisfies RevokeMonitoringSharesRequest,
        );
      }
      setShares((current) => {
        const existingById = new Map(current.map((share) => [share.id, share]));
        const updated = response.shares.map((share) =>
          pendingAction.kind === "extend"
            ? {
                ...share,
                target_update_available:
                  existingById.get(share.id)?.target_update_available ?? false,
                target_update_evidence_available:
                  existingById.get(share.id)
                    ?.target_update_evidence_available ?? false,
              }
            : share,
        );
        return mergeShares(current, updated);
      });
      setFeedback({
        message:
          pendingAction.kind === "extend"
            ? `Extended ${response.shares.length} shared ${response.shares.length === 1 ? "view" : "views"}.`
            : `Revoked ${response.shares.length} shared ${response.shares.length === 1 ? "view" : "views"}. Existing URLs stopped working immediately.`,
        tone: "success",
      });
      setPendingAction(null);
    } catch (error) {
      const message = actionErrorMessage(error);
      setActionError(message);
      setFeedback({ message, tone: "danger" });
    } finally {
      setPending(false);
    }
  }

  async function reviewTargetUpdate(selectedShares: MonitoringShareView[]) {
    if (
      selectedShares.some((share) => !share.target_update_evidence_available)
    ) {
      setFeedback({
        message:
          "Target refresh evidence is unavailable. Frozen targets remain unchanged; repair or retry, then use Update targets with the required access.",
        tone: "warning",
      });
      return;
    }
    enterReviewWorkflow("targets");
    setPending(true);
    try {
      const response = await apiPost<BulkUpdateMonitoringShareTargetsResponse>(
        "/api/v1/monitoring-shares/update-targets",
        apiToken,
        {
          share_ids: selectedShares.map((share) => share.id),
        } satisfies BulkUpdateMonitoringShareTargetsRequest,
      );
      if (!targetChangesPresent(response)) {
        setFeedback({
          message: `The frozen targets already match the saved ${selectedShares.length === 1 ? "selector" : "selectors"}.`,
          tone: "info",
        });
        return;
      }
      setTargetUpdateReview({ response, shares: selectedShares });
    } catch (error) {
      const message = actionErrorMessage(error);
      setActionError(message);
      setFeedback({ message, tone: "danger" });
    } finally {
      setPending(false);
    }
  }

  async function applyTargetUpdate() {
    if (!targetUpdateReview) return;
    setPending(true);
    setActionError(null);
    try {
      const response = await apiPost<BulkUpdateMonitoringShareTargetsResponse>(
        "/api/v1/monitoring-shares/update-targets",
        apiToken,
        {
          confirmed: true,
          preview_hash: targetUpdateReview.response.preview_hash,
          share_ids: targetUpdateReview.shares.map((share) => share.id),
        } satisfies BulkUpdateMonitoringShareTargetsRequest,
      );
      setShares((current) => applyTargetChanges(current, response.changes));
      setFeedback({
        message: `Updated frozen targets for ${response.changes.length} shared ${response.changes.length === 1 ? "view" : "views"}. Existing public client identities were preserved.`,
        tone: "success",
      });
      setTargetUpdateReview(null);
      void loadShares();
    } catch (error) {
      const message = actionErrorMessage(error);
      setActionError(message);
      setFeedback({ message, tone: "danger" });
    } finally {
      setPending(false);
    }
  }

  async function copyShareUrl(share: MonitoringShareView) {
    enterReviewWorkflow("copy");
    setPending(true);
    try {
      const response = await apiGet<MonitoringShareUrlResponse>(
        `/api/v1/monitoring-shares/${encodeURIComponent(share.id)}/url`,
        apiToken,
      );
      const recoveredUrl: SharedViewUrl = {
        createdAt: share.created_at,
        name: share.name,
        shareId: share.id,
        url: absoluteShareUrl(response.fragment_path),
      };
      setSharedViewUrl(recoveredUrl);
      await copyShareUrlText(recoveredUrl);
    } catch (error) {
      const message = actionErrorMessage(error);
      setActionError(message);
      setFeedback({ message, tone: "danger" });
    } finally {
      setPending(false);
    }
  }

  async function copyDisplayedShareUrl() {
    if (!sharedViewUrl) return;
    await copyShareUrlText(sharedViewUrl);
  }

  async function copyShareUrlText(shareUrl: SharedViewUrl) {
    if (!navigator.clipboard?.writeText) {
      setFeedback({
        message:
          "Clipboard access is unavailable. Select the URL below and copy it manually.",
        tone: "warning",
      });
      return;
    }
    try {
      await navigator.clipboard.writeText(shareUrl.url);
      setFeedback({
        message: `Copied the public URL for ${shareUrl.name}.`,
        tone: "success",
      });
    } catch (error) {
      setFeedback({
        message: `Clipboard copy failed. Select and copy the URL manually. ${actionErrorMessage(error)}`,
        tone: "warning",
      });
    }
  }

  const columns = useMemo<ConsoleDataGridColumn<MonitoringShareView>[]>(
    () => [
      {
        cell: (share) => (
          <span className="historyPrimary">
            <strong title={share.name}>{share.name}</strong>
            <small title={`${share.id} · ${shareTargetRefreshTitle(share)}`}>
              {shortShareId(share.id)} · {shareTargetRefreshLabel(share)}
            </small>
          </span>
        ),
        header: "Name",
        id: "name",
        mobilePrimary: true,
        searchValue: (share) => `${share.name} ${share.id}`,
        size: 220,
        sortValue: (share) => share.name,
      },
      {
        align: "end",
        cell: (share) => share.target_count,
        header: "VPSs",
        id: "targets",
        searchValue: (share) => share.target_count,
        size: 82,
        sortValue: (share) => share.target_count,
      },
      {
        cell: (share) => visibleDataLabel(share),
        header: "Visible data",
        id: "visibility",
        searchValue: visibleDataLabel,
        size: 260,
        sortValue: visibleDataLabel,
      },
      {
        cell: (share) => {
          const status = effectiveShareStatus(share);
          return (
            <ConsoleStatusBadge tone={shareStatusTone(status)}>
              {shareStatusLabel(status)}
            </ConsoleStatusBadge>
          );
        },
        header: "Status",
        id: "status",
        mobileState: true,
        searchValue: effectiveShareStatus,
        size: 110,
        sortValue: effectiveShareStatus,
      },
      {
        cell: (share) => (
          <time
            dateTime={share.expires_at}
            title={formatFullTime(share.expires_at)}
          >
            {formatCompactTime(share.expires_at)}
          </time>
        ),
        header: "Expires",
        id: "expires",
        searchValue: (share) => share.expires_at,
        size: 145,
        sortValue: (share) => timestampMillis(share.expires_at),
      },
      {
        cell: (share) =>
          share.last_visited_at ? (
            <time
              dateTime={share.last_visited_at}
              title={formatFullTime(share.last_visited_at)}
            >
              {formatCompactTime(share.last_visited_at)}
            </time>
          ) : (
            <span className="mutedText">Never</span>
          ),
        header: "Last accessed",
        id: "last_accessed",
        searchValue: (share) => share.last_visited_at ?? "never",
        size: 145,
        sortValue: (share) =>
          share.last_visited_at
            ? timestampMillis(share.last_visited_at)
            : Number.NEGATIVE_INFINITY,
      },
      {
        align: "end",
        cell: (share) => share.visitor_count,
        header: "Visitors",
        id: "visitors",
        searchValue: (share) => share.visitor_count,
        size: 90,
        sortValue: (share) => share.visitor_count,
      },
    ],
    [],
  );

  const actions = useMemo<ConsoleDataGridAction<MonitoringShareView>[]>(
    () => [
      {
        description: (rows) =>
          rows.some((share) => effectiveShareStatus(share) !== "active")
            ? "Only active shared views can refresh their frozen targets."
            : rows.some((share) => !share.target_update_evidence_available)
              ? "Target refresh evidence is unavailable. Frozen targets remain unchanged; repair or retry, then use Update targets with the required access."
              : rows.some((share) => share.target_update_available)
                ? `Re-resolve the saved selector for ${rows.length} shared ${rows.length === 1 ? "view" : "views"}, then review exact additions and removals.`
                : "The selected frozen targets already match their saved selectors.",
        disabled: (rows) =>
          pending ||
          rows.length === 0 ||
          rows.some((share) => effectiveShareStatus(share) !== "active") ||
          rows.some((share) => !share.target_update_evidence_available) ||
          !rows.some((share) => share.target_update_available),
        icon: <RefreshCw size={14} />,
        label: "Update targets",
        onSelect: (rows) => void reviewTargetUpdate(rows),
      },
      {
        description: (rows) =>
          rows.some((share) => effectiveShareStatus(share) !== "active")
            ? "Only active shared views can be extended. Expired or revoked links require a replacement."
            : `Extend ${rows.length} selected shared ${rows.length === 1 ? "view" : "views"}; each resulting expiry is capped at 365 days from now.`,
        disabled: (rows) =>
          pending ||
          rows.length === 0 ||
          rows.some((share) => effectiveShareStatus(share) !== "active"),
        icon: <Clock3 size={14} />,
        label: "Extend",
        onSelect: (rows) => {
          enterReviewWorkflow("lifecycle");
          setExtensionValue(24);
          setExtensionUnit("hours");
          setPendingAction({ kind: "extend", shares: rows });
        },
      },
      {
        description: (rows) =>
          rows.some((share) => effectiveShareStatus(share) !== "active")
            ? "Only active shared views can be revoked. Expired and revoked rows are retained evidence."
            : `Immediately and irreversibly revoke ${rows.length} selected shared ${rows.length === 1 ? "view" : "views"}.`,
        disabled: (rows) =>
          pending ||
          rows.length === 0 ||
          rows.some((share) => effectiveShareStatus(share) !== "active"),
        icon: <Ban size={14} />,
        label: "Revoke",
        onSelect: (rows) => {
          enterReviewWorkflow("lifecycle");
          setPendingAction({ kind: "revoke", shares: rows });
        },
        separatorBefore: true,
        tone: "danger",
      },
    ],
    [pending],
  );

  const rowActions = useMemo<ConsoleDataGridAction<MonitoringShareView>[]>(
    () => [
      {
        description: (rows) =>
          rows.length === 1 && effectiveShareStatus(rows[0]) === "active"
            ? `Copy the public URL for ${rows[0].name}.`
            : "Only one active shared view URL can be copied at a time.",
        disabled: (rows) =>
          pending ||
          rows.length !== 1 ||
          effectiveShareStatus(rows[0]) !== "active",
        icon: <Copy size={14} />,
        label: "Copy URL",
        onSelect: (rows) => void copyShareUrl(rows[0]),
      },
      ...actions,
    ],
    [actions, pending],
  );

  const lifecycleItems = pendingAction
    ? [
        {
          label:
            pendingAction.shares.length === 1 ? "Shared view" : "Selection",
          value: shareSelectionLabel(pendingAction.shares),
        },
        {
          label: "VPS scope",
          value: `${pendingAction.shares.reduce((count, share) => count + share.target_count, 0)} total frozen target references`,
        },
        ...(pendingAction.kind === "extend" && extensionSecs !== null
          ? [
              {
                label: "Extension",
                value: durationLabel(extensionValue, extensionUnit),
              },
            ]
          : []),
      ]
    : [];
  const targetUpdateItems = sharedTargetUpdateReviewItems(
    targetUpdateReview,
    agents,
  );

  return (
    <section className="workspace singleColumn observabilitySharedViewsWorkspace">
      <div className="fleetPanel observabilitySharedViewsPanel">
        <div className="sectionHeader">
          <div>
            <h2>Shared views</h2>
            <span>
              Create and govern public read-only monitoring views without
              exposing operator workflows or internal configuration.
            </span>
          </div>
        </div>

        <ActionFeedback
          className="localActionFeedback dashboardActionFeedback"
          message={loadError ?? feedback?.message}
          ref={feedbackRef}
          tone={loadError ? "danger" : feedback?.tone}
        />

        <div
          aria-label="Shared view status filters"
          className="observabilityWorkflowTabs"
          onKeyDown={handleTabListKeyDown}
          role="tablist"
        >
          {STATUS_OPTIONS.map((option) => (
            <button
              aria-controls={`monitoring-share-${option.status}-panel`}
              aria-selected={statusFilter === option.status}
              className={statusFilter === option.status ? "active" : ""}
              id={tabId("monitoring-share", option.status)}
              key={option.status}
              onClick={() => setStatusFilter(option.status)}
              role="tab"
              tabIndex={statusFilter === option.status ? 0 : -1}
              type="button"
            >
              <strong>
                {option.label} · {sharesByStatus[option.status].length}
              </strong>
              <span>{option.detail}</span>
            </button>
          ))}
        </div>

        {sharedViewUrl ? (
          <section
            aria-label="Shared view public URL"
            className="dashboardSection observabilityGroupSection"
            ref={sharedViewUrlRef}
            tabIndex={-1}
          >
            <div className="dashboardSectionHeader">
              <div>
                <h2>Public shared-view URL</h2>
                <span>
                  Copy it now or recover it later from this row's Copy URL
                  action. Treat it as a bearer credential.
                </span>
              </div>
              <div className="sectionActions">
                <button
                  className="primaryAction compactAction"
                  onClick={() => void copyDisplayedShareUrl()}
                  title="Copy the complete bearer URL to the clipboard."
                  type="button"
                >
                  <Copy size={14} />
                  Copy URL
                </button>
                <button
                  className="secondaryAction compactAction"
                  onClick={() => setSharedViewUrl(null)}
                  title="Dismiss the displayed URL. It remains available from the row action while the share is active."
                  type="button"
                >
                  Dismiss
                </button>
              </div>
            </div>
            <div className="consoleInlineDetailGrid">
              <span title="Complete public share URL created for this monitoring view.">
                <strong>{sharedViewUrl.name}</strong>
                <pre>{sharedViewUrl.url}</pre>
              </span>
              <span>
                <strong>Share ID</strong>
                <span>{sharedViewUrl.shareId}</span>
              </span>
              <span>
                <strong>Created</strong>
                <span title={formatFullTime(sharedViewUrl.createdAt)}>
                  {formatCompactTime(sharedViewUrl.createdAt)}
                </span>
              </span>
            </div>
          </section>
        ) : null}

        <div
          aria-labelledby={tabId("monitoring-share", statusFilter)}
          id={`monitoring-share-${statusFilter}-panel`}
          role="tabpanel"
        >
          <ConsoleDataGrid
            actions={actions}
            columns={columns}
            defaultColumnVisibility={{ visitors: false }}
            defaultPageSize={100}
            empty={
              loading
                ? "Loading shared views."
                : `No ${statusFilter} shared views. ${statusFilter === "active" ? "Create one when an external read-only monitoring view is needed." : "Lifecycle evidence will appear here."}`
            }
            getRowId={(share) => share.id}
            itemLabel="shared views"
            renderExpandedRow={(share) => <ShareEvidence share={share} />}
            rowActions={rowActions}
            rows={visibleShares}
            searchPlaceholder="Search name, ID, data, or evidence"
            singleExpandedRow
            storageKey="vpsman.monitoring.sharedViews"
            title={`${shareStatusLabel(statusFilter)} shared views`}
            toolbarActions={
              <div
                aria-label="Shared view table controls"
                className="sectionActions"
              >
                <button
                  className="secondaryAction compactAction"
                  disabled={loading || pending}
                  onClick={() => void loadShares()}
                  title={
                    loading || pending
                      ? "Wait for the current shared-view request to finish"
                      : "Refresh shared-view lifecycle, target, and access evidence"
                  }
                  type="button"
                >
                  <RefreshCw size={14} />
                  {loading ? "Refreshing" : "Refresh"}
                </button>
                <button
                  className="primaryAction compactAction"
                  disabled={pending}
                  onClick={openCreate}
                  title={
                    pending
                      ? "Wait for the current shared-view operation to finish"
                      : "Create a public read-only monitoring view with frozen targets"
                  }
                  type="button"
                >
                  <Plus size={14} />
                  Create shared view
                </button>
              </div>
            }
          />
        </div>
      </div>

      <ConsoleActionDrawer
        description="The selector is resolved on the server at review time. Its frozen VPS list can later be explicitly refreshed; visible-data groups remain immutable."
        onClose={closeCreate}
        open={drawerOpen}
        title="Create shared view"
      >
        <form
          className="consoleFormGrid"
          onSubmit={(event) => {
            event.preventDefault();
            void reviewCreate();
          }}
        >
          <label
            className="consoleField fieldWide"
            title={
              pending
                ? "Display-name editing is disabled while a shared-view operation is pending"
                : "Public display name shown to both operators and visitors"
            }
          >
            <span>Display name</span>
            <input
              aria-label="Shared view display name"
              autoFocus
              data-tooltip-disabled-reason="Wait for the current shared-view operation to finish before editing the display name."
              disabled={pending}
              maxLength={128}
              onChange={(event) => updateDraft({ name: event.target.value })}
              placeholder="Customer A"
              value={draft.name}
            />
            <small>
              Shown to operators and visitors; it does not expose the creator.
            </small>
          </label>

          <div
            className="consoleField fieldFull"
            title={
              pending
                ? "Frozen-scope editing is disabled while a shared-view operation is pending"
                : "The server resolves this selector into an exact frozen public VPS list during review"
            }
          >
            <span>Frozen VPS scope</span>
            <div className="targetSelector">
              <div className="targetSelectorHeader">
                <strong>Target selector</strong>
                <span>
                  {selectorParse.error
                    ? "Fix the expression before review"
                    : selectorEvidenceUnavailable
                      ? VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE
                      : `${localTargets.length}/${agents.length} VPSs in the local preview; review resolves the authoritative list`}
                </span>
              </div>
              <SearchExpressionInput
                agents={agents}
                ariaLabel="Shared view target selector"
                disabled={pending}
                onChange={(selectorExpression) =>
                  updateDraft({ selectorExpression })
                }
                placeholder="* or provider:example && country:SG"
                showMatchCount
                value={draft.selectorExpression}
                verification={
                  selectorParse.error || selectorEvidenceUnavailable
                    ? "invalid"
                    : draft.selectorExpression.trim()
                      ? "valid"
                      : "neutral"
                }
                verificationMessage={
                  selectorParse.error ??
                  (selectorEvidenceUnavailable
                    ? VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE
                    : undefined)
                }
              />
              {!selectorEvidenceUnavailable && (
                <LocalTargetPreview
                  agents={localTargets}
                  ariaLabel="Shared view local VPS preview"
                />
              )}
            </div>
            <small>
              Default * means all current VPSs. Future fleet changes do not
              change this saved scope until an operator chooses Update targets.
            </small>
          </div>

          <label
            className="consoleField"
            title={
              pending
                ? "Expiry editing is disabled while a shared-view operation is pending"
                : "Duration before the public bearer link expires"
            }
          >
            <span>Expiry amount</span>
            <input
              aria-label="Shared view expiry amount"
              data-tooltip-disabled-reason="Wait for the current shared-view operation to finish before editing expiry."
              disabled={pending}
              max={durationMaximum(draft.durationUnit)}
              min={1}
              onChange={(event) =>
                updateDraft({ durationValue: Number(event.target.value) })
              }
              step={1}
              type="number"
              value={draft.durationValue}
            />
          </label>
          <label
            className="consoleField"
            title={
              pending
                ? "Expiry-unit selection is disabled while a shared-view operation is pending"
                : "Unit for the public shared-view expiry duration"
            }
          >
            <span>Expiry unit</span>
            <select
              aria-label="Shared view expiry unit"
              data-tooltip-disabled-reason="Wait for the current shared-view operation to finish before changing expiry units."
              disabled={pending}
              onChange={(event) =>
                updateDraft({
                  durationUnit: event.target.value as DurationUnit,
                })
              }
              value={draft.durationUnit}
            >
              <option value="minutes">Minutes</option>
              <option value="hours">Hours</option>
              <option value="days">Days</option>
            </select>
            <small>Accepted range: one minute through 365 days.</small>
          </label>

          <div className="consoleField fieldFull">
            <span>Visible data</span>
            <small>
              Display name and health are always visible. Internal VPS IDs,
              network-address fields, configuration, actions, jobs, terminals,
              files, backups, audit data, and operator identity are never
              included. Operator-entered display, product, and Ping target names
              appear as entered, so keep sensitive addresses out of public
              labels.
            </small>
          </div>
          {visibilityOptions(draft.visibility).map((option) => (
            <div className="consoleField" key={option.field}>
              <span>{option.label}</span>
              <label
                className="checkLine borderedToggle"
                title={
                  pending
                    ? `${option.label} visibility is disabled while a shared-view operation is pending`
                    : option.disabled
                      ? `${option.label} visibility requires its parent visibility group`
                      : `${option.checked ? "Hide" : "Show"} ${option.label.toLocaleLowerCase()} in the public view`
                }
              >
                <input
                  checked={option.checked}
                  disabled={pending || option.disabled}
                  onChange={(event) =>
                    updateVisibility(option.field, event.target.checked)
                  }
                  type="checkbox"
                />
                <span>{option.detail}</span>
              </label>
            </div>
          ))}

          <ActionFeedback
            className="localActionFeedback fieldFull"
            message={actionError}
            ref={createFeedbackRef}
            tone="danger"
          />
          <div className="consoleFormActions fieldFull">
            <button
              className="secondaryAction"
              disabled={pending}
              onClick={closeCreate}
              title={
                pending
                  ? "Wait for the current shared-view operation to finish before closing the drawer"
                  : "Cancel shared-view creation"
              }
              type="button"
            >
              Cancel
            </button>
            <button
              className="primaryAction"
              disabled={pending || !draftReady}
              title={
                pending
                  ? "The shared-view selector is already being resolved for review"
                  : !draft.name.trim()
                    ? "Enter a public display name"
                    : selectorParse.error
                      ? `Fix the VPS selector: ${selectorParse.error}`
                      : selectorEvidenceUnavailable
                        ? VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE
                        : !draft.selectorExpression.trim()
                          ? "Enter a VPS selector expression"
                          : durationSeconds(
                                draft.durationValue,
                                draft.durationUnit,
                              ) === null
                            ? "Choose an expiry from one minute through 365 days"
                            : "Resolve the selector and review the exact public visibility snapshot"
              }
              type="submit"
            >
              <Link2 size={16} />
              {pending ? "Resolving targets" : "Review creation"}
            </button>
          </div>
          <ConfirmationPrompt
            className="fieldFull"
            confirmLabel="Create shared view"
            detail="Create one public URL for this exact target and visibility snapshot. Frozen targets change only through an explicit reviewed Update targets action."
            error={actionError}
            items={
              review
                ? [
                    { label: "Name", value: review.request.name },
                    {
                      label: "Frozen VPSs",
                      value: review.request.target_client_ids?.length ?? 0,
                    },
                    {
                      label: "Selector evidence",
                      value: review.request.selector_expression ?? "*",
                    },
                    {
                      label: "Visible data",
                      value: visibleDataRequestLabel(review.request.visibility),
                    },
                    {
                      label: "Expiry",
                      value: formatFullTime(
                        new Date(
                          Date.now() + review.request.expires_in_secs * 1_000,
                        ).toISOString(),
                      ),
                    },
                  ]
                : []
            }
            onCancel={() => {
              if (pending) return;
              setReview(null);
            }}
            onConfirm={() => void createShare()}
            open={review !== null}
            pending={pending}
            title="Confirm public monitoring view"
          >
            {review ? (
              <LocalTargetPreview
                agents={review.targets}
                ariaLabel="Reviewed frozen shared view targets"
              />
            ) : null}
          </ConfirmationPrompt>
        </form>
      </ConsoleActionDrawer>

      <ConfirmationPrompt
        confirmLabel="Update targets"
        detail="Replaces only each selected view's frozen VPS list with the current authoritative result of its saved selector. Visibility and URL stay unchanged."
        error={actionError}
        items={targetUpdateItems}
        onCancel={() => {
          if (pending) return;
          setActionError(null);
          setTargetUpdateReview(null);
        }}
        onConfirm={() => void applyTargetUpdate()}
        open={targetUpdateReview !== null}
        pending={pending}
        title="Confirm shared-view target update"
      />

      <ConfirmationPrompt
        confirmDisabled={
          pendingAction?.kind === "extend" && extensionSecs === null
        }
        confirmLabel={
          pendingAction?.kind === "revoke" ? "Revoke now" : "Extend views"
        }
        detail={
          pendingAction?.kind === "revoke"
            ? "Revocation is immediate and irreversible. Expired or revoked links cannot be reactivated; create a replacement if sharing is needed later."
            : "Add the reviewed duration to each selected active view. Resulting expiry is capped at 365 days from now; target and visibility scope remain unchanged."
        }
        error={actionError}
        items={lifecycleItems}
        onCancel={() => {
          if (pending) return;
          setActionError(null);
          setPendingAction(null);
        }}
        onConfirm={() => void applyLifecycleAction()}
        open={pendingAction !== null}
        pending={pending}
        title={
          pendingAction?.kind === "revoke"
            ? "Revoke shared views"
            : "Extend shared views"
        }
        tone={pendingAction?.kind === "revoke" ? "danger" : "normal"}
      >
        {pendingAction?.kind === "extend" ? (
          <div className="formRow">
            <label
              className="consoleField"
              title={
                pending
                  ? "Extension editing is disabled while the lifecycle operation is pending"
                  : "Additional duration applied to every selected active shared view"
              }
            >
              <span>Extension amount</span>
              <input
                aria-label="Shared view extension amount"
                data-tooltip-disabled-reason="Wait for the current shared-view lifecycle operation to finish."
                disabled={pending}
                max={durationMaximum(extensionUnit)}
                min={1}
                onChange={(event) => {
                  setExtensionValue(Number(event.target.value));
                  setActionError(null);
                  setFeedback(null);
                }}
                step={1}
                type="number"
                value={extensionValue}
              />
            </label>
            <label
              className="consoleField"
              title={
                pending
                  ? "Extension-unit selection is disabled while the lifecycle operation is pending"
                  : "Unit for the reviewed shared-view expiry extension"
              }
            >
              <span>Extension unit</span>
              <select
                aria-label="Shared view extension unit"
                data-tooltip-disabled-reason="Wait for the current shared-view lifecycle operation to finish."
                disabled={pending}
                onChange={(event) => {
                  setExtensionUnit(event.target.value as DurationUnit);
                  setActionError(null);
                  setFeedback(null);
                }}
                value={extensionUnit}
              >
                <option value="minutes">Minutes</option>
                <option value="hours">Hours</option>
                <option value="days">Days</option>
              </select>
            </label>
          </div>
        ) : null}
      </ConfirmationPrompt>
    </section>
  );
}

function ShareEvidence({ share }: { share: MonitoringShareView }) {
  const status = effectiveShareStatus(share);
  return (
    <div className="targetDetail">
      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Share ID</strong>
          <span>{share.id}</span>
        </span>
        <span>
          <strong>Status</strong>
          <span>{shareStatusLabel(status)}</span>
        </span>
        <span>
          <strong>Frozen selector</strong>
          <span>{share.selector_expression}</span>
        </span>
        <span>
          <strong>Frozen VPS count</strong>
          <span>{share.target_count}</span>
        </span>
        <span>
          <strong>Target refresh</strong>
          <span title={shareTargetRefreshTitle(share)}>
            {shareTargetRefreshLabel(share)}
          </span>
        </span>
        <span>
          <strong>Always visible</strong>
          <span>Display name · Health</span>
        </span>
        <span>
          <strong>Optional visible data</strong>
          <span>{optionalVisibilityLabel(share.visibility)}</span>
        </span>
        <span>
          <strong>Created</strong>
          <span title={formatFullTime(share.created_at)}>
            {formatCompactTime(share.created_at)}
          </span>
        </span>
        <span>
          <strong>Created by</strong>
          <span>{share.created_by ?? "Operator unavailable"}</span>
        </span>
        <span>
          <strong>Expires</strong>
          <span title={formatFullTime(share.expires_at)}>
            {formatCompactTime(share.expires_at)}
          </span>
        </span>
        {share.revoked_at ? (
          <span>
            <strong>Revoked</strong>
            <span title={formatFullTime(share.revoked_at)}>
              {formatCompactTime(share.revoked_at)}
            </span>
          </span>
        ) : null}
        <span>
          <strong>Unique visitors</strong>
          <span>{share.visitor_count}</span>
        </span>
        <span>
          <strong>First accessed</strong>
          <span
            title={
              share.first_visited_at
                ? formatFullTime(share.first_visited_at)
                : undefined
            }
          >
            {share.first_visited_at
              ? formatCompactTime(share.first_visited_at)
              : "Never"}
          </span>
        </span>
        <span>
          <strong>Last accessed</strong>
          <span
            title={
              share.last_visited_at
                ? formatFullTime(share.last_visited_at)
                : undefined
            }
          >
            {share.last_visited_at
              ? formatCompactTime(share.last_visited_at)
              : "Never"}
          </span>
        </span>
      </div>
      <p className="observabilityMetricDefinition">
        Frozen targets change only through a reviewed Update targets action;
        visibility remains immutable. Extend changes only expiry, Revoke stops
        access immediately, and Copy URL retrieves the active bearer link.
      </p>
    </div>
  );
}

function defaultShareDraft(initialSelectorExpression: string): ShareDraft {
  return {
    durationUnit: "hours",
    durationValue: 24,
    name: "",
    selectorExpression: initialSelectorExpression.trim() || "*",
    visibility: { ...DEFAULT_VISIBILITY },
  };
}

function durationSeconds(value: number, unit: DurationUnit): number | null {
  if (!Number.isSafeInteger(value) || value < 1) {
    return null;
  }
  const multiplier =
    unit === "minutes" ? 60 : unit === "hours" ? 60 * 60 : 24 * 60 * 60;
  const seconds = value * multiplier;
  return seconds >= MIN_EXPIRY_SECS && seconds <= MAX_EXPIRY_SECS
    ? seconds
    : null;
}

function durationMaximum(unit: DurationUnit): number {
  if (unit === "minutes") return 365 * 24 * 60;
  if (unit === "hours") return 365 * 24;
  return 365;
}

function durationLabel(value: number, unit: DurationUnit): string {
  const singular = unit.slice(0, -1);
  return `${value} ${value === 1 ? singular : unit}`;
}

function visibleMetricGroupCount(
  visibility: MonitoringShareVisibilityRequest,
): number {
  return [
    visibility.system_information,
    visibility.resources,
    visibility.network,
    visibility.traffic,
    visibility.ping,
  ].filter(Boolean).length;
}

function visibilityOptions(
  visibility: Required<MonitoringShareVisibilityRequest>,
): Array<{
  checked: boolean;
  detail: string;
  disabled: boolean;
  field: keyof MonitoringShareVisibilityRequest;
  label: string;
}> {
  const metricCount = visibleMetricGroupCount(visibility);
  return [
    {
      checked: visibility.identity_context,
      detail: "May disclose provider, product name, country, region, and tags",
      disabled: false,
      field: "identity_context",
      label: "Identity context",
    },
    {
      checked: visibility.billing,
      detail: "Configured billing price and cycle",
      disabled: false,
      field: "billing",
      label: "Billing",
    },
    {
      checked: visibility.system_information,
      detail: "OS, architecture, CPU, kernel, virtualization, and uptime",
      disabled: false,
      field: "system_information",
      label: "System information",
    },
    {
      checked: visibility.resources,
      detail: "CPU, memory, disk, and load",
      disabled: false,
      field: "resources",
      label: "Resources",
    },
    {
      checked: visibility.network,
      detail: "RX/TX rates without IP addresses",
      disabled: false,
      field: "network",
      label: "Network rate",
    },
    {
      checked: visibility.traffic,
      detail: "Traffic totals, quota, and cycle",
      disabled: false,
      field: "traffic",
      label: "Traffic",
    },
    {
      checked: visibility.ping,
      detail: "Allowed Ping target status and history",
      disabled: false,
      field: "ping",
      label: "Ping",
    },
    {
      checked: visibility.detail_history,
      detail:
        metricCount === 0
          ? "Choose at least one metric group first"
          : "Allow read-only metric detail and history",
      disabled: metricCount === 0,
      field: "detail_history",
      label: "Detail history",
    },
  ];
}

function createDraftError(
  draft: ShareDraft,
  selectorError: string | null,
): string {
  if (!draft.name.trim()) return "Enter a display name for the shared view.";
  if (draft.name.trim().length > 128)
    return "Keep the shared view name within 128 characters.";
  if (!draft.selectorExpression.trim())
    return "Enter a VPS selector expression. Use * for all current VPSs.";
  if (selectorError) return `Fix the VPS selector: ${selectorError}`;
  if (durationSeconds(draft.durationValue, draft.durationUnit) === null)
    return "Choose an expiry from one minute through 365 days.";
  if (
    draft.visibility.detail_history &&
    visibleMetricGroupCount(draft.visibility) === 0
  ) {
    return "Detail history requires at least one visible metric group.";
  }
  return "Review the highlighted fields before resolving targets.";
}

function effectiveShareStatus(share: MonitoringShareView): ShareStatus {
  if (share.revoked_at || share.status === "revoked") return "revoked";
  if (
    share.status === "expired" ||
    timestampMillis(share.expires_at) <= Date.now()
  ) {
    return "expired";
  }
  return "active";
}

function shareStatusLabel(status: ShareStatus): string {
  return status.charAt(0).toUpperCase() + status.slice(1);
}

function shareStatusTone(status: ShareStatus): "neutral" | "ok" | "warning" {
  if (status === "active") return "ok";
  if (status === "expired") return "warning";
  return "neutral";
}

function optionalVisibilityLabel(
  visibility: MonitoringShareView["visibility"],
): string {
  const labels = [
    visibility.identity_context ? "Identity context" : null,
    visibility.billing ? "Billing" : null,
    visibility.system_information ? "System information" : null,
    visibility.resources ? "Resources" : null,
    visibility.network ? "Network rate" : null,
    visibility.traffic ? "Traffic" : null,
    visibility.ping ? "Ping" : null,
    visibility.detail_history ? "Detail history" : null,
  ].filter((label): label is string => Boolean(label));
  return labels.length > 0 ? labels.join(" · ") : "None";
}

function visibleDataLabel(share: MonitoringShareView): string {
  return visibleDataVisibilityLabel(share.visibility);
}

function visibleDataVisibilityLabel(
  visibility: MonitoringShareView["visibility"],
): string {
  const optional = optionalVisibilityLabel(visibility);
  return optional === "None" ? "Name · Health" : `Name · Health · ${optional}`;
}

function visibleDataRequestLabel(
  visibility: MonitoringShareVisibilityRequest,
): string {
  return visibleDataVisibilityLabel({
    billing: Boolean(visibility.billing),
    detail_history: Boolean(visibility.detail_history),
    identity_context: Boolean(visibility.identity_context),
    network: Boolean(visibility.network),
    ping: Boolean(visibility.ping),
    resources: Boolean(visibility.resources),
    system_information: Boolean(visibility.system_information),
    traffic: Boolean(visibility.traffic),
  });
}

function mergeShares(
  current: MonitoringShareView[],
  updated: MonitoringShareView[],
): MonitoringShareView[] {
  const byId = new Map(current.map((share) => [share.id, share]));
  for (const share of updated) byId.set(share.id, share);
  return [...byId.values()].sort(
    (left, right) =>
      timestampMillis(right.created_at) - timestampMillis(left.created_at) ||
      left.id.localeCompare(right.id),
  );
}

function applyTargetChanges(
  shares: MonitoringShareView[],
  changes: BulkUpdateMonitoringShareTargetsResponse["changes"],
): MonitoringShareView[] {
  const byId = new Map(changes.map((change) => [change.share_id, change]));
  return shares.map((share) => {
    const change = byId.get(share.id);
    if (!change) return share;
    const removed = new Set(change.removed_client_ids);
    const targetClientIds = [
      ...share.target_client_ids.filter((clientId) => !removed.has(clientId)),
      ...change.added_client_ids,
    ].sort((left, right) => left.localeCompare(right));
    return {
      ...share,
      target_client_ids: targetClientIds,
      target_count: targetClientIds.length,
      target_update_available: false,
      target_update_evidence_available: true,
    };
  });
}

function shareTargetRefreshLabel(share: MonitoringShareView): string {
  if (effectiveShareStatus(share) !== "active") {
    return "Not applicable while inactive";
  }
  if (!share.target_update_evidence_available) {
    return "Target refresh unavailable";
  }
  return share.target_update_available
    ? "Saved selector now resolves differently"
    : "Frozen targets match the latest server check";
}

function shareTargetRefreshTitle(share: MonitoringShareView): string {
  if (effectiveShareStatus(share) !== "active") {
    return "Expired or revoked shared views keep their frozen targets as retained evidence.";
  }
  if (!share.target_update_evidence_available) {
    return "Target refresh evidence is unavailable. Frozen targets remain unchanged; repair or retry, then use Update targets with the required access.";
  }
  return share.target_update_available
    ? "The saved selector now resolves to a different VPS list."
    : "Frozen targets match the latest server check.";
}

function deduplicateShares(
  shares: MonitoringShareView[],
): MonitoringShareView[] {
  return mergeShares([], shares);
}

function targetChangesPresent(
  response: BulkUpdateMonitoringShareTargetsResponse,
): boolean {
  return response.changes.some(
    (change) =>
      change.added_client_ids.length > 0 ||
      change.removed_client_ids.length > 0,
  );
}

function sharedTargetUpdateReviewItems(
  review: TargetUpdateReview | null,
  agents: AgentView[],
): Array<{ label: string; title?: string; value: ReactNode }> {
  if (!review) return [];
  const agentsById = new Map(agents.map((agent) => [agent.id, agent]));
  return review.response.changes.flatMap((change) => [
    {
      label: `${change.share_name} · selector`,
      value: <code>{change.selector_expression}</code>,
    },
    {
      label: `${change.share_name} · add (${change.added_client_ids.length})`,
      title: change.added_client_ids.join(", "),
      value: exactSharedClientList(change.added_client_ids, agentsById),
    },
    {
      label: `${change.share_name} · remove (${change.removed_client_ids.length})`,
      title: change.removed_client_ids.join(", "),
      value: exactSharedClientList(change.removed_client_ids, agentsById),
    },
    {
      label: `${change.share_name} · unchanged`,
      value: String(change.unchanged_count),
    },
  ]);
}

function exactSharedClientList(
  clientIds: string[],
  agentsById: Map<string, AgentView>,
): ReactNode {
  if (clientIds.length === 0) return "None";
  return (
    <span>
      {clientIds.map((clientId, index) => (
        <span key={clientId} title={clientId}>
          {index > 0 ? ", " : ""}
          {agentsById.get(clientId)?.display_name?.trim() || "Unknown VPS"} (
          {clientId})
        </span>
      ))}
    </span>
  );
}

function absoluteShareUrl(fragmentPath: string): string {
  const url = new URL(window.location.href);
  url.search = "";
  url.hash = fragmentPath.startsWith("#")
    ? fragmentPath.slice(1)
    : fragmentPath;
  return url.toString();
}

function shortShareId(id: string): string {
  return id.length > 12 ? id.slice(0, 8) : id;
}

function shareSelectionLabel(shares: MonitoringShareView[]): string {
  if (shares.length === 1) return shares[0]?.name ?? "One shared view";
  const names = shares.slice(0, 3).map((share) => share.name);
  return `${names.join(", ")}${shares.length > names.length ? ` +${shares.length - names.length} more` : ""}`;
}

function actionErrorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : "The shared-view action returned no diagnostic detail. No success is assumed; refresh current state before retrying.";
}
