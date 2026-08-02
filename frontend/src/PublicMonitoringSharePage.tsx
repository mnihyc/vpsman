import { Activity, Gauge, Network, Radio, Server } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { apiErrorFromResponse, apiFetch, apiJsonFromResponse } from "./api";
import { dashboardChartColors, consolePalette } from "./colorPalette";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "./components/TimeSeriesChart";
import { ConsoleStatusBadge } from "./components/ConsoleLayout";
import {
  pushHistoryEntry,
  replaceHistoryEntry,
  useHistoryEntryState,
} from "./historyEntryState";
import { MiniSparkline } from "./panels/FleetMonitorPanel";
import type {
  MonitoringRangeView as MonitoringRange,
  PublicMonitoringCardView as PublicMonitoringCard,
  PublicMonitoringDataView as PublicMonitoringData,
  PublicMonitoringDetailView as PublicMonitoringDetail,
  PublicNetworkMetricView as PublicNetworkMetric,
  PublicNetworkPointView as PublicNetworkPoint,
  PublicPingMetricView as PublicPingMetric,
  PublicPingPointView as PublicPingPoint,
  PublicResourceMetricView as PublicResourceMetric,
  PublicMonitoringShareBootstrapView,
  PublicMonitoringShareView,
  PublicTrafficMetricView as PublicTrafficMetric,
  TrafficHistoryPointView as PublicTrafficPoint,
} from "./types";
import { formatCompactTime, formatFullTime, timestampMillis } from "./utils";
import { agentStatusPresentation } from "./agentDisplayState";

type PublicMonitoringSharePageProps = {
  initialClientKey?: string | null;
  shareId: string;
  secret: string;
};

type Density = "comfortable" | "compact";
type CardStatusFilter = "all" | "online" | "warning" | "offline";
type MonitoringWindow =
  | "15m"
  | "1h"
  | "8h"
  | "1d"
  | "7d"
  | "30d"
  | "90d"
  | "180d"
  | "1y"
  | "all"
  | "custom";

type CustomBounds = {
  startUnix: number;
  endUnix: number;
};

const RANGE_OPTIONS: ReadonlyArray<{
  label: string;
  title?: string;
  value: MonitoringWindow;
}> = [
  { label: "15m", title: "Realtime · last 15 minutes", value: "15m" },
  { label: "1h", value: "1h" },
  { label: "8h", value: "8h" },
  { label: "1d", value: "1d" },
  { label: "7d", value: "7d" },
  { label: "30d", value: "30d" },
  { label: "90d", value: "90d" },
  { label: "180d", value: "180d" },
  { label: "1y", value: "1y" },
  { label: "All", value: "all" },
  { label: "Custom", value: "custom" },
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
  const [density, setDensity] = useHistoryEntryState<Density>(
    `${historySlot}.density`,
    "comfortable",
  );
  const [search, setSearch] = useHistoryEntryState(`${historySlot}.search`, "");
  const [statusFilter, setStatusFilter] =
    useHistoryEntryState<CardStatusFilter>(`${historySlot}.status`, "all");
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
        setShare(page.share);
        setTotal(page.total);
        combined = [...combined, ...page.cards];
        setCards(combined);
        if (page.next_offset === null) break;
        if (
          page.next_offset <= offset ||
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
          latestShare = page.share;
          latestTotal = page.total;
          combined = [...combined, ...page.cards];
          if (page.next_offset === null) break;
          if (
            page.next_offset <= offset ||
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

  const filteredCards = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return cards.filter((card) => {
      const matchesSearch =
        !query ||
        card.display_name.toLocaleLowerCase().includes(query) ||
        (card.tags ?? []).some((tag) =>
          tag.toLocaleLowerCase().includes(query),
        );
      return (
        matchesSearch &&
        (statusFilter === "all" ||
          publicCardStatusGroup(card, share?.visibility) === statusFilter)
      );
    });
  }, [cards, search, share?.visibility, statusFilter]);
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
              <label>
                <span>Search</span>
                <input
                  aria-label="Search shared VPSs"
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder="Name or tag"
                  type="search"
                  value={search}
                />
              </label>
              <label>
                <span>Status</span>
                <select
                  aria-label="Filter shared VPSs by status"
                  onChange={(event) =>
                    setStatusFilter(event.target.value as CardStatusFilter)
                  }
                  value={statusFilter}
                >
                  <option value="all">All</option>
                  <option value="online">Online</option>
                  <option value="warning">Warning / unknown</option>
                  <option value="offline">Offline</option>
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
                <span>
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
                <span>
                  <small>Realtime bandwidth</small>
                  <strong>↓ {formatRate(fleetSnapshot.network.rxBps)}</strong>
                  <em>
                    ↑ {formatRate(fleetSnapshot.network.txBps)} ·{" "}
                    {fleetSnapshot.network.freshCount} fresh
                    {cardsComplete ? "" : " · partial"}
                  </em>
                </span>
              ) : null}
              {fleetSnapshot.traffic ? (
                <span>
                  <small>Current-cycle traffic</small>
                  <strong>
                    {fleetSnapshot.traffic.count > 0
                      ? formatBytes(fleetSnapshot.traffic.bytes)
                      : "n/a"}
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
                  ? "Clear the search or change the status filter."
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
          onCustomEndChange={setCustomEnd}
          onCustomStartChange={setCustomStart}
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
  const memoryUsed = usedPercent(
    resource?.memory_total_bytes,
    resource?.memory_available_bytes,
  );
  const diskUsed = usedPercent(
    resource?.disk_total_bytes,
    resource?.disk_available_bytes,
  );
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
    ? publicFreshnessProblem(card.network?.observed_at, "Network telemetry")
    : null;
  const warnings = publicCardWarnings(card, visibility);
  const cardTitle = `${card.display_name || "Unnamed VPS"} · ${statusLabel}`;
  const freshness = publicCardFreshness(card, visibility);
  const freshnessLabel = freshness
    ? `Updated ${formatCompactTime(freshness)}`
    : publicCardHasVisibleTelemetry(visibility)
      ? "Visible telemetry unavailable"
      : "Status only";
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
      <strong title={card.display_name || "Unnamed VPS"}>
        {card.display_name || "Unnamed VPS"}
      </strong>
      <small title={freshnessLabel}>{freshnessLabel}</small>
    </>
  );
  return (
    <article
      aria-label={`${cardTitle} shared monitoring card`}
      className={`vpsMonitorCard publicMonitoringCard ${effectiveStatus} ${density}${selected ? " selected" : ""}${detailAllowed ? "" : " publicMonitoringCardStatic"}`}
      onClick={detailAllowed ? onOpen : undefined}
      onKeyDown={
        detailAllowed
          ? (event) => {
              if (event.key !== "Enter") return;
              event.preventDefault();
              onOpen();
            }
          : undefined
      }
      role={detailAllowed ? "link" : undefined}
      tabIndex={detailAllowed ? 0 : undefined}
      title={
        detailAllowed
          ? `Open read-only history for ${card.display_name || "this VPS"}`
          : undefined
      }
    >
      <div className="vpsMonitorCardMain">{cardHeader}</div>

      {density === "comfortable" && visibility?.identity_context ? (
        <div className="vpsMonitorTags" aria-label="Shared identity tags">
          {(card.tags ?? []).length ? (
            (card.tags ?? []).map((tag) => (
              <span key={tag} title={tag}>
                {tag}
              </span>
            ))
          ) : (
            <span>untagged</span>
          )}
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
            percent={cpuPercent}
            stale={Boolean(resourceProblem)}
            value={formatPercent(cpuPercent)}
          />
          <PublicMetric
            caption={capacityCaption(
              resource?.memory_total_bytes,
              resource?.memory_available_bytes,
            )}
            icon={<Gauge size={15} />}
            label="RAM"
            percent={memoryUsed}
            stale={Boolean(resourceProblem)}
            value={formatPercent(memoryUsed)}
          />
          <PublicMetric
            caption={capacityCaption(
              resource?.disk_total_bytes,
              resource?.disk_available_bytes,
            )}
            icon={<Server size={15} />}
            label="Disk"
            percent={diskUsed}
            stale={Boolean(resourceProblem)}
            value={formatPercent(diskUsed)}
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
            value={resource ? formatLoad(resource.load_1) : "No data"}
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
            value={formatRate(card.network?.rx_bps)}
          />
          <PublicFact
            icon={<Network size={13} />}
            label="TX"
            sparkline={
              <MiniSparkline label="TX activity" tone="tx" values={txHistory} />
            }
            stale={Boolean(networkProblem)}
            value={formatRate(card.network?.tx_bps)}
          />
          {density === "comfortable" ? (
            <small className="publicMonitoringFreshness">
              {card.network?.observed_at
                ? `Updated ${formatCompactTime(card.network.observed_at)}`
                : "Network data unavailable"}
            </small>
          ) : null}
        </div>
      ) : null}

      {visibility?.resources ? (
        <div
          className="vpsMonitorAuxFacts"
          aria-label={`Connection counts for ${card.display_name}`}
        >
          <span
            title={publicConnectionTitle(
              "TCP",
              resource?.connections_observed_at,
            )}
          >
            <small>TCP</small>
            <strong>{formatPublicSocketCount(resource?.tcp_sockets)}</strong>
          </span>
          <span
            title={publicConnectionTitle(
              "UDP",
              resource?.connections_observed_at,
            )}
          >
            <small>UDP</small>
            <strong>{formatPublicSocketCount(resource?.udp_sockets)}</strong>
          </span>
        </div>
      ) : null}

      {visibility?.traffic ? <PublicTrafficRow traffic={card.traffic} /> : null}

      {visibility?.ping ? (
        <PublicPingRow
          history={card.primary_ping_history ?? []}
          ping={card.primary_ping}
        />
      ) : null}

      {warnings.length ? (
        <div
          className="publicMonitoringWarnings"
          role="status"
          title={warnings.join("; ")}
        >
          <strong>Needs attention</strong>
          <span>{warnings.join(" · ")}</span>
        </div>
      ) : null}
    </article>
  );
}

function PublicMetric({
  caption,
  icon,
  label,
  percent,
  sparkline,
  stale = false,
  value,
}: {
  caption: string;
  icon: ReactNode;
  label: string;
  percent: number | null;
  sparkline?: ReactNode;
  stale?: boolean;
  value: string;
}) {
  const bounded = percent === null ? null : Math.max(0, Math.min(100, percent));
  return (
    <span
      className={`vpsMonitorMetric${bounded === null ? " missing" : ""}${stale ? " stale" : ""}`}
    >
      <span aria-hidden="true" className="vpsMonitorMetricIcon">
        {icon}
      </span>
      <span className="vpsMonitorMetricLabel">{label}</span>
      <strong>{value}</strong>
      <span
        aria-label={
          bounded === null ? undefined : `${label}: ${value}; ${caption}`
        }
        aria-valuemax={bounded === null ? undefined : 100}
        aria-valuemin={bounded === null ? undefined : 0}
        aria-valuenow={bounded ?? undefined}
        aria-valuetext={bounded === null ? undefined : `${value}; ${caption}`}
        className={`vpsMonitorMetricTrack${bounded === null ? " missing" : ""}`}
        role={bounded === null ? undefined : "meter"}
      >
        <span style={{ width: `${bounded ?? 0}%` }} />
      </span>
      {sparkline}
      <small>{caption}</small>
    </span>
  );
}

function PublicFact({
  icon,
  label,
  sparkline,
  stale = false,
  value,
}: {
  icon?: ReactNode;
  label: string;
  sparkline?: ReactNode;
  stale?: boolean;
  value: string;
}) {
  return (
    <span className={`vpsMonitorFlowFact${stale ? " stale" : ""}`}>
      <small>
        {icon}
        {icon ? " " : ""}
        {label}
      </small>
      <strong title={value}>{value}</strong>
      {sparkline}
    </span>
  );
}

function PublicTrafficRow({ traffic }: { traffic?: PublicTrafficMetric }) {
  if (!traffic) {
    return (
      <div
        className="publicMonitoringTraffic missing"
        aria-label="Traffic unavailable"
      >
        <strong>Traffic unavailable</strong>
        <span>No traffic data was shared.</span>
      </div>
    );
  }
  const percent = traffic.configured
    ? finiteNumber(traffic.cycle_percent)
    : null;
  const fill = percent === null ? 0 : Math.max(0, Math.min(100, percent));
  const problem = publicTrafficProblem(traffic);
  const stateLabel = traffic.configured ? "Traffic" : "Traffic unconfigured";
  const portSpeed = traffic.port_speed?.display;
  const trafficHeading = `${stateLabel}${portSpeed ? ` · ${portSpeed}` : ""}`;
  const cycleSummary = trafficCycleSummary(traffic);
  const trafficDetail =
    problem ??
    `RX ${formatBytes(traffic.rx_bytes)} · TX ${formatBytes(traffic.tx_bytes)}${traffic.cycle_end ? ` · resets ${formatCompactTime(traffic.cycle_end)}` : ""}`;
  return (
    <div
      aria-label={`Traffic cycle: ${stateLabel}`}
      className={`publicMonitoringTraffic ${safeClassToken(traffic.state)}${problem ? " warning" : ""}${percent !== null && percent > 100 ? " overQuota" : ""}`}
    >
      <div>
        <strong
          title={
            portSpeed
              ? `${trafficHeading}; display value only—no shaping or enforcement is implied`
              : trafficHeading
          }
        >
          {trafficHeading}
        </strong>
        <span title={cycleSummary}>{cycleSummary}</span>
      </div>
      <span
        aria-label={
          percent === null
            ? undefined
            : `Traffic quota use: ${formatPercent(percent)}`
        }
        aria-valuemax={percent === null ? undefined : 100}
        aria-valuemin={percent === null ? undefined : 0}
        aria-valuenow={percent === null ? undefined : fill}
        aria-valuetext={percent === null ? undefined : formatPercent(percent)}
        className={`vpsMonitorMetricTrack${percent === null ? " missing" : ""}`}
        role={percent === null ? undefined : "meter"}
      >
        <span style={{ width: `${fill}%` }} />
      </span>
      <small title={trafficDetail}>{trafficDetail}</small>
    </div>
  );
}

function PublicPingRow({
  history,
  ping,
}: {
  history: PublicPingPoint[];
  ping?: PublicPingMetric;
}) {
  if (!ping) {
    return (
      <div
        className="publicMonitoringPing missing"
        aria-label="Primary Ping unconfigured"
      >
        <Radio aria-hidden="true" size={14} />
        <strong>Primary Ping unconfigured</strong>
        <span>No primary Ping target is available.</span>
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
    >
      <Radio aria-hidden="true" size={14} />
      <strong title={ping.target_name}>{ping.target_name}</strong>
      <span title={presentedDetail}>{presentedDetail}</span>
      <MiniSparkline
        label={`${ping.target_name} latency history`}
        tone="ping"
        values={historyValues(history, (point) => point.latency_avg_ms)}
      />
      <small title={statusDetail}>{statusDetail}</small>
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
  const [pingMetric, setPingMetric] = useHistoryEntryState<"latency" | "loss">(
    `public-monitoring.${card.client_key}.ping-metric`,
    "latency",
  );
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const rangeTabsRef = useRef<HTMLDivElement | null>(null);
  const resources = detail?.resources ?? [];
  const network = detail?.network ?? [];
  const traffic = detail?.traffic ?? [];
  const resourceTimeline = regularTimeline(resources, detail?.range);
  const networkTimeline = regularTimeline(network, detail?.range);
  const trafficTimeline = regularTimeline(traffic, detail?.range);
  const trafficResetCount = traffic.reduce(
    (total, point) => total + Math.max(0, point.reset_count),
    0,
  );
  const pingChart = pingChartData(detail?.ping ?? [], detail?.range);
  const exportPrefix = `shared-${card.client_key.slice(0, 12)}-${detail?.range.window ?? window}`;
  const visibleStatusLabel = publicCardVisibleStatusLabel(card, visibility);
  const freshness = publicCardFreshness(card, visibility);
  useEffect(() => {
    headingRef.current?.focus({ preventScroll: false });
  }, [card.client_key]);
  useEffect(() => {
    rangeTabsRef.current
      ?.querySelector<HTMLElement>(`[data-window="${window}"]`)
      ?.scrollIntoView({ block: "nearest", inline: "center" });
  }, [window]);
  return (
    <section
      aria-busy={loading}
      aria-label={`Read-only history for ${card.display_name}`}
      className="publicMonitoringDetail consoleInlineDetail"
    >
      <header className="publicMonitoringDetailHeader">
        <div>
          <h2 ref={headingRef} tabIndex={-1}>
            {card.display_name || "Unnamed VPS"}
          </h2>
          <span>
            {visibleStatusLabel}
            {freshness ? ` · Updated ${formatCompactTime(freshness)}` : ""}
            {` · Read-only history`}
            {detail
              ? ` · ${humanizeToken(detail.range.source)} source · ${detail.range.points} points`
              : ""}
          </span>
        </div>
        <button aria-label="Close VPS history" onClick={onClose} type="button">
          Close
        </button>
      </header>
      <div className="observabilityMetricsControls publicMonitoringRangeControls">
        <div
          className="timeRangeTabs"
          ref={rangeTabsRef}
          role="group"
          aria-label="History range"
        >
          {RANGE_OPTIONS.map((option) => (
            <button
              aria-pressed={window === option.value}
              className={window === option.value ? "active" : ""}
              data-window={option.value}
              key={option.value}
              onClick={() => onWindowChange(option.value)}
              title={option.title}
              type="button"
            >
              {option.label}
            </button>
          ))}
        </div>
      </div>
      {window === "custom" ? (
        <div className="publicMonitoringCustomRange">
          <label>
            <span>Start</span>
            <input
              onChange={(event) => onCustomStartChange(event.target.value)}
              type="datetime-local"
              value={customStart}
            />
          </label>
          <label>
            <span>End</span>
            <input
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
          {detail.ping_targets !== undefined ? (
            <div
              aria-label="Current Ping target evidence"
              className="vpsMonitoringPingTargets publicMonitoringPingTargets"
            >
              {detail.ping_targets.map((target) => {
                const status = publicPingEffectiveStatus(target);
                return (
                  <div
                    className="vpsMonitoringPingTarget"
                    key={target.target_name}
                  >
                    <span>
                      <i
                        style={{
                          background: stableTargetColor(target.target_name),
                        }}
                      />
                      <strong title={target.target_name}>
                        {target.target_name}
                      </strong>
                      {card.primary_ping?.target_name === target.target_name ? (
                        <em>Primary</em>
                      ) : null}
                    </span>
                    <ConsoleStatusBadge tone={publicPingTone(status)}>
                      {publicPingStatusLabel(status)}
                    </ConsoleStatusBadge>
                    <small>
                      {status === "disabled" ? "Last sample: " : ""}
                      {target.latency_avg_ms === null
                        ? "No latency"
                        : `${formatNumber(target.latency_avg_ms)} ms`}
                      {` · ${target.loss_ratio === null ? "No loss evidence" : formatPercent(target.loss_ratio * 100)}`}
                    </small>
                  </div>
                );
              })}
              {!detail.ping_targets.length ? (
                <p className="dashboardEmptyChart">
                  No Ping targets are assigned to this VPS.
                </p>
              ) : null}
            </div>
          ) : null}
          <div className="dashboardWidgetGrid publicMonitoringDetailCharts">
            {detail.resources !== undefined ? (
              <>
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
                  times={resourceTimeline.times}
                  valueFormatter={(value) =>
                    value === null ? "No data" : formatNumber(value)
                  }
                />
                <PublicChart
                  emptyLabel="Memory history is unavailable for this range"
                  exportFileName={`${exportPrefix}-memory`}
                  label="Memory used"
                  lines={[
                    {
                      color: consolePalette.chart.purple,
                      label: "Memory used",
                      values: resourceTimeline.records.map((point) =>
                        usedPercent(
                          point?.memory_total_bytes,
                          point?.memory_available_bytes,
                        ),
                      ),
                    },
                  ]}
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
                        usedPercent(
                          point?.disk_total_bytes,
                          point?.disk_available_bytes,
                        ),
                      ),
                    },
                  ]}
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
                  times={resourceTimeline.times}
                  valueFormatter={(value) =>
                    value === null ? "No data" : formatPublicSocketCount(value)
                  }
                />
              </>
            ) : null}
            {detail.network !== undefined ? (
              <PublicChart
                emptyLabel="Network rate history is unavailable for this range"
                exportFileName={`${exportPrefix}-network`}
                label="Network RX / TX"
                lines={[
                  {
                    color: consolePalette.chart.blue,
                    label: "RX",
                    values: networkTimeline.records.map(
                      (point) => point?.rx_bps ?? null,
                    ),
                  },
                  {
                    color: consolePalette.chart.green,
                    label: "TX",
                    values: networkTimeline.records.map(
                      (point) => point?.tx_bps ?? null,
                    ),
                  },
                ]}
                times={networkTimeline.times}
                valueFormatter={formatRate}
                wide
              />
            ) : null}
            {detail.traffic !== undefined ? (
              <>
                <PublicChart
                  emptyLabel={
                    card.traffic?.configured
                      ? "Traffic volume history is unavailable for this range"
                      : "Traffic unconfigured"
                  }
                  exportFileName={`${exportPrefix}-traffic`}
                  label="Traffic volume"
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
                  times={trafficTimeline.times}
                  valueFormatter={(value) =>
                    value === null ? "No data" : formatBytes(value)
                  }
                  wide
                />
                <PublicTrafficCycle
                  resetCount={trafficResetCount}
                  traffic={card.traffic}
                />
              </>
            ) : null}
            {detail.ping !== undefined ? (
              <div className="publicMonitoringPingHistory wideWidget">
                <div
                  className="segmented vpsMonitoringPingMetric"
                  role="group"
                  aria-label="Ping chart metric"
                >
                  <button
                    aria-pressed={pingMetric === "latency"}
                    className={pingMetric === "latency" ? "selected" : ""}
                    onClick={() => setPingMetric("latency")}
                    type="button"
                  >
                    Latency
                  </button>
                  <button
                    aria-pressed={pingMetric === "loss"}
                    className={pingMetric === "loss" ? "selected" : ""}
                    onClick={() => setPingMetric("loss")}
                    type="button"
                  >
                    Loss
                  </button>
                </div>
                <PublicChart
                  emptyLabel={`Ping ${pingMetric} history is unavailable for this range`}
                  exportFileName={`${exportPrefix}-ping-${pingMetric}`}
                  label={
                    pingMetric === "latency"
                      ? "Ping latency"
                      : "Ping packet loss"
                  }
                  lines={
                    pingMetric === "latency"
                      ? pingChart.latencyLines
                      : pingChart.lossLines
                  }
                  times={pingChart.times}
                  valueFormatter={
                    pingMetric === "latency"
                      ? (value) =>
                          value === null
                            ? "No data"
                            : `${formatNumber(value)} ms`
                      : (value) => formatPercent(value)
                  }
                  wide
                />
              </div>
            ) : null}
          </div>
        </>
      ) : null}
    </section>
  );
}

function PublicTrafficCycle({
  resetCount,
  traffic,
}: {
  resetCount: number;
  traffic?: PublicTrafficMetric;
}) {
  const percent = finiteNumber(traffic?.cycle_percent);
  const fill = percent === null ? 0 : Math.max(0, Math.min(100, percent));
  const overQuota = percent !== null && percent > 100;
  return (
    <div className="dashboardWidgetChart wideWidget vpsMonitoringTrafficCycle publicMonitoringTrafficCycle">
      <div className="dashboardWidgetHeader">
        <strong>Traffic volume / cycle</strong>
        <small>
          {traffic?.configured
            ? `${formatCompactTime(traffic.cycle_start)} – ${formatCompactTime(traffic.cycle_end)}`
            : "Authoritative traffic accounting"}
        </small>
      </div>
      {!traffic?.configured ? (
        <div className="dashboardEmptyChart">Traffic unconfigured</div>
      ) : (
        <>
          <div className="vpsMonitoringTrafficSummary">
            <PublicTrafficCycleFact
              label="RX"
              quota={traffic.quota_rx_bytes}
              value={traffic.rx_bytes}
            />
            <PublicTrafficCycleFact
              label="TX"
              quota={traffic.quota_tx_bytes}
              value={traffic.tx_bytes}
            />
            <PublicTrafficCycleFact
              label="Total"
              quota={traffic.quota_total_bytes}
              value={traffic.total_bytes}
            />
            <span>
              <small>Cycle ends</small>
              <strong>{formatCompactTime(traffic.cycle_end)}</strong>
              <em>
                {traffic.state === "ok"
                  ? "Current accounting evidence"
                  : humanizeToken(traffic.state)}
              </em>
            </span>
          </div>
          {percent !== null ? (
            <div
              className={`vpsMonitoringTrafficProgress${overQuota ? " overLimit" : ""}`}
            >
              <span
                aria-label={`${formatPercent(percent)} of the limiting traffic quota used`}
                aria-valuemax={100}
                aria-valuemin={0}
                aria-valuenow={fill}
                aria-valuetext={formatPercent(percent)}
                className="vpsMonitoringTrafficTrack"
                role="progressbar"
              >
                <i style={{ width: `${fill}%` }} />
              </span>
              <strong>{formatPercent(percent)}</strong>
              <small>
                {overQuota ? "Quota exceeded" : "Limiting quota used"}
              </small>
            </div>
          ) : (
            <p className="vpsMonitoringTrafficNote incomplete">
              Traffic is accounted for, but no quota is configured.
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
        </>
      )}
    </div>
  );
}

function PublicTrafficCycleFact({
  label,
  quota,
  value,
}: {
  label: string;
  quota: number | null;
  value: number;
}) {
  return (
    <span>
      <small>{label}</small>
      <strong>{formatBytes(value)}</strong>
      <em>
        {quota === -1
          ? "Unlimited"
          : quota === null
            ? "No quota"
            : `${formatBytes(value)} / ${formatBytes(quota)}`}
      </em>
    </span>
  );
}

function PublicChart({
  emptyLabel,
  exportFileName,
  label,
  lines,
  times,
  valueFormatter,
  wide = false,
}: {
  emptyLabel: string;
  exportFileName?: string;
  label: string;
  lines: TimeSeriesChartLine[];
  times: string[];
  valueFormatter: (value: number | null) => string;
  wide?: boolean;
}) {
  return (
    <div
      className={`dashboardWidgetChart publicMonitoringChart${wide ? " wideWidget" : ""}`}
    >
      <h3>{label}</h3>
      <TimeSeriesChart
        ariaLabel={`${label} shared monitoring chart`}
        emptyLabel={emptyLabel}
        exportFileName={exportFileName}
        lines={lines}
        times={times}
        valueFormatter={valueFormatter}
      />
    </div>
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
      color: stableTargetColor(targetName),
      label: targetName,
      values: targetValues(targetName, (point) => point.latency_avg_ms),
    };
  });
  const lossLines = targetNames.map((targetName) => {
    return {
      color: stableTargetColor(targetName),
      label: targetName,
      values: targetValues(targetName, (point) => point.loss_ratio * 100),
    };
  });
  return { latencyLines, lossLines, times };
}

function stableTargetColor(targetName: string): string {
  let hash = 2_166_136_261;
  for (const character of targetName) {
    hash ^= character.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 16_777_619);
  }
  return dashboardChartColors[(hash >>> 0) % dashboardChartColors.length];
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
  if (response.status === 404) {
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
        publicTagValue(card.tags ?? [], "country") ??
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
      traffic.bytes += Math.max(0, finiteNumber(card.traffic.total_bytes) ?? 0);
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

function publicTagValue(tags: string[], key: string) {
  const prefix = `${key}:`;
  return (
    tags
      .find((tag) => tag.toLocaleLowerCase().startsWith(prefix))
      ?.slice(prefix.length)
      .trim() || null
  );
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
      ? publicFreshnessProblem(card.network?.observed_at, "Network telemetry")
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

function usedPercent(
  total: number | null | undefined,
  available: number | null | undefined,
): number | null {
  const finiteTotal = finiteNumber(total);
  const finiteAvailable = finiteNumber(available);
  if (finiteTotal === null || finiteAvailable === null || finiteTotal <= 0)
    return null;
  return Math.max(
    0,
    Math.min(100, ((finiteTotal - finiteAvailable) / finiteTotal) * 100),
  );
}

function capacityCaption(
  total: number | null | undefined,
  available: number | null | undefined,
): string {
  const finiteTotal = finiteNumber(total);
  const finiteAvailable = finiteNumber(available);
  if (finiteTotal === null || finiteAvailable === null || finiteTotal <= 0)
    return "unavailable";
  return `${formatBytes(Math.max(0, finiteTotal - finiteAvailable))} / ${formatBytes(finiteTotal)}`;
}

function trafficCycleSummary(traffic: PublicTrafficMetric): string {
  if (!traffic.configured) {
    return `${formatBytes(traffic.total_bytes)} this cycle · quota not configured`;
  }
  if (traffic.quota_total_bytes === -1) {
    return `${formatBytes(traffic.total_bytes)} / Unlimited`;
  }
  const directionalQuotas = [traffic.quota_rx_bytes, traffic.quota_tx_bytes];
  if (
    traffic.quota_total_bytes === null &&
    directionalQuotas.some((quota) => quota === -1) &&
    directionalQuotas.every((quota) => quota === null || quota === -1)
  ) {
    return `${formatBytes(traffic.total_bytes)} / Unlimited`;
  }
  if (
    traffic.cycle_percent !== null &&
    Number.isFinite(traffic.cycle_percent)
  ) {
    return traffic.quota_total_bytes === null
      ? `${formatBytes(traffic.total_bytes)} used · limiting quota ${formatPercent(traffic.cycle_percent)}`
      : `${formatBytes(traffic.total_bytes)} / ${formatBytes(traffic.quota_total_bytes)} · ${formatPercent(traffic.cycle_percent)}`;
  }
  return `${formatBytes(traffic.total_bytes)} this cycle · quota unavailable`;
}

function formatPublicSocketCount(value: number | null | undefined) {
  const finite = finiteNumber(value);
  return finite !== null && finite >= 0
    ? Math.round(finite).toLocaleString()
    : "n/a";
}

function publicConnectionTitle(
  protocol: "TCP" | "UDP",
  observedAt: string | null | undefined,
) {
  const freshness = publicFreshnessProblem(observedAt, "Connection telemetry");
  return `${protocol} entries in the agent's Linux network-namespace socket tables; TCP includes every state and listeners. ${freshness ?? "Current telemetry"}.`;
}

function formatRate(value: number | null | undefined): string {
  const finite = finiteNumber(value);
  return finite === null ? "No data" : `${formatBytes(finite)}/s`;
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value)) return "No data";
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
  let scaled = Math.max(0, value);
  let unit = 0;
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024;
    unit += 1;
  }
  return `${scaled >= 10 || unit === 0 ? Math.round(scaled) : scaled.toFixed(1)} ${units[unit]}`;
}

function formatPercent(value: number | null): string {
  return value === null || !Number.isFinite(value)
    ? "No data"
    : `${value >= 100 ? value.toFixed(0) : value.toFixed(1)}%`;
}

function formatLoad(value: number): string {
  return Number.isFinite(value) ? formatNumber(value) : "No data";
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
