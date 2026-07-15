import { useCallback, useState } from "react";
import { apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import type {
  AllocateTunnelEndpointsRequest,
  AllocateTunnelEndpointsResponse,
  CreateTunnelPlanRequest,
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  TunnelPlan,
  TopologyGraph,
  TunnelPlanOspfJobsResponse,
  TunnelPlanRecord,
  TunnelPlanMutationResponse,
  TunnelPlanRevisionTarget,
  UpdateTunnelConnectionAssessmentRequest,
  UpdateTunnelPlanOspfCostRequest,
  UpdateTunnelPlanRequest,
} from "../types";

export function useTopologyData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
  onRuntimeConfigChanged: () => Promise<void>,
) {
  const [tunnelPlans, setTunnelPlans] = useState<TunnelPlanRecord[]>([]);
  const [networkObservations, setNetworkObservations] = useState<NetworkObservationRecord[]>([]);
  const [networkTrends, setNetworkTrends] = useState<NetworkObservationTrendRecord[]>([]);
  const [ospfRecommendations, setOspfRecommendations] = useState<NetworkOspfRecommendationRecord[]>([]);
  const [ospfUpdatePlans, setOspfUpdatePlans] = useState<NetworkOspfUpdatePlanRecord[]>([]);
  const [topologyGraph, setTopologyGraph] = useState<TopologyGraph>({ nodes: [], edges: [], generated_at: "" });
  const [topologyError, setTopologyError] = useState<string | null>(null);
  const [topologyLoading, setTopologyLoading] = useState(false);

  const loadTunnelPlans = useCallback(async () => {
    setTopologyLoading(true);
    setTopologyError(null);
    try {
      setTunnelPlans(await apiGet<TunnelPlanRecord[]>("/api/v1/tunnel-plans", apiToken));
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setTunnelPlans([]);
        setTopologyError("Operator login required");
        return;
      }
      setTopologyError(error instanceof Error ? error.message : "Tunnel plans unavailable");
    } finally {
      setTopologyLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const loadNetworkObservations = useCallback(async () => {
    try {
      setNetworkObservations(await apiGet<NetworkObservationRecord[]>("/api/v1/network/observations?limit=50", apiToken));
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setNetworkObservations([]);
        setTopologyError("Operator login required");
        return;
      }
      setTopologyError(error instanceof Error ? error.message : "Network observations unavailable");
    }
  }, [apiToken, onUnauthorized]);

  const loadNetworkTrends = useCallback(async () => {
    try {
      setNetworkTrends(
        await apiGet<NetworkObservationTrendRecord[]>("/api/v1/network/observation-trends?limit=50", apiToken),
      );
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setNetworkTrends([]);
        setTopologyError("Operator login required");
        return;
      }
      setTopologyError(error instanceof Error ? error.message : "Network trends unavailable");
    }
  }, [apiToken, onUnauthorized]);

  const loadOspfRecommendations = useCallback(async () => {
    try {
      setOspfRecommendations(
        await apiGet<NetworkOspfRecommendationRecord[]>("/api/v1/network/ospf-recommendations?limit=50", apiToken),
      );
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setOspfRecommendations([]);
        setTopologyError("Operator login required");
        return;
      }
      setTopologyError(error instanceof Error ? error.message : "OSPF recommendations unavailable");
    }
  }, [apiToken, onUnauthorized]);

  const loadOspfUpdatePlans = useCallback(async () => {
    try {
      setOspfUpdatePlans(
        await apiGet<NetworkOspfUpdatePlanRecord[]>("/api/v1/network/ospf-update-plans?limit=50", apiToken),
      );
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setOspfUpdatePlans([]);
        setTopologyError("Operator login required");
        return;
      }
      setTopologyError(error instanceof Error ? error.message : "OSPF update plans unavailable");
    }
  }, [apiToken, onUnauthorized]);

  const loadTopologyGraph = useCallback(async () => {
    try {
      setTopologyGraph(await apiGet<TopologyGraph>("/api/v1/network/topology-graph?limit=1000", apiToken));
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setTopologyGraph({ nodes: [], edges: [], generated_at: "" });
        setTopologyError("Operator login required");
        return;
      }
      setTopologyError(error instanceof Error ? error.message : "Topology graph unavailable");
    }
  }, [apiToken, onUnauthorized]);

  const createTunnelPlan = useCallback(
    async (request: CreateTunnelPlanRequest) => {
      const response = await apiPost<TunnelPlanMutationResponse>("/api/v1/tunnel-plans", apiToken, request);
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged(), onRuntimeConfigChanged()]);
      return response;
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged, onRuntimeConfigChanged],
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

  const allocateTunnelEndpoints = useCallback(
    async (request: AllocateTunnelEndpointsRequest) =>
      apiPost<AllocateTunnelEndpointsResponse>("/api/v1/tunnel-plans/allocate", apiToken, request),
    [apiToken],
  );

  const exportTunnelPlan = useCallback(
    async (planId: string) =>
      apiGet<TunnelPlan>(`/api/v1/tunnel-plans/${encodeURIComponent(planId)}/plan`, apiToken),
    [apiToken],
  );

  const setTunnelPlanEnabled = useCallback(
    async (targets: TunnelPlanRevisionTarget[], enabled: boolean) => {
      const responses = await Promise.all(
        targets.map((target) =>
          apiPost<TunnelPlanMutationResponse>(
            `/api/v1/tunnel-plans/${encodeURIComponent(target.plan_id)}/${enabled ? "enable" : "disable"}`,
            apiToken,
            { confirmed: true, expected_revision: target.expected_revision },
          ),
        ),
      );
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged(), onRuntimeConfigChanged()]);
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

  return {
    allocateTunnelEndpoints,
    createTunnelPlan,
    deleteTunnelPlan,
    exportTunnelPlan,
    loadNetworkObservations,
    loadNetworkTrends,
    loadOspfRecommendations,
    loadOspfUpdatePlans,
    loadTopologyGraph,
    loadTunnelPlans,
    networkObservations,
    networkTrends,
    ospfRecommendations,
    ospfUpdatePlans,
    refreshTunnelPlanOspfStatus,
    setTunnelPlanEnabled,
    updateTunnelConnectionAssessment,
    updateTunnelPlanOspfCost,
    updateTunnelPlan,
    topologyError,
    topologyGraph,
    topologyLoading,
    tunnelPlans,
  };
}
