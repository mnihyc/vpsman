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
  DeleteAgentBatchOutcome,
  DeleteAgentBatchTarget,
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

const FLEET_ERROR_SOURCE_ORDER = ["core", "detail", "telemetry"] as const;
type FleetErrorSource = (typeof FLEET_ERROR_SOURCE_ORDER)[number];

type FleetSnapshotSource<T> = {
  data: T | null;
  error: string | null;
};

type FleetSnapshotRecord = {
  mode: "live" | "full";
  generated_at: string;
  summary: FleetSnapshotSource<FleetSummary>;
  agents: FleetSnapshotSource<AgentView[]>;
  telemetry_rollups: FleetSnapshotSource<TelemetryRollupRecord[]>;
  telemetry_network_rates: FleetSnapshotSource<TelemetryNetworkRateRecord[]>;
  telemetry_tunnels: FleetSnapshotSource<TelemetryTunnelRecord[]>;
  fleet_alerts?: FleetSnapshotSource<FleetAlertRecord[]>;
  fleet_alert_states?: FleetSnapshotSource<FleetAlertStateRecord[]>;
  fleet_alert_policies?: FleetSnapshotSource<FleetAlertPolicyRecord[]>;
  vps_rule_values?: FleetSnapshotSource<VpsRuleValueRecord[]>;
  traffic_accounting?: FleetSnapshotSource<TrafficAccountingRecord[]>;
  policy_alerts?: FleetSnapshotSource<PolicyAlertRecord[]>;
  fleet_alert_notification_channels?: FleetSnapshotSource<
    FleetAlertNotificationChannelRecord[]
  >;
  fleet_alert_notifications?: FleetSnapshotSource<
    FleetAlertNotificationDeliveryRecord[]
  >;
  webhook_rules?: FleetSnapshotSource<WebhookRuleRecord[]>;
  webhook_rule_deliveries?: FleetSnapshotSource<WebhookRuleDeliveryRecord[]>;
};

export function useFleetData(apiToken: string, onUnauthorized: () => void) {
  const apiTokenRef = useRef(apiToken);
  const fleetFullGeneration = useRef(0);
  const fleetSnapshotGeneration = useRef(0);
  const fleetTelemetryInFlight = useRef<{
    token: string;
    promise: Promise<void>;
  } | null>(null);
  const deletedClientIds = useRef(new Set<string>());
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

  const applyLiveFleetSnapshot = useCallback(
    (snapshot: FleetSnapshotRecord) => {
      const nextAgents = snapshot.agents.data;
      const staleDeletedIds = nextAgents
        ? deletedIdsInAgentSnapshot(nextAgents, deletedClientIds.current)
        : [];
      const coreError = snapshotSourceErrorSummary(
        "Core fleet sources are unavailable",
        [
          ["fleet summary", snapshot.summary],
          ["agents", snapshot.agents],
        ],
      );
      setFleetCoreEvidenceAvailable(
        coreError === null && staleDeletedIds.length === 0,
      );
      if (snapshot.summary.data && staleDeletedIds.length === 0) {
        setSummary(snapshot.summary.data);
      }
      if (nextAgents) {
        setAgents(withoutDeletedAgents(nextAgents, deletedClientIds.current));
      }
      publishFleetError(
        "core",
        staleDeletedIds.length > 0
          ? staleFleetSnapshotMessage(staleDeletedIds)
          : coreError,
      );

      if (snapshot.telemetry_rollups.data) {
        setTelemetryRollups(
          withoutDeletedClients(
            snapshot.telemetry_rollups.data,
            deletedClientIds.current,
          ),
        );
      }
      if (snapshot.telemetry_network_rates.data) {
        setTelemetryNetworkRates(
          withoutDeletedClients(
            snapshot.telemetry_network_rates.data,
            deletedClientIds.current,
          ),
        );
      }
      if (snapshot.telemetry_tunnels.data) {
        setTelemetryTunnels(
          withoutDeletedClients(
            snapshot.telemetry_tunnels.data,
            deletedClientIds.current,
          ),
        );
      }
      publishFleetError(
        "telemetry",
        snapshotSourceErrorSummary("Some live fleet sources are unavailable", [
          ["telemetry rollups", snapshot.telemetry_rollups],
          ["network rates", snapshot.telemetry_network_rates],
          ["tunnel telemetry", snapshot.telemetry_tunnels],
        ]),
      );
    },
    [publishFleetError],
  );

  const applyFleetDetailSnapshot = useCallback(
    (snapshot: FleetSnapshotRecord) => {
      const fleetAlertsSource = requiredSnapshotSource(snapshot.fleet_alerts);
      const fleetAlertStatesSource = requiredSnapshotSource(
        snapshot.fleet_alert_states,
      );
      const fleetAlertPoliciesSource = requiredSnapshotSource(
        snapshot.fleet_alert_policies,
      );
      const vpsRuleValuesSource = requiredSnapshotSource(
        snapshot.vps_rule_values,
      );
      const trafficAccountingSource = requiredSnapshotSource(
        snapshot.traffic_accounting,
      );
      const policyAlertsSource = requiredSnapshotSource(snapshot.policy_alerts);
      const notificationChannelsSource = requiredSnapshotSource(
        snapshot.fleet_alert_notification_channels,
      );
      const notificationsSource = requiredSnapshotSource(
        snapshot.fleet_alert_notifications,
      );
      const webhookRulesSource = requiredSnapshotSource(snapshot.webhook_rules);
      const webhookDeliveriesSource = requiredSnapshotSource(
        snapshot.webhook_rule_deliveries,
      );

      setFleetAlertsEvidenceAvailable(sourceAvailable(fleetAlertsSource));
      setConfigPolicyEvidenceAvailable(
        [
          fleetAlertPoliciesSource,
          vpsRuleValuesSource,
          trafficAccountingSource,
        ].every(sourceAvailable),
      );
      if (fleetAlertsSource.data) {
        setFleetAlerts(
          withoutDeletedClients(
            fleetAlertsSource.data,
            deletedClientIds.current,
          ),
        );
      }
      if (fleetAlertStatesSource.data) {
        setFleetAlertStates(fleetAlertStatesSource.data);
      }
      if (fleetAlertPoliciesSource.data) {
        setFleetAlertPolicies(fleetAlertPoliciesSource.data);
      }
      if (vpsRuleValuesSource.data) {
        setVpsRuleValues(
          withoutDeletedClients(
            vpsRuleValuesSource.data,
            deletedClientIds.current,
          ),
        );
      }
      if (trafficAccountingSource.data) {
        setTrafficAccounting(
          withoutDeletedClients(
            trafficAccountingSource.data,
            deletedClientIds.current,
          ),
        );
      }
      if (policyAlertsSource.data) {
        setPolicyAlerts(
          withoutDeletedClients(
            policyAlertsSource.data,
            deletedClientIds.current,
          ),
        );
      }
      if (notificationChannelsSource.data) {
        setFleetAlertNotificationChannels(notificationChannelsSource.data);
      }
      if (notificationsSource.data) {
        setFleetAlertNotifications(notificationsSource.data);
      }
      if (webhookRulesSource.data) {
        setWebhookRules(webhookRulesSource.data);
      }
      if (webhookDeliveriesSource.data) {
        setWebhookRuleDeliveries(webhookDeliveriesSource.data);
      }
      publishFleetError(
        "detail",
        snapshotSourceErrorSummary(
          "Some fleet detail sources are unavailable",
          [
            ["fleet alerts", fleetAlertsSource],
            ["fleet alert states", fleetAlertStatesSource],
            ["fleet alert policies", fleetAlertPoliciesSource],
            ["VPS rules", vpsRuleValuesSource],
            ["traffic accounting", trafficAccountingSource],
            ["policy alerts", policyAlertsSource],
            ["notification channels", notificationChannelsSource],
            ["notification deliveries", notificationsSource],
            ["webhook rules", webhookRulesSource],
            ["webhook deliveries", webhookDeliveriesSource],
          ],
        ),
      );
    },
    [publishFleetError],
  );

  const loadFleet = useCallback(async () => {
    if (apiTokenRef.current !== apiToken) {
      return;
    }
    const fullGeneration = ++fleetFullGeneration.current;
    const requestGeneration = ++fleetSnapshotGeneration.current;
    try {
      const snapshot = await apiGet<FleetSnapshotRecord>(
        "/api/v1/fleet/snapshot?mode=full",
        apiToken,
      );
      if (apiTokenRef.current !== apiToken || snapshot.mode !== "full") {
        return;
      }
      const snapshotIsCurrent =
        requestGeneration === fleetSnapshotGeneration.current;
      const fullLoadIsCurrent = fullGeneration === fleetFullGeneration.current;
      if (snapshotIsCurrent) {
        applyLiveFleetSnapshot(snapshot);
      }
      if (fullLoadIsCurrent) {
        applyFleetDetailSnapshot(snapshot);
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
        fleetSourceErrors.current = { core: "Operator login required" };
        setApiError("Operator login required");
        return;
      }
      const message =
        error instanceof Error ? error.message : "Fleet refresh unavailable";
      if (snapshotIsCurrent) {
        setFleetCoreEvidenceAvailable(false);
        publishFleetError("core", message);
        publishFleetError("telemetry", message);
      }
      if (fullLoadIsCurrent) {
        setFleetAlertsEvidenceAvailable(false);
        setConfigPolicyEvidenceAvailable(false);
        publishFleetError("detail", message);
      }
    }
  }, [
    apiToken,
    applyFleetDetailSnapshot,
    applyLiveFleetSnapshot,
    onUnauthorized,
    publishFleetError,
  ]);

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
        const snapshot = await apiGet<FleetSnapshotRecord>(
          "/api/v1/fleet/snapshot?mode=live",
          apiToken,
        );
        if (
          apiTokenRef.current !== apiToken ||
          requestGeneration !== fleetSnapshotGeneration.current ||
          snapshot.mode !== "live"
        ) {
          return;
        }
        applyLiveFleetSnapshot(snapshot);
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
          fleetSourceErrors.current = { core: "Operator login required" };
          setApiError("Operator login required");
          return;
        }
        setFleetCoreEvidenceAvailable(false);
        const message =
          error instanceof Error
            ? error.message
            : "Live fleet telemetry unavailable";
        publishFleetError("core", message);
        publishFleetError("telemetry", message);
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
  }, [apiToken, applyLiveFleetSnapshot, onUnauthorized, publishFleetError]);

  const replaceFleetSnapshot = useCallback(
    (nextSummary: FleetSummary, nextAgents: AgentView[]) => {
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      fleetSnapshotGeneration.current += 1;
      const staleDeletedIds = deletedIdsInAgentSnapshot(
        nextAgents,
        deletedClientIds.current,
      );
      setAgents(withoutDeletedAgents(nextAgents, deletedClientIds.current));
      if (staleDeletedIds.length > 0) {
        setFleetCoreEvidenceAvailable(false);
        publishFleetError("core", staleFleetSnapshotMessage(staleDeletedIds));
        void loadFleet();
        return;
      }
      setSummary(nextSummary);
      setFleetCoreEvidenceAvailable(true);
      publishFleetError("core", null);
    },
    [apiToken, loadFleet, publishFleetError],
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

  const deleteAgents = useCallback(
    async (
      targets: DeleteAgentBatchTarget[],
    ): Promise<DeleteAgentBatchOutcome[]> => {
      const outcomes: DeleteAgentBatchOutcome[] = [];
      for (let index = 0; index < targets.length; index += 1) {
        const target = targets[index];
        try {
          const response = await apiPost<DeleteAgentResponse>(
            `/api/v1/agents/${encodeURIComponent(target.client_id)}/delete`,
            apiToken,
            target.request,
          );
          outcomes.push({
            client_id: target.client_id,
            response,
            error: null,
          });
          deletedClientIds.current.add(response.client_id);
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          outcomes.push({
            client_id: target.client_id,
            response: null,
            error: message,
          });
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            for (const skipped of targets.slice(index + 1)) {
              outcomes.push({
                client_id: skipped.client_id,
                response: null,
                error: "Not attempted because the operator session expired.",
              });
            }
            break;
          }
        }
      }

      const deletedIds = new Set(
        outcomes.flatMap((outcome) =>
          outcome.response ? [outcome.response.client_id] : [],
        ),
      );
      if (apiTokenRef.current === apiToken && deletedIds.size > 0) {
        setAgents((current) =>
          current.filter((agent) => !deletedIds.has(agent.id)),
        );
        setFleetAlerts((current) =>
          current.filter(
            (alert) => !alert.client_id || !deletedIds.has(alert.client_id),
          ),
        );
        setVpsRuleValues((current) =>
          current.filter((rule) => !deletedIds.has(rule.client_id)),
        );
        setTrafficAccounting((current) =>
          current.filter((record) => !deletedIds.has(record.client_id)),
        );
        setPolicyAlerts((current) =>
          current.filter((alert) => !deletedIds.has(alert.client_id)),
        );
        setTelemetryRollups((current) =>
          current.filter((record) => !deletedIds.has(record.client_id)),
        );
        setTelemetryNetworkRates((current) =>
          current.filter((record) => !deletedIds.has(record.client_id)),
        );
        setTelemetryTunnels((current) =>
          current.filter((record) => !deletedIds.has(record.client_id)),
        );
        await loadFleet();
      }
      return outcomes;
    },
    [apiToken, loadFleet, onUnauthorized],
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

  const loadEffectiveVpsRules = useCallback(
    async (clientId: string) =>
      apiGet<VpsRuleValueRecord[]>(
        `/api/v1/vps-rules/effective/${encodeURIComponent(clientId)}`,
        apiToken,
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
    deletedClientIds.current.clear();
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
    deleteAgents,
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
    loadEffectiveVpsRules,
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

function sourceAvailable(source: FleetSnapshotSource<unknown>): boolean {
  return source.data !== null && source.error === null;
}

function requiredSnapshotSource<T>(
  source: FleetSnapshotSource<T> | undefined,
): FleetSnapshotSource<T> {
  return source ?? { data: null, error: "snapshot source missing" };
}

function snapshotSourceErrorSummary(
  prefix: string,
  sources: ReadonlyArray<readonly [string, FleetSnapshotSource<unknown>]>,
): string | null {
  const failures = sources.flatMap(([label, source]) =>
    sourceAvailable(source)
      ? []
      : [`${label}${source.error ? `: ${source.error}` : ""}`],
  );
  return failures.length > 0 ? `${prefix}: ${failures.join("; ")}` : null;
}

function withoutDeletedClients<T extends { client_id?: string | null }>(
  rows: T[],
  deletedClientIds: ReadonlySet<string>,
): T[] {
  if (deletedClientIds.size === 0) {
    return rows;
  }
  return rows.filter(
    (row) => !row.client_id || !deletedClientIds.has(row.client_id),
  );
}

function withoutDeletedAgents(
  agents: AgentView[],
  deletedClientIds: ReadonlySet<string>,
): AgentView[] {
  if (deletedClientIds.size === 0) {
    return agents;
  }
  return agents.filter((agent) => !deletedClientIds.has(agent.id));
}

function deletedIdsInAgentSnapshot(
  agents: AgentView[],
  deletedClientIds: ReadonlySet<string>,
): string[] {
  if (deletedClientIds.size === 0) {
    return [];
  }
  return Array.from(
    new Set(
      agents
        .map((agent) => agent.id)
        .filter((clientId) => deletedClientIds.has(clientId)),
    ),
  ).sort();
}

function staleFleetSnapshotMessage(clientIds: string[]): string {
  return `Ignored stale fleet data for permanently deleted VPS ${clientIds.join(
    ", ",
  )}; refreshing authoritative inventory`;
}
