import { useCallback, useRef, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPostPreview, isApiUnauthorized } from "../api";
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
  PolicyAlertRecord,
  PolicyDryRunRequest,
  PolicyDryRunResponse,
  TrafficAccountingRecord,
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
  VpsRuleValueRecord,
  VpsRulesBulkUnsetRequest,
  VpsRulesBulkUpsertRequest,
  VpsRulesDryRunRequest,
  VpsRulesDryRunResponse,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
} from "../types";

const FLEET_DETAIL_LIMIT = 200;
const FLEET_TELEMETRY_SNAPSHOT_LIMIT = 5_000;

export function useFleetData(apiToken: string, onUnauthorized: () => void) {
  const apiTokenRef = useRef(apiToken);
  const fleetFullGeneration = useRef(0);
  const fleetSnapshotGeneration = useRef(0);
  const fleetTelemetryInFlight = useRef<Promise<void> | null>(null);
  apiTokenRef.current = apiToken;
  const [summary, setSummary] = useState<FleetSummary>(emptySummary);
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [fleetAlerts, setFleetAlerts] = useState<FleetAlertRecord[]>([]);
  const [fleetAlertStates, setFleetAlertStates] = useState<
    FleetAlertStateRecord[]
  >([]);
  const [fleetAlertPolicies, setFleetAlertPolicies] = useState<
    FleetAlertPolicyRecord[]
  >([]);
  const [vpsRuleValues, setVpsRuleValues] = useState<VpsRuleValueRecord[]>([]);
  const [trafficAccounting, setTrafficAccounting] = useState<
    TrafficAccountingRecord[]
  >([]);
  const [policyAlerts, setPolicyAlerts] = useState<PolicyAlertRecord[]>([]);
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
    const fullGeneration = ++fleetFullGeneration.current;
    const requestGeneration = ++fleetSnapshotGeneration.current;
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
        apiGet<VpsRuleValueRecord[]>(
          `/api/v1/vps-rules?limit=${FLEET_TELEMETRY_SNAPSHOT_LIMIT}`,
          apiToken,
        ),
        apiGet<TrafficAccountingRecord[]>(
          `/api/v1/traffic-accounting?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<PolicyAlertRecord[]>(
          `/api/v1/policy-alerts?limit=${FLEET_DETAIL_LIMIT}`,
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
          `/api/v1/telemetry/rollups?latest=true&limit=${FLEET_TELEMETRY_SNAPSHOT_LIMIT}`,
          apiToken,
        ),
        apiGet<TelemetryNetworkRateRecord[]>(
          `/api/v1/telemetry/network-rates?latest=true&limit=${FLEET_TELEMETRY_SNAPSHOT_LIMIT}`,
          apiToken,
        ),
        apiGet<TelemetryTunnelRecord[]>(
          `/api/v1/telemetry/tunnels?limit=${FLEET_TELEMETRY_SNAPSHOT_LIMIT}`,
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

      const applyOptionalValue = <T>(
        index: number,
        apply: (value: T) => void,
      ) => {
        const result = optionalResults[index];
        if (result.status === "fulfilled") {
          apply(result.value as T);
        }
      };

      if (apiTokenRef.current !== apiToken) {
        return;
      }
      const snapshotIsCurrent =
        requestGeneration === fleetSnapshotGeneration.current;
      const fullLoadIsCurrent = fullGeneration === fleetFullGeneration.current;
      if (snapshotIsCurrent) {
        setSummary(nextSummary);
        setAgents(nextAgents);
      }
      if (fullLoadIsCurrent) {
        applyOptionalValue<FleetAlertRecord[]>(0, setFleetAlerts);
        applyOptionalValue<FleetAlertStateRecord[]>(1, setFleetAlertStates);
        applyOptionalValue<FleetAlertPolicyRecord[]>(2, setFleetAlertPolicies);
        applyOptionalValue<VpsRuleValueRecord[]>(3, setVpsRuleValues);
        applyOptionalValue<TrafficAccountingRecord[]>(4, setTrafficAccounting);
        applyOptionalValue<PolicyAlertRecord[]>(5, setPolicyAlerts);
        applyOptionalValue<FleetAlertNotificationChannelRecord[]>(
          6,
          setFleetAlertNotificationChannels,
        );
        applyOptionalValue<FleetAlertNotificationDeliveryRecord[]>(
          7,
          setFleetAlertNotifications,
        );
        applyOptionalValue<WebhookRuleRecord[]>(8, setWebhookRules);
        applyOptionalValue<WebhookRuleDeliveryRecord[]>(
          9,
          setWebhookRuleDeliveries,
        );
      }
      if (snapshotIsCurrent) {
        applyOptionalValue<TelemetryRollupRecord[]>(10, setTelemetryRollups);
        applyOptionalValue<TelemetryNetworkRateRecord[]>(
          11,
          setTelemetryNetworkRates,
        );
        applyOptionalValue<TelemetryTunnelRecord[]>(12, setTelemetryTunnels);
      }
      if (fullLoadIsCurrent && optionalFailure?.status === "rejected") {
        setApiError(
          optionalFailure.reason instanceof Error
            ? optionalFailure.reason.message
            : "Some fleet details are unavailable",
        );
      } else if (snapshotIsCurrent) {
        setApiError(null);
      }
    } catch (error) {
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setSummary(emptySummary);
        setAgents([]);
        setFleetAlerts([]);
        setFleetAlertStates([]);
        setFleetAlertPolicies([]);
        setVpsRuleValues([]);
        setTrafficAccounting([]);
        setPolicyAlerts([]);
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

  const loadFleetTelemetry = useCallback(() => {
    if (fleetTelemetryInFlight.current) {
      return fleetTelemetryInFlight.current;
    }
    const requestGeneration = ++fleetSnapshotGeneration.current;
    const request = (async () => {
      try {
        const [nextSummary, nextAgents] = await Promise.all([
          apiGet<FleetSummary>("/api/v1/fleet/summary", apiToken),
          apiGet<AgentView[]>("/api/v1/agents", apiToken),
        ]);
        const telemetryResults = await Promise.allSettled([
          apiGet<TelemetryRollupRecord[]>(
            `/api/v1/telemetry/rollups?latest=true&limit=${FLEET_TELEMETRY_SNAPSHOT_LIMIT}`,
            apiToken,
          ),
          apiGet<TelemetryNetworkRateRecord[]>(
            `/api/v1/telemetry/network-rates?latest=true&limit=${FLEET_TELEMETRY_SNAPSHOT_LIMIT}`,
            apiToken,
          ),
          apiGet<TelemetryTunnelRecord[]>(
            `/api/v1/telemetry/tunnels?limit=${FLEET_TELEMETRY_SNAPSHOT_LIMIT}`,
            apiToken,
          ),
        ]);
        const telemetryFailure = telemetryResults.find(
          (result) => result.status === "rejected",
        );
        if (
          telemetryFailure?.status === "rejected" &&
          isApiUnauthorized(telemetryFailure.reason)
        ) {
          throw telemetryFailure.reason;
        }
        if (
          apiTokenRef.current !== apiToken ||
          requestGeneration !== fleetSnapshotGeneration.current
        ) {
          return;
        }
        setSummary(nextSummary);
        setAgents(nextAgents);
        const rollups = telemetryResults[0];
        const rates = telemetryResults[1];
        const tunnels = telemetryResults[2];
        if (rollups.status === "fulfilled") {
          setTelemetryRollups(rollups.value);
        }
        if (rates.status === "fulfilled") {
          setTelemetryNetworkRates(rates.value);
        }
        if (tunnels.status === "fulfilled") {
          setTelemetryTunnels(tunnels.value);
        }
        setApiError(
          telemetryFailure?.status === "rejected"
            ? telemetryFailure.reason instanceof Error
              ? telemetryFailure.reason.message
              : "Some live fleet telemetry is unavailable"
            : null,
        );
      } catch (error) {
        if (apiTokenRef.current !== apiToken) {
          return;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          setSummary(emptySummary);
          setAgents([]);
          setTelemetryRollups([]);
          setTelemetryNetworkRates([]);
          setTelemetryTunnels([]);
          setApiError("Operator login required");
          return;
        }
        setApiError(
          error instanceof Error
            ? error.message
            : "Live fleet telemetry unavailable",
        );
      }
    })();
    const trackedRequest = request.finally(() => {
      if (fleetTelemetryInFlight.current === trackedRequest) {
        fleetTelemetryInFlight.current = null;
      }
    });
    fleetTelemetryInFlight.current = trackedRequest;
    return trackedRequest;
  }, [apiToken, onUnauthorized]);

  const replaceFleetSnapshot = useCallback(
    (nextSummary: FleetSummary, nextAgents: AgentView[]) => {
      fleetSnapshotGeneration.current += 1;
      setSummary(nextSummary);
      setAgents(nextAgents);
      setApiError(null);
    },
    [],
  );

  const updateAgentAlias = useCallback(
    async (clientId: string, displayName: string, confirmed: boolean) => {
      const agent = await apiPost<AgentView>(
        `/api/v1/agents/${encodeURIComponent(clientId)}/alias`,
        apiToken,
        {
          display_name: displayName,
          confirmed,
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
            Number(right.enabled) - Number(left.enabled) ||
            left.name.localeCompare(right.name),
        );
      });
      await loadFleet();
      return policy;
    },
    [apiToken, loadFleet],
  );

  const dryRunFleetAlertPolicy = useCallback(
    async (request: PolicyDryRunRequest) =>
      apiPostPreview<PolicyDryRunResponse>(
        "/api/v1/fleet-alert-policies/dry-run",
        apiToken,
        request,
      ),
    [apiToken],
  );

  const dryRunVpsRules = useCallback(
    async (request: VpsRulesDryRunRequest) =>
      apiPostPreview<VpsRulesDryRunResponse>(
        "/api/v1/vps-rules/dry-run",
        apiToken,
        request,
      ),
    [apiToken],
  );

  const bulkUpsertVpsRules = useCallback(
    async (request: VpsRulesBulkUpsertRequest) => {
      const preview = await (request.confirmed ? apiPost : apiPostPreview)<VpsRulesDryRunResponse>(
        "/api/v1/vps-rules/bulk-upsert",
        apiToken,
        request,
      );
      await loadFleet();
      return preview;
    },
    [apiToken, loadFleet],
  );

  const bulkUnsetVpsRules = useCallback(
    async (request: VpsRulesBulkUnsetRequest) => {
      const preview = await (request.confirmed ? apiPost : apiPostPreview)<VpsRulesDryRunResponse>(
        "/api/v1/vps-rules/bulk-unset",
        apiToken,
        request,
      );
      await loadFleet();
      return preview;
    },
    [apiToken, loadFleet],
  );

  const deleteFleetAlertPolicy = useCallback(
    async (policyId: string, reviewedName: string) => {
      await apiDelete(
        `/api/v1/fleet-alert-policies/${encodeURIComponent(policyId)}`,
        apiToken,
        { confirmed: true, reviewed_name: reviewedName },
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
    async (channelId: string, reviewedName: string) => {
      await apiDelete(
        `/api/v1/fleet-alert-notification-channels/${encodeURIComponent(channelId)}`,
        apiToken,
        { confirmed: true, reviewed_name: reviewedName },
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
      const deliveries = await (request.dry_run ? apiPostPreview : apiPost)<FleetAlertNotificationDeliveryRecord[]>(
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
      const deliveries = await (request.dry_run ? apiPostPreview : apiPost)<FleetAlertNotificationDeliveryRecord[]>(
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
    async (ruleId: string, reviewedName: string) => {
      await apiDelete(
        `/api/v1/webhook-rules/${encodeURIComponent(ruleId)}`,
        apiToken,
        { confirmed: true, reviewed_name: reviewedName },
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
      apiPostPreview<WebhookRuleDryRunRecord>(
        "/api/v1/webhook-rules/dry-run",
        apiToken,
        request,
      ),
    [apiToken],
  );

  const dispatchWebhookRules = useCallback(
    async (request: WebhookRuleDispatchRequest) => {
      const deliveries = await (request.dry_run ? apiPostPreview : apiPost)<WebhookRuleDeliveryRecord[]>(
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
      const deliveries = await (request.dry_run ? apiPostPreview : apiPost)<WebhookRuleDeliveryRecord[]>(
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
      const response = await (request.confirmed ? apiPost : apiPostPreview)<WebhookDeliveryRotationResponse>(
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
    setVpsRuleValues([]);
    setTrafficAccounting([]);
    setPolicyAlerts([]);
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
    vpsRuleValues,
    trafficAccounting,
    policyAlerts,
    fleetAlertNotificationChannels,
    fleetAlertNotifications,
    webhookRules,
    webhookRuleDeliveries,
    deleteAgent,
    loadFleet,
    loadFleetTelemetry,
    replaceFleetSnapshot,
    updateAgentAlias,
    summary,
    telemetryNetworkRates,
    telemetryRollups,
    telemetryTunnels,
    upsertFleetAlertPolicy,
    dryRunFleetAlertPolicy,
    deleteFleetAlertPolicy,
    dryRunVpsRules,
    bulkUpsertVpsRules,
    bulkUnsetVpsRules,
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
