import { useCallback, useState } from "react";
import { apiGet, apiPost, isApiUnauthorized } from "../api";
import { emptySummary } from "../constants";
import type {
  AgentView,
  FleetAlertPolicyRecord,
  FleetAlertPolicyRequest,
  FleetAlertRecord,
  FleetAlertNotificationChannelRecord,
  FleetAlertNotificationChannelRequest,
  FleetAlertNotificationDeliveryRecord,
  FleetAlertNotificationDispatchRequest,
  FleetAlertNotificationProcessRequest,
  FleetAlertStateRecord,
  FleetAlertStateRequest,
  FleetSummary,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
} from "../types";

export function useFleetData(apiToken: string, onUnauthorized: () => void) {
  const [summary, setSummary] = useState<FleetSummary>(emptySummary);
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [fleetAlerts, setFleetAlerts] = useState<FleetAlertRecord[]>([]);
  const [fleetAlertStates, setFleetAlertStates] = useState<FleetAlertStateRecord[]>([]);
  const [fleetAlertPolicies, setFleetAlertPolicies] = useState<FleetAlertPolicyRecord[]>([]);
  const [fleetAlertNotificationChannels, setFleetAlertNotificationChannels] = useState<
    FleetAlertNotificationChannelRecord[]
  >([]);
  const [fleetAlertNotifications, setFleetAlertNotifications] = useState<FleetAlertNotificationDeliveryRecord[]>([]);
  const [telemetryRollups, setTelemetryRollups] = useState<TelemetryRollupRecord[]>([]);
  const [telemetryNetworkRates, setTelemetryNetworkRates] = useState<TelemetryNetworkRateRecord[]>([]);
  const [telemetryTunnels, setTelemetryTunnels] = useState<TelemetryTunnelRecord[]>([]);
  const [apiError, setApiError] = useState<string | null>(null);

  const loadFleet = useCallback(async () => {
    try {
      const [nextSummary, nextAgents] = await Promise.all([
        apiGet<FleetSummary>("/api/v1/fleet/summary", apiToken),
        apiGet<AgentView[]>("/api/v1/agents", apiToken),
      ]);
      const optionalResults = await Promise.allSettled([
        apiGet<FleetAlertRecord[]>("/api/v1/fleet-alerts?limit=1000&include_muted=true", apiToken),
        apiGet<FleetAlertStateRecord[]>("/api/v1/fleet-alert-states?limit=1000", apiToken),
        apiGet<FleetAlertPolicyRecord[]>("/api/v1/fleet-alert-policies?limit=1000", apiToken),
        apiGet<FleetAlertNotificationChannelRecord[]>("/api/v1/fleet-alert-notification-channels?limit=1000", apiToken),
        apiGet<FleetAlertNotificationDeliveryRecord[]>("/api/v1/fleet-alert-notifications?limit=1000", apiToken),
        apiGet<TelemetryRollupRecord[]>("/api/v1/telemetry/rollups?limit=1000", apiToken),
        apiGet<TelemetryNetworkRateRecord[]>("/api/v1/telemetry/network-rates?limit=1000", apiToken),
        apiGet<TelemetryTunnelRecord[]>("/api/v1/telemetry/tunnels?limit=1000", apiToken),
      ]);

      const optionalFailure = optionalResults.find((result) => result.status === "rejected");
      if (optionalFailure?.status === "rejected" && isApiUnauthorized(optionalFailure.reason)) {
        throw optionalFailure.reason;
      }

      const valueAt = <T,>(index: number, fallback: T): T => {
        const result = optionalResults[index];
        return result.status === "fulfilled" ? (result.value as T) : fallback;
      };

      setSummary(nextSummary);
      setAgents(nextAgents);
      setFleetAlerts(valueAt<FleetAlertRecord[]>(0, []));
      setFleetAlertStates(valueAt<FleetAlertStateRecord[]>(1, []));
      setFleetAlertPolicies(valueAt<FleetAlertPolicyRecord[]>(2, []));
      setFleetAlertNotificationChannels(valueAt<FleetAlertNotificationChannelRecord[]>(3, []));
      setFleetAlertNotifications(valueAt<FleetAlertNotificationDeliveryRecord[]>(4, []));
      setTelemetryRollups(valueAt<TelemetryRollupRecord[]>(5, []));
      setTelemetryNetworkRates(valueAt<TelemetryNetworkRateRecord[]>(6, []));
      setTelemetryTunnels(valueAt<TelemetryTunnelRecord[]>(7, []));
      setApiError(
        optionalFailure?.status === "rejected"
          ? optionalFailure.reason instanceof Error
            ? optionalFailure.reason.message
            : "Some fleet details are unavailable"
          : null,
      );
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setSummary(emptySummary);
        setAgents([]);
        setFleetAlerts([]);
        setFleetAlertStates([]);
        setFleetAlertPolicies([]);
        setFleetAlertNotificationChannels([]);
        setFleetAlertNotifications([]);
        setTelemetryRollups([]);
        setTelemetryNetworkRates([]);
        setTelemetryTunnels([]);
        setApiError("Operator login required");
        return;
      }
      setApiError(error instanceof Error ? error.message : "API unavailable");
    }
  }, [apiToken, onUnauthorized]);

  const replaceFleetSnapshot = useCallback((nextSummary: FleetSummary, nextAgents: AgentView[]) => {
    setSummary(nextSummary);
    setAgents(nextAgents);
    setApiError(null);
  }, []);

  const updateAgentAlias = useCallback(
    async (clientId: string, displayName: string) => {
      const agent = await apiPost<AgentView>(`/api/v1/agents/${encodeURIComponent(clientId)}/alias`, apiToken, {
        display_name: displayName,
      });
      setAgents((current) => current.map((stored) => (stored.id === agent.id ? agent : stored)));
      await loadFleet();
      return agent;
    },
    [apiToken, loadFleet],
  );

  const upsertFleetAlertPolicy = useCallback(
    async (request: FleetAlertPolicyRequest) => {
      const policy = await apiPost<FleetAlertPolicyRecord>("/api/v1/fleet-alert-policies", apiToken, request);
      setFleetAlertPolicies((current) => {
        const withoutPolicy = current.filter((stored) => stored.id !== policy.id && stored.name !== policy.name);
        return [policy, ...withoutPolicy].sort((left, right) => right.priority - left.priority || left.name.localeCompare(right.name));
      });
      await loadFleet();
      return policy;
    },
    [apiToken, loadFleet],
  );

  const updateFleetAlertState = useCallback(
    async (request: FleetAlertStateRequest) => {
      const state = await apiPost<FleetAlertStateRecord>("/api/v1/fleet-alert-states", apiToken, request);
      setFleetAlertStates((current) => {
        const withoutState = current.filter((stored) => stored.alert_id !== state.alert_id);
        return [state, ...withoutState].sort((left, right) => right.updated_at.localeCompare(left.updated_at));
      });
      await loadFleet();
      return state;
    },
    [apiToken, loadFleet],
  );

  const upsertFleetAlertNotificationChannel = useCallback(
    async (request: FleetAlertNotificationChannelRequest) => {
      const channel = await apiPost<FleetAlertNotificationChannelRecord>(
        "/api/v1/fleet-alert-notification-channels",
        apiToken,
        request,
      );
      setFleetAlertNotificationChannels((current) => {
        const withoutChannel = current.filter((stored) => stored.id !== channel.id && stored.name !== channel.name);
        return [channel, ...withoutChannel].sort((left, right) => left.name.localeCompare(right.name));
      });
      await loadFleet();
      return channel;
    },
    [apiToken, loadFleet],
  );

  const dispatchFleetAlertNotifications = useCallback(
    async (request: FleetAlertNotificationDispatchRequest) => {
      const deliveries = await apiPost<FleetAlertNotificationDeliveryRecord[]>(
        "/api/v1/fleet-alert-notifications/dispatch",
        apiToken,
        request,
      );
      if (!request.dry_run) {
        setFleetAlertNotifications((current) => {
          const seen = new Set(deliveries.map((delivery) => delivery.id));
          return [...deliveries, ...current.filter((delivery) => !seen.has(delivery.id))].sort((left, right) =>
            right.created_at.localeCompare(left.created_at),
          );
        });
        await loadFleet();
      }
      return deliveries;
    },
    [apiToken, loadFleet],
  );

  const processFleetAlertNotifications = useCallback(
    async (request: FleetAlertNotificationProcessRequest) => {
      const deliveries = await apiPost<FleetAlertNotificationDeliveryRecord[]>(
        "/api/v1/fleet-alert-notifications/process",
        apiToken,
        request,
      );
      if (!request.dry_run) {
        setFleetAlertNotifications((current) => {
          const nextById = new Map(current.map((delivery) => [delivery.id, delivery]));
          for (const delivery of deliveries) {
            nextById.set(delivery.id, delivery);
          }
          return Array.from(nextById.values()).sort((left, right) => right.created_at.localeCompare(left.created_at));
        });
        await loadFleet();
      }
      return deliveries;
    },
    [apiToken, loadFleet],
  );

  const clearFleet = useCallback(() => {
    setSummary(emptySummary);
    setAgents([]);
    setFleetAlerts([]);
    setFleetAlertStates([]);
    setFleetAlertPolicies([]);
    setFleetAlertNotificationChannels([]);
    setFleetAlertNotifications([]);
    setTelemetryRollups([]);
    setTelemetryNetworkRates([]);
    setTelemetryTunnels([]);
  }, []);

  return {
    agents,
    apiError,
    clearFleet,
    fleetAlerts,
    fleetAlertStates,
    fleetAlertPolicies,
    fleetAlertNotificationChannels,
    fleetAlertNotifications,
    loadFleet,
    replaceFleetSnapshot,
    updateAgentAlias,
    summary,
    telemetryNetworkRates,
    telemetryRollups,
    telemetryTunnels,
    upsertFleetAlertPolicy,
    upsertFleetAlertNotificationChannel,
    dispatchFleetAlertNotifications,
    processFleetAlertNotifications,
    updateFleetAlertState,
  };
}
