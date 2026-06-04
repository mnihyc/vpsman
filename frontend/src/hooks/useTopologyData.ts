import { useCallback, useState } from "react";
import { apiGet, apiPost, isApiUnauthorized } from "../api";
import type {
  CreateTunnelPlanRequest,
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  PromoteTelemetryTunnelRequest,
  TopologyGraph,
  TunnelPlanRecord,
} from "../types";
import type { PromoteTunnelPlanToAdapterRequest } from "../typesTopology";

export function useTopologyData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
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
      setTopologyGraph(await apiGet<TopologyGraph>("/api/v1/network/topology-graph?limit=100", apiToken));
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
      await apiPost<TunnelPlanRecord>("/api/v1/tunnel-plans", apiToken, request);
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged()]);
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged],
  );

  const promoteTelemetryTunnel = useCallback(
    async (request: PromoteTelemetryTunnelRequest) => {
      await apiPost<TunnelPlanRecord>("/api/v1/tunnel-plans/promote-telemetry", apiToken, request);
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged()]);
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged],
  );

  const promoteTunnelPlanToAdapter = useCallback(
    async (request: PromoteTunnelPlanToAdapterRequest) => {
      await apiPost<TunnelPlanRecord>("/api/v1/tunnel-plans/promote-adapter", apiToken, request);
      await Promise.all([loadTunnelPlans(), loadTopologyGraph(), loadOspfUpdatePlans(), onAuditChanged()]);
    },
    [apiToken, loadOspfUpdatePlans, loadTopologyGraph, loadTunnelPlans, onAuditChanged],
  );

  return {
    createTunnelPlan,
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
    promoteTelemetryTunnel,
    promoteTunnelPlanToAdapter,
    topologyError,
    topologyGraph,
    topologyLoading,
    tunnelPlans,
  };
}
