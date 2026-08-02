import { Ban, Clock3, Copy, Link2, Plus, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
} from "../../searchExpression";
import { LocalTargetPreview } from "../TargetImpactPreview";
import type {
  AgentView,
  BulkResolveResponse,
  CreateMonitoringShareRequest,
  CreateMonitoringShareResponse,
  ExtendMonitoringSharesRequest,
  MonitoringSharesMutationResponse,
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

type OneTimeShareUrl = {
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
  detail_history: true,
  identity_context: false,
  network: true,
  ping: true,
  resources: true,
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
  const [statusFilter, setStatusFilter] = useHistoryEntryState<ShareStatus>(
    "observability.shared-views.status",
    "active",
  );
  const [oneTimeUrl, setOneTimeUrl] =
    useHistoryEntryState<OneTimeShareUrl | null>(
      "observability.shared-views.one-time-url",
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
  const [extensionValue, setExtensionValue] = useState(24);
  const [extensionUnit, setExtensionUnit] = useState<DurationUnit>("hours");
  const [pending, setPending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const loadGeneration = useRef(0);
  const initialSelectorSnapshot = useRef(initialSelectorExpression);
  const initialSelectorConsumed = useRef(onInitialSelectorConsumed);
  const feedbackRef = useRef<HTMLDivElement | null>(null);
  const createFeedbackRef = useRef<HTMLDivElement | null>(null);
  const oneTimeUrlRef = useRef<HTMLElement | null>(null);
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
      !oneTimeUrl ||
      drawerOpen ||
      review !== null ||
      createdUrlFocusShareIdRef.current !== oneTimeUrl.shareId
    ) {
      return undefined;
    }
    const frame = window.requestAnimationFrame(() => {
      const outcome = oneTimeUrlRef.current;
      if (!outcome) {
        return;
      }
      createdUrlFocusShareIdRef.current = null;
      scrollIntoViewWithMotion(outcome, { block: "start" });
      outcome.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [drawerOpen, oneTimeUrl, review]);

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
  const localTargets = useMemo(
    () =>
      draft.selectorExpression.trim() && !selectorParse.error
        ? agentsMatchingExpression(agents, draft.selectorExpression)
        : [],
    [agents, draft.selectorExpression, selectorParse.error],
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

  function openCreate() {
    setDraft(defaultShareDraft(initialSelectorSnapshot.current));
    setReview(null);
    setActionError(null);
    setFeedback(null);
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
      setShares((current) => mergeShares(current, [response.share]));
      createdUrlFocusShareIdRef.current = response.share.id;
      setOneTimeUrl({
        createdAt: response.share.created_at,
        name: response.share.name,
        shareId: response.share.id,
        url: absoluteShareUrl(response.fragment_path),
      });
      setFeedback({
        message: `Created ${response.share.name} for ${response.share.target_count} ${response.share.target_count === 1 ? "VPS" : "VPSs"}. Copy the public URL before refreshing this page.`,
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
      setShares((current) => mergeShares(current, response.shares));
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

  async function copyOneTimeUrl() {
    if (!oneTimeUrl) return;
    if (!navigator.clipboard?.writeText) {
      setFeedback({
        message:
          "Clipboard access is unavailable. Select the one-time URL and copy it manually before refreshing.",
        tone: "warning",
      });
      return;
    }
    try {
      await navigator.clipboard.writeText(oneTimeUrl.url);
      setFeedback({
        message: `Copied the one-time URL for ${oneTimeUrl.name}.`,
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
            <strong>{share.name}</strong>
            <small>{shortShareId(share.id)}</small>
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
            ? "Only active shared views can be extended. Expired or revoked links require a replacement."
            : `Extend ${rows.length} selected shared ${rows.length === 1 ? "view" : "views"}; each resulting expiry is capped at 365 days from now.`,
        disabled: (rows) =>
          pending ||
          rows.length === 0 ||
          rows.some((share) => effectiveShareStatus(share) !== "active"),
        icon: <Clock3 size={14} />,
        label: "Extend",
        onSelect: (rows) => {
          setActionError(null);
          setFeedback(null);
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
          setActionError(null);
          setFeedback(null);
          setPendingAction({ kind: "revoke", shares: rows });
        },
        separatorBefore: true,
        tone: "danger",
      },
    ],
    [pending],
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

        {oneTimeUrl ? (
          <section
            aria-label="One-time shared view URL"
            className="dashboardSection observabilityGroupSection"
            ref={oneTimeUrlRef}
            tabIndex={-1}
          >
            <div className="dashboardSectionHeader">
              <div>
                <h2>Save this public URL now</h2>
                <span>
                  The control plane stores only its digest. This browser keeps
                  the URL for Back/Forward in the current history entry, but a
                  browser reload removes it.
                </span>
              </div>
              <div className="sectionActions">
                <button
                  className="primaryAction compactAction"
                  onClick={() => void copyOneTimeUrl()}
                  type="button"
                >
                  <Copy size={14} />
                  Copy URL
                </button>
                <button
                  className="secondaryAction compactAction"
                  onClick={() => setOneTimeUrl(null)}
                  title="Dismiss this one-time URL after saving it. It cannot be recovered from the control plane."
                  type="button"
                >
                  Dismiss
                </button>
              </div>
            </div>
            <div className="consoleInlineDetailGrid">
              <span>
                <strong>{oneTimeUrl.name}</strong>
                <pre>{oneTimeUrl.url}</pre>
              </span>
              <span>
                <strong>Share ID</strong>
                <span>{oneTimeUrl.shareId}</span>
              </span>
              <span>
                <strong>Created</strong>
                <span title={formatFullTime(oneTimeUrl.createdAt)}>
                  {formatCompactTime(oneTimeUrl.createdAt)}
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
                  type="button"
                >
                  <RefreshCw size={14} />
                  {loading ? "Refreshing" : "Refresh"}
                </button>
                <button
                  className="primaryAction compactAction"
                  disabled={pending}
                  onClick={openCreate}
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
        description="The selector is resolved on the server at review time. That exact VPS list and the selected visible-data groups become immutable after creation."
        onClose={closeCreate}
        open={drawerOpen && review === null}
        title="Create shared view"
      >
        <form
          className="consoleFormGrid"
          onSubmit={(event) => {
            event.preventDefault();
            void reviewCreate();
          }}
        >
          <label className="consoleField fieldWide">
            <span>Display name</span>
            <input
              aria-label="Shared view display name"
              autoFocus
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

          <div className="consoleField fieldFull">
            <span>Frozen VPS scope</span>
            <div className="targetSelector">
              <div className="targetSelectorHeader">
                <strong>Target selector</strong>
                <span>
                  {selectorParse.error
                    ? "Fix the expression before review"
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
                  selectorParse.error
                    ? "invalid"
                    : draft.selectorExpression.trim()
                      ? "valid"
                      : "neutral"
                }
                verificationMessage={selectorParse.error ?? undefined}
              />
              <LocalTargetPreview
                agents={localTargets}
                ariaLabel="Shared view local VPS preview"
              />
            </div>
            <small>
              Default * means all current VPSs. Future fleet changes never
              change this saved scope; create a replacement for a different
              selector or target set.
            </small>
          </div>

          <label className="consoleField">
            <span>Expiry amount</span>
            <input
              aria-label="Shared view expiry amount"
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
          <label className="consoleField">
            <span>Expiry unit</span>
            <select
              aria-label="Shared view expiry unit"
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
              included. Operator-entered display and Ping target names appear as
              entered, so keep sensitive addresses out of public labels.
            </small>
          </div>
          {visibilityOptions(draft.visibility).map((option) => (
            <div className="consoleField" key={option.field}>
              <span>{option.label}</span>
              <label className="checkLine borderedToggle">
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
              type="button"
            >
              Cancel
            </button>
            <button
              className="primaryAction"
              disabled={pending || !draftReady}
              type="submit"
            >
              <Link2 size={16} />
              {pending ? "Resolving targets" : "Review creation"}
            </button>
          </div>
        </form>
      </ConsoleActionDrawer>

      <ConfirmationPrompt
        confirmLabel="Create shared view"
        detail="Create one public URL for this exact target and visibility snapshot. The URL is shown once; scope cannot be edited later."
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
            <label className="consoleField">
              <span>Extension amount</span>
              <input
                aria-label="Shared view extension amount"
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
            <label className="consoleField">
              <span>Extension unit</span>
              <select
                aria-label="Shared view extension unit"
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
        Target and visibility scope are immutable evidence. Extend changes only
        expiry; Revoke stops access immediately. The secret URL cannot be
        recovered from this record.
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
      detail: "Provider and region context",
      disabled: false,
      field: "identity_context",
      label: "Identity context",
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
    detail_history: Boolean(visibility.detail_history),
    identity_context: Boolean(visibility.identity_context),
    network: Boolean(visibility.network),
    ping: Boolean(visibility.ping),
    resources: Boolean(visibility.resources),
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

function deduplicateShares(
  shares: MonitoringShareView[],
): MonitoringShareView[] {
  return mergeShares([], shares);
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
