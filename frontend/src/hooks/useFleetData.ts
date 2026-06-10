import { useCallback, useState } from "react";
import { apiDelete, apiGet, apiPost, isApiUnauthorized } from "../api";
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
  WebhookDeliveryRotationRequest,
  WebhookDeliveryRotationResponse,
  WebhookRuleDeliveryRecord,
  WebhookRuleDispatchRequest,
  WebhookRuleDryRunRecord,
  WebhookRuleDryRunRequest,
  WebhookRuleProcessRequest,
  WebhookRuleRecord,
  WebhookRuleRequest,
  DeleteAgentRequest,
  DeleteAgentResponse,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
} from "../types";

const FLEET_DETAIL_LIMIT = 200;

export function useFleetData(apiToken: string, onUnauthorized: () => void) {
  const [summary, setSummary] = useState<FleetSummary>(emptySummary);
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [fleetAlerts, setFleetAlerts] = useState<FleetAlertRecord[]>([]);
  const [fleetAlertStates, setFleetAlertStates] = useState<
    FleetAlertStateRecord[]
  >([]);
  const [fleetAlertPolicies, setFleetAlertPolicies] = useState<
    FleetAlertPolicyRecord[]
  >([]);
  const [fleetAlertNotificationChannels, setFleetAlertNotificationChannels] =
    useState<FleetAlertNotificationChannelRecord[]>([]);
  const [fleetAlertNotifications, setFleetAlertNotifications] = useState<
    FleetAlertNotificationDeliveryRecord[]
  >([]);
  const [webhookRules, setWebhookRules] = useState<WebhookRuleRecord[]>([]);
  const [webhookRuleDeliveries, setWebhookRuleDeliveries] = useState<
    WebhookRuleDeliveryRecord[]
  >([]);
  const [telemetryRollups, setTelemetryRollups] = useState<
    TelemetryRollupRecord[]
  >([]);
  const [telemetryNetworkRates, setTelemetryNetworkRates] = useState<
    TelemetryNetworkRateRecord[]
  >([]);
  const [telemetryTunnels, setTelemetryTunnels] = useState<
    TelemetryTunnelRecord[]
  >([]);
  const [apiError, setApiError] = useState<string | null>(null);

  const loadFleet = useCallback(async () => {
    try {
      const [nextSummary, nextAgents] = await Promise.all([
        apiGet<FleetSummary>("/api/v1/fleet/summary", apiToken),
        apiGet<AgentView[]>("/api/v1/agents", apiToken),
      ]);
      const optionalResults = await Promise.allSettled([
        apiGet<FleetAlertRecord[]>(
          `/api/v1/fleet-alerts?limit=${FLEET_DETAIL_LIMIT}&include_muted=true`,
          apiToken,
        ),
        apiGet<FleetAlertStateRecord[]>(
          `/api/v1/fleet-alert-states?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<FleetAlertPolicyRecord[]>(
          `/api/v1/fleet-alert-policies?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<FleetAlertNotificationChannelRecord[]>(
          `/api/v1/fleet-alert-notification-channels?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<FleetAlertNotificationDeliveryRecord[]>(
          `/api/v1/fleet-alert-notifications?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<WebhookRuleRecord[]>(
          `/api/v1/webhook-rules?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<WebhookRuleDeliveryRecord[]>(
          `/api/v1/webhook-deliveries?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<TelemetryRollupRecord[]>(
          `/api/v1/telemetry/rollups?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<TelemetryNetworkRateRecord[]>(
          `/api/v1/telemetry/network-rates?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<TelemetryTunnelRecord[]>(
          `/api/v1/telemetry/tunnels?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
      ]);

      const optionalFailure = optionalResults.find(
        (result) => result.status === "rejected",
      );
      if (
        optionalFailure?.status === "rejected" &&
        isApiUnauthorized(optionalFailure.reason)
      ) {
        throw optionalFailure.reason;
      }

      const valueAt = <T>(index: number, fallback: T): T => {
        const result = optionalResults[index];
        return result.status === "fulfilled" ? (result.value as T) : fallback;
      };

      setSummary(nextSummary);
      setAgents(nextAgents);
      setFleetAlerts(valueAt<FleetAlertRecord[]>(0, []));
      setFleetAlertStates(valueAt<FleetAlertStateRecord[]>(1, []));
      setFleetAlertPolicies(valueAt<FleetAlertPolicyRecord[]>(2, []));
      setFleetAlertNotificationChannels(
        valueAt<FleetAlertNotificationChannelRecord[]>(3, []),
      );
      setFleetAlertNotifications(
        valueAt<FleetAlertNotificationDeliveryRecord[]>(4, []),
      );
      setWebhookRules(valueAt<WebhookRuleRecord[]>(5, []));
      setWebhookRuleDeliveries(valueAt<WebhookRuleDeliveryRecord[]>(6, []));
      setTelemetryRollups(valueAt<TelemetryRollupRecord[]>(7, []));
      setTelemetryNetworkRates(valueAt<TelemetryNetworkRateRecord[]>(8, []));
      setTelemetryTunnels(valueAt<TelemetryTunnelRecord[]>(9, []));
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
        setWebhookRules([]);
        setWebhookRuleDeliveries([]);
        setTelemetryRollups([]);
        setTelemetryNetworkRates([]);
        setTelemetryTunnels([]);
        setApiError("Operator login required");
        return;
      }
      setApiError(error instanceof Error ? error.message : "API unavailable");
    }
  }, [apiToken, onUnauthorized]);

  const replaceFleetSnapshot = useCallback(
    (nextSummary: FleetSummary, nextAgents: AgentView[]) => {
      setSummary(nextSummary);
      setAgents(nextAgents);
      setApiError(null);
    },
    [],
  );

  const updateAgentAlias = useCallback(
    async (clientId: string, displayName: string) => {
      const agent = await apiPost<AgentView>(
        `/api/v1/agents/${encodeURIComponent(clientId)}/alias`,
        apiToken,
        {
          display_name: displayName,
        },
      );
      setAgents((current) =>
        current.map((stored) => (stored.id === agent.id ? agent : stored)),
      );
      await loadFleet();
      return agent;
    },
    [apiToken, loadFleet],
  );

  const deleteAgent = useCallback(
    async (clientId: string, request: DeleteAgentRequest) => {
      const response = await apiPost<DeleteAgentResponse>(
        `/api/v1/agents/${encodeURIComponent(clientId)}/delete`,
        apiToken,
        request,
      );
      setAgents((current) =>
        current.filter((agent) => agent.id !== response.client_id),
      );
      await loadFleet();
      return response;
    },
    [apiToken, loadFleet],
  );

  const upsertFleetAlertPolicy = useCallback(
    async (request: FleetAlertPolicyRequest) => {
      const policy = await apiPost<FleetAlertPolicyRecord>(
        "/api/v1/fleet-alert-policies",
        apiToken,
        request,
      );
      setFleetAlertPolicies((current) => {
        const withoutPolicy = current.filter(
          (stored) => stored.id !== policy.id && stored.name !== policy.name,
        );
        return [policy, ...withoutPolicy].sort(
          (left, right) =>
            right.priority - left.priority ||
            left.name.localeCompare(right.name),
        );
      });
      await loadFleet();
      return policy;
    },
    [apiToken, loadFleet],
  );

  const deleteFleetAlertPolicy = useCallback(
    async (policyId: string) => {
      await apiDelete(
        `/api/v1/fleet-alert-policies/${encodeURIComponent(policyId)}`,
        apiToken,
      );
      setFleetAlertPolicies((current) =>
        current.filter((policy) => policy.id !== policyId),
      );
      await loadFleet();
    },
    [apiToken, loadFleet],
  );

  const updateFleetAlertState = useCallback(
    async (request: FleetAlertStateRequest) => {
      const state = await apiPost<FleetAlertStateRecord>(
        "/api/v1/fleet-alert-states",
        apiToken,
        request,
      );
      setFleetAlertStates((current) => {
        const withoutState = current.filter(
          (stored) => stored.alert_id !== state.alert_id,
        );
        return [state, ...withoutState].sort((left, right) =>
          right.updated_at.localeCompare(left.updated_at),
        );
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
        const withoutChannel = current.filter(
          (stored) => stored.id !== channel.id && stored.name !== channel.name,
        );
        return [channel, ...withoutChannel].sort((left, right) =>
          left.name.localeCompare(right.name),
        );
      });
      await loadFleet();
      return channel;
    },
    [apiToken, loadFleet],
  );

  const deleteFleetAlertNotificationChannel = useCallback(
    async (channelId: string) => {
      await apiDelete(
        `/api/v1/fleet-alert-notification-channels/${encodeURIComponent(channelId)}`,
        apiToken,
      );
      setFleetAlertNotificationChannels((current) =>
        current.filter((channel) => channel.id !== channelId),
      );
      await loadFleet();
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
          return [
            ...deliveries,
            ...current.filter((delivery) => !seen.has(delivery.id)),
          ].sort((left, right) =>
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
          const nextById = new Map(
            current.map((delivery) => [delivery.id, delivery]),
          );
          for (const delivery of deliveries) {
            nextById.set(delivery.id, delivery);
          }
          return Array.from(nextById.values()).sort((left, right) =>
            right.created_at.localeCompare(left.created_at),
          );
        });
        await loadFleet();
      }
      return deliveries;
    },
    [apiToken, loadFleet],
  );

  const upsertWebhookRule = useCallback(
    async (request: WebhookRuleRequest) => {
      const rule = await apiPost<WebhookRuleRecord>(
        "/api/v1/webhook-rules",
        apiToken,
        request,
      );
      setWebhookRules((current) => {
        const withoutRule = current.filter(
          (stored) => stored.id !== rule.id && stored.name !== rule.name,
        );
        return [rule, ...withoutRule].sort((left, right) =>
          left.name.localeCompare(right.name),
        );
      });
      void loadFleet();
      return rule;
    },
    [apiToken, loadFleet],
  );

  const deleteWebhookRule = useCallback(
    async (ruleId: string) => {
      await apiDelete(
        `/api/v1/webhook-rules/${encodeURIComponent(ruleId)}`,
        apiToken,
      );
      setWebhookRules((current) =>
        current.filter((rule) => rule.id !== ruleId),
      );
      setWebhookRuleDeliveries((current) =>
        current.filter((delivery) => delivery.rule_id !== ruleId),
      );
      await loadFleet();
    },
    [apiToken, loadFleet],
  );

  const dryRunWebhookRule = useCallback(
    async (request: WebhookRuleDryRunRequest) =>
      apiPost<WebhookRuleDryRunRecord>(
        "/api/v1/webhook-rules/dry-run",
        apiToken,
        request,
      ),
    [apiToken],
  );

  const dispatchWebhookRules = useCallback(
    async (request: WebhookRuleDispatchRequest) => {
      const deliveries = await apiPost<WebhookRuleDeliveryRecord[]>(
        "/api/v1/webhook-rules/dispatch",
        apiToken,
        request,
      );
      if (!request.dry_run) {
        setWebhookRuleDeliveries((current) => {
          const seen = new Set(deliveries.map((delivery) => delivery.id));
          return [
            ...deliveries,
            ...current.filter((delivery) => !seen.has(delivery.id)),
          ].sort((left, right) =>
            right.created_at.localeCompare(left.created_at),
          );
        });
        await loadFleet();
      }
      return deliveries;
    },
    [apiToken, loadFleet],
  );

  const processWebhookRuleDeliveries = useCallback(
    async (request: WebhookRuleProcessRequest) => {
      const deliveries = await apiPost<WebhookRuleDeliveryRecord[]>(
        "/api/v1/webhook-deliveries/process",
        apiToken,
        request,
      );
      if (!request.dry_run) {
        setWebhookRuleDeliveries((current) => {
          const nextById = new Map(
            current.map((delivery) => [delivery.id, delivery]),
          );
          for (const delivery of deliveries) {
            nextById.set(delivery.id, delivery);
          }
          return Array.from(nextById.values()).sort((left, right) =>
            right.created_at.localeCompare(left.created_at),
          );
        });
        await loadFleet();
      }
      return deliveries;
    },
    [apiToken, loadFleet],
  );

  const rotateWebhookDeliveryHistory = useCallback(
    async (request: WebhookDeliveryRotationRequest) => {
      const response = await apiPost<WebhookDeliveryRotationResponse>(
        "/api/v1/webhook-deliveries/rotate",
        apiToken,
        request,
      );
      if (request.confirmed) {
        await loadFleet();
      }
      return response;
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
    setWebhookRules([]);
    setWebhookRuleDeliveries([]);
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
    webhookRules,
    webhookRuleDeliveries,
    deleteAgent,
    loadFleet,
    replaceFleetSnapshot,
    updateAgentAlias,
    summary,
    telemetryNetworkRates,
    telemetryRollups,
    telemetryTunnels,
    upsertFleetAlertPolicy,
    deleteFleetAlertPolicy,
    upsertFleetAlertNotificationChannel,
    deleteFleetAlertNotificationChannel,
    dispatchFleetAlertNotifications,
    processFleetAlertNotifications,
    upsertWebhookRule,
    deleteWebhookRule,
    dryRunWebhookRule,
    dispatchWebhookRules,
    processWebhookRuleDeliveries,
    rotateWebhookDeliveryHistory,
    updateFleetAlertState,
  };
}
