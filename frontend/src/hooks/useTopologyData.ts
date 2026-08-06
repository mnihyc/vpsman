import { useCallback, useRef, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import { TOPOLOGY_EVIDENCE_LIMIT } from "../constants";
import type {
  AllocateTunnelEndpointsRequest,
  AllocateTunnelEndpointsResponse,
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
  const [topologyGraph, setTopologyGraph] = useState<TopologyGraph>({ nodes: [], edges: [], generated_at: "" });
  const [topologyError, setTopologyError] = useState<string | null>(null);
  const [topologyLoading, setTopologyLoading] = useState(false);
  const topologyErrors = useRef<Partial<Record<TopologySource, string>>>({});
  const topologyPendingLoads = useRef(new Set<string>());
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

  const loadNetworkObservations = useCallback(
    () =>
      loadTopologySource(
        "networkObservations",
        () =>
          apiGet<NetworkObservationRecord[]>(
            `/api/v1/network/observations?limit=${TOPOLOGY_EVIDENCE_LIMIT}`,
            apiToken,
          ),
        setNetworkObservations,
        () => setNetworkObservations([]),
        "Network observations unavailable",
      ),
    [apiToken, loadTopologySource],
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

  const loadNetworkTrends = useCallback(
    () =>
      loadTopologySource(
        "networkTrends",
        () =>
          apiGet<NetworkObservationTrendRecord[]>(
            `/api/v1/network/observation-trends?limit=${TOPOLOGY_EVIDENCE_LIMIT}`,
            apiToken,
          ),
        setNetworkTrends,
        () => setNetworkTrends([]),
        "Network trends unavailable",
      ),
    [apiToken, loadTopologySource],
  );

  const loadOspfRecommendations = useCallback(
    () =>
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
      ),
    [apiToken, loadTopologySource],
  );

  const loadOspfUpdatePlans = useCallback(
    () =>
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
      ),
    [apiToken, loadTopologySource],
  );

  const loadTopologyGraph = useCallback(
    () =>
      loadTopologySource(
        "topologyGraph",
        () =>
          apiGet<TopologyGraph>(
            "/api/v1/network/topology-graph?limit=1000",
            apiToken,
          ),
        setTopologyGraph,
        () => setTopologyGraph({ nodes: [], edges: [], generated_at: "" }),
        "Topology graph unavailable",
      ),
    [apiToken, loadTopologySource],
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
    setTopologyGraph({ nodes: [], edges: [], generated_at: "" });
    setTopologyError(null);
    setTopologyLoading(false);
  }, []);

  return {
    allocateTunnelEndpoints,
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
