import { useCallback, useRef, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import { TOPOLOGY_EVIDENCE_LIMIT } from "../constants";
import type {
  AllocateTunnelEndpointsRequest,
  AllocateTunnelEndpointsResponse,
  ClearTunnelPlanEvidenceOutcome,
  ClearTunnelPlanEvidenceRequest,
  ClearTunnelPlanEvidenceResponse,
  CreateTunnelPlanRequest,
  NetworkAdapterDefinitionRecord,
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  TunnelPlanExport,
  TunnelPlanCorruptRecord,
  TunnelPlanListItem,
  TopologyGraph,
  TunnelPlanOspfJobsResponse,
  TunnelPlanRecord,
  TunnelPlanMutationResponse,
  TunnelPlanRevisionTarget,
  UpsertNetworkAdapterDefinitionRequest,
  UpdateTunnelConnectionAssessmentRequest,
  UpdateTunnelPlanOspfCostRequest,
  UpdateTunnelPlanRequest,
} from "../types";
import { retainMutationSuccessAfterRefresh } from "../utils";
import {
  buildNetworkEvidenceSearch,
  type NetworkEvidenceQuery,
} from "../networkEvidence";

const TOPOLOGY_SOURCE_ORDER = [
  "tunnelPlans",
  "networkAdapterDefinitions",
  "networkObservations",
  "networkTrends",
  "ospfRecommendations",
  "ospfUpdatePlans",
  "topologyGraph",
] as const;

type TopologySource = (typeof TOPOLOGY_SOURCE_ORDER)[number];

const TOPOLOGY_SOURCE_LABELS: Record<TopologySource, string> = {
  tunnelPlans: "Tunnel plans",
  networkAdapterDefinitions: "Network adapter definitions",
  networkObservations: "Network observations",
  networkTrends: "Network trends",
  ospfRecommendations: "OSPF recommendations",
  ospfUpdatePlans: "OSPF update plans",
  topologyGraph: "Topology graph",
};

export function useTopologyData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
  onRuntimeConfigChanged: () => Promise<void>,
) {
  const apiTokenRef = useRef(apiToken);
  apiTokenRef.current = apiToken;
  const [tunnelPlans, setTunnelPlans] = useState<TunnelPlanRecord[]>([]);
  const [networkAdapterDefinitions, setNetworkAdapterDefinitions] = useState<
    NetworkAdapterDefinitionRecord[]
  >([]);
  const [tunnelPlanCorruptions, setTunnelPlanCorruptions] =
    useState<TunnelPlanCorruptRecord[]>([]);
  const [networkObservations, setNetworkObservations] = useState<NetworkObservationRecord[]>([]);
  const [networkTrends, setNetworkTrends] = useState<NetworkObservationTrendRecord[]>([]);
  const [ospfRecommendations, setOspfRecommendations] = useState<NetworkOspfRecommendationRecord[]>([]);
  const [ospfUpdatePlans, setOspfUpdatePlans] = useState<NetworkOspfUpdatePlanRecord[]>([]);
  const [topologyGraph, setTopologyGraph] = useState<TopologyGraph>(emptyTopologyGraph());
  const [topologyError, setTopologyError] = useState<string | null>(null);
  const [topologyLoading, setTopologyLoading] = useState(false);
  const topologyErrors = useRef<Partial<Record<TopologySource, string>>>({});
  const topologyPendingLoads = useRef(new Set<string>());
  const networkObservationQuery = useRef<NetworkEvidenceQuery>({});
  const networkTrendQuery = useRef<NetworkEvidenceQuery>({});
  const topologyGraphQuery = useRef<NetworkEvidenceQuery>({});
  const topologyLoadGenerations = useRef<Record<TopologySource, number>>({
    tunnelPlans: 0,
    networkAdapterDefinitions: 0,
    networkObservations: 0,
    networkTrends: 0,
    ospfRecommendations: 0,
    ospfUpdatePlans: 0,
    topologyGraph: 0,
  });

  const beginTopologyLoad = useCallback((source: TopologySource) => {
    const generation = topologyLoadGenerations.current[source] + 1;
    topologyLoadGenerations.current[source] = generation;
    for (const pending of topologyPendingLoads.current) {
      if (pending.startsWith(`${source}:`)) {
        topologyPendingLoads.current.delete(pending);
      }
    }
    topologyPendingLoads.current.add(`${source}:${generation}`);
    setTopologyLoading(true);
    delete topologyErrors.current[source];
    setTopologyError(summarizeTopologyErrors(topologyErrors.current));
    return generation;
  }, []);

  const finishTopologyLoad = useCallback(
    (source: TopologySource, generation: number) => {
      topologyPendingLoads.current.delete(`${source}:${generation}`);
      setTopologyLoading(topologyPendingLoads.current.size > 0);
    },
    [],
  );

  const loadTopologySource = useCallback(
    async <T,>(
      source: TopologySource,
      request: () => Promise<T>,
      apply: (value: T) => void,
      reset: () => void,
      fallback: string,
      rethrow = false,
    ) => {
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      const generation = beginTopologyLoad(source);
      try {
        const value = await request();
        if (
          apiTokenRef.current !== apiToken ||
          topologyLoadGenerations.current[source] !== generation
        ) {
          return;
        }
        apply(value);
        delete topologyErrors.current[source];
        setTopologyError(summarizeTopologyErrors(topologyErrors.current));
      } catch (error) {
        if (
          apiTokenRef.current !== apiToken ||
          topologyLoadGenerations.current[source] !== generation
        ) {
          return;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          reset();
          topologyErrors.current[source] = "Operator login required";
        } else {
          topologyErrors.current[source] =
            error instanceof Error ? error.message : fallback;
        }
        setTopologyError(summarizeTopologyErrors(topologyErrors.current));
        if (rethrow) {
          throw error;
        }
      } finally {
        finishTopologyLoad(source, generation);
      }
    },
    [apiToken, beginTopologyLoad, finishTopologyLoad, onUnauthorized],
  );

  const loadTunnelPlans = useCallback(
    () =>
      loadTopologySource(
        "tunnelPlans",
        () => apiGet<TunnelPlanListItem[]>("/api/v1/tunnel-plans", apiToken),
        (items) => {
          setTunnelPlans(items.filter(isHealthyTunnelPlan));
          setTunnelPlanCorruptions(items.filter(isCorruptTunnelPlan));
        },
        () => {
          setTunnelPlans([]);
          setTunnelPlanCorruptions([]);
        },
        "Tunnel plans unavailable",
      ),
    [apiToken, loadTopologySource],
  );

  const queryNetworkObservations = useCallback(
    (query: NetworkEvidenceQuery = {}) =>
      apiGet<NetworkObservationRecord[]>(
        `/api/v1/network/observations?${buildNetworkEvidenceSearch(query)}`,
        apiToken,
      ),
    [apiToken],
  );

  const loadNetworkObservations = useCallback(
    (query: NetworkEvidenceQuery = {}, rethrow = false) => {
      networkObservationQuery.current = copyNetworkEvidenceQuery(query);
      return loadTopologySource(
        "networkObservations",
        () => queryNetworkObservations(query),
        setNetworkObservations,
        () => setNetworkObservations([]),
        "Network observations unavailable",
        rethrow,
      );
    },
    [loadTopologySource, queryNetworkObservations],
  );

  const loadNetworkAdapterDefinitions = useCallback(
    () =>
      loadTopologySource(
        "networkAdapterDefinitions",
        () =>
          apiGet<NetworkAdapterDefinitionRecord[]>(
            "/api/v1/network-adapter-definitions",
            apiToken,
          ),
        setNetworkAdapterDefinitions,
        () => setNetworkAdapterDefinitions([]),
        "Network adapter definitions unavailable",
      ),
    [apiToken, loadTopologySource],
  );

  const queryNetworkTrends = useCallback(
    (query: NetworkEvidenceQuery = {}) =>
      apiGet<NetworkObservationTrendRecord[]>(
        `/api/v1/network/observation-trends?${buildNetworkEvidenceSearch(query)}`,
        apiToken,
      ),
    [apiToken],
  );

  const loadNetworkTrends = useCallback(
    (query: NetworkEvidenceQuery = {}, rethrow = false) => {
      networkTrendQuery.current = copyNetworkEvidenceQuery(query);
      return loadTopologySource(
        "networkTrends",
        () => queryNetworkTrends(query),
        setNetworkTrends,
        () => setNetworkTrends([]),
        "Network trends unavailable",
        rethrow,
      );
    },
    [loadTopologySource, queryNetworkTrends],
  );

  const loadOspfRecommendations = useCallback(
    (rethrow = false) =>
      loadTopologySource(
        "ospfRecommendations",
        () =>
          apiGet<NetworkOspfRecommendationRecord[]>(
            `/api/v1/network/ospf-recommendations?limit=${TOPOLOGY_EVIDENCE_LIMIT}`,
            apiToken,
          ),
        setOspfRecommendations,
        () => setOspfRecommendations([]),
        "OSPF recommendations unavailable",
        rethrow,
      ),
    [apiToken, loadTopologySource],
  );

  const loadOspfUpdatePlans = useCallback(
    (rethrow = false) =>
      loadTopologySource(
        "ospfUpdatePlans",
        () =>
          apiGet<NetworkOspfUpdatePlanRecord[]>(
            `/api/v1/network/ospf-update-plans?limit=${TOPOLOGY_EVIDENCE_LIMIT}`,
            apiToken,
          ),
        setOspfUpdatePlans,
        () => setOspfUpdatePlans([]),
        "OSPF update plans unavailable",
        rethrow,
      ),
    [apiToken, loadTopologySource],
  );

  const loadTopologyGraph = useCallback(
    (query: NetworkEvidenceQuery = {}, rethrow = false) => {
      topologyGraphQuery.current = copyNetworkEvidenceQuery(query);
      return loadTopologySource(
        "topologyGraph",
        () =>
          apiGet<TopologyGraph>(
            `/api/v1/network/topology-graph?${buildNetworkEvidenceSearch({
              ...query,
              limit: query.limit ?? 240,
            })}`,
            apiToken,
          ),
        setTopologyGraph,
        () => setTopologyGraph(emptyTopologyGraph()),
        "Topology graph unavailable",
        rethrow,
      );
    },
    [apiToken, loadTopologySource],
  );

  const refreshRecentNetworkObservations = useCallback(
    (query: NetworkEvidenceQuery) => {
      if (query.window === "custom") {
        return Promise.resolve();
      }
      const recentQuery: NetworkEvidenceQuery = {
        ...query,
        endAt: undefined,
        limit: 10_000,
        startAt: undefined,
        window: "15m",
      };
      return loadTopologySource(
        "networkObservations",
        () => queryNetworkObservations(recentQuery),
        (recent) =>
          setNetworkObservations((current) =>
            mergeRecentNetworkObservations(current, recent, query),
          ),
        () => setNetworkObservations([]),
        "Network observations unavailable",
      );
    },
    [loadTopologySource, queryNetworkObservations],
  );

  const refreshNetworkEvidence = useCallback(
    async (includeTopologyGraph: boolean) => {
      const observationQuery = networkObservationQuery.current;
      const trendQuery = networkTrendQuery.current;
      const refreshShortRangeTrends = matchesLiveEvidenceWindow(
        trendQuery.window,
      );
      await Promise.all([
        refreshRecentNetworkObservations(observationQuery),
        ...(refreshShortRangeTrends ? [loadNetworkTrends(trendQuery)] : []),
        loadOspfRecommendations(),
        loadOspfUpdatePlans(),
        ...(includeTopologyGraph && topologyGraphQuery.current.window !== "custom"
          ? [loadTopologyGraph(topologyGraphQuery.current)]
          : []),
      ]);
    },
    [
      loadNetworkTrends,
      loadOspfRecommendations,
      loadOspfUpdatePlans,
      loadTopologyGraph,
      refreshRecentNetworkObservations,
    ],
  );

  const createTunnelPlan = useCallback(
    async (request: CreateTunnelPlanRequest) => {
      const response = await apiPost<TunnelPlanMutationResponse>("/api/v1/tunnel-plans", apiToken, request);
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged(), onRuntimeConfigChanged()]);
      return response;
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged, onRuntimeConfigChanged],
  );

  const createNetworkAdapterDefinition = useCallback(
    async (request: UpsertNetworkAdapterDefinitionRequest) => {
      const response = await apiPost<NetworkAdapterDefinitionRecord>(
        "/api/v1/network-adapter-definitions",
        apiToken,
        request,
      );
      await retainMutationSuccessAfterRefresh(() =>
        Promise.all([
          loadNetworkAdapterDefinitions(),
          onAuditChanged(),
        ]).then(() => undefined),
      );
      return response;
    },
    [apiToken, loadNetworkAdapterDefinitions, onAuditChanged],
  );

  const updateNetworkAdapterDefinition = useCallback(
    async (
      adapterId: string,
      request: UpsertNetworkAdapterDefinitionRequest,
    ) => {
      const response = await apiPut<NetworkAdapterDefinitionRecord>(
        `/api/v1/network-adapter-definitions/${encodeURIComponent(adapterId)}`,
        apiToken,
        request,
      );
      await retainMutationSuccessAfterRefresh(() =>
        Promise.all([
          loadNetworkAdapterDefinitions(),
          loadTunnelPlans(),
          onAuditChanged(),
        ]).then(() => undefined),
      );
      return response;
    },
    [
      apiToken,
      loadNetworkAdapterDefinitions,
      loadTunnelPlans,
      onAuditChanged,
    ],
  );

  const deleteNetworkAdapterDefinition = useCallback(
    async (adapterId: string) => {
      await apiDelete(
        `/api/v1/network-adapter-definitions/${encodeURIComponent(adapterId)}`,
        apiToken,
      );
      await retainMutationSuccessAfterRefresh(() =>
        Promise.all([
          loadNetworkAdapterDefinitions(),
          onAuditChanged(),
        ]).then(() => undefined),
      );
    },
    [apiToken, loadNetworkAdapterDefinitions, onAuditChanged],
  );

  const updateTunnelPlan = useCallback(
    async (planId: string, request: UpdateTunnelPlanRequest) => {
      const response = await apiPut<TunnelPlanMutationResponse>(
        `/api/v1/tunnel-plans/${encodeURIComponent(planId)}`,
        apiToken,
        request,
      );
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged(), onRuntimeConfigChanged()]);
      return response;
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged, onRuntimeConfigChanged],
  );

  const rotateTunnelPlanCredentials = useCallback(
    async (target: TunnelPlanRevisionTarget) => {
      const response = await apiPost<TunnelPlanMutationResponse>(
        `/api/v1/tunnel-plans/${encodeURIComponent(target.plan_id)}/credentials/rotate`,
        apiToken,
        { confirmed: true, expected_revision: target.expected_revision },
      );
      await Promise.all([
        loadTunnelPlans(),
        loadTopologyGraph(),
        loadOspfUpdatePlans(),
        onAuditChanged(),
        onRuntimeConfigChanged(),
      ]);
      return response;
    },
    [
      apiToken,
      loadOspfUpdatePlans,
      loadTopologyGraph,
      loadTunnelPlans,
      onAuditChanged,
      onRuntimeConfigChanged,
    ],
  );

  const allocateTunnelEndpoints = useCallback(
    async (request: AllocateTunnelEndpointsRequest) =>
      apiPost<AllocateTunnelEndpointsResponse>("/api/v1/tunnel-plans/allocate", apiToken, request),
    [apiToken],
  );

  const exportTunnelPlan = useCallback(
    async (planId: string) =>
      apiGet<TunnelPlanExport>(`/api/v1/tunnel-plans/${encodeURIComponent(planId)}/plan`, apiToken),
    [apiToken],
  );

  const setTunnelPlanEnabled = useCallback(
    async (targets: TunnelPlanRevisionTarget[], enabled: boolean) => {
      const mutationResults = await Promise.allSettled(
        targets.map((target) =>
          apiPost<TunnelPlanMutationResponse>(
            `/api/v1/tunnel-plans/${encodeURIComponent(target.plan_id)}/${enabled ? "enable" : "disable"}`,
            apiToken,
            { confirmed: true, expected_revision: target.expected_revision },
          ),
        ),
      );
      const refreshResults = await Promise.allSettled([
        loadTunnelPlans(),
        loadTopologyGraph(),
        loadOspfUpdatePlans(),
        onAuditChanged(),
        onRuntimeConfigChanged(),
      ]);
      const failures = mutationResults.flatMap((result, index) =>
        result.status === "rejected"
          ? [
              `Tunnel plan ${targets[index].plan_id}: ${
                result.reason instanceof Error
                  ? result.reason.message
                  : "mutation failed"
              }`,
            ]
          : [],
      );
      const refreshLabels = [
        "tunnel plans",
        "topology graph",
        "OSPF update plans",
        "audit log",
        "runtime configuration",
      ];
      failures.push(
        ...refreshResults.flatMap((result, index) =>
          result.status === "rejected"
            ? [
                `Refresh ${refreshLabels[index]}: ${
                  result.reason instanceof Error
                    ? result.reason.message
                    : "source unavailable"
                }`,
              ]
            : [],
        ),
      );
      if (failures.length > 0) {
        throw new Error(failures.join("; "));
      }
      const responses = mutationResults.flatMap((result) =>
        result.status === "fulfilled" ? [result.value] : [],
      );
      return responses;
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged, onRuntimeConfigChanged],
  );

  const deleteTunnelPlan = useCallback(
    async (target: TunnelPlanRevisionTarget) => {
      const response = await apiPost<TunnelPlanMutationResponse>(
        `/api/v1/tunnel-plans/${encodeURIComponent(target.plan_id)}/delete`,
        apiToken,
        { confirmed: true, expected_revision: target.expected_revision },
      );
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged(), onRuntimeConfigChanged()]);
      return response;
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged, onRuntimeConfigChanged],
  );

  const clearTunnelPlanEvidence = useCallback(
    async (
      targets: TunnelPlanRevisionTarget[],
    ): Promise<ClearTunnelPlanEvidenceOutcome> => {
      const request: ClearTunnelPlanEvidenceRequest = {
        confirmed: true,
        targets,
      };
      const response = await apiPost<ClearTunnelPlanEvidenceResponse>(
        "/api/v1/tunnel-plans/evidence/clear",
        apiToken,
        request,
      );
      const refreshes = await Promise.allSettled([
        loadNetworkObservations(networkObservationQuery.current, true),
        loadNetworkTrends(networkTrendQuery.current, true),
        loadOspfRecommendations(true),
        loadOspfUpdatePlans(true),
        loadTopologyGraph(topologyGraphQuery.current, true),
        onAuditChanged(),
      ]);
      const labels = [
        "network observations",
        "network trends",
        "OSPF recommendations",
        "OSPF update plans",
        "topology graph",
        "audit log",
      ];
      return {
        ...response,
        refresh_warnings: refreshes.flatMap((result, index) =>
          result.status === "rejected"
            ? [
                `${labels[index]}: ${
                  result.reason instanceof Error
                    ? result.reason.message
                    : "refresh failed"
                }`,
              ]
            : [],
        ),
      };
    },
    [
      apiToken,
      loadNetworkObservations,
      loadNetworkTrends,
      loadOspfRecommendations,
      loadOspfUpdatePlans,
      loadTopologyGraph,
      onAuditChanged,
    ],
  );

  const updateTunnelConnectionAssessment = useCallback(
    async (planId: string, request: UpdateTunnelConnectionAssessmentRequest) => {
      await apiPut<TunnelPlanRecord>(
        `/api/v1/tunnel-plans/${encodeURIComponent(planId)}/connection-assessment`,
        apiToken,
        request,
      );
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), onAuditChanged()]);
    },
    [apiToken, loadTopologyGraph, loadTunnelPlans, onAuditChanged],
  );

  const updateTunnelPlanOspfCost = useCallback(
    async (planId: string, request: UpdateTunnelPlanOspfCostRequest) => {
      const response = await apiPost<TunnelPlanOspfJobsResponse>(
        `/api/v1/tunnel-plans/${encodeURIComponent(planId)}/ospf-cost`,
        apiToken,
        request,
      );
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged],
  );

  const refreshTunnelPlanOspfStatus = useCallback(
    async (planId: string) => {
      const response = await apiPost<TunnelPlanOspfJobsResponse>(
        `/api/v1/tunnel-plans/${encodeURIComponent(planId)}/ospf-status`,
        apiToken,
        {},
      );
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged],
  );

  const clearTopology = useCallback(() => {
    apiTokenRef.current = "";
    for (const source of TOPOLOGY_SOURCE_ORDER) {
      topologyLoadGenerations.current[source] += 1;
    }
    topologyPendingLoads.current.clear();
    topologyErrors.current = {};
    setTunnelPlans([]);
    setNetworkAdapterDefinitions([]);
    setTunnelPlanCorruptions([]);
    setNetworkObservations([]);
    setNetworkTrends([]);
    setOspfRecommendations([]);
    setOspfUpdatePlans([]);
    setTopologyGraph(emptyTopologyGraph());
    setTopologyError(null);
    setTopologyLoading(false);
  }, []);

  return {
    allocateTunnelEndpoints,
    clearTunnelPlanEvidence,
    clearTopology,
    createTunnelPlan,
    createNetworkAdapterDefinition,
    deleteNetworkAdapterDefinition,
    deleteTunnelPlan,
    exportTunnelPlan,
    loadNetworkObservations,
    loadNetworkAdapterDefinitions,
    loadNetworkTrends,
    loadOspfRecommendations,
    loadOspfUpdatePlans,
    loadTopologyGraph,
    loadTunnelPlans,
    networkObservations,
    networkAdapterDefinitions,
    queryNetworkObservations,
    queryNetworkTrends,
    refreshNetworkEvidence,
    networkTrends,
    ospfRecommendations,
    ospfUpdatePlans,
    refreshTunnelPlanOspfStatus,
    rotateTunnelPlanCredentials,
    setTunnelPlanEnabled,
    updateTunnelConnectionAssessment,
    updateNetworkAdapterDefinition,
    updateTunnelPlanOspfCost,
    updateTunnelPlan,
    topologyError,
    topologyGraph,
    topologyLoading,
    tunnelPlans,
    tunnelPlanCorruptions,
  };
}

function copyNetworkEvidenceQuery(
  query: NetworkEvidenceQuery,
): NetworkEvidenceQuery {
  return {
    ...query,
    planIds: query.planIds ? [...query.planIds] : undefined,
  };
}

function emptyTopologyGraph(): TopologyGraph {
  return {
    edges: [],
    end_unix: 0,
    generated_at: "",
    nodes: [],
    start_unix: 0,
  };
}

function matchesLiveEvidenceWindow(
  window: NetworkEvidenceQuery["window"],
): boolean {
  return window === undefined || ["15m", "1h", "8h", "1d"].includes(window);
}

function mergeRecentNetworkObservations(
  current: NetworkObservationRecord[],
  recent: NetworkObservationRecord[],
  query: NetworkEvidenceQuery,
): NetworkObservationRecord[] {
  const byId = new Map(current.map((observation) => [observation.id, observation]));
  for (const observation of recent) {
    byId.set(observation.id, observation);
  }
  const merged = Array.from(byId.values()).sort(
    (left, right) =>
      Date.parse(right.observed_at) - Date.parse(left.observed_at) ||
      left.id.localeCompare(right.id),
  );
  const windowSecs = evidenceWindowSecs(query.window);
  const newestMillis = merged.reduce(
    (latest, observation) => Math.max(latest, Date.parse(observation.observed_at)),
    Number.NEGATIVE_INFINITY,
  );
  const retained =
    windowSecs === null || !Number.isFinite(newestMillis)
      ? merged
      : merged.filter(
          (observation) =>
            Date.parse(observation.observed_at) >= newestMillis - windowSecs * 1_000,
        );
  return retained.slice(0, query.limit ?? 100_000);
}

function evidenceWindowSecs(
  window: NetworkEvidenceQuery["window"],
): number | null {
  switch (window ?? "1d") {
    case "15m":
      return 15 * 60;
    case "1h":
      return 60 * 60;
    case "8h":
      return 8 * 60 * 60;
    case "1d":
      return 24 * 60 * 60;
    case "7d":
      return 7 * 24 * 60 * 60;
    case "30d":
      return 30 * 24 * 60 * 60;
    case "90d":
      return 90 * 24 * 60 * 60;
    case "180d":
      return 180 * 24 * 60 * 60;
    case "1y":
      return 365 * 24 * 60 * 60;
    case "all":
    case "custom":
      return null;
  }
}

function isCorruptTunnelPlan(
  plan: TunnelPlanListItem,
): plan is TunnelPlanCorruptRecord {
  return "configuration_error" in plan;
}

function isHealthyTunnelPlan(
  plan: TunnelPlanListItem,
): plan is TunnelPlanRecord {
  return !isCorruptTunnelPlan(plan);
}

function summarizeTopologyErrors(
  errors: Partial<Record<TopologySource, string>>,
): string | null {
  const entries = TOPOLOGY_SOURCE_ORDER.flatMap((source) => {
    const message = errors[source];
    return message ? [{ label: TOPOLOGY_SOURCE_LABELS[source], message }] : [];
  });
  if (entries.some((entry) => entry.message === "Operator login required")) {
    return "Operator login required";
  }
  const labelsByMessage = new Map<string, string[]>();
  for (const entry of entries) {
    const labels = labelsByMessage.get(entry.message) ?? [];
    labels.push(entry.label);
    labelsByMessage.set(entry.message, labels);
  }
  const failures = Array.from(labelsByMessage, ([message, labels]) =>
    `${labels.join(", ")}: ${message}`,
  );
  return failures.length > 0 ? failures.join(" · ") : null;
}
