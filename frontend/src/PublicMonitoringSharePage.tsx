import { Activity, Gauge, Network, Radio, Server } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { apiErrorFromResponse, apiFetch, apiJsonFromResponse } from "./api";
import { dashboardChartColors, consolePalette } from "./colorPalette";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "./components/TimeSeriesChart";
import { ConsoleStatusBadge } from "./components/ConsoleLayout";
import { CountryFlag } from "./components/CountryFlag";
import {
  MonitoringRangeTabs,
  type MonitoringWindow,
} from "./components/MonitoringRangeTabs";
import {
  pushHistoryEntry,
  replaceHistoryEntry,
  useHistoryEntryState,
} from "./historyEntryState";
import {
  PUBLIC_MONITOR_DENSITY_STORAGE_KEY,
  usePersistentMonitorCardDensity,
  type MonitorCardDensity,
} from "./monitorCardDensity";
import { countryTagValue } from "./tagDisplay";
import {
  MiniSparkline,
  MonitorFact,
  MonitorMetric,
  VpsMonitorCardSurface,
  formatTrafficReset,
} from "./panels/FleetMonitorPanel";
import type {
  PublicMonitoringCardView as PublicMonitoringCard,
  PublicMonitoringDataView as PublicMonitoringData,
  PublicMonitoringDetailView as PublicMonitoringDetail,
  PublicMonitoringRangeView as MonitoringRange,
  PublicNetworkMetricView as PublicNetworkMetric,
  PublicNetworkPointView as PublicNetworkPoint,
  PublicPingMetricView as PublicPingMetric,
  PublicPingPointView as PublicPingPoint,
  PublicResourceMetricView as PublicResourceMetric,
  PublicMonitoringShareBootstrapView,
  PublicMonitoringShareView,
  PublicTrafficMetricView as PublicTrafficMetric,
  PublicTrafficHistoryPointView as PublicTrafficPoint,
} from "./types";
import {
  formatBillingRenewal,
  formatCompactTime,
  formatFullTime,
  formatVirtualizationLabel,
  trafficLimitingQuota,
  trafficUnlimitedQuota,
  trafficQuotaState,
  timestampMillis,
} from "./utils";
import { agentStatusPresentation } from "./agentDisplayState";
import {
  formatByteCount as formatBytes,
  formatByteRateFromBitsPerSecond,
  formatUptime,
} from "./telemetryMetrics";

type PublicMonitoringSharePageProps = {
  initialClientKey?: string | null;
  shareId: string;
  secret: string;
};

type Density = MonitorCardDensity;
type CardStatusFilter = "all" | "online" | "warning" | "offline";
type PublicMonitorSort =
  | "warning"
  | "traffic"
  | "cpu"
  | "memory"
  | "region"
  | "provider";
type CustomBounds = {
  startUnix: number;
  endUnix: number;
};

const publicMonitorSortOptions: Array<{
  label: string;
  value: PublicMonitorSort;
}> = [
  { label: "Warnings first", value: "warning" },
  { label: "Traffic use", value: "traffic" },
  { label: "CPU use", value: "cpu" },
  { label: "Memory", value: "memory" },
  { label: "Region", value: "region" },
  { label: "Provider", value: "provider" },
];

// React StrictMode remounts effects in development. Sharing only an in-flight
// bootstrap avoids recording that remount as a second visitor. The secret is
// removed from process memory as soon as the request settles.
const bootstrapRequests = new Map<
  string,
  Promise<PublicMonitoringShareBootstrapView>
>();

export function PublicMonitoringSharePage({
  initialClientKey = null,
  shareId,
  secret,
}: PublicMonitoringSharePageProps) {
  const historySlot = `public-monitoring-share.${shareId}`;
  const [density, setDensity] = usePersistentMonitorCardDensity(
    historySlot,
    PUBLIC_MONITOR_DENSITY_STORAGE_KEY,
  );
  const [search, setSearch] = useHistoryEntryState(`${historySlot}.search`, "");
  const [statusFilter, setStatusFilter] =
    useHistoryEntryState<CardStatusFilter>(`${historySlot}.status`, "all");
  const [tagFilter, setTagFilter] = useHistoryEntryState(
    `${historySlot}.tag`,
    "all",
  );
  const [providerFilter, setProviderFilter] = useHistoryEntryState(
    `${historySlot}.provider`,
    "all",
  );
  const [sortMode, setSortMode] = useHistoryEntryState<PublicMonitorSort>(
    `${historySlot}.sort`,
    "warning",
  );
  const [window, setWindow] = useHistoryEntryState<MonitoringWindow>(
    `${historySlot}.window`,
    "1d",
  );
  const [share, setShare] = useState<PublicMonitoringShareView | null>(null);
  const [visitorId, setVisitorId] = useState<string | null>(null);
  const [cards, setCards] = useState<PublicMonitoringCard[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [retryRevision, setRetryRevision] = useState(0);
  const [selectedCardKey, setSelectedCardKey] = useState<string | null>(
    initialClientKey,
  );
  const [detailOpenedFromGrid, setDetailOpenedFromGrid] = useState(false);
  const [gridScrollY, setGridScrollY] = useHistoryEntryState(
    `${historySlot}.grid-scroll-y`,
    0,
  );
  const [renderLimit, setRenderLimit] = useHistoryEntryState(
    `${historySlot}.render-limit`,
    100,
  );
  const loadMoreRef = useRef<HTMLDivElement | null>(null);
  const [detail, setDetail] = useState<PublicMonitoringDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const now = Date.now();
  const [customStart, setCustomStart] = useHistoryEntryState(
    `${historySlot}.custom-start`,
    () => dateTimeLocalValue(now - 24 * 60 * 60 * 1_000),
  );
  const [customEnd, setCustomEnd] = useHistoryEntryState(
    `${historySlot}.custom-end`,
    () => dateTimeLocalValue(now),
  );
  const [customBounds, setCustomBounds] = useHistoryEntryState<CustomBounds>(
    `${historySlot}.custom-bounds`,
    () => ({
      endUnix: Math.floor(now / 1_000),
      startUnix: Math.floor(now / 1_000) - 24 * 60 * 60,
    }),
  );
  const [customError, setCustomError] = useState<string | null>(null);
  const [detailRevision, setDetailRevision] = useState(0);

  useEffect(() => {
    const applyLocation = () => {
      const clientKey = publicShareClientKeyFromLocation(shareId, secret);
      if (clientKey !== undefined) {
        setSelectedCardKey(clientKey);
      }
    };
    globalThis.addEventListener("hashchange", applyLocation);
    globalThis.addEventListener("popstate", applyLocation);
    return () => {
      globalThis.removeEventListener("hashchange", applyLocation);
      globalThis.removeEventListener("popstate", applyLocation);
    };
  }, [secret, shareId]);

  useEffect(() => {
    let active = true;
    const controller = new AbortController();
    setCards([]);
    setTotal(0);
    setShare(null);
    setVisitorId(null);
    setDetail(null);
    setError(null);
    setLoading(true);

    void (async () => {
      if (!shareId.trim() || !secret) {
        throw new Error("This shared-view link is incomplete.");
      }
      const bootstrap = await bootstrapMonitoringShare(shareId, secret);
      if (!active) return;
      setShare(bootstrap.share);
      setVisitorId(bootstrap.visitor_id);

      let offset = 0;
      let combined: PublicMonitoringCard[] = [];
      const loadedKeys = new Set<string>();
      for (;;) {
        const params = new URLSearchParams({
          limit: "1000",
          offset: String(offset),
        });
        const page = await publicShareJson<PublicMonitoringData>(
          publicDataPath(shareId, params),
          secret,
          controller.signal,
          bootstrap.visitor_id,
        );
        if (!active) return;
        if (page.offset !== offset) {
          throw new Error(
            "The shared view returned the wrong pagination offset.",
          );
        }
        if (page.cards.some((card) => loadedKeys.has(card.client_key))) {
          throw new Error(
            "The shared view returned the same VPS more than once.",
          );
        }
        page.cards.forEach((card) => loadedKeys.add(card.client_key));
        setShare(page.share);
        setTotal(page.total);
        combined = [...combined, ...page.cards];
        setCards(combined);
        if (combined.length > page.total) {
          throw new Error(
            "The shared view returned more cards than its reported total.",
          );
        }
        if (page.next_offset === null) {
          if (combined.length !== page.total) {
            throw new Error(
              "The shared view ended before every shared VPS was returned.",
            );
          }
          break;
        }
        if (
          page.next_offset !== offset + page.cards.length ||
          page.next_offset > page.total ||
          page.cards.length === 0
        ) {
          throw new Error(
            "The shared view returned an invalid pagination cursor; no cards were inferred beyond the last complete page.",
          );
        }
        offset = page.next_offset;
      }
    })()
      .catch((reason: unknown) => {
        if (active && !isAbortError(reason)) {
          setError(errorMessage(reason));
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });

    return () => {
      active = false;
      controller.abort();
    };
  }, [retryRevision, secret, shareId]);

  const selectedCard = useMemo(
    () => cards.find((card) => card.client_key === selectedCardKey) ?? null,
    [cards, selectedCardKey],
  );
  const selectedClientKey = selectedCard?.client_key ?? null;

  useEffect(() => {
    if (!visitorId || !shareId.trim() || !secret) return;
    let active = true;
    let inFlight = false;
    const refreshCards = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        let offset = 0;
        let combined: PublicMonitoringCard[] = [];
        const loadedKeys = new Set<string>();
        let latestShare = share;
        let latestTotal = total;
        for (;;) {
          const params = new URLSearchParams({
            limit: "1000",
            offset: String(offset),
          });
          const page = await publicShareJson<PublicMonitoringData>(
            publicDataPath(shareId, params),
            secret,
            undefined,
            visitorId,
          );
          if (!active) return;
          if (page.offset !== offset) {
            throw new Error(
              "The shared view returned the wrong pagination offset; the previous complete card set remains visible.",
            );
          }
          if (page.cards.some((card) => loadedKeys.has(card.client_key))) {
            throw new Error(
              "The shared view returned the same VPS more than once; the previous complete card set remains visible.",
            );
          }
          page.cards.forEach((card) => loadedKeys.add(card.client_key));
          latestShare = page.share;
          latestTotal = page.total;
          combined = [...combined, ...page.cards];
          if (combined.length > page.total) {
            throw new Error(
              "The shared view exceeded its reported total; the previous complete card set remains visible.",
            );
          }
          if (page.next_offset === null) {
            if (combined.length !== page.total) {
              throw new Error(
                "The shared view ended early; the previous complete card set remains visible.",
              );
            }
            break;
          }
          if (
            page.next_offset !== offset + page.cards.length ||
            page.next_offset > page.total ||
            page.cards.length === 0
          ) {
            throw new Error(
              "The shared view returned an invalid pagination cursor; the previous complete card set remains visible.",
            );
          }
          offset = page.next_offset;
        }
        if (!active) return;
        setShare(latestShare);
        setTotal(latestTotal);
        setCards(combined);
        setError(null);
      } catch (reason) {
        if (active) setError(errorMessage(reason));
      } finally {
        inFlight = false;
      }
    };
    const timer = globalThis.setInterval(() => void refreshCards(), 60_000);
    return () => {
      active = false;
      globalThis.clearInterval(timer);
    };
  }, [secret, shareId, visitorId]);

  useEffect(() => {
    if (!selectedClientKey || !share?.visibility.detail_history || !visitorId) {
      setDetail(null);
      setDetailError(null);
      setDetailLoading(false);
      return;
    }
    let active = true;
    let inFlight = false;
    const controller = new AbortController();
    setDetail(null);
    setDetailError(null);
    setDetailLoading(true);
    const params = new URLSearchParams({
      client_key: selectedClientKey,
      limit: "1",
      offset: "0",
      points: "360",
      window,
    });
    if (window === "custom") {
      params.set("start_unix", String(customBounds.startUnix));
      params.set("end_unix", String(customBounds.endUnix));
    }
    const loadDetail = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const response = await publicShareJson<PublicMonitoringData>(
          publicDataPath(shareId, params),
          secret,
          controller.signal,
          visitorId,
        );
        if (!active) return;
        if (!response.detail) {
          throw new Error("History is not available for this VPS.");
        }
        setDetail(response.detail);
        setDetailError(null);
      } catch (reason) {
        if (active && !isAbortError(reason)) {
          setDetailError(errorMessage(reason));
        }
      } finally {
        inFlight = false;
        if (active) setDetailLoading(false);
      }
    };
    void loadDetail();
    const timer =
      window === "15m"
        ? globalThis.setInterval(() => void loadDetail(), 60_000)
        : null;
    return () => {
      active = false;
      controller.abort();
      if (timer !== null) globalThis.clearInterval(timer);
    };
  }, [
    customBounds,
    detailRevision,
    secret,
    selectedClientKey,
    share?.visibility.detail_history,
    shareId,
    visitorId,
    window,
  ]);

  const identityContextVisible = share?.visibility.identity_context === true;
  const tagOptions = useMemo(
    () =>
      identityContextVisible
        ? Array.from(new Set(cards.flatMap((card) => card.tags ?? []))).sort()
        : [],
    [cards, identityContextVisible],
  );
  const providerOptions = useMemo(
    () =>
      identityContextVisible
        ? Array.from(
            new Set(
              cards.flatMap((card) =>
                publicTagValues(card.tags ?? [], "provider"),
              ),
            ),
          ).sort()
        : [],
    [cards, identityContextVisible],
  );
  const effectiveTagFilter = tagOptions.includes(tagFilter) ? tagFilter : "all";
  const effectiveProviderFilter = providerOptions.includes(providerFilter)
    ? providerFilter
    : "all";
  const visibleSortOptions = identityContextVisible
    ? publicMonitorSortOptions
    : publicMonitorSortOptions.filter(
        (option) => option.value !== "region" && option.value !== "provider",
      );
  const effectiveSortMode = visibleSortOptions.some(
    (option) => option.value === sortMode,
  )
    ? sortMode
    : "warning";
  const filteredCards = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return cards
      .filter((card) => {
        const tags = card.tags ?? [];
        const matchesSearch =
          !query ||
          card.display_name.toLocaleLowerCase().includes(query) ||
          tags.some((tag) => tag.toLocaleLowerCase().includes(query));
        return (
          matchesSearch &&
          (statusFilter === "all" ||
            publicCardStatusGroup(card, share?.visibility) === statusFilter) &&
          (effectiveTagFilter === "all" || tags.includes(effectiveTagFilter)) &&
          (effectiveProviderFilter === "all" ||
            publicTagValues(tags, "provider").includes(effectiveProviderFilter))
        );
      })
      .sort((left, right) =>
        comparePublicMonitoringCards(
          left,
          right,
          effectiveSortMode,
          share?.visibility,
        ),
      );
  }, [
    cards,
    effectiveProviderFilter,
    effectiveSortMode,
    effectiveTagFilter,
    search,
    share?.visibility,
    statusFilter,
  ]);
  const summary = useMemo(
    () => summarizeCards(cards, share?.visibility),
    [cards, share?.visibility],
  );
  const fleetSnapshot = useMemo(
    () => summarizePublicFleet(cards, share?.visibility),
    [cards, share?.visibility],
  );
  const cardsComplete = !loading && cards.length >= total;
  const visibleCards = filteredCards.slice(0, Math.max(100, renderLimit));
  useEffect(() => {
    if (selectedCard || visibleCards.length >= filteredCards.length) return;
    const node = loadMoreRef.current;
    if (!node) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setRenderLimit((current) =>
            Math.min(filteredCards.length, Math.max(100, current) + 100),
          );
        }
      },
      { rootMargin: "800px 0px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [filteredCards.length, selectedCard, setRenderLimit, visibleCards.length]);

  function applyCustomRange() {
    const startUnix = Math.floor(Date.parse(customStart) / 1_000);
    const endUnix = Math.floor(Date.parse(customEnd) / 1_000);
    if (!Number.isFinite(startUnix) || !Number.isFinite(endUnix)) {
      setCustomError("Choose valid start and end times.");
      return;
    }
    if (startUnix >= endUnix) {
      setCustomError("Start time must be before end time.");
      return;
    }
    if (endUnix - startUnix > 3_650 * 24 * 60 * 60) {
      setCustomError("Custom history can span at most ten years.");
      return;
    }
    setCustomError(null);
    setCustomBounds({ endUnix, startUnix });
    setWindow("custom");
    setDetailRevision((current) => current + 1);
  }

  function openCard(card: PublicMonitoringCard) {
    setGridScrollY(globalThis.scrollY);
    pushHistoryEntry(publicShareUrl(shareId, secret, card.client_key));
    setDetailOpenedFromGrid(true);
    setSelectedCardKey(card.client_key);
  }

  function closeDetail() {
    if (detailOpenedFromGrid) {
      globalThis.history.back();
      return;
    }
    replaceHistoryEntry(publicShareUrl(shareId, secret));
    setSelectedCardKey(null);
  }

  useEffect(() => {
    const frame = globalThis.requestAnimationFrame(() => {
      globalThis.scrollTo({ top: selectedCardKey ? 0 : gridScrollY });
    });
    return () => globalThis.cancelAnimationFrame(frame);
  }, [gridScrollY, selectedCardKey]);

  const selectionError =
    selectedCardKey && !loading
      ? !selectedCard
        ? "This VPS is not part of the shared view."
        : !share?.visibility.detail_history
          ? "This shared view does not include VPS history access."
          : null
      : null;

  if (error && !share) {
    return (
      <main className="publicMonitoringSharePage workspace singleColumn">
        <section
          className="emptyState publicMonitoringUnavailable"
          role="alert"
        >
          <Server aria-hidden="true" size={24} />
          <strong>Shared view unavailable</strong>
          <span>{error}</span>
          <button
            className="secondaryAction"
            onClick={() => setRetryRevision((current) => current + 1)}
            type="button"
          >
            Retry
          </button>
        </section>
      </main>
    );
  }

  return (
    <main
      aria-busy={loading}
      className="publicMonitoringSharePage workspace singleColumn fleetMonitorWorkspace"
    >
      <header className="fleetMonitorToolbar publicMonitoringShareHeader">
        <div>
          <h1>{share?.name || "Shared VPS monitoring"}</h1>
          <span>
            Read-only monitoring view
            {share
              ? ` · available until ${formatFullTime(share.expires_at)}`
              : ""}
          </span>
        </div>
      </header>

      {share && !selectedCardKey ? (
        <section
          className="fleetMonitorSurface fleetPanel"
          aria-label="Shared fleet"
        >
          <div
            aria-label="Shared fleet summary and filters"
            className="publicMonitoringControls fleetMonitorToolbar"
          >
            <div
              className="publicMonitoringSummary"
              aria-label="Fleet status summary"
            >
              <SummaryFact label="Total" value={summary.total} />
              <SummaryFact label="Online" value={summary.online} />
              <SummaryFact label="Warning" value={summary.warning} />
              <SummaryFact label="Offline" value={summary.offline} />
            </div>
            <span className="publicMonitoringLoadEvidence">
              {cardsComplete
                ? `${total} VPS${total === 1 ? "" : "s"}`
                : `${cards.length} of ${total} VPSs loaded`}
            </span>
            <div className="fleetMonitorControls">
              <label htmlFor="public-monitoring-search">
                <span>Search</span>
                <input
                  aria-label="Search shared VPSs"
                  id="public-monitoring-search"
                  name="public-monitoring-search"
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder={
                    share.visibility.identity_context ? "Name or tag" : "Name"
                  }
                  type="search"
                  value={search}
                />
              </label>
              <label htmlFor="public-monitoring-status">
                <span>Status</span>
                <select
                  aria-label="Filter shared VPSs by status"
                  id="public-monitoring-status"
                  name="public-monitoring-status"
                  onChange={(event) =>
                    setStatusFilter(event.target.value as CardStatusFilter)
                  }
                  value={statusFilter}
                >
                  <option value="all">All statuses</option>
                  <option value="online">Online</option>
                  <option value="warning">Warning</option>
                  <option value="offline">Offline</option>
                </select>
              </label>
              {identityContextVisible ? (
                <>
                  <label htmlFor="public-monitoring-tag">
                    <span>Tag</span>
                    <select
                      aria-label="Filter shared VPSs by tag"
                      id="public-monitoring-tag"
                      name="public-monitoring-tag"
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
                  <label htmlFor="public-monitoring-provider">
                    <span>Provider</span>
                    <select
                      aria-label="Filter shared VPSs by provider"
                      id="public-monitoring-provider"
                      name="public-monitoring-provider"
                      onChange={(event) =>
                        setProviderFilter(event.target.value)
                      }
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
                </>
              ) : null}
              <label htmlFor="public-monitoring-sort">
                <span>Sort</span>
                <select
                  aria-label="Shared VPS sort"
                  id="public-monitoring-sort"
                  name="public-monitoring-sort"
                  onChange={(event) =>
                    setSortMode(event.target.value as PublicMonitorSort)
                  }
                  value={effectiveSortMode}
                >
                  {visibleSortOptions.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
              <div
                aria-label="Shared view density"
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
            </div>
          </div>
          {fleetSnapshot.visible ? (
            <div
              className="fleetMonitorSnapshot"
              aria-label="Shared fleet current totals"
            >
              {fleetSnapshot.locations ? (
                <span
                  title={`Locations disclosed by this Shared view: ${fleetSnapshot.locations.values.length}. ${formatPublicLocationSummary(
                    fleetSnapshot.locations.values,
                    fleetSnapshot.locations.unspecified,
                  )}`}
                >
                  <small>Locations</small>
                  <strong>{fleetSnapshot.locations.values.length}</strong>
                  <em>
                    {formatPublicLocationSummary(
                      fleetSnapshot.locations.values,
                      fleetSnapshot.locations.unspecified,
                    )}
                  </em>
                </span>
              ) : null}
              {fleetSnapshot.network ? (
                <span
                  title={`Aggregate current rates from fresh shared interface evidence. RX ${formatOptionalRate(fleetSnapshot.network.rxBps)}; TX ${formatOptionalRate(fleetSnapshot.network.txBps)}; ${fleetSnapshot.network.freshCount} VPSs have fresh evidence`}
                >
                  <small>Realtime speed</small>
                  <strong>
                    ↓ {formatOptionalRate(fleetSnapshot.network.rxBps)}
                  </strong>
                  <em>
                    ↑ {formatOptionalRate(fleetSnapshot.network.txBps)} ·{" "}
                    {fleetSnapshot.network.freshCount} fresh
                    {cardsComplete ? "" : " · partial"}
                  </em>
                </span>
              ) : null}
              {fleetSnapshot.traffic ? (
                <span
                  title={
                    fleetSnapshot.traffic.count > 0
                      ? `Aggregate configured traffic accounting across reset cycles and accumulated totals: ${formatBytes(fleetSnapshot.traffic.bytes)} across ${fleetSnapshot.traffic.count} VPSs`
                      : "No VPS in this Shared view has configured traffic accounting"
                  }
                >
                  <small>Traffic</small>
                  <strong>
                    {fleetSnapshot.traffic.count > 0
                      ? formatBytes(fleetSnapshot.traffic.bytes)
                      : "-"}
                  </strong>
                  <em>
                    {fleetSnapshot.traffic.count > 0
                      ? `${fleetSnapshot.traffic.count} configured${cardsComplete ? "" : " · partial"}`
                      : "No configured accounting"}
                  </em>
                </span>
              ) : null}
            </div>
          ) : null}

          {error ? (
            <p className="panelError publicMonitoringPageError" role="alert">
              {error}
            </p>
          ) : null}

          {!loading && filteredCards.length === 0 ? (
            <section className="emptyState publicMonitoringEmpty">
              <Server aria-hidden="true" size={22} />
              <strong>
                {cards.length
                  ? "No VPSs match these filters"
                  : "No VPSs in this shared view"}
              </strong>
              <span>
                {cards.length
                  ? "Clear the search or change the filters."
                  : "The owner shared an empty frozen target set."}
              </span>
            </section>
          ) : (
            <section
              aria-label="Shared VPS cards"
              className={`vpsMonitorGrid publicMonitoringGrid ${density}`}
              data-density={density}
            >
              {visibleCards.map((card) => (
                <PublicMonitoringCardView
                  card={card}
                  density={density}
                  detailAllowed={share.visibility.detail_history === true}
                  key={card.client_key}
                  onOpen={() => openCard(card)}
                  selected={false}
                  visibility={share.visibility}
                />
              ))}
            </section>
          )}
          {visibleCards.length < filteredCards.length ? (
            <div
              className="fleetMonitorProgress"
              ref={loadMoreRef}
              role="status"
            >
              Showing {visibleCards.length} of {filteredCards.length} shared
              VPSs…
            </div>
          ) : null}
        </section>
      ) : null}

      {error && selectedCardKey ? (
        <p className="panelError publicMonitoringPageError" role="alert">
          {error}
        </p>
      ) : null}

      {selectedCard && share?.visibility.detail_history ? (
        <PublicMonitoringDetailPanel
          card={selectedCard}
          customEnd={customEnd}
          customError={customError}
          customStart={customStart}
          detail={detail}
          error={detailError}
          loading={detailLoading}
          onApplyCustom={applyCustomRange}
          onClose={closeDetail}
          onCustomEndChange={(value) => {
            setCustomError(null);
            setCustomEnd(value);
          }}
          onCustomStartChange={(value) => {
            setCustomError(null);
            setCustomStart(value);
          }}
          onWindowChange={setWindow}
          visibility={share.visibility}
          window={window}
        />
      ) : null}

      {selectionError ? (
        <section className="emptyState publicMonitoringEmpty" role="alert">
          <Server aria-hidden="true" size={22} />
          <strong>VPS history unavailable</strong>
          <span>{selectionError}</span>
          <button onClick={closeDetail} type="button">
            Return to shared fleet
          </button>
        </section>
      ) : null}
    </main>
  );
}

function PublicMonitoringCardView({
  card,
  density,
  detailAllowed,
  onOpen,
  selected,
  visibility,
}: {
  card: PublicMonitoringCard;
  density: Density;
  detailAllowed: boolean;
  onOpen: () => void;
  selected: boolean;
  visibility: PublicMonitoringShareView["visibility"] | undefined;
}) {
  const resource = card.resources;
  const cpuPercent = ratioToPercent(resource?.cpu_usage_avg);
  const memoryUsed =
    resource && resource.memory_total_bytes > 0
      ? ratioToPercent(resource.memory_used_ratio_avg)
      : null;
  const diskUsed =
    resource && resource.disk_total_bytes > 0
      ? ratioToPercent(resource.disk_used_ratio_avg)
      : null;
  const loadPressure =
    resource && resource.cpu_cores > 0
      ? (resource.load_1 / resource.cpu_cores) * 100
      : null;
  const effectiveStatus = publicCardStatusGroup(card, visibility);
  const reportedStatus = agentStatusPresentation(card.status);
  const statusLabel = reportedStatus.label;
  const visibleStatusLabel = publicCardVisibleStatusLabel(card, visibility);
  const loadHistory = historyValues(
    card.resource_history ?? [],
    (point) => point.load_1,
  );
  const rxHistory = historyValues(
    card.network_history ?? [],
    (point) => point.rx_bps,
  );
  const txHistory = historyValues(
    card.network_history ?? [],
    (point) => point.tx_bps,
  );
  const resourceProblem = visibility?.resources
    ? publicFreshnessProblem(card.resources?.observed_at, "Resource telemetry")
    : null;
  const networkProblem = visibility?.network
    ? card.network?.rate_expected === false
      ? null
      : publicFreshnessProblem(card.network?.observed_at, "Network telemetry")
    : null;
  const warnings = publicCardWarnings(card, visibility);
  const country = visibility?.identity_context
    ? countryTagValue(card.tags ?? [])
    : null;
  const identitySummary = visibility?.identity_context
    ? publicIdentitySummary(card.tags ?? [])
    : "";
  const cardTitle = `${card.display_name || "Unnamed VPS"} · ${visibleStatusLabel}`;
  const freshness = publicCardFreshness(card, visibility);
  const freshnessLabel = freshness
    ? `Updated ${formatCompactTime(freshness)}`
    : publicCardHasVisibleTelemetry(visibility)
      ? "Visible telemetry unavailable"
      : "Status only";
  const auxiliaryFacts = publicMonitoringAuxiliaryFacts(card, visibility);
  const cardHeader = (
    <>
      <span
        className="vpsMonitorStatus"
        title={
          effectiveStatus === "warning"
            ? `Reported agent status: ${statusLabel}; visible monitoring needs attention`
            : `Reported status: ${statusLabel}`
        }
      >
        <span aria-hidden="true" />
        {visibleStatusLabel}
      </span>
      <strong
        className="vpsMonitorCardName"
        title={card.display_name || "Unnamed VPS"}
      >
        {country ? (
          <CountryFlag country={country} decorative fallback="none" />
        ) : null}
        <span>{card.display_name || "Unnamed VPS"}</span>
      </strong>
      <small>{freshnessLabel}</small>
    </>
  );
  return (
    <VpsMonitorCardSurface
      ariaLabel={`${cardTitle} shared monitoring card`}
      className={`vpsMonitorCard publicMonitoringCard ${effectiveStatus} ${density}${selected ? " selected" : ""}${detailAllowed ? "" : " publicMonitoringCardStatic"}`}
      header={cardHeader}
      onOpen={detailAllowed ? onOpen : undefined}
      title={
        detailAllowed
          ? `Open read-only history for ${card.display_name || "this VPS"}`
          : undefined
      }
    >
      {density === "comfortable" && visibility?.identity_context ? (
        <div
          className="publicMonitoringIdentityContext"
          aria-label="Shared identity context"
          title="Provider, region, country, and tags disclosed by this Shared view"
        >
          {identitySummary || "Identity context unavailable"}
        </div>
      ) : null}

      {visibility?.resources ? (
        <div
          aria-label={`Current resources for ${card.display_name}`}
          className={`vpsMonitorMetrics publicMonitoringMetricMatrix${resourceProblem ? " stale" : ""}`}
        >
          <PublicMetric
            caption={
              resource
                ? `${resource.cpu_cores} reported core${resource.cpu_cores === 1 ? "" : "s"}`
                : "unavailable"
            }
            icon={<Activity size={15} />}
            label="CPU"
            context={
              resource && resource.cpu_cores > 0
                ? `${resource.cpu_cores}-core`
                : undefined
            }
            percent={cpuPercent}
            showCaption={false}
            stale={Boolean(resourceProblem)}
            title="CPU time used during the latest shared reporting interval; - means no usable CPU sample was shared"
            value={formatOptionalPercent(cpuPercent)}
          />
          <PublicMetric
            caption={maximumCapacityCaption(resource?.memory_total_bytes)}
            icon={<Gauge size={15} />}
            label="RAM"
            context={maximumCapacity(resource?.memory_total_bytes)}
            percent={memoryUsed}
            showCaption={false}
            stale={Boolean(resourceProblem)}
            title="Used memory as a percentage of the maximum reported RAM capacity; - means memory evidence was not shared"
            value={formatOptionalPercent(memoryUsed)}
          />
          <PublicMetric
            caption={maximumCapacityCaption(resource?.disk_total_bytes)}
            icon={<Server size={15} />}
            label="Disk"
            context={maximumCapacity(resource?.disk_total_bytes)}
            percent={diskUsed}
            showCaption={false}
            stale={Boolean(resourceProblem)}
            title="Used disk space as a percentage of the maximum reported disk capacity; - means disk evidence was not shared"
            value={formatOptionalPercent(diskUsed)}
          />
          <PublicMetric
            caption={
              resource
                ? `5m ${formatLoad(resource.load_5)} · 15m ${formatLoad(resource.load_15)}`
                : "unavailable"
            }
            icon={<Gauge size={15} />}
            label="Load"
            percent={loadPressure}
            showCaption={density === "comfortable"}
            sparkline={
              density === "comfortable" ? (
                <MiniSparkline
                  label="Load history"
                  tone="load"
                  values={loadHistory}
                />
              ) : undefined
            }
            stale={Boolean(resourceProblem)}
            title="Linux load average divided by reported CPU cores; - means load evidence was not shared"
            value={
              resource
                ? density === "compact"
                  ? [resource.load_1, resource.load_5, resource.load_15]
                      .map(formatLoad)
                      .join("/")
                  : formatLoad(resource.load_1)
                : "-"
            }
          />
        </div>
      ) : null}

      {visibility?.network ? (
        <div
          aria-label={`Current network rate for ${card.display_name}`}
          className={`vpsMonitorFlowFacts publicMonitoringNetwork ${density}${networkProblem ? " stale" : ""}`}
        >
          <PublicFact
            icon={<Network size={13} />}
            label="RX"
            sparkline={
              <MiniSparkline label="RX activity" tone="rx" values={rxHistory} />
            }
            stale={Boolean(networkProblem)}
            title={publicNetworkRateTitle("received", card.network)}
            value={formatOptionalRate(card.network?.rx_bps)}
          />
          <PublicFact
            icon={<Network size={13} />}
            label="TX"
            sparkline={
              <MiniSparkline label="TX activity" tone="tx" values={txHistory} />
            }
            stale={Boolean(networkProblem)}
            title={publicNetworkRateTitle("sent", card.network)}
            value={formatOptionalRate(card.network?.tx_bps)}
          />
          {density === "comfortable" ? (
            <small className="publicMonitoringFreshness">
              {card.network?.rate_expected === false
                ? "Network rates not selected"
                : card.network?.observed_at
                  ? `Updated ${formatCompactTime(card.network.observed_at)}`
                  : "Network data unavailable"}
            </small>
          ) : null}
        </div>
      ) : null}

      {auxiliaryFacts.length ? (
        <div
          className="vpsMonitorAuxFacts publicMonitoringAuxFacts"
          aria-label={`Additional current facts for ${card.display_name}`}
        >
          {auxiliaryFacts.map((fact) => (
            <span
              data-fact-kind={fact.kind}
              key={fact.label}
              title={fact.title}
            >
              <small className="vpsMonitorAuxFactHeading">
                <b>{fact.label}</b>
                {fact.context ? <span> · {fact.context}</span> : null}
              </small>
              <strong>{fact.value}</strong>
            </span>
          ))}
        </div>
      ) : null}

      {visibility?.traffic ? <PublicTrafficRow traffic={card.traffic} /> : null}

      {visibility?.ping ? (
        <PublicPingRow
          density={density}
          history={card.primary_ping_history ?? []}
          ping={card.primary_ping}
        />
      ) : null}

      {density === "comfortable" && warnings.length ? (
        <div
          className="publicMonitoringWarnings"
          role="status"
          title="Derived from current shared resource, network, traffic, and Ping evidence"
        >
          <strong>Needs attention</strong>
          <span>{warnings.join(" · ")}</span>
        </div>
      ) : null}
    </VpsMonitorCardSurface>
  );
}

function PublicMetric({
  caption,
  context,
  icon,
  label,
  percent,
  showCaption,
  sparkline,
  stale = false,
  title,
  value,
}: {
  caption: string;
  context?: string;
  icon: ReactNode;
  label: string;
  percent: number | null;
  showCaption: boolean;
  sparkline?: ReactNode;
  stale?: boolean;
  title: string;
  value: string;
}) {
  return (
    <MonitorMetric
      context={context}
      icon={icon}
      label={label}
      meterCaption={caption}
      meterMax={100}
      meterValue={percent}
      showCaption={showCaption}
      sparkline={sparkline}
      stale={stale}
      title={title}
      value={value}
    />
  );
}

function PublicFact({
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
    <MonitorFact
      icon={icon}
      label={label}
      sparkline={sparkline}
      stale={stale}
      title={title}
      value={value}
    />
  );
}

function PublicTrafficRow({ traffic }: { traffic?: PublicTrafficMetric }) {
  if (!traffic) {
    return (
      <div
        className="publicMonitoringTraffic missing"
        aria-label="Traffic unavailable"
        title="Traffic is shared, but current traffic configuration and accounting evidence are unavailable"
      >
        <div className="publicMonitoringTrafficHeading">
          <small className="vpsMonitorRowHeading">
            <strong>Traffic</strong>
          </small>
          <span className="vpsMonitorRowEvidence">
            <strong>-</strong>
          </span>
        </div>
        <span
          aria-label="Traffic progress is unavailable"
          className="vpsMonitorMetricTrack missing"
          title="No traffic progress can be drawn without current accounting evidence"
        >
          <span />
        </span>
      </div>
    );
  }
  const quotaState = traffic.configured ? trafficQuotaState(traffic) : "unset";
  const percent = traffic.configured
    ? finiteNumber(traffic.cycle_percent)
    : null;
  const quotaPercent = quotaState === "finite" ? percent : null;
  const fill =
    quotaPercent === null ? 0 : Math.max(0, Math.min(100, quotaPercent));
  const problem = publicTrafficProblem(traffic);
  const portSpeed = traffic.port_speed?.display;
  const cycleSummary = traffic.configured ? trafficCycleSummary(traffic) : "";
  const resetContext = traffic.configured
    ? traffic.reset_day === -1
      ? null
      : traffic.cycle_end
        ? formatTrafficReset(traffic.cycle_end)
        : null
    : null;
  const trafficDetail = traffic.configured
    ? `RX ${formatOptionalBytes(traffic.diagnostic_rx_bytes)} · TX ${formatOptionalBytes(traffic.diagnostic_tx_bytes)}${problem ? ` · ${problem}` : traffic.reset_day !== -1 && traffic.cycle_end ? ` · resets ${formatCompactTime(traffic.cycle_end)}` : ""}`
    : "Authoritative traffic accounting is not configured for this VPS.";
  const trafficRowTitle =
    quotaState === "unlimited"
      ? `${trafficDetail} Traffic is accumulated without a finite quota; blue blocks distinguish unlimited accounting from an empty track.`
      : trafficDetail;
  return (
    <div
      aria-label={`Traffic: ${traffic.configured ? "configured" : "unconfigured"}`}
      className={`publicMonitoringTraffic ${safeClassToken(traffic.state)}${traffic.configured ? "" : " unconfigured"}${resetContext ? " contextual" : ""}${problem ? " warning" : ""}${quotaPercent !== null && quotaPercent > 100 ? " overQuota" : ""}`}
      title={trafficRowTitle}
    >
      <div className="publicMonitoringTrafficHeading">
        <small
          className="vpsMonitorRowHeading"
          title={
            traffic.configured
              ? `Traffic accounting${resetContext ? `; ${resetContext}` : ""}`
              : "Traffic accounting is unconfigured"
          }
        >
          <strong>Traffic</strong>
          {resetContext ? ` · ${resetContext}` : ""}
        </small>
        <span className="vpsMonitorRowEvidence">
          {portSpeed ? (
            <span
              className="publicMonitoringPortSpeed"
              title={`${portSpeed} port capacity; display value only—no shaping or enforcement is implied`}
            >
              {portSpeed}
            </span>
          ) : null}
          {cycleSummary ? (
            <strong>{cycleSummary}</strong>
          ) : (
            <strong>Unconfigured</strong>
          )}
        </span>
      </div>
      {quotaPercent !== null ? (
        <span
          aria-label={`Traffic quota use: ${formatPercent(quotaPercent)}`}
          aria-valuemax={100}
          aria-valuemin={0}
          aria-valuenow={fill}
          aria-valuetext={formatPercent(quotaPercent)}
          className="vpsMonitorMetricTrack"
          role="meter"
        >
          <span style={{ width: `${fill}%` }} />
        </span>
      ) : quotaState === "unlimited" ? (
        <span
          aria-label="Traffic quota is unlimited"
          className="vpsMonitorMetricTrack unlimitedTrafficTrack"
        >
          <span />
        </span>
      ) : (
        <span
          aria-label={
            traffic.configured
              ? "Traffic quota progress is unavailable"
              : "Traffic accounting is unconfigured"
          }
          className="vpsMonitorMetricTrack missing"
          title={
            traffic.configured
              ? "No finite or unlimited traffic quota is available"
              : "Traffic accounting is unconfigured; this empty track is not zero percent"
          }
        >
          <span />
        </span>
      )}
      <small>{trafficDetail}</small>
    </div>
  );
}

function PublicPingRow({
  density,
  history,
  ping,
}: {
  density: Density;
  history: PublicPingPoint[];
  ping?: PublicPingMetric;
}) {
  if (!ping) {
    return (
      <div
        className="publicMonitoringPing missing"
        aria-label="Primary Ping unconfigured"
        title="No primary Ping target is configured for this VPS; configure and share one to show latency, loss, and history."
      >
        <Radio aria-hidden="true" size={14} />
        <small className="vpsMonitorRowHeading">
          <strong>Ping</strong> · Unconfigured
        </small>
        <span>-</span>
        <MiniSparkline label="Primary Ping history" tone="ping" values={[]} />
      </div>
    );
  }
  const detail = [
    ping.latency_avg_ms === null
      ? "latency unavailable"
      : `${formatNumber(ping.latency_avg_ms)} ms`,
    ping.loss_ratio === null
      ? "loss unavailable"
      : `${formatPercent(ping.loss_ratio * 100)} loss`,
  ].join(" · ");
  const problem = publicPingProblem(ping);
  const effectiveStatus = publicPingEffectiveStatus(ping);
  const statusLabel = publicPingStatusLabel(effectiveStatus);
  const presentedDetail =
    effectiveStatus === "disabled" ? `Last sample ${detail}` : detail;
  const statusDetail =
    problem ??
    `${statusLabel}${ping.checked_at ? ` · ${formatCompactTime(ping.checked_at)}` : " · not checked"}`;
  return (
    <div
      aria-label={`Primary Ping ${ping.target_name}: ${statusLabel}`}
      className={`publicMonitoringPing ${safeClassToken(ping.state)} ${safeClassToken(effectiveStatus)}${problem ? " warning" : ""}`}
      title={`${ping.target_name}: ${presentedDetail}; ${statusDetail}`}
    >
      <Radio aria-hidden="true" size={14} />
      <small className="vpsMonitorRowHeading">
        <strong>Ping</strong> · {ping.target_name}
      </small>
      <span>{presentedDetail}</span>
      <MiniSparkline
        label={`${ping.target_name} latency history`}
        tone="ping"
        values={historyValues(history, (point) => point.latency_avg_ms)}
      />
      {density === "comfortable" ? <small>{statusDetail}</small> : null}
    </div>
  );
}

function PublicMonitoringDetailPanel({
  card,
  customEnd,
  customError,
  customStart,
  detail,
  error,
  loading,
  onApplyCustom,
  onClose,
  onCustomEndChange,
  onCustomStartChange,
  onWindowChange,
  window,
  visibility,
}: {
  card: PublicMonitoringCard;
  customEnd: string;
  customError: string | null;
  customStart: string;
  detail: PublicMonitoringDetail | null;
  error: string | null;
  loading: boolean;
  onApplyCustom: () => void;
  onClose: () => void;
  onCustomEndChange: (value: string) => void;
  onCustomStartChange: (value: string) => void;
  onWindowChange: (window: MonitoringWindow) => void;
  window: MonitoringWindow;
  visibility: PublicMonitoringShareView["visibility"] | undefined;
}) {
  const [section, setSection] = useHistoryEntryState<"resources" | "ping">(
    `public-monitoring.${card.client_key}.section`,
    "resources",
  );
  const [pingMetric, setPingMetric] = useHistoryEntryState<"latency" | "loss">(
    `public-monitoring.${card.client_key}.ping-metric`,
    "latency",
  );
  const [hiddenPingTargets, setHiddenPingTargets] = useHistoryEntryState<
    string[]
  >(`public-monitoring.${card.client_key}.hidden-ping-targets`, []);
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const resources = detail?.resources ?? [];
  const network = detail?.network ?? [];
  const traffic = detail?.traffic ?? [];
  const resourceTimeline = regularTimeline(resources, detail?.range);
  const networkTimeline = regularTimeline(network, detail?.range);
  const trafficTimeline = regularTimeline(traffic, detail?.range);
  const hasSwapHistory = resourceTimeline.records.some((point) => {
    if (!point) return false;
    const total = finiteNumber(point.swap_total_bytes);
    return (
      point.swap_sample_count > 0 &&
      total !== null &&
      total > 0 &&
      finiteNumber(point.swap_used_ratio_avg) !== null
    );
  });
  const memoryCapacityMaximum = maximumResourceCapacity(
    resourceTimeline.records,
    (point) => point.memory_total_bytes,
  );
  const swapCapacityMaximum = maximumResourceCapacity(
    resourceTimeline.records,
    (point) => point.swap_total_bytes,
  );
  const diskCapacityMaximum = maximumResourceCapacity(
    resourceTimeline.records,
    (point) => point.disk_total_bytes,
  );
  const hasTrafficHistory = traffic.some((point) =>
    [point.rx_bytes, point.tx_bytes, point.total_bytes].some(
      (value) => finiteNumber(value) !== null,
    ),
  );
  const currentTrafficConfigured = card.traffic?.configured === true;
  const trafficResetCount = traffic.reduce(
    (total, point) => total + Math.max(0, point.reset_count),
    0,
  );
  const pingTargetNames = Array.from(
    new Set([
      ...(detail?.ping_targets ?? []).map((target) => target.target_name),
      ...(detail?.ping ?? []).map((point) => point.target_name),
    ]),
  ).sort((left, right) => left.localeCompare(right));
  const hiddenPingTargetSet = new Set(hiddenPingTargets);
  const selectedPingTargetNames = pingTargetNames.filter(
    (targetName) => !hiddenPingTargetSet.has(targetName),
  );
  const pingTargetColors = stableTargetColors(pingTargetNames);
  const pingChart = pingChartData(detail?.ping ?? [], detail?.range);
  const pingLines =
    pingMetric === "latency" ? pingChart.latencyLines : pingChart.lossLines;
  const visiblePingSeriesCount = pingLines.filter(
    (line) =>
      selectedPingTargetNames.includes(line.seriesKey ?? line.label) &&
      line.values.some((value) => value !== null && Number.isFinite(value)),
  ).length;
  const exportPrefix = `shared-${card.client_key.slice(0, 12)}-${detail?.range.window ?? window}`;
  const visibleStatusLabel = publicCardVisibleStatusLabel(card, visibility);
  const freshness = publicCardFreshness(card, visibility);
  const country = visibility?.identity_context
    ? countryTagValue(card.tags ?? [])
    : null;
  const identitySummary = visibility?.identity_context
    ? publicIdentitySummary(card.tags ?? [])
    : "";
  const resourcesAvailable = Boolean(
    detail &&
    (detail.resources !== undefined ||
      detail.network !== undefined ||
      (detail.traffic !== undefined &&
        (currentTrafficConfigured || hasTrafficHistory))),
  );
  const pingAvailable = Boolean(
    detail && (detail.ping_targets !== undefined || detail.ping !== undefined),
  );
  const resourceSectionSummary = [
    detail?.resources !== undefined ? "resources" : null,
    detail?.network !== undefined ? "network" : null,
    detail?.traffic !== undefined &&
    (currentTrafficConfigured || hasTrafficHistory)
      ? "traffic"
      : null,
  ]
    .filter((label): label is string => Boolean(label))
    .join(" · ");
  useEffect(() => {
    headingRef.current?.focus({ preventScroll: false });
  }, [card.client_key]);
  useEffect(() => {
    if (section === "resources" && !resourcesAvailable && pingAvailable) {
      setSection("ping");
    } else if (section === "ping" && !pingAvailable && resourcesAvailable) {
      setSection("resources");
    }
  }, [pingAvailable, resourcesAvailable, section, setSection]);

  const togglePingTarget = (targetName: string) => {
    setHiddenPingTargets((current) =>
      current.includes(targetName)
        ? current.filter((candidate) => candidate !== targetName)
        : [...current, targetName],
    );
  };
  return (
    <section
      aria-busy={loading}
      aria-label={`Read-only history for ${card.display_name}`}
      className="publicMonitoringDetail consoleInlineDetail"
    >
      <header className="publicMonitoringDetailHeader">
        <div>
          <h2 ref={headingRef} tabIndex={-1}>
            {country ? (
              <CountryFlag country={country} decorative fallback="none" />
            ) : null}
            <span>{card.display_name || "Unnamed VPS"}</span>
          </h2>
          <p>
            {visibleStatusLabel}
            {freshness ? ` · Updated ${formatCompactTime(freshness)}` : ""}
            {` · Read-only history`}
            {identitySummary ? ` · ${identitySummary}` : ""}
            {detail
              ? ` · ${humanizeToken(detail.range.source)} source · ${detail.range.points} points`
              : ""}
          </p>
        </div>
        <button
          aria-label="Back to shared fleet"
          onClick={onClose}
          type="button"
        >
          Back to fleet
        </button>
      </header>
      <PublicMonitoringKpiStrip card={card} visibility={visibility} />
      <PublicMonitoringInformationGroups card={card} visibility={visibility} />
      <div className="publicMonitoringHistoryControls">
        {resourcesAvailable && pingAvailable ? (
          <div
            aria-label="Shared VPS detail section"
            className="dashboardSectionSelector publicMonitoringSectionSelector"
          >
            <button
              aria-pressed={section === "resources"}
              className={section === "resources" ? "active" : ""}
              onClick={() => setSection("resources")}
              title={`Show retained resource and network history for ${card.display_name || "this shared VPS"}`}
              type="button"
            >
              <strong>Resources</strong>
              <small>{resourceSectionSummary}</small>
            </button>
            <button
              aria-pressed={section === "ping"}
              className={section === "ping" ? "active" : ""}
              onClick={() => setSection("ping")}
              title={`Show shared Ping target latency and loss history for ${card.display_name || "this shared VPS"}`}
              type="button"
            >
              <strong>Ping</strong>
              <small>Targets · latency · loss</small>
            </button>
          </div>
        ) : null}
        <div className="observabilityMetricsControls publicMonitoringRangeControls">
          <MonitoringRangeTabs
            ariaLabel="History range"
            onChange={onWindowChange}
            value={window}
          />
        </div>
        {window === "custom" ? (
          <div className="publicMonitoringCustomRange">
            <label>
              <span>Start</span>
              <input
                id="public-monitoring-custom-start"
                name="public-monitoring-custom-start"
                onChange={(event) => onCustomStartChange(event.target.value)}
                type="datetime-local"
                value={customStart}
              />
            </label>
            <label>
              <span>End</span>
              <input
                id="public-monitoring-custom-end"
                name="public-monitoring-custom-end"
                onChange={(event) => onCustomEndChange(event.target.value)}
                type="datetime-local"
                value={customEnd}
              />
            </label>
            <button onClick={onApplyCustom} type="button">
              Apply range
            </button>
            {customError ? (
              <span className="panelError" role="alert">
                {customError}
              </span>
            ) : null}
          </div>
        ) : null}
      </div>
      {error ? (
        <p className="panelError" role="alert">
          {error}
        </p>
      ) : null}
      {loading ? (
        <div className="emptyState publicMonitoringDetailLoading">
          <Activity aria-hidden="true" size={20} />
          <strong>Loading history</strong>
          <span>Reading the retained monitoring range.</span>
        </div>
      ) : detail ? (
        <>
          {section === "resources" ? (
            <div className="publicMonitoringChartGroups">
              {detail.resources !== undefined ? (
                <section className="publicMonitoringChartGroup">
                  <header>
                    <strong>Resources</strong>
                    <small>Utilization, capacity, load, and connections</small>
                  </header>
                  <div className="dashboardWidgetGrid publicMonitoringDetailCharts">
                    <PublicChart
                      emptyLabel="CPU utilization is unavailable for this range"
                      exportFileName={`${exportPrefix}-cpu`}
                      label="CPU utilization"
                      lines={[
                        {
                          color: consolePalette.chart.blue,
                          label: "CPU",
                          values: resourceTimeline.records.map((point) =>
                            ratioToPercent(point?.cpu_usage_avg),
                          ),
                        },
                      ]}
                      summary={formatPercent(
                        ratioToPercent(card.resources?.cpu_usage_avg),
                      )}
                      times={resourceTimeline.times}
                      valueFormatter={(value) => formatPercent(value)}
                    />
                    <PublicChart
                      emptyLabel="Load history is unavailable for this range"
                      exportFileName={`${exportPrefix}-load`}
                      label="Load 1 / 5 / 15"
                      lines={[
                        {
                          color: consolePalette.chart.blue,
                          label: "Load 1",
                          values: resourceTimeline.records.map(
                            (point) => point?.load_1 ?? null,
                          ),
                        },
                        {
                          color: consolePalette.chart.green,
                          label: "Load 5",
                          values: resourceTimeline.records.map(
                            (point) => point?.load_5 ?? null,
                          ),
                        },
                        {
                          color: consolePalette.chart.orange,
                          label: "Load 15",
                          values: resourceTimeline.records.map(
                            (point) => point?.load_15 ?? null,
                          ),
                        },
                      ]}
                      summary={
                        card.resources
                          ? [
                              card.resources.load_1,
                              card.resources.load_5,
                              card.resources.load_15,
                            ]
                              .map(formatLoad)
                              .join(" / ")
                          : "-"
                      }
                      times={resourceTimeline.times}
                      valueFormatter={(value) =>
                        value === null ? "-" : formatNumber(value)
                      }
                    />
                    <PublicChart
                      emptyLabel="Memory history is unavailable for this range"
                      exportFileName={`${exportPrefix}-memory`}
                      label={
                        hasSwapHistory ? "Memory / swap used" : "Memory used"
                      }
                      lines={[
                        {
                          color: consolePalette.chart.purple,
                          label: "Memory used",
                          values: resourceTimeline.records.map((point) =>
                            point && point.memory_total_bytes > 0
                              ? point.memory_used_ratio_avg * 100
                              : null,
                          ),
                        },
                        ...(hasSwapHistory
                          ? [
                              {
                                color: consolePalette.chart.orange,
                                label: "Swap used",
                                values: resourceTimeline.records.map((point) =>
                                  point &&
                                  point.swap_sample_count > 0 &&
                                  (finiteNumber(point.swap_total_bytes) ?? 0) >
                                    0
                                    ? finiteNumber(
                                        point.swap_used_ratio_avg,
                                      ) === null
                                      ? null
                                      : (point.swap_used_ratio_avg ?? 0) * 100
                                    : null,
                                ),
                              },
                            ]
                          : []),
                      ]}
                      summary={memorySwapCapacityCaption(
                        memoryCapacityMaximum,
                        swapCapacityMaximum,
                        hasSwapHistory,
                      )}
                      times={resourceTimeline.times}
                      valueFormatter={(value) => formatPercent(value)}
                    />
                    <PublicChart
                      emptyLabel="Disk history is unavailable for this range"
                      exportFileName={`${exportPrefix}-disk`}
                      label="Aggregate reported disk used"
                      lines={[
                        {
                          color: consolePalette.chart.orange,
                          label: "Disk used",
                          values: resourceTimeline.records.map((point) =>
                            point && point.disk_total_bytes > 0
                              ? point.disk_used_ratio_avg * 100
                              : null,
                          ),
                        },
                      ]}
                      summary={maximumCapacityCaption(diskCapacityMaximum)}
                      times={resourceTimeline.times}
                      valueFormatter={(value) => formatPercent(value)}
                    />
                    <PublicChart
                      emptyLabel="TCP and UDP connection history is unavailable for this range"
                      exportFileName={`${exportPrefix}-connections`}
                      label="TCP / UDP connections"
                      lines={[
                        {
                          color: consolePalette.chart.purple,
                          label: "TCP",
                          values: resourceTimeline.records.map(
                            (point) => point?.tcp_sockets ?? null,
                          ),
                        },
                        {
                          color: consolePalette.chart.green,
                          label: "UDP",
                          values: resourceTimeline.records.map(
                            (point) => point?.udp_sockets ?? null,
                          ),
                        },
                      ]}
                      summary={`TCP ${formatPublicSocketCount(card.resources?.tcp_sockets)} · UDP ${formatPublicSocketCount(card.resources?.udp_sockets)}`}
                      times={resourceTimeline.times}
                      valueFormatter={(value) =>
                        value === null ? "-" : formatPublicSocketCount(value)
                      }
                      wide
                    />
                  </div>
                </section>
              ) : null}
              {detail.network !== undefined ||
              (detail.traffic !== undefined &&
                (currentTrafficConfigured || hasTrafficHistory)) ? (
                <section className="publicMonitoringChartGroup">
                  <header>
                    <strong>Network &amp; traffic</strong>
                    <small>
                      Live rates and authoritative accounting evidence
                    </small>
                  </header>
                  <div className="dashboardWidgetGrid publicMonitoringDetailCharts">
                    {detail.network !== undefined ? (
                      <PublicChart
                        emptyLabel="Network rate history is unavailable for this range"
                        exportFileName={`${exportPrefix}-network`}
                        label="Network RX / TX"
                        lines={[
                          {
                            color: consolePalette.chart.blue,
                            exportLabel: "RX (bps)",
                            label: "RX",
                            values: networkTimeline.records.map(
                              (point) => point?.rx_bps ?? null,
                            ),
                          },
                          {
                            color: consolePalette.chart.green,
                            exportLabel: "TX (bps)",
                            label: "TX",
                            values: networkTimeline.records.map(
                              (point) => point?.tx_bps ?? null,
                            ),
                          },
                        ]}
                        summary={`↓ ${formatOptionalRate(card.network?.rx_bps)} · ↑ ${formatOptionalRate(card.network?.tx_bps)}`}
                        times={networkTimeline.times}
                        valueFormatter={formatOptionalRate}
                      />
                    ) : null}
                    {detail.traffic !== undefined &&
                    (currentTrafficConfigured || hasTrafficHistory) ? (
                      <>
                        <PublicChart
                          emptyLabel={
                            currentTrafficConfigured
                              ? "Traffic volume history is unavailable for this range"
                              : "Prior traffic accounting history is unavailable for this range"
                          }
                          exportFileName={`${exportPrefix}-traffic`}
                          label={
                            currentTrafficConfigured
                              ? "Traffic volume"
                              : "Prior traffic accounting history"
                          }
                          lines={[
                            {
                              color: consolePalette.chart.orange,
                              initiallyHidden: true,
                              label: "Total volume",
                              values: trafficTimeline.records.map(
                                (point) => point?.total_bytes ?? null,
                              ),
                            },
                            {
                              color: consolePalette.chart.blue,
                              label: "RX volume",
                              values: trafficTimeline.records.map(
                                (point) => point?.rx_bytes ?? null,
                              ),
                            },
                            {
                              color: consolePalette.chart.green,
                              label: "TX volume",
                              values: trafficTimeline.records.map(
                                (point) => point?.tx_bytes ?? null,
                              ),
                            },
                          ]}
                          summary={
                            currentTrafficConfigured
                              ? `RX ${formatOptionalBytes(card.traffic?.diagnostic_rx_bytes)} · TX ${formatOptionalBytes(card.traffic?.diagnostic_tx_bytes)}`
                              : "Current accounting unconfigured"
                          }
                          times={trafficTimeline.times}
                          valueFormatter={(value) =>
                            value === null ? "-" : formatBytes(value)
                          }
                        />
                        {currentTrafficConfigured ? (
                          <PublicTrafficCycle
                            resetCount={trafficResetCount}
                            traffic={card.traffic}
                          />
                        ) : hasTrafficHistory ? (
                          <p className="publicMonitoringPriorTrafficNote">
                            Retained volume predates the current unconfigured
                            accounting state; it is historical evidence, not a
                            current cycle total.
                          </p>
                        ) : null}
                      </>
                    ) : null}
                  </div>
                </section>
              ) : null}
            </div>
          ) : null}
          {section === "ping" && pingAvailable ? (
            <section
              aria-label="Shared Ping history"
              className="publicMonitoringPingHistory"
            >
              <header className="publicMonitoringPingToolbar">
                <div>
                  <strong>Ping targets</strong>
                  <small>
                    {selectedPingTargetNames.length}/{pingTargetNames.length}{" "}
                    selected · missing samples remain gaps
                  </small>
                </div>
                <div className="publicMonitoringPingActions">
                  <div
                    className="segmented vpsMonitoringPingMetric"
                    role="group"
                    aria-label="Ping chart metric"
                  >
                    <button
                      aria-pressed={pingMetric === "latency"}
                      className={pingMetric === "latency" ? "selected" : ""}
                      onClick={() => setPingMetric("latency")}
                      title="Plot retained latency measurements for the selected shared Ping targets"
                      type="button"
                    >
                      Latency
                    </button>
                    <button
                      aria-pressed={pingMetric === "loss"}
                      className={pingMetric === "loss" ? "selected" : ""}
                      onClick={() => setPingMetric("loss")}
                      title="Plot retained packet-loss measurements for the selected shared Ping targets"
                      type="button"
                    >
                      Loss
                    </button>
                  </div>
                  <button
                    className="secondaryAction compactAction"
                    disabled={
                      selectedPingTargetNames.length === pingTargetNames.length
                    }
                    onClick={() => setHiddenPingTargets([])}
                    title={
                      selectedPingTargetNames.length === pingTargetNames.length
                        ? "Every shared Ping target is already selected"
                        : "Select every shared Ping target series"
                    }
                    type="button"
                  >
                    Select all
                  </button>
                  <button
                    className="secondaryAction compactAction"
                    disabled={selectedPingTargetNames.length === 0}
                    onClick={() => setHiddenPingTargets(pingTargetNames)}
                    title={
                      selectedPingTargetNames.length === 0
                        ? "No shared Ping target series is currently selected"
                        : "Hide every shared Ping target series"
                    }
                    type="button"
                  >
                    Select none
                  </button>
                </div>
              </header>
              <div
                aria-label="Selectable current Ping target evidence"
                className="publicMonitoringPingTargets"
              >
                {pingTargetNames.map((targetName) => {
                  const target = detail.ping_targets?.find(
                    (candidate) => candidate.target_name === targetName,
                  );
                  const status = target
                    ? publicPingEffectiveStatus(target)
                    : "unavailable";
                  const selected = !hiddenPingTargetSet.has(targetName);
                  const latency =
                    target?.latency_avg_ms === null || !target
                      ? "No latency"
                      : `${formatNumber(target.latency_avg_ms)} ms`;
                  const loss =
                    target?.loss_ratio === null || !target
                      ? "No loss evidence"
                      : formatPercent(target.loss_ratio * 100);
                  const samplePrefix =
                    status === "disabled" ? "Last sample: " : "";
                  return (
                    <button
                      aria-label={`${selected ? "Hide" : "Show"} ${targetName} Ping history. ${publicPingStatusLabel(status)}. ${samplePrefix}${latency}. ${loss}`}
                      aria-pressed={selected}
                      className={`publicMonitoringPingTarget${selected ? " selected" : ""}`}
                      key={targetName}
                      onClick={() => togglePingTarget(targetName)}
                      type="button"
                    >
                      <span>
                        <i
                          style={{
                            background:
                              pingTargetColors.get(targetName) ??
                              consolePalette.chart.neutral,
                          }}
                        />
                        <strong title={targetName}>{targetName}</strong>
                        {card.primary_ping?.target_name === targetName ? (
                          <em>Primary</em>
                        ) : null}
                      </span>
                      <ConsoleStatusBadge tone={publicPingTone(status)}>
                        {publicPingStatusLabel(status)}
                      </ConsoleStatusBadge>
                      <small>
                        {samplePrefix}
                        {latency}
                        {` · ${loss}`}
                      </small>
                    </button>
                  );
                })}
                {!pingTargetNames.length ? (
                  <p className="dashboardEmptyChart">
                    No Ping targets are assigned to this VPS.
                  </p>
                ) : null}
              </div>
              <PublicChart
                allowNoVisibleSeries
                emptyLabel={
                  pingTargetNames.length && !selectedPingTargetNames.length
                    ? "Select at least one Ping target to display its history"
                    : `Ping ${pingMetric} history is unavailable for this range`
                }
                exportFileName={`${exportPrefix}-ping-${pingMetric}`}
                height={190}
                label={
                  pingMetric === "latency" ? "Ping latency" : "Ping packet loss"
                }
                lines={pingLines}
                onVisibleSeriesKeysChange={(visibleTargetNames) =>
                  setHiddenPingTargets(
                    pingTargetNames.filter(
                      (targetName) => !visibleTargetNames.includes(targetName),
                    ),
                  )
                }
                summary={`${visiblePingSeriesCount} series with data`}
                times={pingChart.times}
                valueFormatter={
                  pingMetric === "latency"
                    ? (value) =>
                        value === null ? "-" : `${formatNumber(value)} ms`
                    : (value) => formatPercent(value)
                }
                visibleSeriesKeys={selectedPingTargetNames}
                wide
              />
            </section>
          ) : null}
        </>
      ) : null}
    </section>
  );
}

function PublicMonitoringKpiStrip({
  card,
  visibility,
}: {
  card: PublicMonitoringCard;
  visibility: PublicMonitoringShareView["visibility"] | undefined;
}) {
  const resource = card.resources;
  const facts: Array<{
    context?: string;
    detail: string;
    kind: "billing" | "connection" | "ping" | "traffic" | "uptime";
    label: string;
    value: string;
  }> = [];
  if (visibility?.billing) {
    const renewal = formatBillingRenewal(
      card.billing?.cycle,
      card.billing?.period_code,
    );
    const billingValue =
      card.billing && !card.billing.disabled ? card.billing.display : "-";
    facts.push({
      context: renewal ?? undefined,
      detail: card.billing
        ? card.billing.disabled
          ? "Billing is explicitly disabled"
          : (renewal ?? "Configured billing rule")
        : "Billing is shared but no billing rule is configured; - is shown",
      kind: "billing",
      label: "Billing",
      value: billingValue,
    });
  }
  if (visibility?.system_information) {
    facts.push({
      detail: card.system_information?.uptime_observed_at
        ? `Observed ${formatCompactTime(card.system_information.uptime_observed_at)}`
        : "Latest reported uptime is unavailable",
      kind: "uptime",
      label: "Uptime",
      value: formatUptime(card.system_information?.uptime_secs),
    });
  }
  const traffic = card.traffic;
  const trafficTotal = finiteNumber(traffic?.diagnostic_total_bytes);
  if (
    visibility?.traffic &&
    traffic?.configured &&
    trafficTotal !== null &&
    trafficTotal >= 0
  ) {
    facts.push({
      detail: `${trafficCycleSummary(traffic)}${traffic.reset_day !== -1 && traffic.cycle_end ? ` · resets ${formatCompactTime(traffic.cycle_end)}` : ""}`,
      kind: "traffic",
      label: "Traffic",
      value: formatBytes(trafficTotal),
    });
  }
  const connections = [
    resource?.tcp_sockets === null || resource?.tcp_sockets === undefined
      ? null
      : `TCP ${formatPublicSocketCount(resource.tcp_sockets)}`,
    resource?.udp_sockets === null || resource?.udp_sockets === undefined
      ? null
      : `UDP ${formatPublicSocketCount(resource.udp_sockets)}`,
  ].filter((value): value is string => Boolean(value));
  if (visibility?.resources && connections.length) {
    facts.push({
      detail: resource?.connections_observed_at
        ? `Observed ${formatCompactTime(resource.connections_observed_at)}`
        : "Latest socket-table evidence is unavailable",
      kind: "connection",
      label: "Connections",
      value: connections.join(" · "),
    });
  }
  if (visibility?.ping && card.primary_ping) {
    const ping = card.primary_ping;
    const status = publicPingEffectiveStatus(ping);
    const latency =
      ping.latency_avg_ms === null
        ? null
        : `${formatNumber(ping.latency_avg_ms)} ms`;
    facts.push({
      detail: `${publicPingStatusLabel(status)} · ${latency ?? "No latency"} · ${ping.loss_ratio === null ? "No loss evidence" : `${formatPercent(ping.loss_ratio * 100)} loss`}`,
      kind: "ping",
      label: "Primary Ping",
      value: `${ping.target_name} · ${status === "disabled" ? "Disabled" : (latency ?? publicPingStatusLabel(status))}`,
    });
  }
  if (!facts.length) return null;
  return (
    <div
      aria-label="Current shared VPS evidence"
      className="publicMonitoringKpiStrip"
    >
      {facts.map((fact) => (
        <span data-fact-kind={fact.kind} key={fact.label} title={fact.detail}>
          <small className="publicMonitoringKpiLabel">
            <b>{fact.label}</b>
            {fact.context ? <span> · {fact.context}</span> : null}
          </small>
          <strong>{fact.value}</strong>
        </span>
      ))}
    </div>
  );
}

function PublicMonitoringInformationGroups({
  card,
  visibility,
}: {
  card: PublicMonitoringCard;
  visibility: PublicMonitoringShareView["visibility"] | undefined;
}) {
  const resource = card.resources;
  const information = card.system_information;
  type InformationFact = { label: string; title?: string; value: string };
  const groups: Array<{ facts: InformationFact[]; label: string }> = [];
  const hardware: InformationFact[] = [];
  if (visibility?.system_information && information?.cpu_model) {
    hardware.push({
      label: "CPU",
      title: "CPU model reported by the agent",
      value: information.cpu_model,
    });
  }
  if (visibility?.resources && resource && resource.cpu_cores > 0) {
    hardware.push({
      label: "Cores",
      title: "Maximum CPU core capacity reported in current resource evidence",
      value: resource.cpu_cores.toLocaleString(),
    });
  }
  if (visibility?.resources && resource && resource.memory_total_bytes > 0) {
    hardware.push({
      label: "RAM",
      title: "Average used-memory percentage and maximum reported RAM capacity",
      value: resourceUsageSummary(
        resource.memory_used_ratio_avg,
        resource.memory_total_bytes,
      ),
    });
  }
  if (
    visibility?.resources &&
    resource &&
    finiteNumber(resource.swap_total_bytes) !== null
  ) {
    hardware.push({
      label: "Swap",
      title: "Average used-swap percentage and maximum reported swap capacity",
      value:
        resource.swap_total_bytes === 0
          ? "None"
          : resourceUsageSummary(
              resource.swap_used_ratio_avg,
              resource.swap_total_bytes,
            ),
    });
  }
  if (hardware.length) groups.push({ facts: hardware, label: "Hardware" });

  const system: InformationFact[] = [];
  if (visibility?.system_information && information) {
    if (information.os_name)
      system.push({
        label: "OS",
        title: "Operating system reported by the agent",
        value: information.os_name,
      });
    if (information.kernel_release)
      system.push({
        label: "Kernel",
        title: "Kernel release reported by the agent",
        value: information.kernel_release,
      });
    if (information.architecture)
      system.push({
        label: "Architecture",
        title: "Machine architecture reported by the agent",
        value: information.architecture,
      });
    if (information.virtualization)
      system.push({
        label: "Virtualization",
        title: "Virtualization environment reported by the agent",
        value: formatVirtualizationLabel(information.virtualization),
      });
  }
  if (system.length) groups.push({ facts: system, label: "System" });

  if (visibility?.resources && resource && resource.disk_total_bytes > 0) {
    groups.push({
      facts: [
        {
          label: "Reported filesystems",
          title:
            "Average used-disk percentage and maximum aggregate filesystem capacity",
          value: resourceUsageSummary(
            resource.disk_used_ratio_avg,
            resource.disk_total_bytes,
          ),
        },
      ],
      label: "Storage",
    });
  }

  const network: InformationFact[] = [];
  if (visibility?.network && card.network && card.network.rx_bps !== null) {
    network.push({
      label: "RX",
      title: publicNetworkRateTitle("received", card.network),
      value: formatOptionalRate(card.network.rx_bps),
    });
  }
  if (visibility?.network && card.network && card.network.tx_bps !== null) {
    network.push({
      label: "TX",
      title: publicNetworkRateTitle("sent", card.network),
      value: formatOptionalRate(card.network.tx_bps),
    });
  }
  if (visibility?.traffic && card.traffic?.port_speed) {
    network.push({
      label: "Port",
      title: "Display capacity only; no shaping or enforcement is implied",
      value: card.traffic.port_speed.display,
    });
  }
  if (network.length) groups.push({ facts: network, label: "Network" });
  if (!groups.length) return null;
  return (
    <div
      aria-label="Shared VPS system information"
      className="publicMonitoringInformationGroups"
    >
      {groups.map((group) => (
        <section
          key={group.label}
          title={`${group.label} information shared for this VPS`}
        >
          <h3>{group.label}</h3>
          <dl>
            {group.facts.map((fact) => (
              <div key={fact.label} title={fact.title}>
                <dt>{fact.label}</dt>
                <dd>{fact.value}</dd>
              </div>
            ))}
          </dl>
        </section>
      ))}
    </div>
  );
}

function PublicTrafficCycle({
  resetCount,
  traffic,
}: {
  resetCount: number;
  traffic?: PublicTrafficMetric;
}) {
  if (!traffic?.configured) return null;
  const percent = finiteNumber(traffic?.cycle_percent);
  const quotaState = trafficQuotaState(traffic);
  const quotaPercent = quotaState === "finite" ? percent : null;
  const fill =
    quotaPercent === null ? 0 : Math.max(0, Math.min(100, quotaPercent));
  const overQuota = quotaPercent !== null && quotaPercent > 100;
  const limitingQuota = trafficLimitingQuota(traffic);
  const limitValue = limitingQuota
    ? `${limitingQuota.direction} ${formatBytes(limitingQuota.quota)}`
    : quotaState === "finite"
      ? "-"
      : quotaState === "unlimited"
        ? "Unlimited"
        : "-";
  const cycleWindow =
    traffic.reset_day === -1
      ? "Accumulated total"
      : traffic.cycle_start && traffic.cycle_end
        ? `${formatCompactTime(traffic.cycle_start)} – ${formatCompactTime(traffic.cycle_end)}`
        : "Current accounting cycle";
  return (
    <div
      className="dashboardWidgetChart vpsMonitoringTrafficCycle publicMonitoringTrafficCycle"
      title={
        traffic.reset_day === -1
          ? "Traffic totals accumulate across retained accounting evidence and do not reset"
          : "Traffic totals and quota progress for the current reset cycle"
      }
    >
      <div className="dashboardWidgetHeader publicMonitoringTrafficCycleHeader">
        <div>
          <strong>
            {traffic.reset_day === -1 ? "Traffic" : "Traffic cycle"}
          </strong>
          <small>{cycleWindow}</small>
        </div>
        {traffic.port_speed ? (
          <span
            className="publicMonitoringPortSpeed"
            title="Display capacity only; no shaping or enforcement is implied"
          >
            {traffic.port_speed.display}
          </span>
        ) : null}
      </div>
      <div className="vpsMonitoringTrafficSummary">
        <PublicTrafficCycleFact
          label="Observed RX"
          title={trafficQuotaTitle("RX", traffic.quota_rx_bytes)}
          value={formatOptionalBytes(traffic.diagnostic_rx_bytes)}
        />
        <PublicTrafficCycleFact
          label="Observed TX"
          title={trafficQuotaTitle("TX", traffic.quota_tx_bytes)}
          value={formatOptionalBytes(traffic.diagnostic_tx_bytes)}
        />
        <PublicTrafficCycleFact
          label="Counted total"
          title="Traffic included by the configured selector directions; this value drives quota progress"
          value={formatOptionalBytes(traffic.total_bytes)}
        />
        <PublicTrafficCycleFact
          label="Limit"
          title={
            limitingQuota
              ? `${limitingQuota.direction} is the most-used finite quota at ${formatPercent(limitingQuota.percent)}`
              : "No finite limiting quota is available"
          }
          value={limitValue}
        />
      </div>
      {quotaPercent !== null ? (
        <div
          className={`vpsMonitoringTrafficProgress${overQuota ? " overLimit" : ""}`}
          title={`${formatPercent(quotaPercent)} of the limiting traffic quota has been used`}
        >
          <span
            aria-label={`${formatPercent(quotaPercent)} of the limiting traffic quota used`}
            aria-valuemax={100}
            aria-valuemin={0}
            aria-valuenow={fill}
            aria-valuetext={formatPercent(quotaPercent)}
            className="vpsMonitoringTrafficTrack"
            role="progressbar"
          >
            <i style={{ width: `${fill}%` }} />
          </span>
          <strong>{formatPercent(quotaPercent)}</strong>
          <small>{overQuota ? "Quota exceeded" : "Limit used"}</small>
        </div>
      ) : quotaState === "unlimited" ? (
        <div
          className="vpsMonitoringTrafficProgress unlimited"
          title="Traffic is accumulated without a finite quota; blue blocks distinguish unlimited accounting from empty progress"
        >
          <span
            aria-label="Traffic quota is unlimited"
            className="vpsMonitoringTrafficTrack unlimitedTrafficTrack"
          >
            <i />
          </span>
          <strong>Unlimited</strong>
          <small>No traffic limit</small>
        </div>
      ) : quotaState === "finite" ? (
        <p className="vpsMonitoringTrafficNote incomplete">
          Limiting quota utilization is unavailable.
        </p>
      ) : (
        <p className="vpsMonitoringTrafficNote incomplete">
          Accounting is active without a quota.
        </p>
      )}
      {traffic.state !== "ok" || resetCount > 0 ? (
        <p className="vpsMonitoringTrafficNote">
          {traffic.state !== "ok"
            ? `Evidence is ${humanizeToken(traffic.state).toLocaleLowerCase()}.`
            : ""}
          {resetCount > 0
            ? ` ${resetCount} counter-reset ${resetCount === 1 ? "interval was" : "intervals were"} excluded.`
            : ""}
        </p>
      ) : null}
    </div>
  );
}

function PublicTrafficCycleFact({
  label,
  title,
  value,
}: {
  label: string;
  title: string;
  value: string;
}) {
  return (
    <span title={title}>
      <small>{label}</small>
      <strong>{value}</strong>
    </span>
  );
}

function trafficQuotaTitle(label: string, quota: number | undefined): string {
  if (quota === -1) return `${label} quota is unlimited`;
  if (quota === undefined) return `${label} quota is not configured`;
  return `${label} quota ${formatBytes(quota)}`;
}

function PublicChart({
  allowNoVisibleSeries = false,
  emptyLabel,
  exportFileName,
  height = 170,
  label,
  lines,
  onVisibleSeriesKeysChange,
  summary,
  times,
  valueFormatter,
  visibleSeriesKeys,
  wide = false,
}: {
  allowNoVisibleSeries?: boolean;
  emptyLabel: string;
  exportFileName?: string;
  height?: number;
  label: string;
  lines: TimeSeriesChartLine[];
  onVisibleSeriesKeysChange?: (seriesKeys: string[]) => void;
  summary?: string;
  times: string[];
  valueFormatter: (value: number | null) => string;
  visibleSeriesKeys?: readonly string[];
  wide?: boolean;
}) {
  return (
    <section
      aria-label={`${label} chart`}
      className={`dashboardWidgetChart publicMonitoringChart${wide ? " wideWidget" : ""}`}
      title={`${label}: ${summary && summary !== "-" ? summary : emptyLabel}`}
    >
      <div className="dashboardWidgetHeader">
        <h3 title={`${label} retained shared monitoring chart`}>{label}</h3>
        {summary ? <small>{summary}</small> : null}
      </div>
      <TimeSeriesChart
        allowNoVisibleSeries={allowNoVisibleSeries}
        ariaLabel={`${label} shared monitoring chart`}
        emptyLabel={emptyLabel}
        exportFileName={exportFileName}
        height={height}
        lines={lines}
        onVisibleSeriesKeysChange={onVisibleSeriesKeysChange}
        times={times}
        valueFormatter={valueFormatter}
        visibleSeriesKeys={visibleSeriesKeys}
      />
    </section>
  );
}

function SummaryFact({ label, value }: { label: string; value: number }) {
  return (
    <span
      className={`publicMonitoringSummaryFact ${label.toLocaleLowerCase()}`}
    >
      <strong>{value}</strong>
      <small>{label}</small>
    </span>
  );
}

function pingChartData(
  points: PublicPingPoint[],
  range: MonitoringRange | undefined,
): {
  latencyLines: TimeSeriesChartLine[];
  lossLines: TimeSeriesChartLine[];
  times: string[];
} {
  const times = regularTimeline([], range).times;
  const stepSecs = range ? Math.max(60, Math.round(range.step_secs)) : 60;
  const firstSlot = range
    ? Math.floor(range.start_unix / stepSecs) * stepSecs
    : 0;
  const targetNames = Array.from(
    new Set(points.map((point) => point.target_name)),
  ).sort();
  const targetColors = stableTargetColors(targetNames);
  const targetValues = (
    targetName: string,
    value: (point: PublicPingPoint) => number | null,
  ) => {
    const values = times.map(() => null as number | null);
    const timestamps = times.map(() => Number.NEGATIVE_INFINITY);
    for (const point of points) {
      if (point.target_name !== targetName) continue;
      const timestamp = Math.floor(timestampMillis(point.bucket_start) / 1_000);
      if (!Number.isFinite(timestamp)) continue;
      const slot = Math.floor(timestamp / stepSecs) * stepSecs;
      const index = Math.round((slot - firstSlot) / stepSecs);
      if (index < 0 || index >= values.length || timestamp < timestamps[index])
        continue;
      values[index] = finiteNumber(value(point));
      timestamps[index] = timestamp;
    }
    return values;
  };
  const latencyLines = targetNames.map((targetName) => {
    return {
      color: targetColors.get(targetName) ?? consolePalette.chart.neutral,
      label: targetName,
      seriesKey: targetName,
      values: targetValues(targetName, (point) => point.latency_avg_ms),
    };
  });
  const lossLines = targetNames.map((targetName) => {
    return {
      color: targetColors.get(targetName) ?? consolePalette.chart.neutral,
      label: targetName,
      seriesKey: targetName,
      values: targetValues(targetName, (point) => point.loss_ratio * 100),
    };
  });
  return { latencyLines, lossLines, times };
}

function stableTargetColors(targetNames: string[]): Map<string, string> {
  const used = new Set<number>();
  return new Map<string, string>(
    targetNames.map((targetName) => {
      let hash = 2_166_136_261;
      for (const character of targetName) {
        hash ^= character.codePointAt(0) ?? 0;
        hash = Math.imul(hash, 16_777_619);
      }
      const preferred = (hash >>> 0) % dashboardChartColors.length;
      let colorIndex = preferred;
      for (let offset = 0; offset < dashboardChartColors.length; offset += 1) {
        const candidate = (preferred + offset) % dashboardChartColors.length;
        if (!used.has(candidate)) {
          colorIndex = candidate;
          break;
        }
      }
      used.add(colorIndex);
      return [targetName, dashboardChartColors[colorIndex]] as const;
    }),
  );
}

async function bootstrapMonitoringShare(
  shareId: string,
  secret: string,
): Promise<PublicMonitoringShareBootstrapView> {
  const requestKey = `${shareId}\u0000${secret}`;
  const existing = bootstrapRequests.get(requestKey);
  if (existing) return existing;
  const request = publicShareJson<PublicMonitoringShareBootstrapView>(
    `/api/v1/public/monitoring-shares/${encodeURIComponent(shareId)}/bootstrap`,
    secret,
  ).finally(() => bootstrapRequests.delete(requestKey));
  bootstrapRequests.set(requestKey, request);
  return request;
}

async function publicShareJson<T>(
  path: string,
  secret: string,
  signal?: AbortSignal,
  visitorId?: string,
): Promise<T> {
  const response = await apiFetch(path, {
    cache: "no-store",
    credentials: "same-origin",
    headers: {
      "x-vpsman-share-token": secret,
      ...(visitorId ? { "x-vpsman-share-visitor": visitorId } : {}),
    },
    signal,
  });
  if (response.status === 404 || response.status === 410) {
    throw new Error("This shared view link is invalid, expired, or revoked.");
  }
  if (!response.ok) throw await apiErrorFromResponse(response);
  return apiJsonFromResponse<T>(response, `GET ${path}`);
}

function publicDataPath(shareId: string, params: URLSearchParams): string {
  return `/api/v1/public/monitoring-shares/${encodeURIComponent(shareId)}/data?${params.toString()}`;
}

function publicShareClientKeyFromLocation(
  expectedShareId: string,
  expectedSecret: string,
): string | null | undefined {
  const match = globalThis.location.hash.match(
    /^#\/share\/([^/]+)\/([^/]+)(?:\/vps\/([^/]+))?$/,
  );
  if (!match) return undefined;
  try {
    if (
      decodeURIComponent(match[1]) !== expectedShareId ||
      decodeURIComponent(match[2]) !== expectedSecret
    ) {
      return undefined;
    }
    return match[3] ? decodeURIComponent(match[3]) : null;
  } catch {
    return undefined;
  }
}

function publicShareUrl(
  shareId: string,
  secret: string,
  clientKey?: string,
): string {
  const detail = clientKey ? `/vps/${encodeURIComponent(clientKey)}` : "";
  return `${globalThis.location.pathname}${globalThis.location.search}#/share/${encodeURIComponent(shareId)}/${encodeURIComponent(secret)}${detail}`;
}

function summarizeCards(
  cards: PublicMonitoringCard[],
  visibility: PublicMonitoringShareView["visibility"] | undefined,
) {
  return cards.reduce(
    (summary, card) => {
      summary.total += 1;
      summary[publicCardStatusGroup(card, visibility)] += 1;
      return summary;
    },
    { offline: 0, online: 0, total: 0, warning: 0 },
  );
}

function summarizePublicFleet(
  cards: PublicMonitoringCard[],
  visibility: PublicMonitoringShareView["visibility"] | undefined,
) {
  const locations = visibility?.identity_context
    ? { values: new Set<string>(), unspecified: 0 }
    : null;
  const network = visibility?.network
    ? { freshCount: 0, rxBps: 0, txBps: 0 }
    : null;
  const traffic = visibility?.traffic ? { bytes: 0, count: 0 } : null;
  for (const card of cards) {
    if (locations) {
      const location =
        countryTagValue(card.tags ?? []) ??
        publicTagValue(card.tags ?? [], "region");
      if (location) locations.values.add(location);
      else locations.unspecified += 1;
    }
    if (
      network &&
      statusGroup(card.status) === "online" &&
      publicFreshnessProblem(card.network?.observed_at, "Network telemetry") ===
        null
    ) {
      network.rxBps += finiteNumber(card.network?.rx_bps) ?? 0;
      network.txBps += finiteNumber(card.network?.tx_bps) ?? 0;
      network.freshCount += 1;
    }
    if (traffic && card.traffic?.configured) {
      traffic.bytes += Math.max(
        0,
        finiteNumber(card.traffic.diagnostic_total_bytes) ?? 0,
      );
      traffic.count += 1;
    }
  }
  return {
    locations: locations
      ? {
          unspecified: locations.unspecified,
          values: Array.from(locations.values).sort((left, right) =>
            left.localeCompare(right),
          ),
        }
      : null,
    network: network
      ? {
          freshCount: network.freshCount,
          rxBps: network.freshCount > 0 ? network.rxBps : null,
          txBps: network.freshCount > 0 ? network.txBps : null,
        }
      : null,
    traffic,
    visible: Boolean(locations || network || traffic),
  };
}

function publicTagValues(tags: string[], key: string) {
  const prefix = `${key}:`;
  return Array.from(
    new Set(
      tags.flatMap((tag) => {
        if (!tag.toLocaleLowerCase().startsWith(prefix)) return [];
        const value = tag.slice(prefix.length).trim();
        return value ? [value] : [];
      }),
    ),
  );
}

function publicTagValue(tags: string[], key: string) {
  return publicTagValues(tags, key)[0] ?? null;
}

function publicIdentitySummary(tags: string[]) {
  return ["provider", "region", "country"]
    .flatMap((key) => publicTagValues(tags, key))
    .join(" · ");
}

function comparePublicMonitoringCards(
  left: PublicMonitoringCard,
  right: PublicMonitoringCard,
  mode: PublicMonitorSort,
  visibility: PublicMonitoringShareView["visibility"] | undefined,
): number {
  const name = () =>
    (left.display_name || "Unnamed VPS").localeCompare(
      right.display_name || "Unnamed VPS",
    );
  const provider = (card: PublicMonitoringCard) =>
    publicTagValues(card.tags ?? [], "provider")[0] ?? "provider unset";
  const region = (card: PublicMonitoringCard) =>
    countryTagValue(card.tags ?? []) ??
    publicTagValues(card.tags ?? [], "region")[0] ??
    "region unset";
  if (mode === "provider") {
    return (
      provider(left).localeCompare(provider(right)) ||
      region(left).localeCompare(region(right)) ||
      name()
    );
  }
  if (mode === "region") {
    return (
      region(left).localeCompare(region(right)) ||
      provider(left).localeCompare(provider(right)) ||
      name()
    );
  }
  const statusRank = (card: PublicMonitoringCard) => {
    switch (publicCardStatusGroup(card, visibility)) {
      case "offline":
        return 3;
      case "warning":
        return 2;
      default:
        return 0;
    }
  };
  const statusDelta = statusRank(right) - statusRank(left);
  if (mode === "warning" && statusDelta !== 0) return statusDelta;
  const trafficUse = (card: PublicMonitoringCard) => {
    if (!card.traffic?.configured) return -1;
    return (
      trafficLimitingQuota(card.traffic)?.percent ??
      finiteNumber(card.traffic.cycle_percent) ??
      -1
    );
  };
  const leftTraffic = trafficUse(left);
  const rightTraffic = trafficUse(right);
  if (mode === "traffic" && rightTraffic !== leftTraffic) {
    return rightTraffic - leftTraffic;
  }
  const cpuUse = (card: PublicMonitoringCard) =>
    finiteNumber(card.resources?.cpu_usage_avg) ?? -1;
  const leftCpu = cpuUse(left);
  const rightCpu = cpuUse(right);
  if (mode === "cpu" && rightCpu !== leftCpu) return rightCpu - leftCpu;
  const memoryUse = (card: PublicMonitoringCard) =>
    card.resources && card.resources.memory_total_bytes > 0
      ? (ratioToPercent(card.resources.memory_used_ratio_avg) ?? -1)
      : -1;
  const leftMemory = memoryUse(left);
  const rightMemory = memoryUse(right);
  if (mode === "memory" && rightMemory !== leftMemory) {
    return rightMemory - leftMemory;
  }
  if (statusDelta !== 0) return statusDelta;
  const networkUse = (card: PublicMonitoringCard) =>
    Math.max(0, finiteNumber(card.network?.rx_bps) ?? 0) +
    Math.max(0, finiteNumber(card.network?.tx_bps) ?? 0);
  const networkDelta = networkUse(right) - networkUse(left);
  if (networkDelta !== 0) return networkDelta;
  if (rightCpu !== leftCpu) return rightCpu - leftCpu;
  return name();
}

function formatPublicLocationSummary(locations: string[], unspecified: number) {
  if (!locations.length) {
    return unspecified ? `${unspecified} unspecified` : "No VPS locations";
  }
  const names = locations.slice(0, 2).join(", ");
  const hidden = locations.length - Math.min(2, locations.length);
  const known = hidden ? `${names} +${hidden}` : names;
  return unspecified ? `${known} · ${unspecified} unspecified` : known;
}

function historyValues<T extends { bucket_start: string; bucket_secs: number }>(
  records: T[],
  value: (record: T) => number | null,
) {
  const rows = [...records]
    .sort(
      (left, right) =>
        timestampMillis(left.bucket_start) -
        timestampMillis(right.bucket_start),
    )
    .slice(-18);
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
    values.push(finiteNumber(value(row)));
    previous = row;
  }
  return values;
}

function regularTimeline<T extends { bucket_start: string }>(
  records: T[],
  range: MonitoringRange | undefined,
): { records: Array<T | null>; times: string[] } {
  if (!range) return { records: [], times: [] };
  const stepSecs = Math.max(60, Math.round(range.step_secs));
  const firstSlot = Math.floor(range.start_unix / stepSecs) * stepSecs;
  const lastSlot = Math.floor(range.end_unix / stepSecs) * stepSecs;
  const recordsBySlot = new Map<number, { record: T; timestamp: number }>();
  for (const record of records) {
    const timestamp = Math.floor(timestampMillis(record.bucket_start) / 1_000);
    if (!Number.isFinite(timestamp)) continue;
    const slot = Math.floor(timestamp / stepSecs) * stepSecs;
    const existing = recordsBySlot.get(slot);
    if (!existing || timestamp >= existing.timestamp) {
      recordsBySlot.set(slot, { record, timestamp });
    }
  }
  const times: string[] = [];
  const regularRecords: Array<T | null> = [];
  for (let slot = firstSlot; slot <= lastSlot; slot += stepSecs) {
    times.push(new Date(slot * 1_000).toISOString());
    regularRecords.push(recordsBySlot.get(slot)?.record ?? null);
  }
  return { records: regularRecords, times };
}

function statusGroup(status: string): Exclude<CardStatusFilter, "all"> {
  const normalized = status.trim().toLocaleLowerCase();
  if (normalized === "online") return "online";
  if (normalized === "offline" || normalized === "disconnected")
    return "offline";
  return "warning";
}

function publicFreshnessProblem(
  observedAt: string | null | undefined,
  label: string,
): string | null {
  if (!observedAt) return `${label} unavailable`;
  const timestamp = timestampMillis(observedAt);
  if (!Number.isFinite(timestamp)) return `${label} timestamp invalid`;
  return Date.now() - timestamp > 3 * 60_000 ? `${label} stale` : null;
}

function publicCardHasVisibleTelemetry(
  visibility: PublicMonitoringShareView["visibility"] | undefined,
): boolean {
  return Boolean(
    visibility?.resources ||
    visibility?.network ||
    visibility?.traffic ||
    visibility?.ping,
  );
}

function publicCardFreshness(
  card: PublicMonitoringCard,
  visibility: PublicMonitoringShareView["visibility"] | undefined,
): string | null {
  const candidates = [
    visibility?.resources ? card.resources?.observed_at : null,
    visibility?.network ? card.network?.observed_at : null,
    visibility?.traffic ? card.traffic?.observed_at : null,
    visibility?.ping ? card.primary_ping?.checked_at : null,
  ].filter((value): value is string => Boolean(value));
  let latest: { timestamp: number; value: string } | null = null;
  for (const value of candidates) {
    const timestamp = timestampMillis(value);
    if (
      Number.isFinite(timestamp) &&
      (!latest || timestamp > latest.timestamp)
    ) {
      latest = { timestamp, value };
    }
  }
  return latest?.value ?? null;
}

function publicTrafficProblem(
  traffic: PublicTrafficMetric | undefined,
): string | null {
  if (!traffic) return "Traffic data unavailable";
  if (!traffic.configured) return null;
  const percent = finiteNumber(traffic.cycle_percent);
  if (percent !== null && percent > 100)
    return `Traffic quota exceeded at ${formatPercent(percent)}`;
  return traffic.state === "ok"
    ? null
    : `Traffic evidence ${humanizeToken(traffic.state || "unknown").toLocaleLowerCase()}`;
}

function publicPingProblem(ping: PublicPingMetric | undefined): string | null {
  if (!ping) return null;
  const status = publicPingEffectiveStatus(ping);
  if (
    !["ok", "up", "success", "reachable"].includes(status.toLocaleLowerCase())
  ) {
    return `Primary Ping ${publicPingStatusLabel(status).toLocaleLowerCase()}`;
  }
  return publicFreshnessProblem(ping.checked_at, "Primary Ping");
}

function publicPingEffectiveStatus(ping: PublicPingMetric): string {
  const state = ping.state.trim().toLocaleLowerCase();
  if (state === "disabled") return "disabled";
  if (state && state !== "ok") return state;
  return ping.status?.trim() || ping.state || "unknown";
}

function publicCardWarnings(
  card: PublicMonitoringCard,
  visibility: PublicMonitoringShareView["visibility"] | undefined,
): string[] {
  return [
    visibility?.resources
      ? publicFreshnessProblem(
          card.resources?.observed_at,
          "Resource telemetry",
        )
      : null,
    visibility?.network
      ? card.network?.rate_expected === false
        ? null
        : publicFreshnessProblem(card.network?.observed_at, "Network telemetry")
      : null,
    visibility?.traffic ? publicTrafficProblem(card.traffic) : null,
    visibility?.ping ? publicPingProblem(card.primary_ping) : null,
  ].filter((warning): warning is string => Boolean(warning));
}

function publicCardStatusGroup(
  card: PublicMonitoringCard,
  visibility: PublicMonitoringShareView["visibility"] | undefined,
): Exclude<CardStatusFilter, "all"> {
  const reported = statusGroup(card.status);
  if (reported !== "online") return reported;
  return publicCardWarnings(card, visibility).length ? "warning" : "online";
}

function publicCardVisibleStatusLabel(
  card: PublicMonitoringCard,
  visibility: PublicMonitoringShareView["visibility"] | undefined,
): string {
  const reported = agentStatusPresentation(card.status).label;
  return publicCardStatusGroup(card, visibility) === "warning" &&
    statusGroup(card.status) === "online"
    ? `${reported} · Warning`
    : reported;
}

function publicPingTone(
  status: string,
): "critical" | "warning" | "ok" | "info" | "neutral" {
  switch (status.toLocaleLowerCase()) {
    case "ok":
      return "ok";
    case "degraded":
      return "warning";
    case "down":
    case "error":
    case "failed":
    case "timeout":
    case "unreachable":
      return "critical";
    case "pending":
    case "disabled":
      return "info";
    default:
      return "neutral";
  }
}

function publicPingStatusLabel(status: string): string {
  return ["ok", "up", "success", "reachable"].includes(
    status.trim().toLocaleLowerCase(),
  )
    ? "Reachable"
    : humanizeToken(status);
}

function ratioToPercent(value: number | null | undefined): number | null {
  const finite = finiteNumber(value);
  return finite === null ? null : Math.max(0, Math.min(100, finite * 100));
}

function maximumCapacityCaption(total: number | null | undefined): string {
  const finiteTotal = finiteNumber(total);
  if (finiteTotal === null || finiteTotal <= 0) return "unavailable";
  return `${formatBytes(finiteTotal)} maximum`;
}

function maximumResourceCapacity(
  records: Array<PublicResourceMetric | null>,
  value: (record: PublicResourceMetric) => number | null | undefined,
): number | undefined {
  let maximum: number | undefined;
  for (const record of records) {
    if (!record) continue;
    const candidate = finiteNumber(value(record));
    if (candidate === null || candidate <= 0) continue;
    maximum = maximum === undefined ? candidate : Math.max(maximum, candidate);
  }
  return maximum;
}

function memorySwapCapacityCaption(
  memoryTotal: number | null | undefined,
  swapTotal: number | null | undefined,
  includeSwap: boolean,
): string {
  if (!includeSwap) return maximumCapacityCaption(memoryTotal);
  const memory = maximumCapacity(memoryTotal);
  const swap = maximumCapacity(swapTotal);
  if (memory && swap) {
    return `Max · RAM\u00a0${memory.replace(" ", "\u00a0")} · Swap\u00a0${swap.replace(" ", "\u00a0")}`;
  }
  return maximumCapacityCaption(memoryTotal);
}

function resourceUsageSummary(
  usedRatio: number | null | undefined,
  total: number | null | undefined,
): string {
  const capacity = maximumCapacity(total);
  return `${formatPercent(ratioToPercent(usedRatio))}${capacity ? ` (${capacity})` : ""}`;
}

function maximumCapacity(total: number | null | undefined): string | undefined {
  const finiteTotal = finiteNumber(total);
  return finiteTotal !== null && finiteTotal > 0
    ? formatBytes(finiteTotal)
    : undefined;
}

function trafficCycleSummary(traffic: PublicTrafficMetric): string {
  if (!traffic.configured) return "Unconfigured";
  const limitingQuota = trafficLimitingQuota(traffic);
  if (limitingQuota) {
    return `${formatBytes(limitingQuota.used)} / ${formatBytes(limitingQuota.quota)} · ${limitingQuota.direction} · ${formatPercent(limitingQuota.percent)}`;
  }
  const countedTotal = finiteNumber(traffic.total_bytes);
  if (countedTotal === null || countedTotal < 0)
    return "Current total unavailable";
  const quotas = [
    traffic.quota_total_bytes,
    traffic.quota_rx_bytes,
    traffic.quota_tx_bytes,
  ];
  if (quotas.some((quota) => quota === -1)) {
    const unlimited = trafficUnlimitedQuota(traffic);
    return unlimited
      ? `${formatBytes(unlimited.used)} / Unlimited · ${unlimited.direction}`
      : `${formatBytes(countedTotal)} / Unlimited`;
  }
  if (quotas.some((quota) => quota !== undefined && quota > 0)) {
    return `${formatBytes(countedTotal)} · quota evidence incomplete`;
  }
  return `${formatBytes(countedTotal)} · quota unavailable`;
}

function formatPublicSocketCount(value: number | null | undefined) {
  const finite = finiteNumber(value);
  return finite !== null && finite >= 0
    ? Math.round(finite).toLocaleString()
    : "-";
}

function publicConnectionTitle(
  protocol: "TCP" | "UDP",
  observedAt: string | null | undefined,
) {
  const freshness = publicFreshnessProblem(observedAt, "Connection telemetry");
  return `${protocol} entries in the agent's Linux network-namespace socket tables; TCP includes every state and listeners. ${freshness ?? "Current telemetry"}.`;
}

function publicNetworkRateTitle(
  direction: "received" | "sent",
  network: PublicNetworkMetric | undefined,
): string {
  if (network?.rate_expected === false) {
    return `Network ${direction} rate is intentionally not selected; - is shown`;
  }
  if (
    !network ||
    finiteNumber(direction === "received" ? network.rx_bps : network.tx_bps) ===
      null
  ) {
    return `Network ${direction} rate is unavailable; - is shown`;
  }
  const freshness = publicFreshnessProblem(
    network.observed_at,
    `Network ${direction} rate`,
  );
  return `Interval-average network ${direction} rate${freshness ? `; ${freshness.toLocaleLowerCase()}` : "; current shared telemetry"}`;
}

function publicMonitoringAuxiliaryFacts(
  card: PublicMonitoringCard,
  visibility: PublicMonitoringShareView["visibility"] | undefined,
): Array<{
  context?: string;
  kind: "billing" | "connection" | "uptime";
  label: string;
  title: string;
  value: string;
}> {
  const facts: Array<{
    context?: string;
    kind: "billing" | "connection" | "uptime";
    label: string;
    title: string;
    value: string;
  }> = [];
  if (visibility?.billing) {
    const renewal = formatBillingRenewal(
      card.billing?.cycle,
      card.billing?.period_code,
    );
    const value =
      card.billing && !card.billing.disabled ? card.billing.display : "-";
    facts.push({
      context: renewal ?? undefined,
      kind: "billing",
      label: "Billing",
      title: card.billing
        ? card.billing.disabled
          ? "Billing is explicitly disabled; - is shown"
          : renewal
            ? `${value} · ${renewal}`
            : `${value}; no renewal anchor is configured`
        : "Billing is shared but no billing rule is configured; - is shown",
      value,
    });
  }
  const resource = card.resources;
  if (visibility?.resources) {
    facts.push({
      kind: "connection",
      label: "TCP",
      title: resource
        ? publicConnectionTitle("TCP", resource.connections_observed_at)
        : "TCP connection telemetry is unavailable",
      value: formatPublicSocketCount(resource?.tcp_sockets),
    });
    facts.push({
      kind: "connection",
      label: "UDP",
      title: resource
        ? publicConnectionTitle("UDP", resource.connections_observed_at)
        : "UDP connection telemetry is unavailable",
      value: formatPublicSocketCount(resource?.udp_sockets),
    });
  }
  if (visibility?.system_information) {
    facts.push({
      kind: "uptime",
      label: "Uptime",
      title: card.system_information?.uptime_observed_at
        ? `Observed ${formatFullTime(card.system_information.uptime_observed_at)}`
        : "Latest reported uptime is unavailable",
      value: formatUptime(card.system_information?.uptime_secs),
    });
  }
  return facts;
}

function formatOptionalBytes(value: number | null | undefined): string {
  const finite = finiteNumber(value);
  return finite === null || finite < 0 ? "-" : formatBytes(finite);
}

function formatPercent(value: number | null): string {
  return value === null || !Number.isFinite(value)
    ? "-"
    : `${value >= 100 ? value.toFixed(0) : value.toFixed(1)}%`;
}

function formatOptionalPercent(value: number | null | undefined): string {
  const finite = finiteNumber(value);
  return finite === null ? "-" : formatPercent(finite);
}

function formatOptionalRate(value: number | null | undefined): string {
  const finite = finiteNumber(value);
  return finite === null ? "-" : formatByteRateFromBitsPerSecond(finite);
}

function formatLoad(value: number): string {
  return Number.isFinite(value) ? formatNumber(value) : "-";
}

function formatNumber(value: number): string {
  return value >= 10 ? value.toFixed(1) : value.toFixed(2);
}

function finiteNumber(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function humanizeToken(value: string): string {
  const normalized = value.trim().replace(/[_-]+/g, " ");
  return normalized
    ? normalized.replace(/\b\p{L}/gu, (letter) => letter.toLocaleUpperCase())
    : "Unknown";
}

function safeClassToken(value: string): string {
  return value.toLocaleLowerCase().replace(/[^a-z0-9_-]+/g, "-") || "unknown";
}

function dateTimeLocalValue(timestamp: number): string {
  const date = new Date(timestamp);
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}

function isAbortError(reason: unknown): boolean {
  return reason instanceof DOMException && reason.name === "AbortError";
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error && reason.message.trim()
    ? reason.message
    : "The shared monitoring request did not return readable data.";
}
