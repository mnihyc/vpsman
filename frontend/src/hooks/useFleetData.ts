import { useCallback, useRef, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPostPreview, isApiUnauthorized } from "../api";
import {
  emptySummary,
  FLEET_DETAIL_LIMIT,
  FLEET_TELEMETRY_SNAPSHOT_LIMIT,
} from "../constants";
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

const FLEET_DETAIL_SOURCE_LABELS = [
  "fleet alerts",
  "fleet alert states",
  "fleet alert policies",
  "VPS rules",
  "traffic accounting",
  "policy alerts",
  "notification channels",
  "notification deliveries",
  "webhook rules",
  "webhook deliveries",
] as const;

const FLEET_CORE_SOURCE_LABELS = ["fleet summary", "agents"] as const;

const FLEET_TELEMETRY_SOURCE_LABELS = [
  "telemetry rollups",
  "network rates",
  "tunnel telemetry",
] as const;

const FLEET_ERROR_SOURCE_ORDER = ["core", "detail", "telemetry"] as const;
type FleetErrorSource = (typeof FLEET_ERROR_SOURCE_ORDER)[number];

export function useFleetData(apiToken: string, onUnauthorized: () => void) {
  const apiTokenRef = useRef(apiToken);
  const fleetFullGeneration = useRef(0);
  const fleetSnapshotGeneration = useRef(0);
  const fleetTelemetryInFlight = useRef<{
    token: string;
    promise: Promise<void>;
  } | null>(null);
  const fleetSourceErrors = useRef<
    Partial<Record<FleetErrorSource, string>>
  >({});
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
  const [fleetCoreEvidenceAvailable, setFleetCoreEvidenceAvailable] =
    useState(false);
  const [fleetAlertsEvidenceAvailable, setFleetAlertsEvidenceAvailable] =
    useState(false);
  const [configPolicyEvidenceAvailable, setConfigPolicyEvidenceAvailable] =
    useState(false);

  const publishFleetError = useCallback(
    (source: FleetErrorSource, error: string | null) => {
      if (error) {
        fleetSourceErrors.current[source] = error;
      } else {
        delete fleetSourceErrors.current[source];
      }
      const errors = FLEET_ERROR_SOURCE_ORDER.flatMap((key) => {
        const current = fleetSourceErrors.current[key];
        return current ? [current] : [];
      });
      setApiError(errors.length > 0 ? errors.join("; ") : null);
    },
    [],
  );

  const loadFleet = useCallback(async () => {
    if (apiTokenRef.current !== apiToken) {
      return;
    }
    const fullGeneration = ++fleetFullGeneration.current;
    const requestGeneration = ++fleetSnapshotGeneration.current;
    try {
      const [coreResults, optionalResults] = await Promise.all([
        Promise.allSettled([
          apiGet<FleetSummary>("/api/v1/fleet/summary", apiToken),
          apiGet<AgentView[]>("/api/v1/agents", apiToken),
        ]),
        Promise.allSettled([
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
        ]),
      ]);

      if (apiTokenRef.current !== apiToken) {
        return;
      }
      const snapshotIsCurrent =
        requestGeneration === fleetSnapshotGeneration.current;
      const fullLoadIsCurrent = fullGeneration === fleetFullGeneration.current;
      if (!snapshotIsCurrent && !fullLoadIsCurrent) {
        return;
      }

      const unauthorizedFailure = [...coreResults, ...optionalResults].find(
        (result) =>
          result.status === "rejected" &&
          isApiUnauthorized(result.reason),
      );
      if (unauthorizedFailure?.status === "rejected") {
        throw unauthorizedFailure.reason;
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

      if (snapshotIsCurrent) {
        const summaryResult = coreResults[0];
        const agentsResult = coreResults[1];
        setFleetCoreEvidenceAvailable(
          coreResults.every((result) => result.status === "fulfilled"),
        );
        if (summaryResult.status === "fulfilled") {
          setSummary(summaryResult.value as FleetSummary);
        }
        if (agentsResult.status === "fulfilled") {
          setAgents(agentsResult.value as AgentView[]);
        }
        publishFleetError(
          "core",
          unavailableSourceSummary(
            "Core fleet sources are unavailable",
            coreResults,
            FLEET_CORE_SOURCE_LABELS,
          ),
        );
      }
      if (fullLoadIsCurrent) {
        setFleetAlertsEvidenceAvailable(
          optionalResults[0].status === "fulfilled",
        );
        setConfigPolicyEvidenceAvailable(
          optionalResults
            .slice(2, 5)
            .every((result) => result.status === "fulfilled"),
        );
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
        publishFleetError(
          "detail",
          unavailableSourceSummary(
            "Some fleet detail sources are unavailable",
            optionalResults.slice(0, FLEET_DETAIL_SOURCE_LABELS.length),
            FLEET_DETAIL_SOURCE_LABELS,
          ),
        );
      }
      if (snapshotIsCurrent) {
        applyOptionalValue<TelemetryRollupRecord[]>(10, setTelemetryRollups);
        applyOptionalValue<TelemetryNetworkRateRecord[]>(
          11,
          setTelemetryNetworkRates,
        );
        applyOptionalValue<TelemetryTunnelRecord[]>(12, setTelemetryTunnels);
        publishFleetError(
          "telemetry",
          unavailableSourceSummary(
            "Some live fleet sources are unavailable",
            optionalResults.slice(FLEET_DETAIL_SOURCE_LABELS.length),
            FLEET_TELEMETRY_SOURCE_LABELS,
          ),
        );
      }
    } catch (error) {
      const fullLoadIsCurrent = fullGeneration === fleetFullGeneration.current;
      const snapshotIsCurrent =
        requestGeneration === fleetSnapshotGeneration.current;
      if (
        apiTokenRef.current !== apiToken ||
        (!fullLoadIsCurrent && !snapshotIsCurrent)
      ) {
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
        setFleetCoreEvidenceAvailable(false);
        setFleetAlertsEvidenceAvailable(false);
        setConfigPolicyEvidenceAvailable(false);
        fleetSourceErrors.current = {
          core: "Operator login required",
        };
        setApiError("Operator login required");
        return;
      }
      const message =
        error instanceof Error ? error.message : "Fleet refresh unavailable";
      if (snapshotIsCurrent) {
        setFleetCoreEvidenceAvailable(false);
        publishFleetError("core", message);
      } else if (fullLoadIsCurrent) {
        setFleetAlertsEvidenceAvailable(false);
        setConfigPolicyEvidenceAvailable(false);
        publishFleetError("detail", message);
      }
    }
  }, [apiToken, onUnauthorized, publishFleetError]);

  const loadFleetTelemetry = useCallback(() => {
    if (apiTokenRef.current !== apiToken) {
      return Promise.resolve();
    }
    if (fleetTelemetryInFlight.current?.token === apiToken) {
      return fleetTelemetryInFlight.current.promise;
    }
    const requestGeneration = ++fleetSnapshotGeneration.current;
    const request = (async () => {
      try {
        const [coreResults, telemetryResults] = await Promise.all([
          Promise.allSettled([
            apiGet<FleetSummary>("/api/v1/fleet/summary", apiToken),
            apiGet<AgentView[]>("/api/v1/agents", apiToken),
          ]),
          Promise.allSettled([
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
          ]),
        ]);
        if (
          apiTokenRef.current !== apiToken ||
          requestGeneration !== fleetSnapshotGeneration.current
        ) {
          return;
        }
        const unauthorizedFailure = [...coreResults, ...telemetryResults].find(
          (result) =>
            result.status === "rejected" &&
            isApiUnauthorized(result.reason),
        );
        if (unauthorizedFailure?.status === "rejected") {
          throw unauthorizedFailure.reason;
        }
        const summaryResult = coreResults[0];
        const agentsResult = coreResults[1];
        setFleetCoreEvidenceAvailable(
          coreResults.every((result) => result.status === "fulfilled"),
        );
        if (summaryResult.status === "fulfilled") {
          setSummary(summaryResult.value as FleetSummary);
        }
        if (agentsResult.status === "fulfilled") {
          setAgents(agentsResult.value as AgentView[]);
        }
        publishFleetError(
          "core",
          unavailableSourceSummary(
            "Core fleet sources are unavailable",
            coreResults,
            FLEET_CORE_SOURCE_LABELS,
          ),
        );
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
        publishFleetError(
          "telemetry",
          unavailableSourceSummary(
            "Some live fleet sources are unavailable",
            telemetryResults,
            FLEET_TELEMETRY_SOURCE_LABELS,
          ),
        );
      } catch (error) {
        if (
          apiTokenRef.current !== apiToken ||
          requestGeneration !== fleetSnapshotGeneration.current
        ) {
          return;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          setSummary(emptySummary);
          setAgents([]);
          setTelemetryRollups([]);
          setTelemetryNetworkRates([]);
          setTelemetryTunnels([]);
          setFleetCoreEvidenceAvailable(false);
          setFleetAlertsEvidenceAvailable(false);
          fleetSourceErrors.current = {
            core: "Operator login required",
          };
          setApiError("Operator login required");
          return;
        }
        setFleetCoreEvidenceAvailable(false);
        publishFleetError(
          "core",
          error instanceof Error
            ? error.message
            : "Live fleet telemetry unavailable",
        );
      }
    })();
    const trackedRequest = request.finally(() => {
      if (fleetTelemetryInFlight.current?.promise === trackedRequest) {
        fleetTelemetryInFlight.current = null;
      }
    });
    fleetTelemetryInFlight.current = {
      token: apiToken,
      promise: trackedRequest,
    };
    return trackedRequest;
  }, [apiToken, onUnauthorized, publishFleetError]);

  const replaceFleetSnapshot = useCallback(
    (nextSummary: FleetSummary, nextAgents: AgentView[]) => {
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      fleetSnapshotGeneration.current += 1;
      setSummary(nextSummary);
      setAgents(nextAgents);
      setFleetCoreEvidenceAvailable(true);
      publishFleetError("core", null);
    },
    [apiToken, publishFleetError],
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
      if (apiTokenRef.current !== apiToken) {
        return agent;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return response;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return policy;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return state;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return channel;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return deliveries;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return deliveries;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return rule;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      setWebhookRules((current) =>
        current.filter((rule) => rule.id !== ruleId),
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
      if (apiTokenRef.current !== apiToken) {
        return deliveries;
      }
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
      if (apiTokenRef.current !== apiToken) {
        return deliveries;
      }
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
    apiTokenRef.current = "";
    fleetFullGeneration.current += 1;
    fleetSnapshotGeneration.current += 1;
    fleetTelemetryInFlight.current = null;
    fleetSourceErrors.current = {};
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
    setFleetCoreEvidenceAvailable(false);
    setFleetAlertsEvidenceAvailable(false);
    setConfigPolicyEvidenceAvailable(false);
    setApiError(null);
  }, []);

  return {
    agents,
    apiError,
    clearFleet,
    configPolicyEvidenceAvailable,
    fleetAlertsEvidenceAvailable,
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
    fleetCoreEvidenceAvailable,
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

function unavailableSourceSummary(
  prefix: string,
  results: readonly PromiseSettledResult<unknown>[],
  labels: readonly string[],
): string | null {
  const failedLabels = results.flatMap((result, index) =>
    result.status === "rejected" ? [labels[index]] : [],
  );
  return failedLabels.length > 0
    ? `${prefix}: ${failedLabels.join(", ")}`
    : null;
}
