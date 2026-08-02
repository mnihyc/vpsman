import { Activity, Gauge, Link2, Search, Server } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  agentDisplayState,
  type AgentDisplayState,
} from "../agentDisplayState";
import { apiGet } from "../api";
import { ActionFeedback } from "../components/ActionFeedback";
import {
  formatLowerBoundCount,
  isActionableFleetAlertState,
} from "../constants";
import type { FileTransferSessionRecord } from "../typesFileTransfer";
import type {
  AgentView,
  BackupRequestRecord,
  BillingPlanView,
  FleetAlertRecord,
  JobHistoryRecord,
  CurrentPingView,
  MonitoringCardView,
  MonitoringCardsPageView,
  PingRollupView,
  PortSpeedView,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TrafficAccountingRecord,
} from "../types";
import {
  formatByteCount as formatBytes,
  INTERFACE_RATE_DEFINITION,
} from "../telemetryMetrics";
import { useHistoryEntryState } from "../historyEntryState";
import {
  OPERATOR_MONITOR_DENSITY_STORAGE_KEY,
  usePersistentMonitorCardDensity,
  type MonitorCardDensity,
} from "../monitorCardDensity";
import { selectorExpressionForClientIds } from "../searchExpression";
import { displayNameOrUnnamed, formatTime, timestampMillis } from "../utils";

type FleetMonitorPanelProps = {
  agents: AgentView[];
  apiToken?: string;
  apiError?: string | null;
  ariaLabel?: string;
  description?: string;
  embedded?: boolean;
  backups?: BackupRequestRecord[];
  failedJobCount?: number;
  fileTransfers?: FileTransferSessionRecord[];
  fleetAlerts?: FleetAlertRecord[];
  jobs?: JobHistoryRecord[];
  maxCards?: number;
  recordBounds: MonitorRecordBounds;
  runningJobCount?: number;
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  title?: string;
  toolbarAction?: ReactNode;
  onOpenVpsDetail: (agent: AgentView) => void;
  onOpenSharedViews?: (selectorExpression: string) => void;
};

export type FleetMonitorDensity = MonitorCardDensity;
type FleetMonitorSort =
  "warning" | "traffic" | "cpu" | "memory" | "region" | "provider";
type FleetMonitorStatusFilter = "all" | "online" | "warning" | "offline";
type MonitoringEvidenceState = "loading" | "ready" | "unavailable";
type MonitorRecordBounds = {
  backups: boolean;
  fileTransfers: boolean;
  fleetAlerts: boolean;
};
const monitorSortOptions: Array<{ label: string; value: FleetMonitorSort }> = [
  { label: "Warnings first", value: "warning" },
  { label: "Traffic use", value: "traffic" },
  { label: "CPU use", value: "cpu" },
  { label: "Memory", value: "memory" },
  { label: "Region", value: "region" },
  { label: "Provider", value: "provider" },
];
const NETWORK_SNAPSHOT_COHERENCE_MS = 180_000;

export function FleetMonitorPanel({
  agents,
  apiToken = "",
  apiError = null,
  ariaLabel = "VPS monitor cards",
  description = "VPS health cards for scanning state, resources, network, and alerts. Open a card for canonical VPS detail.",
  embedded = false,
  backups = [],
  failedJobCount,
  fileTransfers = [],
  fleetAlerts = [],
  jobs = [],
  maxCards,
  recordBounds,
  runningJobCount,
  telemetryNetworkRates,
  telemetryRollups,
  title = "Fleet monitor",
  toolbarAction,
  onOpenVpsDetail,
  onOpenSharedViews,
}: FleetMonitorPanelProps) {
  const historySlot = embedded ? "home.fleet-monitor" : "fleet.monitor";
  const controlIdPrefix = embedded ? "home-fleet-monitor" : "fleet-monitor";
  const [density, setDensity] = usePersistentMonitorCardDensity(
    historySlot,
    OPERATOR_MONITOR_DENSITY_STORAGE_KEY,
  );
  const [sortMode, setSortMode] = useHistoryEntryState<FleetMonitorSort>(
    `${historySlot}.sort`,
    "warning",
  );
  const [searchQuery, setSearchQuery] = useHistoryEntryState(
    `${historySlot}.search`,
    "",
  );
  const [statusFilter, setStatusFilter] =
    useHistoryEntryState<FleetMonitorStatusFilter>(
      `${historySlot}.status`,
      "all",
    );
  const [tagFilter, setTagFilter] = useHistoryEntryState(
    `${historySlot}.tag`,
    "all",
  );
  const [providerFilter, setProviderFilter] = useHistoryEntryState(
    `${historySlot}.provider`,
    "all",
  );
  const [savedScrollY, setSavedScrollY] = useHistoryEntryState(
    `${historySlot}.scrollY`,
    0,
    !embedded,
  );
  const [renderLimit, setRenderLimit] = useHistoryEntryState(
    `${historySlot}.render-limit`,
    100,
    !embedded,
  );
  const loadMoreRef = useRef<HTMLDivElement | null>(null);
  const leavingForDetailRef = useRef(false);
  const savedScrollRef = useRef(savedScrollY);
  savedScrollRef.current = savedScrollY;
  const [monitoringCards, setMonitoringCards] = useState<MonitoringCardView[]>(
    [],
  );
  const [monitoringError, setMonitoringError] = useState<string | null>(null);
  const [monitoringLoading, setMonitoringLoading] = useState(false);
  useEffect(() => {
    if (!apiToken) {
      setMonitoringCards([]);
      setMonitoringError(null);
      setMonitoringLoading(false);
      return;
    }
    let active = true;
    let inFlight = false;
    setMonitoringCards([]);
    setMonitoringError(null);
    setMonitoringLoading(true);
    const loadCards = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        let offset = 0;
        const loaded: MonitoringCardView[] = [];
        const loadedIds = new Set<string>();
        for (;;) {
          const page = await apiGet<MonitoringCardsPageView>(
            `/api/v1/monitoring/cards?limit=1000&offset=${offset}`,
            apiToken,
          );
          if (!active) return;
          if (page.offset !== offset) {
            throw new Error("Monitoring card pagination returned the wrong offset");
          }
          if (page.items.some((item) => loadedIds.has(item.client.id))) {
            throw new Error("Monitoring card pagination returned a duplicate VPS");
          }
          page.items.forEach((item) => loadedIds.add(item.client.id));
          loaded.push(...page.items);
          if (loaded.length > page.total) {
            throw new Error("Monitoring card pagination exceeded its reported total");
          }
          if (page.next_offset === null) {
            if (loaded.length !== page.total) {
              throw new Error(
                "Monitoring card pagination ended before every VPS was returned",
              );
            }
            break;
          }
          if (
            page.next_offset !== offset + page.items.length ||
            page.next_offset > page.total ||
            page.items.length === 0
          ) {
            throw new Error("Monitoring card pagination did not advance");
          }
          offset = page.next_offset;
        }
        if (active) {
          setMonitoringCards(loaded);
          setMonitoringError(null);
        }
      } catch (error) {
        if (active) {
          setMonitoringError(
            error instanceof Error
              ? `Monitoring cards: ${error.message}`
              : "Monitoring cards are unavailable",
          );
        }
      } finally {
        inFlight = false;
        if (active) setMonitoringLoading(false);
      }
    };
    void loadCards();
    const refreshTimer = window.setInterval(() => void loadCards(), 60_000);
    return () => {
      active = false;
      window.clearInterval(refreshTimer);
    };
  }, [apiToken]);
  useEffect(() => {
    if (embedded) return;
    leavingForDetailRef.current = false;
    const content = document.querySelector<HTMLElement>(".content");
    const scrollTarget: HTMLElement | Window =
      window.innerWidth > 640 && content ? content : window;
    const restore = window.requestAnimationFrame(() => {
      scrollTarget.scrollTo({ top: savedScrollRef.current });
    });
    let scheduled = 0;
    const remember = () => {
      if (scheduled || leavingForDetailRef.current) return;
      scheduled = window.requestAnimationFrame(() => {
        scheduled = 0;
        setSavedScrollY(
          scrollTarget === window
            ? window.scrollY
            : (scrollTarget as HTMLElement).scrollTop,
        );
      });
    };
    scrollTarget.addEventListener("scroll", remember, { passive: true });
    return () => {
      window.cancelAnimationFrame(restore);
      if (scheduled) window.cancelAnimationFrame(scheduled);
      scrollTarget.removeEventListener("scroll", remember);
    };
  }, [embedded, setSavedScrollY]);
  const openVpsDetail = (agent: AgentView) => {
    if (!embedded) {
      const content = document.querySelector<HTMLElement>(".content");
      const scrollY =
        window.innerWidth > 640 && content ? content.scrollTop : window.scrollY;
      leavingForDetailRef.current = true;
      setSavedScrollY(scrollY);
    }
    onOpenVpsDetail(agent);
  };
  const agentIds = useMemo(
    () => new Set(agents.map((agent) => agent.id)),
    [agents],
  );
  const cardsByClient = useMemo(
    () =>
      new Map(
        monitoringCards
          .filter((card) => agentIds.has(card.client.id))
          .map((card) => [card.client.id, card]),
      ),
    [agentIds, monitoringCards],
  );
  const cardAgents = agents;
  const rollups = latestRollupsByClient([
    ...telemetryRollups,
    ...monitoringCards.flatMap((card) =>
      card.resources ? [card.resources] : [],
    ),
  ]);
  const rates = latestRatesByClient([
    ...telemetryNetworkRates,
    ...monitoringCards.flatMap((card) => card.network),
  ]);
  const rollupHistory = historyRollupsByClient([
    ...telemetryRollups,
    ...monitoringCards.flatMap((card) => card.resource_history),
  ]);
  const rateHistory = historyRatesByClient([
    ...telemetryNetworkRates,
    ...monitoringCards.flatMap((card) => card.network_history),
  ]);
  const trafficByClient = new Map(
    monitoringCards.map((card) => [card.client.id, card.traffic]),
  );
  const billingByClient = new Map(
    monitoringCards.flatMap((card) =>
      card.billing ? [[card.client.id, card.billing] as const] : [],
    ),
  );
  const portSpeedByClient = new Map(
    monitoringCards.flatMap((card) =>
      card.port_speed ? [[card.client.id, card.port_speed] as const] : [],
    ),
  );
  const primaryPingByClient = new Map(
    monitoringCards.flatMap((card) =>
      card.primary_ping ? [[card.client.id, card.primary_ping] as const] : [],
    ),
  );
  const cardSignals = buildCardSignals({
    backups,
    failedJobCount,
    fileTransfers,
    fleetAlerts,
    jobs,
    recordBounds,
    runningJobCount,
  });
  const tagOptions = useMemo(
    () => Array.from(new Set(cardAgents.flatMap((agent) => agent.tags))).sort(),
    [cardAgents],
  );
  const providerOptions = useMemo(
    () =>
      Array.from(new Set(cardAgents.map(providerSortValue)))
        .filter((provider) => provider !== "provider unset")
        .sort(),
    [cardAgents],
  );
  const effectiveTagFilter = tagOptions.includes(tagFilter) ? tagFilter : "all";
  const effectiveProviderFilter = providerOptions.includes(providerFilter)
    ? providerFilter
    : "all";
  const fleetCounts = useMemo(
    () =>
      monitorFleetCounts(
        cardAgents,
        cardSignals,
        rollups,
        rates,
        trafficByClient,
        primaryPingByClient,
      ),
    [
      cardAgents,
      cardSignals,
      primaryPingByClient,
      rates,
      rollups,
      trafficByClient,
    ],
  );
  const fleetSnapshot = monitorFleetSnapshot(
    cardAgents,
    rates,
    trafficByClient,
  );
  const filteredAgents = useMemo(() => {
    const query = searchQuery.trim().toLowerCase();
    return cardAgents.filter((agent) => {
      const category = monitorFleetCategory(
        agent,
        cardSignals,
        rollups,
        rates,
        trafficByClient,
        primaryPingByClient,
      );
      if (statusFilter !== "all" && category !== statusFilter) return false;
      if (
        effectiveTagFilter !== "all" &&
        !agent.tags.includes(effectiveTagFilter)
      )
        return false;
      if (
        effectiveProviderFilter !== "all" &&
        providerSortValue(agent) !== effectiveProviderFilter
      )
        return false;
      if (!query) return true;
      return [agent.id, agent.display_name, ...agent.tags]
        .join(" ")
        .toLowerCase()
        .includes(query);
    });
  }, [
    cardAgents,
    cardSignals,
    effectiveProviderFilter,
    effectiveTagFilter,
    primaryPingByClient,
    rates,
    rollups,
    searchQuery,
    statusFilter,
    trafficByClient,
  ]);
  const sortedAgents = useMemo(
    () =>
      [...filteredAgents].sort(
        compareMonitorAgents({
          mode: sortMode,
          rates,
          rollups,
          signals: cardSignals,
          traffic: trafficByClient,
          primaryPing: primaryPingByClient,
        }),
      ),
    [
      cardSignals,
      filteredAgents,
      primaryPingByClient,
      rates,
      rollups,
      sortMode,
      trafficByClient,
    ],
  );
  const visibleAgents =
    typeof maxCards === "number"
      ? sortedAgents.slice(0, maxCards)
      : sortedAgents.slice(0, Math.max(100, renderLimit));
  const hiddenCount =
    typeof maxCards === "number"
      ? Math.max(0, sortedAgents.length - visibleAgents.length)
      : 0;
  useEffect(() => {
    if (
      typeof maxCards === "number" ||
      visibleAgents.length >= sortedAgents.length
    )
      return;
    const node = loadMoreRef.current;
    if (!node) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setRenderLimit((current) =>
            Math.min(sortedAgents.length, Math.max(100, current) + 200),
          );
        }
      },
      { rootMargin: "800px 0px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [maxCards, setRenderLimit, sortedAgents.length, visibleAgents.length]);
  const rootClassName = embedded
    ? "fleetMonitorWorkspace embedded"
    : "workspace singleColumn fleetMonitorWorkspace";
  const displayControls = (
    <>
      <label>
        <span>Sort</span>
        <select
          aria-label={`${title} sort`}
          id={`${controlIdPrefix}-sort`}
          name={`${controlIdPrefix}-sort`}
          onChange={(event) =>
            setSortMode(event.target.value as FleetMonitorSort)
          }
          value={sortMode}
        >
          {monitorSortOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
      <div
        aria-label={`${title} density`}
        className="segmented vpsMonitorDensityControl"
        role="group"
      >
        {(["compact", "comfortable"] as const).map((option) => (
          <button
            aria-pressed={density === option}
            className={density === option ? "selected" : ""}
            key={option}
            onClick={() => setDensity(option)}
            type="button"
          >
            {option === "compact" ? "Compact" : "Comfortable"}
          </button>
        ))}
      </div>
    </>
  );
  const sharedViewsAction = onOpenSharedViews ? (
    <button
      className="secondaryAction compactAction"
      onClick={() =>
        onOpenSharedViews(fleetShareSelector(filteredAgents, cardAgents))
      }
      title="Manage persistent read-only views for the currently matched VPSs"
      type="button"
    >
      <Link2 size={15} />
      <span>Shared views</span>
    </button>
  ) : null;
  const locationSummary = formatLocationSummary(
    fleetSnapshot.locations,
    fleetSnapshot.unspecifiedLocations,
  );
  const rxSummary = `↓ ${formatRateOrUnavailable(fleetSnapshot.rxBps)}`;
  const txSummary = `↑ ${formatRateOrUnavailable(fleetSnapshot.txBps)} · ${fleetSnapshot.freshNetworkCount} fresh`;
  const trafficTotal =
    fleetSnapshot.trafficCount > 0
      ? formatBytes(fleetSnapshot.trafficBytes)
      : "n/a";
  const trafficSummary =
    fleetSnapshot.trafficCount > 0
      ? `${fleetSnapshot.trafficCount} configured VPS${fleetSnapshot.trafficCount === 1 ? "" : "s"}`
      : "No configured accounting";

  return (
    <section className={rootClassName}>
      {embedded ? (
        <div className="fleetMonitorToolbar">
          <div>
            <h2>{title}</h2>
            <span>{description}</span>
          </div>
          <div className="fleetMonitorToolbarRight">
            <div
              className="fleetMonitorControls"
              aria-label={`${title} controls`}
            >
              {displayControls}
            </div>
            {toolbarAction}
          </div>
        </div>
      ) : null}
      <div className="fleetMonitorSurface fleetPanel">
        <div
          className="fleetMonitorOverview"
          aria-label={`${title} fleet summary`}
        >
          <button
            aria-pressed={statusFilter === "all"}
            className={statusFilter === "all" ? "selected" : ""}
            onClick={() => setStatusFilter("all")}
            type="button"
          >
            <strong>{fleetCounts.total}</strong>
            <span>Total</span>
          </button>
          <button
            aria-pressed={statusFilter === "online"}
            className={statusFilter === "online" ? "selected online" : "online"}
            onClick={() => setStatusFilter("online")}
            type="button"
          >
            <strong>{fleetCounts.online}</strong>
            <span>Online</span>
          </button>
          <button
            aria-pressed={statusFilter === "warning"}
            className={
              statusFilter === "warning" ? "selected warning" : "warning"
            }
            onClick={() => setStatusFilter("warning")}
            type="button"
          >
            <strong>{fleetCounts.warning}</strong>
            <span>Warning</span>
          </button>
          <button
            aria-pressed={statusFilter === "offline"}
            className={
              statusFilter === "offline" ? "selected offline" : "offline"
            }
            onClick={() => setStatusFilter("offline")}
            type="button"
          >
            <strong>{fleetCounts.offline}</strong>
            <span>Offline</span>
          </button>
        </div>
        <div
          className="fleetMonitorSnapshot"
          aria-label={`${title} current totals`}
        >
          <span>
            <small>Locations</small>
            <strong title={`${fleetSnapshot.locations.length} locations`}>
              {fleetSnapshot.locations.length}
            </strong>
            <em title={locationSummary}>{locationSummary}</em>
          </span>
          <span>
            <small>Realtime bandwidth</small>
            <strong title={rxSummary}>{rxSummary}</strong>
            <em title={txSummary}>{txSummary}</em>
          </span>
          <span>
            <small>Current-cycle traffic</small>
            <strong title={trafficTotal}>{trafficTotal}</strong>
            <em title={trafficSummary}>{trafficSummary}</em>
          </span>
        </div>
        <div className="fleetMonitorFilters" aria-label={`${title} filters`}>
          <label className="fleetMonitorSearch">
            <Search aria-hidden="true" size={15} />
            <span className="srOnly">Search VPS cards</span>
            <input
              aria-label="Search VPS cards"
              id={`${controlIdPrefix}-search`}
              name={`${controlIdPrefix}-search`}
              onChange={(event) => setSearchQuery(event.target.value)}
              placeholder="Search name, ID, or tag"
              type="search"
              value={searchQuery}
            />
          </label>
          <label>
            <span>Status</span>
            <select
              aria-label="Filter VPS cards by status"
              id={`${controlIdPrefix}-status`}
              name={`${controlIdPrefix}-status`}
              onChange={(event) =>
                setStatusFilter(event.target.value as FleetMonitorStatusFilter)
              }
              value={statusFilter}
            >
              <option value="all">All statuses</option>
              <option value="online">Online</option>
              <option value="warning">Warning</option>
              <option value="offline">Offline</option>
            </select>
          </label>
          <label>
            <span>Tag</span>
            <select
              aria-label="Filter VPS cards by tag"
              id={`${controlIdPrefix}-tag`}
              name={`${controlIdPrefix}-tag`}
              onChange={(event) => setTagFilter(event.target.value)}
              value={effectiveTagFilter}
            >
              <option value="all">All tags</option>
              {tagOptions.map((tag) => (
                <option key={tag} value={tag}>
                  {tag}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Provider</span>
            <select
              aria-label="Filter VPS cards by provider"
              id={`${controlIdPrefix}-provider`}
              name={`${controlIdPrefix}-provider`}
              onChange={(event) => setProviderFilter(event.target.value)}
              value={effectiveProviderFilter}
            >
              <option value="all">All providers</option>
              {providerOptions.map((provider) => (
                <option key={provider} value={provider}>
                  {provider}
                </option>
              ))}
            </select>
          </label>
          {!embedded ? displayControls : null}
          {!embedded ? sharedViewsAction : null}
          {!embedded ? toolbarAction : null}
          <span className="fleetMonitorMatchCount">
            {monitoringLoading
              ? "Loading monitoring evidence…"
              : `${filteredAgents.length} matched`}
            {hiddenCount > 0 ? ` · ${hiddenCount} beyond this preview` : ""}
          </span>
        </div>
        <ActionFeedback
          className="localActionFeedback"
          message={
            [apiError, monitoringError].filter(Boolean).join(" · ") || null
          }
          tone="danger"
        />

        {sortedAgents.length === 0 ? (
          <div className="emptyState">
            <Server size={22} />
            <strong>No VPS cards to show</strong>
            <span>
              Adjust the fleet scope or wait for agents to report telemetry.
            </span>
          </div>
        ) : (
          <div
            className={`vpsMonitorGrid ${density}`}
            aria-label={ariaLabel}
            data-density={density}
            data-sort={sortMode}
          >
            {visibleAgents.map((agent) => {
              const monitoringCard = cardsByClient.get(agent.id);
              const monitoringState: MonitoringEvidenceState = monitoringLoading
                ? "loading"
                : monitoringCard
                  ? "ready"
                  : "unavailable";
              return (
                <VpsMonitorCard
                  agent={agent}
                  billing={billingByClient.get(agent.id) ?? null}
                  density={density}
                  key={agent.id}
                  monitoringState={monitoringState}
                  onOpenVpsDetail={openVpsDetail}
                  primaryPing={monitoringCard?.primary_ping ?? null}
                  primaryPingHistory={
                    monitoringCard?.primary_ping_history ?? []
                  }
                  rateHistory={rateHistory.get(agent.id) ?? []}
                  rates={rates.get(agent.id) ?? []}
                  portSpeed={portSpeedByClient.get(agent.id) ?? null}
                  rollupHistory={rollupHistory.get(agent.id) ?? []}
                  rollup={rollups.get(agent.id) ?? null}
                  signals={
                    cardSignals.records.get(agent.id) ??
                    defaultCardSignal(cardSignals.global)
                  }
                  statusCategory={monitorFleetCategory(
                    agent,
                    cardSignals,
                    rollups,
                    rates,
                    trafficByClient,
                    primaryPingByClient,
                  )}
                  traffic={monitoringCard?.traffic ?? null}
                />
              );
            })}
          </div>
        )}
        {typeof maxCards !== "number" &&
        visibleAgents.length < sortedAgents.length ? (
          <div className="fleetMonitorProgress" ref={loadMoreRef} role="status">
            Showing {visibleAgents.length} of {sortedAgents.length} matched
            VPSs…
          </div>
        ) : null}
      </div>
    </section>
  );
}

export type VpsMonitorCardProps = {
  agent: AgentView;
  billing: BillingPlanView | null;
  density: FleetMonitorDensity;
  monitoringState: MonitoringEvidenceState;
  onOpenVpsDetail: (agent: AgentView) => void;
  primaryPing: CurrentPingView | null;
  primaryPingHistory: PingRollupView[];
  portSpeed: PortSpeedView | null;
  rateHistory: TelemetryNetworkRateRecord[];
  rates: TelemetryNetworkRateRecord[];
  rollupHistory: TelemetryRollupRecord[];
  rollup: TelemetryRollupRecord | null;
  signals: VpsMonitorCardSignal;
  statusCategory: Exclude<FleetMonitorStatusFilter, "all">;
  traffic: TrafficAccountingRecord | null;
};

export function VpsMonitorCardSurface({
  ariaLabel,
  children,
  className,
  header,
  onOpen,
  title,
}: {
  ariaLabel: string;
  children: ReactNode;
  className: string;
  header: ReactNode;
  onOpen?: () => void;
  title?: string;
}) {
  const interactive = Boolean(onOpen);
  return (
    <article
      aria-label={ariaLabel}
      className={className}
      onClick={onOpen}
      onKeyDown={
        interactive
          ? (event) => {
              if (event.key !== "Enter") return;
              event.preventDefault();
              onOpen?.();
            }
          : undefined
      }
      role={interactive ? "link" : undefined}
      tabIndex={interactive ? 0 : undefined}
      title={title}
    >
      <div className="vpsMonitorCardMain">{header}</div>
      {children}
    </article>
  );
}

export function VpsMonitorCard({
  agent,
  billing,
  density,
  monitoringState,
  onOpenVpsDetail,
  primaryPing,
  primaryPingHistory,
  portSpeed,
  rateHistory,
  rates,
  rollupHistory,
  rollup,
  signals,
  statusCategory,
  traffic,
}: VpsMonitorCardProps) {
  const displayState = agentDisplayState(agent);
  const provider = tagValue(agent.tags, "provider") ?? "provider unset";
  const region =
    tagValue(agent.tags, "country") ??
    tagValue(agent.tags, "region") ??
    "region unset";
  const currentRates = coherentNetworkRates(rates);
  const rxBps = sumNetworkRate(currentRates, "rx");
  const txBps = sumNetworkRate(currentRates, "tx");
  const load = finiteMetric(rollup?.cpu_load_1_avg);
  const load5 = finiteMetric(rollup?.cpu_load_5_avg);
  const load15 = finiteMetric(rollup?.cpu_load_15_avg);
  const loadHistory = recentMetricValues(rollupHistory, (row) =>
    finiteMetric(row.cpu_load_1_avg),
  );
  const cpuUsageRatio = finiteRatio(rollup?.cpu_usage_avg);
  const cpuUsed = cpuUsageRatio === null ? null : cpuUsageRatio * 100;
  const cpuCores = finiteMetric(rollup?.cpu_cores_max);
  const memoryUsedBytes = rollup
    ? usedCapacity(
        rollup.memory_total_bytes_max,
        rollup.memory_available_bytes_avg,
      )
    : null;
  const diskUsedBytes = rollup
    ? usedCapacity(rollup.disk_total_bytes_max, rollup.disk_available_bytes_avg)
    : null;
  const memoryUsed = rollup
    ? percentValue(memoryUsedBytes ?? Number.NaN, rollup.memory_total_bytes_max)
    : null;
  const diskUsed = rollup
    ? percentValue(diskUsedBytes ?? Number.NaN, rollup.disk_total_bytes_max)
    : null;
  const loadPressure =
    load !== null && cpuCores !== null && cpuCores > 0 ? load / cpuCores : null;
  const resourceFreshness = rollup?.latest_observed_at ?? null;
  const networkFreshness = latestTimestamp(
    currentRates.map((rate) => rate.bucket_start),
  );
  const pingFreshness = primaryPing?.checked_at ?? null;
  const pingHistory = recentPingValues(primaryPingHistory);
  const resourceTelemetryState = monitorTelemetryState(
    displayState,
    resourceFreshness,
  );
  const networkTelemetryState = monitorTelemetryState(
    displayState,
    networkFreshness,
  );
  const pingTelemetryState = monitorTelemetryState(displayState, pingFreshness);
  const telemetryState = monitorTelemetrySummary(
    resourceTelemetryState,
    networkTelemetryState,
    latestTimestamp([resourceFreshness, networkFreshness]),
  );
  const lastContact = agent.last_seen_at ?? agent.stale_since ?? null;
  const rxHistory = networkDirectionHistory(rateHistory, "rx");
  const txHistory = networkDirectionHistory(rateHistory, "tx");
  const trafficConfigured = Boolean(
    traffic && traffic.selectors.length > 0 && traffic.reset_day !== null,
  );
  const trafficPercent =
    trafficConfigured && traffic?.cycle_percent !== null
      ? finiteMetric(traffic?.cycle_percent)
      : null;
  const trafficWarning = trafficWarningRank(traffic);
  const trafficProblem =
    trafficWarning > 0 && traffic
      ? trafficPercent !== null && trafficPercent > 100
        ? `Quota exceeded at ${trafficPercent.toFixed(trafficPercent >= 10 ? 0 : 1)}%`
        : (traffic.incomplete_reasons[0] ?? `Traffic evidence ${traffic.state}`)
      : null;
  const pingWarning = pingWarningRank(primaryPing);
  const connectionsTelemetryState = monitorTelemetryState(
    displayState,
    rollup?.connections_observed_at ?? null,
  );
  const pingProblem =
    pingWarning > 0 && primaryPing
      ? (primaryPing.reason ??
        (!primaryPing.enabled || primaryPing.state === "disabled"
          ? "Primary Ping target disabled"
          : primaryPing.state !== "ok"
            ? `Primary Ping ${primaryPing.state}`
            : pingTelemetryState.label))
      : null;
  const noteworthyEvidence =
    telemetryState.kind !== "fresh" ||
    signals.alertTone === "critical" ||
    signals.alertTone === "warning" ||
    signals.backupTone === "critical" ||
    signals.transferTone === "critical" ||
    Boolean(agent.stale_reason);
  const effectiveStatusLabel =
    statusCategory === "warning" && displayState.label === "Online"
      ? "Online · Warning"
      : displayState.label;
  const effectiveStatusTitle =
    statusCategory === "warning" && displayState.label === "Online"
      ? [
          `${displayState.detail}.`,
          "Visible monitoring needs attention.",
          telemetryState.kind !== "fresh" ? telemetryState.title : null,
          trafficProblem,
          pingProblem,
          agent.stale_reason,
          signals.alertTone === "critical" || signals.alertTone === "warning"
            ? signals.alertText
            : null,
        ]
          .filter(Boolean)
          .join(" ")
      : displayState.detail;
  const trafficEvidenceLabel =
    monitoringState === "loading"
      ? "Loading…"
      : monitoringState === "unavailable"
        ? "Unavailable"
        : trafficConfigured && traffic
          ? formatTrafficUsage(traffic)
          : "Unconfigured";
  const pingEvidenceLabel =
    monitoringState === "loading"
      ? "Loading…"
      : monitoringState === "unavailable"
        ? "Unavailable"
        : formatPrimaryPing(primaryPing);
  const trafficHeading = `Traffic${portSpeed ? ` · ${portSpeed.display}` : ""}`;
  const trafficDetail =
    trafficConfigured && traffic
      ? (trafficProblem ?? formatTrafficReset(traffic.cycle_end))
      : null;
  const pingHeading = `Ping${primaryPing ? ` · ${primaryPing.target_name}` : ""}`;

  const cardHeader = (
    <>
      <span className="vpsMonitorStatus" title={effectiveStatusTitle}>
        <span aria-hidden="true" />
        {density === "compact" && effectiveStatusLabel === "Contact unknown"
          ? "No contact"
          : effectiveStatusLabel}
      </span>
      <strong title={displayNameOrUnnamed(agent.display_name)}>
        {displayNameOrUnnamed(agent.display_name)}
      </strong>
      <small title={`${provider} / ${region}`}>
        {provider} / {region}
      </small>
    </>
  );

  return (
    <VpsMonitorCardSurface
      ariaLabel={`${displayNameOrUnnamed(agent.display_name)} ${effectiveStatusLabel} monitor card`}
      className={`vpsMonitorCard ${statusCategory} ${density}`}
      header={cardHeader}
      onOpen={() => onOpenVpsDetail(agent)}
      title={`Open ${displayNameOrUnnamed(agent.display_name)} detail`}
    >
      <div
        aria-label={`Current resource use for ${displayNameOrUnnamed(agent.display_name)}`}
        className="vpsMonitorMetrics"
      >
        <MonitorMetric
          icon={<Activity size={15} />}
          label="CPU"
          meterCaption={
            cpuCores && cpuCores > 0
              ? `${Math.round(cpuCores)} core${cpuCores === 1 ? "" : "s"}`
              : "reported use"
          }
          meterMax={100}
          meterValue={cpuUsed}
          stale={resourceTelemetryState.kind === "stale"}
          title="CPU time used during the latest reporting interval; unavailable until the agent has two valid CPU counter samples"
          value={formatPercent(cpuUsed)}
        />
        <MonitorMetric
          icon={<Gauge size={15} />}
          label={density === "compact" ? "RAM" : "Memory"}
          meterCaption={formatCapacity(
            memoryUsedBytes,
            rollup?.memory_total_bytes_max,
          )}
          meterMax={100}
          meterValue={memoryUsed}
          stale={resourceTelemetryState.kind === "stale"}
          title="Used memory as a percentage of reported total memory"
          value={formatPercent(memoryUsed)}
        />
        <MonitorMetric
          icon={<Server size={15} />}
          label="Disk"
          meterCaption={formatCapacity(
            diskUsedBytes,
            rollup?.disk_total_bytes_max,
          )}
          meterMax={100}
          meterValue={diskUsed}
          stale={resourceTelemetryState.kind === "stale"}
          title="Used disk space as a percentage of reported total disk space"
          value={formatPercent(diskUsed)}
        />
        <MonitorMetric
          icon={<Gauge size={15} />}
          label={density === "compact" ? "Load" : "1m load"}
          meterCaption={`5m ${formatLoad(load5)} · 15m ${formatLoad(load15)}${cpuCores && cpuCores > 0 ? ` · ${Math.round(cpuCores)} cores` : ""}`}
          meterMax={1}
          meterValue={loadPressure}
          sparkline={
            density === "comfortable" ? (
              <MiniSparkline
                label="Load history"
                tone="load"
                values={loadHistory}
              />
            ) : undefined
          }
          stale={resourceTelemetryState.kind === "stale"}
          title="Linux load average, not CPU utilization. The bar shows 1-minute load divided by reported CPU cores; full means one runnable task per core"
          value={formatLoad(load)}
          showCaption={density === "comfortable"}
        />
      </div>
      <div
        aria-label={`Current network activity for ${displayNameOrUnnamed(agent.display_name)}`}
        className={`vpsMonitorFlowFacts ${density}`}
      >
        <MonitorFact
          label={density === "compact" ? "RX" : "Network RX"}
          sparkline={
            <MiniSparkline label="RX activity" tone="rx" values={rxHistory} />
          }
          stale={networkTelemetryState.kind === "stale"}
          title={networkMetricTitle(
            "received",
            currentRates.length,
            networkTelemetryState,
          )}
          value={formatRateOrUnavailable(rxBps)}
        />
        <MonitorFact
          label={density === "compact" ? "TX" : "Network TX"}
          sparkline={
            <MiniSparkline label="TX activity" tone="tx" values={txHistory} />
          }
          stale={networkTelemetryState.kind === "stale"}
          title={networkMetricTitle(
            "sent",
            currentRates.length,
            networkTelemetryState,
          )}
          value={formatRateOrUnavailable(txBps)}
        />
      </div>
      <div
        className="vpsMonitorAuxFacts"
        aria-label={`Connection and billing facts for ${displayNameOrUnnamed(agent.display_name)}`}
      >
        <span
          title={billing ? billingTitle(billing) : "Billing is not configured"}
        >
          <small>Billing</small>
          <strong>{billing?.display ?? "—"}</strong>
          {billing?.cycle ? <em>Renews {billing.cycle}</em> : null}
        </span>
        <span title={connectionCountTitle("TCP", connectionsTelemetryState)}>
          <small>TCP</small>
          <strong>{formatSocketCount(rollup?.tcp_sockets_latest)}</strong>
        </span>
        <span title={connectionCountTitle("UDP", connectionsTelemetryState)}>
          <small>UDP</small>
          <strong>{formatSocketCount(rollup?.udp_sockets_latest)}</strong>
        </span>
      </div>
      <div
        className={`vpsMonitorTraffic${trafficWarning > 0 ? " warning" : ""}${trafficPercent !== null && trafficPercent > 100 ? " exceeded" : ""}`}
      >
        <span>
          <small
            title={
              portSpeed
                ? `${trafficHeading}; display value only—no shaping or enforcement is implied`
                : trafficHeading
            }
          >
            {trafficHeading}
          </small>
          <strong title={trafficEvidenceLabel}>{trafficEvidenceLabel}</strong>
        </span>
        <span
          aria-label={
            trafficPercent === null
              ? undefined
              : `Traffic quota ${trafficPercent.toFixed(1)} percent`
          }
          aria-valuemax={trafficPercent === null ? undefined : 100}
          aria-valuemin={trafficPercent === null ? undefined : 0}
          aria-valuenow={
            trafficPercent === null
              ? undefined
              : Math.min(100, Math.max(0, trafficPercent))
          }
          aria-valuetext={
            trafficPercent === null
              ? undefined
              : `${trafficPercent.toFixed(1)} percent`
          }
          className={`vpsMonitorTrafficTrack${trafficPercent === null ? " missing" : ""}`}
          role={trafficPercent === null ? undefined : "meter"}
          title={
            monitoringState === "loading"
              ? "Reading traffic configuration and current accounting evidence"
              : monitoringState === "unavailable"
                ? "Monitoring card evidence is unavailable; traffic configuration is not inferred"
                : trafficConfigured && traffic
                  ? formatTrafficTitle(traffic)
                  : "Traffic accounting rules and reset cycle are not configured"
          }
        >
          <span style={{ width: `${Math.min(100, trafficPercent ?? 0)}%` }} />
        </span>
        {trafficConfigured && traffic ? (
          <small
            className={trafficWarning > 0 ? "exceptionEvidence" : undefined}
            title={`Current billing-cycle traffic: RX ${formatBytes(traffic.rx_bytes)}; TX ${formatBytes(traffic.tx_bytes)}.${trafficDetail ? ` ${trafficDetail}.` : ""}`}
          >
            ↓ {formatBytes(traffic.rx_bytes)} · ↑{" "}
            {formatBytes(traffic.tx_bytes)}
            {density === "comfortable" || trafficWarning > 0
              ? ` · ${trafficDetail}`
              : ""}
          </small>
        ) : null}
      </div>
      <div
        className={`vpsMonitorPing ${pingWarning >= 2 ? "failed" : pingWarning > 0 ? "stale" : (primaryPing?.state ?? "unconfigured")}`}
      >
        <span>
          <small title={pingHeading}>{pingHeading}</small>
          <strong title={pingEvidenceLabel}>{pingEvidenceLabel}</strong>
        </span>
        <span className="vpsMonitorPingVisual" aria-hidden="true">
          <MiniSparkline
            label="Primary Ping history"
            tone="ping"
            values={pingHistory}
          />
        </span>
        {(density === "comfortable" || pingWarning > 0) && pingProblem ? (
          <small
            className={pingWarning > 0 ? "exceptionEvidence" : undefined}
            title={pingProblem}
          >
            {pingProblem}
          </small>
        ) : null}
      </div>
      {density === "comfortable" ? (
        <>
          {noteworthyEvidence ? (
            <div className="vpsMonitorEvidence comfortableSummary">
              <span>
                {formatMonitorContactEvidence(agent, displayState, lastContact)}
              </span>
              <span
                className={`telemetryEvidence ${telemetryState.kind}`}
                title={telemetryState.title}
              >
                {telemetryState.label}
              </span>
              <span>{agent.stale_reason ?? signals.statusText}</span>
            </div>
          ) : null}
          {signals.alertTone !== "neutral" ||
          signals.backupTone !== "neutral" ||
          signals.transferTone !== "neutral" ? (
            <div
              className="vpsMonitorSignals"
              aria-label={`Operational signals for ${displayNameOrUnnamed(agent.display_name)}`}
            >
              <MonitorSignal
                tone={signals.alertTone}
                label="Alerts"
                value={signals.alertText}
              />
              {signals.backupTone !== "neutral" ? (
                <MonitorSignal
                  tone={signals.backupTone}
                  label="Backup"
                  value={signals.backupText}
                />
              ) : null}
              {signals.transferTone !== "neutral" ? (
                <MonitorSignal
                  tone={signals.transferTone}
                  label="Transfer"
                  value={signals.transferText}
                />
              ) : null}
            </div>
          ) : null}
        </>
      ) : null}
    </VpsMonitorCardSurface>
  );
}

export function MonitorMetric({
  icon,
  label,
  meterCaption,
  meterMax,
  meterValue,
  sparkline,
  showCaption = false,
  stale = false,
  title,
  value,
}: {
  icon: ReactNode;
  label: string;
  meterCaption: string;
  meterMax: number;
  meterValue: number | null;
  sparkline?: ReactNode;
  showCaption?: boolean;
  stale?: boolean;
  title?: string;
  value: string;
}) {
  const metricTitle = [
    title,
    meterCaption,
    stale ? "Last-known value; current telemetry is stale" : null,
  ]
    .filter(Boolean)
    .join(". ");
  const boundedValue =
    meterValue === null || !Number.isFinite(meterValue)
      ? null
      : Math.max(0, Math.min(meterMax, meterValue));
  const fillPercent =
    boundedValue === null || meterMax <= 0
      ? 0
      : (boundedValue / meterMax) * 100;
  return (
    <span
      className={`vpsMonitorMetric${stale ? " stale" : ""}`}
      title={metricTitle || undefined}
    >
      <span aria-hidden="true" className="vpsMonitorMetricIcon">
        {icon}
      </span>
      <span className="vpsMonitorMetricLabel">{label}</span>
      <strong>{value}</strong>
      <span
        aria-hidden={boundedValue === null ? true : undefined}
        aria-label={
          boundedValue === null
            ? undefined
            : `${label}: ${value}; ${meterCaption}`
        }
        aria-valuemax={boundedValue === null ? undefined : meterMax}
        aria-valuemin={boundedValue === null ? undefined : 0}
        aria-valuenow={boundedValue ?? undefined}
        aria-valuetext={
          boundedValue === null ? undefined : `${value}; ${meterCaption}`
        }
        className={`vpsMonitorMetricTrack${boundedValue === null ? " missing" : ""}`}
        role={boundedValue === null ? undefined : "meter"}
      >
        <span style={{ width: `${fillPercent}%` }} />
      </span>
      {sparkline}
      {showCaption ? (
        <small>{boundedValue === null ? "unavailable" : meterCaption}</small>
      ) : null}
    </span>
  );
}

export function MonitorFact({
  icon,
  label,
  sparkline,
  stale = false,
  title,
  value,
}: {
  icon?: ReactNode;
  label: string;
  sparkline?: ReactNode;
  stale?: boolean;
  title: string;
  value: string;
}) {
  return (
    <span
      className={`vpsMonitorFlowFact${stale ? " stale" : ""}`}
      title={title}
    >
      <small title={label}>
        {icon}
        {icon ? " " : ""}
        {label}
      </small>
      <strong title={title}>{value}</strong>
      {sparkline}
    </span>
  );
}

export function MiniSparkline({
  label,
  tone,
  values,
}: {
  label: string;
  tone: "load" | "ping" | "rx" | "tx";
  values: Array<number | null>;
}) {
  const width = 96;
  const height = 18;
  const finite = values.filter(
    (value): value is number => value !== null && Number.isFinite(value),
  );
  if (finite.length < 2) {
    return <span className="vpsMonitorSparkline empty">No recent history</span>;
  }
  const maximum = Math.max(...finite, 1);
  const denominator = Math.max(1, values.length - 1);
  const segments: string[] = [];
  let current: string[] = [];
  values.forEach((value, index) => {
    if (value === null || !Number.isFinite(value)) {
      if (current.length > 1) segments.push(current.join(" "));
      current = [];
      return;
    }
    const x = (index / denominator) * width;
    const y = height - (Math.max(0, value) / maximum) * (height - 2) - 1;
    current.push(`${x.toFixed(1)},${y.toFixed(1)}`);
  });
  if (current.length > 1) segments.push(current.join(" "));
  if (segments.length === 0) {
    return (
      <span className="vpsMonitorSparkline empty">No continuous history</span>
    );
  }
  return (
    <svg
      aria-label={label}
      className={`vpsMonitorSparkline ${tone}`}
      preserveAspectRatio="none"
      role="img"
      viewBox={`0 0 ${width} ${height}`}
    >
      {segments.map((points, index) => (
        <polyline
          fill="none"
          key={`${points}-${index}`}
          points={points}
          vectorEffect="non-scaling-stroke"
        />
      ))}
    </svg>
  );
}

type MonitorTelemetryState = {
  kind: "fresh" | "missing" | "partial" | "stale";
  label: string;
  title: string;
};

function monitorTelemetryState(
  displayState: AgentDisplayState,
  latestAt: string | null,
): MonitorTelemetryState {
  if (!latestAt) {
    return {
      kind: "missing",
      label: "Telemetry unavailable",
      title: "This VPS has not reported retained resource or network telemetry",
    };
  }
  const latestMs = timestampMillis(latestAt);
  if (!Number.isFinite(latestMs)) {
    return {
      kind: "stale",
      label: "Telemetry time invalid",
      title:
        "The latest telemetry timestamp is invalid and cannot be treated as current",
    };
  }
  const ageMs = Math.max(0, Date.now() - latestMs);
  const stale = displayState.label !== "Online" || ageMs > 3 * 60_000;
  return {
    kind: stale ? "stale" : "fresh",
    label: `Telemetry ${stale ? "stale" : "current"} · ${formatTime(latestAt)}`,
    title: stale
      ? "Last-known telemetry is retained for diagnosis and is not current state"
      : "Latest telemetry is within the current-state freshness window",
  };
}

function monitorTelemetrySummary(
  resource: MonitorTelemetryState,
  network: MonitorTelemetryState,
  latestAt: string | null,
): MonitorTelemetryState {
  if (resource.kind === "missing" && network.kind === "missing") {
    return {
      kind: "missing",
      label: "Telemetry unavailable",
      title: "Resource and network telemetry have not been reported",
    };
  }
  const latestLabel = latestAt ? ` · ${formatTime(latestAt)}` : "";
  if (resource.kind === "stale" || network.kind === "stale") {
    return {
      kind: "stale",
      label: `Telemetry stale${latestLabel}`,
      title: `Resource: ${resource.title}. Network: ${network.title}`,
    };
  }
  if (resource.kind === "missing" || network.kind === "missing") {
    return {
      kind: "partial",
      label: `Telemetry partial${latestLabel}`,
      title: `Resource: ${resource.title}. Network: ${network.title}`,
    };
  }
  return {
    kind: "fresh",
    label: `Telemetry current${latestLabel}`,
    title:
      "Latest resource and network telemetry are within the current-state freshness window",
  };
}

function MonitorSignal({
  label,
  tone,
  value,
}: {
  label: string;
  tone: "critical" | "warning" | "info" | "ok" | "neutral";
  value: string;
}) {
  return (
    <span className={`vpsMonitorSignal ${tone}`} title={`${label}: ${value}`}>
      <span title={label}>{label}</span>
      <strong title={value}>{value}</strong>
    </span>
  );
}

export type VpsMonitorCardSignal = {
  alertText: string;
  alertTone: "critical" | "warning" | "info" | "ok" | "neutral";
  backupText: string;
  backupTone: "critical" | "warning" | "info" | "ok" | "neutral";
  fleetJobText: string;
  statusText: string;
  transferText: string;
  transferTone: "critical" | "warning" | "info" | "ok" | "neutral";
};

type CardSignalContext = {
  global: {
    failedJobs: number;
    recordBounds: MonitorRecordBounds;
    runningJobs: number;
  };
  records: Map<string, VpsMonitorCardSignal>;
};

function buildCardSignals({
  backups,
  failedJobCount,
  fileTransfers,
  fleetAlerts,
  jobs,
  recordBounds,
  runningJobCount,
}: {
  backups: BackupRequestRecord[];
  failedJobCount?: number;
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  recordBounds: MonitorRecordBounds;
  runningJobCount?: number;
}): CardSignalContext {
  const runningJobs =
    runningJobCount ??
    jobs.filter((job) => isActiveJobStatus(job.status)).length;
  const failedJobs =
    failedJobCount ??
    jobs.filter((job) => isFailedJobStatus(job.status)).length;
  const clientIds = new Set<string>([
    ...backups.map((record) => record.client_id),
    ...fileTransfers.map((record) => record.client_id),
    ...fleetAlerts.flatMap((record) =>
      record.client_id ? [record.client_id] : [],
    ),
  ]);
  const records = new Map<string, VpsMonitorCardSignal>();
  for (const clientId of clientIds) {
    records.set(
      clientId,
      buildClientSignal({
        alerts: fleetAlerts.filter(
          (alert) =>
            alert.client_id === clientId &&
            isActionableFleetAlertState(alert.operator_state),
        ),
        backups: backups.filter((backup) => backup.client_id === clientId),
        failedJobs,
        recordBounds,
        runningJobs,
        transfers: fileTransfers.filter(
          (transfer) => transfer.client_id === clientId,
        ),
      }),
    );
  }
  return { global: { failedJobs, recordBounds, runningJobs }, records };
}

function defaultCardSignal(
  global: CardSignalContext["global"],
): VpsMonitorCardSignal {
  return buildClientSignal({
    alerts: [],
    backups: [],
    failedJobs: global.failedJobs,
    recordBounds: global.recordBounds,
    runningJobs: global.runningJobs,
    transfers: [],
  });
}

function buildClientSignal({
  alerts,
  backups,
  failedJobs,
  recordBounds,
  runningJobs,
  transfers,
}: {
  alerts: FleetAlertRecord[];
  backups: BackupRequestRecord[];
  failedJobs: number;
  recordBounds: MonitorRecordBounds;
  runningJobs: number;
  transfers: FileTransferSessionRecord[];
}): VpsMonitorCardSignal {
  const criticalAlerts = alerts.filter(
    (alert) => alert.severity === "critical",
  ).length;
  const warningAlerts = alerts.filter(
    (alert) => alert.severity === "warning",
  ).length;
  const infoAlerts = alerts.length - criticalAlerts - warningAlerts;
  const failedBackups = backups.filter((backup) =>
    isFailedBackupStatus(backup.status),
  ).length;
  const failedTransfers = transfers.filter((transfer) =>
    isFailedTransferStatus(transfer.status),
  ).length;
  const activeTransfers = transfers.filter((transfer) =>
    isActiveTransferStatus(transfer.status),
  ).length;
  const recordPageCapped =
    recordBounds.fleetAlerts ||
    recordBounds.backups ||
    recordBounds.fileTransfers;
  const knownIssue =
    criticalAlerts > 0 ||
    warningAlerts > 0 ||
    infoAlerts > 0 ||
    failedBackups > 0 ||
    failedTransfers > 0;
  const fleetJobText =
    failedJobs > 0
      ? `Fleet-wide jobs: ${failedJobs} failed`
      : runningJobs > 0
        ? `Fleet-wide jobs: ${runningJobs} running`
        : "Fleet-wide jobs: idle";
  return {
    alertText:
      criticalAlerts > 0
        ? `${formatLowerBoundCount(criticalAlerts, recordBounds.fleetAlerts)} critical`
        : warningAlerts > 0
          ? `${formatLowerBoundCount(warningAlerts, recordBounds.fleetAlerts)} warning`
          : infoAlerts > 0
            ? `${formatLowerBoundCount(infoAlerts, recordBounds.fleetAlerts)} info`
            : recordBounds.fleetAlerts
              ? "None in loaded page"
              : "Clear",
    alertTone:
      criticalAlerts > 0
        ? "critical"
        : warningAlerts > 0
          ? "warning"
          : infoAlerts > 0 || recordBounds.fleetAlerts
            ? "info"
            : "neutral",
    backupText:
      failedBackups > 0
        ? `${formatLowerBoundCount(failedBackups, recordBounds.backups)} failed`
        : backups.length > 0
          ? `${formatLowerBoundCount(backups.length, recordBounds.backups)} recorded`
          : recordBounds.backups
            ? "None in loaded page"
            : "No run",
    backupTone:
      failedBackups > 0
        ? "critical"
        : recordBounds.backups
          ? "info"
          : "neutral",
    fleetJobText,
    statusText: knownIssue
      ? `${criticalAlerts} critical / ${warningAlerts} warning / ${infoAlerts} info alerts; ${failedBackups} backup failures; ${failedTransfers} transfer failures${recordPageCapped ? "; counts use capped loaded pages" : ""}`
      : recordPageCapped
        ? "No card-local warnings in loaded pages; older records may not be shown"
        : "No card-local alert, backup, or transfer warnings",
    transferText:
      failedTransfers > 0
        ? `${formatLowerBoundCount(failedTransfers, recordBounds.fileTransfers)} failed`
        : activeTransfers > 0
          ? `${formatLowerBoundCount(activeTransfers, recordBounds.fileTransfers)} active`
          : recordBounds.fileTransfers
            ? "No issue loaded"
            : "Clear",
    transferTone:
      failedTransfers > 0
        ? "critical"
        : activeTransfers > 0 || recordBounds.fileTransfers
          ? "info"
          : "neutral",
  };
}

function latestRollupsByClient(records: TelemetryRollupRecord[]) {
  const latest = new Map<string, TelemetryRollupRecord>();
  for (const record of records) {
    const current = latest.get(record.client_id);
    if (
      !current ||
      timestampMillis(record.latest_observed_at) >
        timestampMillis(current.latest_observed_at)
    ) {
      latest.set(record.client_id, record);
    }
  }
  return latest;
}

function latestRatesByClient(records: TelemetryNetworkRateRecord[]) {
  const latest = new Map<string, Map<string, TelemetryNetworkRateRecord>>();
  for (const record of records) {
    const byInterface =
      latest.get(record.client_id) ??
      new Map<string, TelemetryNetworkRateRecord>();
    const current = byInterface.get(record.interface);
    if (
      !current ||
      timestampMillis(record.bucket_start) >
        timestampMillis(current.bucket_start)
    ) {
      byInterface.set(record.interface, record);
    }
    latest.set(record.client_id, byInterface);
  }
  return new Map(
    Array.from(latest.entries()).map(([clientId, byInterface]) => [
      clientId,
      Array.from(byInterface.values()),
    ]),
  );
}

function historyRollupsByClient(records: TelemetryRollupRecord[]) {
  const indexed = new Map<string, Map<string, TelemetryRollupRecord>>();
  for (const record of records) {
    const byBucket = indexed.get(record.client_id) ?? new Map();
    const key = `${record.bucket_start}\0${record.bucket_secs}`;
    const current = byBucket.get(key);
    if (
      !current ||
      timestampMillis(record.latest_observed_at) >=
        timestampMillis(current.latest_observed_at)
    ) {
      byBucket.set(key, record);
    }
    indexed.set(record.client_id, byBucket);
  }
  const grouped = new Map(
    Array.from(indexed.entries()).map(([clientId, rows]) => [
      clientId,
      Array.from(rows.values()),
    ]),
  );
  for (const rows of grouped.values()) {
    rows.sort(
      (left, right) =>
        timestampMillis(left.bucket_start) -
        timestampMillis(right.bucket_start),
    );
  }
  return grouped;
}

function historyRatesByClient(records: TelemetryNetworkRateRecord[]) {
  const indexed = new Map<string, Map<string, TelemetryNetworkRateRecord>>();
  for (const record of records) {
    const byBucket = indexed.get(record.client_id) ?? new Map();
    const key = `${record.interface}\0${record.bucket_start}\0${record.bucket_secs}`;
    byBucket.set(key, record);
    indexed.set(record.client_id, byBucket);
  }
  const grouped = new Map(
    Array.from(indexed.entries()).map(([clientId, rows]) => [
      clientId,
      Array.from(rows.values()),
    ]),
  );
  for (const rows of grouped.values()) {
    rows.sort(
      (left, right) =>
        timestampMillis(left.bucket_start) -
        timestampMillis(right.bucket_start),
    );
  }
  return grouped;
}

function recentMetricValues<
  T extends { bucket_start: string; bucket_secs: number },
>(records: T[], value: (record: T) => number | null) {
  const rows = records.slice(-18);
  const values: Array<number | null> = [];
  let previous: T | null = null;
  for (const row of rows) {
    if (
      previous &&
      timestampMillis(row.bucket_start) -
        timestampMillis(previous.bucket_start) >
        Math.max(previous.bucket_secs, row.bucket_secs) * 2_000
    ) {
      values.push(null);
    }
    values.push(value(row));
    previous = row;
  }
  return values;
}

function networkDirectionHistory(
  records: TelemetryNetworkRateRecord[],
  direction: "rx" | "tx",
) {
  const totals = new Map<string, { bucketSecs: number; value: number }>();
  for (const row of records) {
    const stored = totals.get(row.bucket_start) ?? {
      bucketSecs: row.bucket_secs,
      value: 0,
    };
    stored.bucketSecs = Math.max(stored.bucketSecs, row.bucket_secs);
    stored.value += direction === "rx" ? row.rx_bps_avg : row.tx_bps_avg;
    totals.set(row.bucket_start, stored);
  }
  return recentMetricValues(
    Array.from(totals, ([bucket_start, point]) => ({
      bucket_start,
      bucket_secs: point.bucketSecs,
      value: point.value,
    })).sort(
      (left, right) =>
        timestampMillis(left.bucket_start) -
        timestampMillis(right.bucket_start),
    ),
    (row) => finiteMetric(row.value),
  );
}

function recentPingValues(records: PingRollupView[]) {
  return recentMetricValues(
    [...records]
      .sort(
        (left, right) =>
          timestampMillis(left.bucket_start) -
          timestampMillis(right.bucket_start),
      )
      .slice(-18),
    (row) => finiteMetric(row.latency_avg_ms),
  );
}

function compareMonitorAgents({
  mode,
  primaryPing,
  rates,
  rollups,
  signals,
  traffic,
}: {
  mode: FleetMonitorSort;
  primaryPing: Map<string, CurrentPingView>;
  rates: Map<string, TelemetryNetworkRateRecord[]>;
  rollups: Map<string, TelemetryRollupRecord>;
  signals: CardSignalContext;
  traffic: Map<string, TrafficAccountingRecord>;
}) {
  return (left: AgentView, right: AgentView) => {
    if (mode === "provider") {
      return (
        providerSortValue(left).localeCompare(providerSortValue(right)) ||
        regionSortValue(left).localeCompare(regionSortValue(right)) ||
        displayNameOrUnnamed(left.display_name).localeCompare(
          displayNameOrUnnamed(right.display_name),
        )
      );
    }
    if (mode === "region") {
      return (
        regionSortValue(left).localeCompare(regionSortValue(right)) ||
        providerSortValue(left).localeCompare(providerSortValue(right)) ||
        displayNameOrUnnamed(left.display_name).localeCompare(
          displayNameOrUnnamed(right.display_name),
        )
      );
    }
    const warningDelta =
      monitorWarningRank(
        right,
        signals,
        rollups,
        rates,
        traffic.get(right.id),
        primaryPing.get(right.id),
      ) -
      monitorWarningRank(
        left,
        signals,
        rollups,
        rates,
        traffic.get(left.id),
        primaryPing.get(left.id),
      );
    if (mode === "warning" && warningDelta !== 0) return warningDelta;
    const leftTraffic = trafficSortValue(traffic.get(left.id));
    const rightTraffic = trafficSortValue(traffic.get(right.id));
    if (mode === "traffic" && rightTraffic !== leftTraffic)
      return rightTraffic - leftTraffic;
    const leftNetwork = networkRateTotal(rates.get(left.id) ?? []);
    const rightNetwork = networkRateTotal(rates.get(right.id) ?? []);
    const leftRollup = rollups.get(left.id);
    const rightRollup = rollups.get(right.id);
    const leftCpu = finiteRatio(leftRollup?.cpu_usage_avg) ?? -1;
    const rightCpu = finiteRatio(rightRollup?.cpu_usage_avg) ?? -1;
    if (mode === "cpu" && rightCpu !== leftCpu) return rightCpu - leftCpu;
    const leftMemory = memoryUsedRatio(leftRollup);
    const rightMemory = memoryUsedRatio(rightRollup);
    if (mode === "memory" && rightMemory !== leftMemory)
      return rightMemory - leftMemory;
    const statusDelta = monitorStatusRank(right) - monitorStatusRank(left);
    if (statusDelta !== 0) return statusDelta;
    if (warningDelta !== 0) return warningDelta;
    if (rightNetwork !== leftNetwork) return rightNetwork - leftNetwork;
    if (rightCpu !== leftCpu) return rightCpu - leftCpu;
    return displayNameOrUnnamed(left.display_name).localeCompare(
      displayNameOrUnnamed(right.display_name),
    );
  };
}

function networkRateTotal(rates: TelemetryNetworkRateRecord[]) {
  return coherentNetworkRates(rates).reduce(
    (total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg,
    0,
  );
}

function coherentNetworkRates(rates: TelemetryNetworkRateRecord[]) {
  const latest = Math.max(
    ...rates
      .map((rate) => timestampMillis(rate.bucket_start))
      .filter(Number.isFinite),
  );
  if (!Number.isFinite(latest)) {
    return [];
  }
  return rates.filter(
    (rate) =>
      latest - timestampMillis(rate.bucket_start) <=
      NETWORK_SNAPSHOT_COHERENCE_MS,
  );
}

function memoryUsedRatio(rollup: TelemetryRollupRecord | undefined) {
  if (!rollup || rollup.memory_total_bytes_max <= 0) {
    return -1;
  }
  return (
    (rollup.memory_total_bytes_max - rollup.memory_available_bytes_avg) /
    rollup.memory_total_bytes_max
  );
}

function providerSortValue(agent: AgentView) {
  return tagValue(agent.tags, "provider") ?? "provider unset";
}

function regionSortValue(agent: AgentView) {
  return (
    tagValue(agent.tags, "country") ??
    tagValue(agent.tags, "region") ??
    "region unset"
  );
}

function monitorWarningRank(
  agent: AgentView,
  signals: CardSignalContext,
  rollups: Map<string, TelemetryRollupRecord>,
  rates: Map<string, TelemetryNetworkRateRecord[]>,
  traffic: TrafficAccountingRecord | undefined,
  primaryPing: CurrentPingView | undefined,
) {
  const localSignals =
    signals.records.get(agent.id) ?? defaultCardSignal(signals.global);
  return (
    monitorStatusRank(agent) * 10 +
    monitorEvidenceWarningRank(agent, rollups, rates, traffic, primaryPing) *
      5 +
    signalToneRank(localSignals.alertTone) +
    signalToneRank(localSignals.backupTone) +
    signalToneRank(localSignals.transferTone)
  );
}

function signalToneRank(tone: VpsMonitorCardSignal["alertTone"]) {
  if (tone === "critical") return 4;
  if (tone === "warning") return 3;
  if (tone === "info") return 2;
  if (tone === "neutral") return 1;
  return 0;
}

function monitorStatusRank(agent: AgentView) {
  const displayState = agentDisplayState(agent);
  if (displayState.label === "Offline") return 3;
  if (
    displayState.tone === "warning" ||
    agent.stale_since ||
    agent.stale_reason
  )
    return 2;
  if (agent.capabilities.privilege_mode === "unknown") return 1;
  return 0;
}

function monitorStatusTone(
  agent: AgentView,
  displayState = agentDisplayState(agent),
) {
  if (displayState.label === "Online") return "online";
  if (displayState.label === "Stale") return "stale";
  if (displayState.label === "Offline") return "offline";
  if (
    displayState.tone === "warning" ||
    agent.stale_since ||
    agent.stale_reason
  )
    return "warning";
  if (agent.capabilities.privilege_mode === "unknown") return "warning";
  return "offline";
}

function monitorFleetCategory(
  agent: AgentView,
  signals: CardSignalContext,
  rollups: Map<string, TelemetryRollupRecord>,
  rates: Map<string, TelemetryNetworkRateRecord[]>,
  traffic: Map<string, TrafficAccountingRecord>,
  primaryPing: Map<string, CurrentPingView>,
): Exclude<FleetMonitorStatusFilter, "all"> {
  const status = monitorStatusTone(agent);
  if (status === "offline") return "offline";
  const local =
    signals.records.get(agent.id) ?? defaultCardSignal(signals.global);
  if (
    status === "stale" ||
    status === "warning" ||
    monitorEvidenceWarningRank(
      agent,
      rollups,
      rates,
      traffic.get(agent.id),
      primaryPing.get(agent.id),
    ) > 0 ||
    local.alertTone === "critical" ||
    local.alertTone === "warning" ||
    local.backupTone === "critical" ||
    local.transferTone === "critical"
  ) {
    return "warning";
  }
  return "online";
}

function monitorFleetCounts(
  agents: AgentView[],
  signals: CardSignalContext,
  rollups: Map<string, TelemetryRollupRecord>,
  rates: Map<string, TelemetryNetworkRateRecord[]>,
  traffic: Map<string, TrafficAccountingRecord>,
  primaryPing: Map<string, CurrentPingView>,
) {
  const counts = { offline: 0, online: 0, total: agents.length, warning: 0 };
  for (const agent of agents) {
    counts[
      monitorFleetCategory(agent, signals, rollups, rates, traffic, primaryPing)
    ] += 1;
  }
  return counts;
}

function monitorFleetSnapshot(
  agents: AgentView[],
  rates: Map<string, TelemetryNetworkRateRecord[]>,
  traffic: Map<string, TrafficAccountingRecord>,
) {
  const locations = new Set<string>();
  let unspecifiedLocations = 0;
  let rxBps = 0;
  let txBps = 0;
  let freshNetworkCount = 0;
  let trafficBytes = 0;
  let trafficCount = 0;
  for (const agent of agents) {
    const location =
      tagValue(agent.tags, "country") ?? tagValue(agent.tags, "region");
    if (location) locations.add(location);
    else unspecifiedLocations += 1;
    const currentRates = coherentNetworkRates(rates.get(agent.id) ?? []);
    const observedAt = latestTimestamp(
      currentRates.map((rate) => rate.bucket_start),
    );
    if (
      monitorTelemetryState(agentDisplayState(agent), observedAt).kind ===
      "fresh"
    ) {
      rxBps += sumNetworkRate(currentRates, "rx") ?? 0;
      txBps += sumNetworkRate(currentRates, "tx") ?? 0;
      freshNetworkCount += 1;
    }
    const accounting = traffic.get(agent.id);
    if (
      accounting &&
      accounting.selectors.length > 0 &&
      accounting.reset_day !== null
    ) {
      trafficBytes += Math.max(0, accounting.total_bytes);
      trafficCount += 1;
    }
  }
  return {
    freshNetworkCount,
    locations: Array.from(locations).sort((left, right) =>
      left.localeCompare(right),
    ),
    rxBps: freshNetworkCount > 0 ? rxBps : null,
    trafficBytes,
    trafficCount,
    txBps: freshNetworkCount > 0 ? txBps : null,
    unspecifiedLocations,
  };
}

function formatLocationSummary(locations: string[], unspecified = 0) {
  if (locations.length === 0) {
    return unspecified > 0 ? `${unspecified} unspecified` : "No VPS locations";
  }
  const visible = locations.slice(0, 2).join(", ");
  const remaining = locations.length - Math.min(2, locations.length);
  const known = remaining > 0 ? `${visible} +${remaining}` : visible;
  return unspecified > 0 ? `${known} · ${unspecified} unspecified` : known;
}

function monitorEvidenceWarningRank(
  agent: AgentView,
  rollups: Map<string, TelemetryRollupRecord>,
  rates: Map<string, TelemetryNetworkRateRecord[]>,
  traffic: TrafficAccountingRecord | undefined,
  primaryPing: CurrentPingView | undefined,
) {
  return Math.max(
    monitorTelemetryWarningRank(
      agent,
      rollups.get(agent.id),
      rates.get(agent.id) ?? [],
    ),
    trafficWarningRank(traffic),
    pingWarningRank(primaryPing),
  );
}

function trafficWarningRank(
  traffic: TrafficAccountingRecord | null | undefined,
) {
  if (!traffic || traffic.selectors.length === 0) return 0;
  if ((finiteMetric(traffic.cycle_percent) ?? 0) > 100) return 2;
  return traffic.state !== "ok" || traffic.incomplete_reasons.length > 0
    ? 1
    : 0;
}

function pingWarningRank(ping: CurrentPingView | null | undefined) {
  if (!ping) return 0;
  if (!ping.enabled || ["down", "error", "failed"].includes(ping.state))
    return 2;
  if (ping.state !== "ok") return 1;
  const checkedAt = ping.checked_at
    ? timestampMillis(ping.checked_at)
    : Number.NaN;
  return !Number.isFinite(checkedAt) || Date.now() - checkedAt > 3 * 60_000
    ? 1
    : 0;
}

function monitorTelemetryWarningRank(
  agent: AgentView,
  rollup: TelemetryRollupRecord | undefined,
  rates: TelemetryNetworkRateRecord[],
) {
  const displayState = agentDisplayState(agent);
  const resource = monitorTelemetryState(
    displayState,
    rollup?.latest_observed_at ?? null,
  );
  const network = monitorTelemetryState(
    displayState,
    latestTimestamp(
      coherentNetworkRates(rates).map((rate) => rate.bucket_start),
    ),
  );
  const summary = monitorTelemetrySummary(
    resource,
    network,
    latestTimestamp([
      rollup?.latest_observed_at,
      ...coherentNetworkRates(rates).map((rate) => rate.bucket_start),
    ]),
  );
  if (summary.kind === "missing" || summary.kind === "stale") return 2;
  return summary.kind === "partial" ? 1 : 0;
}

function trafficSortValue(traffic: TrafficAccountingRecord | undefined) {
  if (!traffic || traffic.selectors.length === 0 || traffic.reset_day === null)
    return -1;
  return (
    finiteMetric(traffic.cycle_percent) ??
    finiteMetric(traffic.total_bytes) ??
    -1
  );
}

function fleetShareSelector(agents: AgentView[], allAgents: AgentView[]) {
  if (
    agents.length === allAgents.length &&
    agents.every((agent) =>
      allAgents.some((candidate) => candidate.id === agent.id),
    )
  ) {
    return "*";
  }
  return (
    selectorExpressionForClientIds(agents.map((agent) => agent.id)) ||
    "id:__no_match__"
  );
}

function tagValue(tags: string[], key: string) {
  const prefix = `${key}:`;
  return (
    tags
      .find((tag) => tag.toLowerCase().startsWith(prefix))
      ?.slice(prefix.length) ?? null
  );
}

function finiteMetric(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
}

function finiteRatio(value: number | null | undefined) {
  return typeof value === "number" &&
    Number.isFinite(value) &&
    value >= 0 &&
    value <= 1
    ? value
    : null;
}

function sumNetworkRate(
  rates: TelemetryNetworkRateRecord[],
  direction: "rx" | "tx",
) {
  if (rates.length === 0) return null;
  return finiteMetric(
    rates.reduce(
      (total, rate) =>
        total + (direction === "rx" ? rate.rx_bps_avg : rate.tx_bps_avg),
      0,
    ),
  );
}

function usedCapacity(total: number, available: number) {
  if (
    !Number.isFinite(total) ||
    !Number.isFinite(available) ||
    total <= 0 ||
    available < 0
  ) {
    return null;
  }
  return Math.max(0, Math.min(total, total - available));
}

function percentValue(used: number, total: number) {
  if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0) {
    return null;
  }
  return Math.max(0, Math.min(100, (used / total) * 100));
}

function formatPercent(value: number | null) {
  return value === null ? "n/a" : `${Math.round(value)}%`;
}

function formatCapacity(used: number | null, total: number | null | undefined) {
  if (
    used === null ||
    typeof total !== "number" ||
    !Number.isFinite(total) ||
    total <= 0
  ) {
    return "capacity unavailable";
  }
  return `${formatBytes(used)} / ${formatBytes(total)}`;
}

function formatTrafficUsage(traffic: TrafficAccountingRecord) {
  const percent = traffic.cycle_percent;
  const used = formatBytes(traffic.total_bytes);
  if (traffic.quota_total_bytes === -1) {
    return `${used} / Unlimited`;
  }
  const directionalQuotas = [traffic.quota_rx_bytes, traffic.quota_tx_bytes];
  if (
    traffic.quota_total_bytes === null &&
    directionalQuotas.some((quota) => quota === -1) &&
    directionalQuotas.every((quota) => quota === null || quota === -1)
  ) {
    return `${used} / Unlimited`;
  }
  if (traffic.quota_total_bytes !== null && percent !== null) {
    return `${used} / ${formatBytes(traffic.quota_total_bytes)} · ${percent.toFixed(percent >= 10 ? 0 : 1)}%`;
  }
  return percent === null
    ? `${used} used`
    : `${used} used · limiting quota ${percent.toFixed(percent >= 10 ? 0 : 1)}%`;
}

function formatSocketCount(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.round(value).toLocaleString()
    : "n/a";
}

function connectionCountTitle(
  protocol: "TCP" | "UDP",
  telemetry: MonitorTelemetryState,
) {
  return `${protocol} entries in the agent's Linux network-namespace socket tables; TCP includes every state and listeners. ${telemetry.title}`;
}

function billingTitle(billing: BillingPlanView) {
  if (billing.disabled) {
    return "Billing is explicitly disabled with -1; the card therefore shows n/a.";
  }
  return billing.cycle
    ? `${billing.display}; renewal anchor ${billing.cycle}. Billing cycle is independent of the traffic reset day.`
    : `${billing.display}; no renewal anchor is configured. Billing cycle is independent of the traffic reset day.`;
}

function formatTrafficTitle(traffic: TrafficAccountingRecord) {
  const state = traffic.state === "ok" ? "Current" : traffic.state;
  const overage =
    traffic.cycle_percent !== null && traffic.cycle_percent > 100
      ? ` Quota exceeded by ${(traffic.cycle_percent - 100).toFixed(1)}%.`
      : "";
  return `${state} authoritative traffic-accounting cycle. RX ${formatBytes(traffic.rx_bytes)}; TX ${formatBytes(traffic.tx_bytes)}.${overage}`;
}

function formatTrafficReset(cycleEnd: string) {
  const end = timestampMillis(cycleEnd);
  if (!Number.isFinite(end)) return "Reset time unavailable";
  const remaining = end - Date.now();
  if (remaining <= 0) return "Cycle reset due";
  const days = Math.ceil(remaining / 86_400_000);
  return days <= 1 ? "Resets within 1 day" : `Resets in ${days} days`;
}

function formatPrimaryPing(ping: CurrentPingView | null) {
  if (!ping) return "Unconfigured";
  if (!ping.enabled || ping.state === "disabled") return "Disabled";
  if (ping.state === "pending" || ping.checked_at === null)
    return "Waiting for first result";
  const latency =
    ping.latency_avg_ms === null
      ? "Latency unavailable"
      : `${ping.latency_avg_ms.toFixed(1)} ms`;
  const loss =
    ping.loss_ratio === null
      ? "loss unavailable"
      : `${(ping.loss_ratio * 100).toFixed(ping.loss_ratio > 0 && ping.loss_ratio < 0.01 ? 1 : 0)}% loss`;
  return `${latency} · ${loss}`;
}

function formatLoad(value: number | null) {
  return value === null ? "n/a" : value.toFixed(2);
}

function formatRateOrUnavailable(value: number | null) {
  return value === null ? "n/a" : formatRate(value);
}

function networkMetricTitle(
  direction: "received" | "sent",
  interfaceCount: number,
  telemetryState: MonitorTelemetryState,
) {
  return `${INTERFACE_RATE_DEFINITION} Latest ${direction} interval-average rates are summed across ${interfaceCount} concurrently reported interface${interfaceCount === 1 ? "" : "s"}; virtual paths can overlap. ${telemetryState.title}`;
}

function formatMonitorContactEvidence(
  agent: AgentView,
  displayState: AgentDisplayState,
  lastContact: string | null,
) {
  if (displayState.label === "Contact unknown") {
    return "Contact unknown; no gateway timestamp";
  }
  if (lastContact) {
    return `Last contact ${formatTime(lastContact)}`;
  }
  return displayState.detail;
}

function latestTimestamp(values: Array<string | null | undefined>) {
  const latest = values
    .map((value) => (value ? timestampMillis(value) : Number.NaN))
    .filter((value) => Number.isFinite(value))
    .sort((left, right) => right - left)[0];
  return latest === undefined ? null : new Date(latest).toISOString();
}

function formatRate(value: number) {
  if (!Number.isFinite(value) || value <= 0) {
    return "0 bps";
  }
  if (value >= 1_000_000_000)
    return `${(value / 1_000_000_000).toFixed(1)} Gbps`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)} Mbps`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)} Kbps`;
  return `${Math.round(value)} bps`;
}

function isActiveJobStatus(status: string) {
  return ["queued", "dispatching", "running"].includes(status);
}

function isFailedJobStatus(status: string) {
  return [
    "failed",
    "rejected",
    "agent_lost",
    "agent_timeout",
    "control_timeout",
    "deadline_expired",
  ].includes(status);
}

function isFailedBackupStatus(status: string) {
  return status === "execution_failed" || status === "execution_canceled";
}

function isActiveTransferStatus(status: string) {
  return status === "started" || status === "transferring";
}

function isFailedTransferStatus(status: string) {
  return status === "aborted" || status === "unknown";
}
