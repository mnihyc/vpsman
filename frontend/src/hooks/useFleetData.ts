import { useCallback, useRef, useState } from "react";
import {
  apiDelete,
  apiGet,
  apiPost,
  apiPostPreview,
  isApiUnauthorized,
} from "../api";
import { emptySummary } from "../constants";
import type { SnapshotSource } from "../homeSnapshot";
import type {
  AgentSuspensionBatchOutcome,
  AgentSuspensionBatchTarget,
  AgentSuspensionMutationResponse,
  AgentView,
  FleetAlertPolicyRecord,
  FleetAlertPolicyRequest,
  FleetAlertRecord,
  FleetAlertEventPage,
  FleetAlertResolveRequest,
  FleetAlertNotificationChannelRecord,
  FleetAlertNotificationChannelRequest,
  FleetAlertNotificationDeliveryRecord,
  FleetAlertNotificationDispatchRequest,
  FleetAlertNotificationProcessRequest,
  FleetAlertStateBulkRequest,
  FleetAlertStateBulkResponse,
  FleetAlertStateRecord,
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
  TelemetryUptimeRecord,
} from "../types";

const FLEET_ERROR_SOURCE_ORDER = ["core", "detail", "telemetry"] as const;
type FleetErrorSource = (typeof FLEET_ERROR_SOURCE_ORDER)[number];

type FleetSnapshotSource<T> = SnapshotSource<T>;

type HomeFleetSnapshotRecord = {
  summary: SnapshotSource<FleetSummary>;
  agents: SnapshotSource<AgentView[]>;
  telemetry_rollups: SnapshotSource<TelemetryRollupRecord[]>;
  telemetry_network_rates: SnapshotSource<TelemetryNetworkRateRecord[]>;
  fleet_alerts: SnapshotSource<FleetAlertRecord[]>;
  fleet_alerts_truncated?: boolean;
};

type HomeFleetHydrationFence = {
  core: number;
  detail: number;
  telemetry: number;
};

type FleetSnapshotRecord = {
  mode: "live" | "full";
  generated_at: string;
  summary: FleetSnapshotSource<FleetSummary>;
  agents: FleetSnapshotSource<AgentView[]>;
  telemetry_rollups: FleetSnapshotSource<TelemetryRollupRecord[]>;
  telemetry_network_rates: FleetSnapshotSource<TelemetryNetworkRateRecord[]>;
  telemetry_tunnels: FleetSnapshotSource<TelemetryTunnelRecord[]>;
  telemetry_uptimes: FleetSnapshotSource<TelemetryUptimeRecord[]>;
  fleet_alerts?: FleetSnapshotSource<FleetAlertRecord[]>;
  fleet_alerts_truncated?: boolean;
  fleet_alert_history?: FleetSnapshotSource<FleetAlertRecord[]>;
  fleet_alert_history_truncated?: boolean;
  fleet_alert_policies?: FleetSnapshotSource<FleetAlertPolicyRecord[]>;
  vps_rule_values?: FleetSnapshotSource<VpsRuleValueRecord[]>;
  traffic_accounting?: FleetSnapshotSource<TrafficAccountingRecord[]>;
  policy_alerts?: FleetSnapshotSource<PolicyAlertRecord[]>;
  policy_alerts_truncated?: boolean;
  current_policy_alerts?: FleetSnapshotSource<PolicyAlertRecord[]>;
  current_policy_alerts_truncated?: boolean;
  fleet_alert_notification_channels?: FleetSnapshotSource<
    FleetAlertNotificationChannelRecord[]
  >;
  fleet_alert_notifications?: FleetSnapshotSource<
    FleetAlertNotificationDeliveryRecord[]
  >;
  fleet_alert_notifications_truncated?: boolean;
  webhook_rules?: FleetSnapshotSource<WebhookRuleRecord[]>;
  webhook_rule_deliveries?: FleetSnapshotSource<WebhookRuleDeliveryRecord[]>;
  webhook_rule_deliveries_truncated?: boolean;
};

export function useFleetData(apiToken: string, onUnauthorized: () => void) {
  const apiTokenRef = useRef(apiToken);
  const fleetFullGeneration = useRef(0);
  const fleetCoreGeneration = useRef(0);
  const fleetTelemetryGeneration = useRef(0);
  const fleetTelemetryInFlight = useRef<{
    token: string;
    promise: Promise<void>;
  } | null>(null);
  const fleetFullInFlight = useRef<{
    token: string;
    promise: Promise<void>;
  } | null>(null);
  const fleetFullRefreshPending = useRef(false);
  const fleetTelemetryRefreshPending = useRef(false);
  const loadFleetRef = useRef<(trailing?: boolean) => Promise<void>>(() =>
    Promise.resolve(),
  );
  const loadFleetTelemetryRef = useRef<(trailing?: boolean) => Promise<void>>(
    () => Promise.resolve(),
  );
  const fleetAlertEventReviewInFlight = useRef(false);
  const deletedClientIds = useRef(new Set<string>());
  const fleetSourceErrors = useRef<Partial<Record<FleetErrorSource, string>>>(
    {},
  );
  apiTokenRef.current = apiToken;
  const [summary, setSummary] = useState<FleetSummary>(emptySummary);
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [fleetAlerts, setFleetAlerts] = useState<FleetAlertRecord[]>([]);
  const [fleetAlertsTruncated, setFleetAlertsTruncated] = useState(false);
  const [fleetAlertHistory, setFleetAlertHistory] = useState<
    FleetAlertRecord[]
  >([]);
  const [fleetAlertHistoryTruncated, setFleetAlertHistoryTruncated] =
    useState(false);
  const [fleetAlertEventReviewItems, setFleetAlertEventReviewItems] = useState<
    FleetAlertRecord[]
  >([]);
  const [fleetAlertEventReviewCursor, setFleetAlertEventReviewCursor] =
    useState<string | null>(null);
  const [fleetAlertEventReviewHasMore, setFleetAlertEventReviewHasMore] =
    useState(false);
  const [fleetAlertEventReviewStarted, setFleetAlertEventReviewStarted] =
    useState(false);
  const [fleetAlertEventReviewLoading, setFleetAlertEventReviewLoading] =
    useState(false);
  const [fleetAlertEventReviewError, setFleetAlertEventReviewError] = useState<
    string | null
  >(null);
  const [fleetAlertPolicies, setFleetAlertPolicies] = useState<
    FleetAlertPolicyRecord[]
  >([]);
  const [vpsRuleValues, setVpsRuleValues] = useState<VpsRuleValueRecord[]>([]);
  const [trafficAccounting, setTrafficAccounting] = useState<
    TrafficAccountingRecord[]
  >([]);
  const [policyAlerts, setPolicyAlerts] = useState<PolicyAlertRecord[]>([]);
  const [policyAlertsTruncated, setPolicyAlertsTruncated] = useState(false);
  const [currentPolicyAlerts, setCurrentPolicyAlerts] = useState<
    PolicyAlertRecord[]
  >([]);
  const [currentPolicyAlertsTruncated, setCurrentPolicyAlertsTruncated] =
    useState(false);
  const [fleetAlertNotificationChannels, setFleetAlertNotificationChannels] =
    useState<FleetAlertNotificationChannelRecord[]>([]);
  const [fleetAlertNotifications, setFleetAlertNotifications] = useState<
    FleetAlertNotificationDeliveryRecord[]
  >([]);
  const [fleetAlertNotificationsTruncated, setFleetAlertNotificationsTruncated] =
    useState(false);
  const [webhookRules, setWebhookRules] = useState<WebhookRuleRecord[]>([]);
  const [webhookRuleDeliveries, setWebhookRuleDeliveries] = useState<
    WebhookRuleDeliveryRecord[]
  >([]);
  const [webhookRuleDeliveriesTruncated, setWebhookRuleDeliveriesTruncated] =
    useState(false);
  const [telemetryRollups, setTelemetryRollups] = useState<
    TelemetryRollupRecord[]
  >([]);
  const [telemetryNetworkRates, setTelemetryNetworkRates] = useState<
    TelemetryNetworkRateRecord[]
  >([]);
  const [telemetryTunnels, setTelemetryTunnels] = useState<
    TelemetryTunnelRecord[]
  >([]);
  const [telemetryUptimes, setTelemetryUptimes] = useState<
    TelemetryUptimeRecord[]
  >([]);
  const [apiError, setApiError] = useState<string | null>(null);
  const [fleetCoreEvidenceAvailable, setFleetCoreEvidenceAvailable] =
    useState(false);
  const [fleetAlertsEvidenceAvailable, setFleetAlertsEvidenceAvailable] =
    useState(false);
  const [
    fleetAlertHistoryEvidenceAvailable,
    setFleetAlertHistoryEvidenceAvailable,
  ] = useState(false);
  const [policyAlertsEvidenceAvailable, setPolicyAlertsEvidenceAvailable] =
    useState(false);
  const [
    currentPolicyAlertsEvidenceAvailable,
    setCurrentPolicyAlertsEvidenceAvailable,
  ] = useState(false);
  const [configPolicyEvidenceAvailable, setConfigPolicyEvidenceAvailable] =
    useState(false);
  const [vpsRuleEvidenceAvailable, setVpsRuleEvidenceAvailable] =
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

  const applyFleetCoreSnapshot = useCallback(
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
    },
    [publishFleetError],
  );

  const applyFleetTelemetrySnapshot = useCallback(
    (snapshot: FleetSnapshotRecord) => {
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
      if (snapshot.telemetry_uptimes.data) {
        setTelemetryUptimes(
          withoutDeletedClients(
            snapshot.telemetry_uptimes.data,
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
          ["uptime telemetry", snapshot.telemetry_uptimes],
        ]),
      );
    },
    [publishFleetError],
  );

  const applyFleetDetailSnapshot = useCallback(
    (snapshot: FleetSnapshotRecord) => {
      const fleetAlertsSource = requiredSnapshotSource(snapshot.fleet_alerts);
      const fleetAlertHistorySource = requiredSnapshotSource(
        snapshot.fleet_alert_history,
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
      const currentPolicyAlertsSource = requiredSnapshotSource(
        snapshot.current_policy_alerts,
      );
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
      setFleetAlertHistoryEvidenceAvailable(
        sourceAvailable(fleetAlertHistorySource),
      );
      setPolicyAlertsEvidenceAvailable(sourceAvailable(policyAlertsSource));
      setCurrentPolicyAlertsEvidenceAvailable(
        sourceAvailable(currentPolicyAlertsSource),
      );
      setVpsRuleEvidenceAvailable(sourceAvailable(vpsRuleValuesSource));
      setConfigPolicyEvidenceAvailable(
        [
          fleetAlertPoliciesSource,
          vpsRuleValuesSource,
          trafficAccountingSource,
        ].every(sourceAvailable),
      );
      if (fleetAlertsSource.data) {
        setFleetAlerts(fleetAlertsSource.data);
        setFleetAlertsTruncated(snapshot.fleet_alerts_truncated !== false);
      } else {
        // Current alert episodes are temporal state. Never retain them as
        // current when the source that can prove their lifecycle is missing.
        setFleetAlerts([]);
        setFleetAlertsTruncated(false);
      }
      if (fleetAlertHistorySource.data) {
        setFleetAlertHistory(fleetAlertHistorySource.data);
        setFleetAlertHistoryTruncated(
          snapshot.fleet_alert_history_truncated !== false,
        );
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
        setPolicyAlerts(policyAlertsSource.data);
        setPolicyAlertsTruncated(snapshot.policy_alerts_truncated !== false);
      }
      if (currentPolicyAlertsSource.data) {
        setCurrentPolicyAlerts(currentPolicyAlertsSource.data);
        setCurrentPolicyAlertsTruncated(
          snapshot.current_policy_alerts_truncated !== false,
        );
      } else {
        // Missing or unavailable current evidence cannot retain stale episodes
        // that may since have resolved.
        setCurrentPolicyAlerts([]);
        setCurrentPolicyAlertsTruncated(false);
      }
      if (notificationChannelsSource.data) {
        setFleetAlertNotificationChannels(notificationChannelsSource.data);
      }
      if (notificationsSource.data) {
        setFleetAlertNotifications(notificationsSource.data);
        setFleetAlertNotificationsTruncated(
          snapshot.fleet_alert_notifications_truncated !== false,
        );
      }
      if (webhookRulesSource.data) {
        setWebhookRules(webhookRulesSource.data);
      }
      if (webhookDeliveriesSource.data) {
        setWebhookRuleDeliveries(webhookDeliveriesSource.data);
        setWebhookRuleDeliveriesTruncated(
          snapshot.webhook_rule_deliveries_truncated !== false,
        );
      }
      publishFleetError(
        "detail",
        snapshotSourceErrorSummary(
          "Some fleet detail sources are unavailable",
          [
            ["fleet alerts", fleetAlertsSource],
            ["fleet alert history", fleetAlertHistorySource],
            ["fleet alert policies", fleetAlertPoliciesSource],
            ["VPS rules", vpsRuleValuesSource],
            ["traffic accounting", trafficAccountingSource],
            ["policy alert history", policyAlertsSource],
            ["current policy alerts", currentPolicyAlertsSource],
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

  const beginHomeFleetHydration = useCallback(
    (): HomeFleetHydrationFence => ({
      core: ++fleetCoreGeneration.current,
      detail: ++fleetFullGeneration.current,
      telemetry: ++fleetTelemetryGeneration.current,
    }),
    [],
  );

  const hydrateHomeFleet = useCallback(
    (fence: HomeFleetHydrationFence, snapshot: HomeFleetSnapshotRecord) => {
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      if (fence.core === fleetCoreGeneration.current) {
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
      }

      if (fence.telemetry === fleetTelemetryGeneration.current) {
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
        publishFleetError(
          "telemetry",
          snapshotSourceErrorSummary(
            "Some live fleet sources are unavailable",
            [
              ["telemetry rollups", snapshot.telemetry_rollups],
              ["network rates", snapshot.telemetry_network_rates],
            ],
          ),
        );
      }

      if (fence.detail === fleetFullGeneration.current) {
        setFleetAlertsEvidenceAvailable(sourceAvailable(snapshot.fleet_alerts));
        if (snapshot.fleet_alerts.data) {
          setFleetAlerts(snapshot.fleet_alerts.data);
          setFleetAlertsTruncated(snapshot.fleet_alerts_truncated !== false);
        } else {
          setFleetAlerts([]);
          setFleetAlertsTruncated(false);
        }
        publishFleetError(
          "detail",
          snapshotSourceErrorSummary(
            "Some fleet detail sources are unavailable",
            [["fleet alerts", snapshot.fleet_alerts]],
          ),
        );
      }
    },
    [apiToken, publishFleetError],
  );

  const loadFleet = useCallback(
    (trailingRefresh = false) => {
      if (apiTokenRef.current !== apiToken) {
        return Promise.resolve();
      }
      if (fleetFullInFlight.current?.token === apiToken) {
        if (trailingRefresh) fleetFullRefreshPending.current = true;
        return fleetFullInFlight.current.promise;
      }
      const request = (async () => {
        if (fleetTelemetryInFlight.current?.token === apiToken) {
          await fleetTelemetryInFlight.current.promise;
        }
        if (apiTokenRef.current !== apiToken) {
          return;
        }
        const fullGeneration = ++fleetFullGeneration.current;
        const coreGeneration = ++fleetCoreGeneration.current;
        const telemetryGeneration = ++fleetTelemetryGeneration.current;
        try {
          const snapshot = await apiGet<FleetSnapshotRecord>(
            "/api/v1/fleet/snapshot?mode=full",
            apiToken,
          );
          if (apiTokenRef.current !== apiToken || snapshot.mode !== "full") {
            return;
          }
          const coreIsCurrent = coreGeneration === fleetCoreGeneration.current;
          const telemetryIsCurrent =
            telemetryGeneration === fleetTelemetryGeneration.current;
          const fullLoadIsCurrent =
            fullGeneration === fleetFullGeneration.current;
          if (coreIsCurrent) {
            applyFleetCoreSnapshot(snapshot);
          }
          if (telemetryIsCurrent) {
            applyFleetTelemetrySnapshot(snapshot);
          }
          if (fullLoadIsCurrent) {
            applyFleetDetailSnapshot(snapshot);
          }
        } catch (error) {
          const fullLoadIsCurrent =
            fullGeneration === fleetFullGeneration.current;
          const coreIsCurrent = coreGeneration === fleetCoreGeneration.current;
          const telemetryIsCurrent =
            telemetryGeneration === fleetTelemetryGeneration.current;
          if (
            apiTokenRef.current !== apiToken ||
            (!fullLoadIsCurrent && !coreIsCurrent && !telemetryIsCurrent)
          ) {
            return;
          }
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            setSummary(emptySummary);
            setAgents([]);
            setFleetAlerts([]);
            setFleetAlertsTruncated(false);
            setFleetAlertHistory([]);
            setFleetAlertHistoryTruncated(false);
            setFleetAlertPolicies([]);
            setVpsRuleValues([]);
            setTrafficAccounting([]);
            setPolicyAlerts([]);
            setPolicyAlertsTruncated(false);
            setCurrentPolicyAlerts([]);
            setCurrentPolicyAlertsTruncated(false);
            setFleetAlertNotificationChannels([]);
            setFleetAlertNotifications([]);
            setFleetAlertNotificationsTruncated(false);
            setWebhookRules([]);
            setWebhookRuleDeliveries([]);
            setWebhookRuleDeliveriesTruncated(false);
            setTelemetryRollups([]);
            setTelemetryNetworkRates([]);
            setTelemetryTunnels([]);
            setTelemetryUptimes([]);
            setFleetCoreEvidenceAvailable(false);
            setFleetAlertsEvidenceAvailable(false);
            setFleetAlertHistoryEvidenceAvailable(false);
            setPolicyAlertsEvidenceAvailable(false);
            setCurrentPolicyAlertsEvidenceAvailable(false);
            setConfigPolicyEvidenceAvailable(false);
            setVpsRuleEvidenceAvailable(false);
            fleetSourceErrors.current = { core: "Operator login required" };
            setApiError("Operator login required");
            return;
          }
          const message =
            error instanceof Error
              ? error.message
              : "Fleet refresh unavailable";
          if (coreIsCurrent) {
            setFleetCoreEvidenceAvailable(false);
            publishFleetError("core", message);
          }
          if (telemetryIsCurrent) {
            publishFleetError("telemetry", message);
          }
          if (fullLoadIsCurrent) {
            setFleetAlertsEvidenceAvailable(false);
            setFleetAlerts([]);
            setFleetAlertsTruncated(false);
            setFleetAlertHistoryEvidenceAvailable(false);
            setPolicyAlertsEvidenceAvailable(false);
            setCurrentPolicyAlertsEvidenceAvailable(false);
            setConfigPolicyEvidenceAvailable(false);
            setVpsRuleEvidenceAvailable(false);
            setCurrentPolicyAlerts([]);
            setCurrentPolicyAlertsTruncated(false);
            publishFleetError("detail", message);
          }
        }
      })();
      const trackedRequest = request.finally(() => {
        if (fleetFullInFlight.current?.promise === trackedRequest) {
          fleetFullInFlight.current = null;
          if (
            fleetFullRefreshPending.current &&
            apiTokenRef.current === apiToken
          ) {
            fleetFullRefreshPending.current = false;
            fleetTelemetryRefreshPending.current = false;
            queueMicrotask(() => void loadFleetRef.current());
          } else if (
            fleetTelemetryRefreshPending.current &&
            apiTokenRef.current === apiToken
          ) {
            fleetTelemetryRefreshPending.current = false;
            queueMicrotask(() => void loadFleetTelemetryRef.current());
          }
        }
      });
      fleetFullInFlight.current = { token: apiToken, promise: trackedRequest };
      return trackedRequest;
    },
    [
      apiToken,
      applyFleetCoreSnapshot,
      applyFleetDetailSnapshot,
      applyFleetTelemetrySnapshot,
      onUnauthorized,
      publishFleetError,
    ],
  );
  loadFleetRef.current = loadFleet;

  const loadFleetTelemetry = useCallback(
    (trailingRefresh = false) => {
      if (apiTokenRef.current !== apiToken) {
        return Promise.resolve();
      }
      if (fleetFullInFlight.current?.token === apiToken) {
        if (trailingRefresh) fleetTelemetryRefreshPending.current = true;
        return fleetFullInFlight.current.promise;
      }
      if (fleetTelemetryInFlight.current?.token === apiToken) {
        if (trailingRefresh) fleetTelemetryRefreshPending.current = true;
        return fleetTelemetryInFlight.current.promise;
      }
      const coreGeneration = ++fleetCoreGeneration.current;
      const telemetryGeneration = ++fleetTelemetryGeneration.current;
      const request = (async () => {
        try {
          const snapshot = await apiGet<FleetSnapshotRecord>(
            "/api/v1/fleet/snapshot?mode=live",
            apiToken,
          );
          if (apiTokenRef.current !== apiToken || snapshot.mode !== "live") {
            return;
          }
          if (coreGeneration === fleetCoreGeneration.current) {
            applyFleetCoreSnapshot(snapshot);
          }
          if (telemetryGeneration === fleetTelemetryGeneration.current) {
            applyFleetTelemetrySnapshot(snapshot);
          }
        } catch (error) {
          const coreIsCurrent = coreGeneration === fleetCoreGeneration.current;
          const telemetryIsCurrent =
            telemetryGeneration === fleetTelemetryGeneration.current;
          if (
            apiTokenRef.current !== apiToken ||
            (!coreIsCurrent && !telemetryIsCurrent)
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
            setTelemetryUptimes([]);
            setFleetCoreEvidenceAvailable(false);
            setFleetAlertsEvidenceAvailable(false);
            fleetSourceErrors.current = { core: "Operator login required" };
            setApiError("Operator login required");
            return;
          }
          const message =
            error instanceof Error
              ? error.message
              : "Live fleet telemetry unavailable";
          if (coreIsCurrent) {
            setFleetCoreEvidenceAvailable(false);
            publishFleetError("core", message);
          }
          if (telemetryIsCurrent) {
            publishFleetError("telemetry", message);
          }
        }
      })();
      const trackedRequest = request.finally(() => {
        if (fleetTelemetryInFlight.current?.promise === trackedRequest) {
          fleetTelemetryInFlight.current = null;
          if (
            fleetTelemetryRefreshPending.current &&
            apiTokenRef.current === apiToken
          ) {
            fleetTelemetryRefreshPending.current = false;
            queueMicrotask(() => void loadFleetTelemetryRef.current());
          }
        }
      });
      fleetTelemetryInFlight.current = {
        token: apiToken,
        promise: trackedRequest,
      };
      return trackedRequest;
    },
    [
      apiToken,
      applyFleetCoreSnapshot,
      applyFleetTelemetrySnapshot,
      onUnauthorized,
      publishFleetError,
    ],
  );
  loadFleetTelemetryRef.current = loadFleetTelemetry;
  const replaceFleetSnapshot = useCallback(
    (nextSummary: FleetSummary, nextAgents: AgentView[]) => {
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      fleetCoreGeneration.current += 1;
      const staleDeletedIds = deletedIdsInAgentSnapshot(
        nextAgents,
        deletedClientIds.current,
      );
      setAgents(withoutDeletedAgents(nextAgents, deletedClientIds.current));
      if (staleDeletedIds.length > 0) {
        setFleetCoreEvidenceAvailable(false);
        publishFleetError("core", staleFleetSnapshotMessage(staleDeletedIds));
        void loadFleet(true);
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
      await loadFleet(true);
      return agent;
    },
    [apiToken, loadFleet],
  );

  const mutateAgentSuspensions = useCallback(
    async (
      targets: AgentSuspensionBatchTarget[],
    ): Promise<AgentSuspensionBatchOutcome[]> => {
      const outcomes: AgentSuspensionBatchOutcome[] = [];
      for (let index = 0; index < targets.length; index += 1) {
        const target = targets[index];
        try {
          const response = await apiPost<AgentSuspensionMutationResponse>(
            `/api/v1/agents/${encodeURIComponent(target.client_id)}/${target.action}`,
            apiToken,
            target.action === "suspend"
              ? { confirmed: true, reason: target.reason ?? null }
              : { confirmed: true },
          );
          outcomes.push({
            action: target.action,
            client_id: target.client_id,
            response,
            error: null,
          });
          if (apiTokenRef.current === apiToken) {
            setAgents((current) =>
              current.map((agent) =>
                agent.id === response.agent.id ? response.agent : agent,
              ),
            );
          }
        } catch (error) {
          const message =
            error instanceof Error ? error.message : String(error);
          outcomes.push({
            action: target.action,
            client_id: target.client_id,
            response: null,
            error: message,
          });
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            for (const skipped of targets.slice(index + 1)) {
              outcomes.push({
                action: skipped.action,
                client_id: skipped.client_id,
                response: null,
                error: "Not attempted because the operator session expired.",
              });
            }
            break;
          }
        }
      }
      if (
        apiTokenRef.current === apiToken &&
        outcomes.some((outcome) => outcome.response !== null)
      ) {
        await loadFleet(true);
      }
      return outcomes;
    },
    [apiToken, loadFleet, onUnauthorized],
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
          const message =
            error instanceof Error ? error.message : String(error);
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
        setVpsRuleValues((current) =>
          current.filter((rule) => !deletedIds.has(rule.client_id)),
        );
        setTrafficAccounting((current) =>
          current.filter((record) => !deletedIds.has(record.client_id)),
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
        setTelemetryUptimes((current) =>
          current.filter((record) => !deletedIds.has(record.client_id)),
        );
        await loadFleet(true);
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
      await loadFleet(true);
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
      const preview = await (
        request.confirmed ? apiPost : apiPostPreview
      )<VpsRulesDryRunResponse>(
        "/api/v1/vps-rules/bulk-upsert",
        apiToken,
        request,
      );
      await loadFleet(true);
      return preview;
    },
    [apiToken, loadFleet],
  );

  const bulkUnsetVpsRules = useCallback(
    async (request: VpsRulesBulkUnsetRequest) => {
      const preview = await (
        request.confirmed ? apiPost : apiPostPreview
      )<VpsRulesDryRunResponse>(
        "/api/v1/vps-rules/bulk-unset",
        apiToken,
        request,
      );
      await loadFleet(true);
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
      await loadFleet(true);
    },
    [apiToken, loadFleet],
  );

  const bulkUpdateFleetAlertStates = useCallback(
    async (request: FleetAlertStateBulkRequest) => {
      try {
        const response = await apiPost<FleetAlertStateBulkResponse>(
          "/api/v1/fleet-alert-states/bulk",
          apiToken,
          request,
        );
        if (apiTokenRef.current !== apiToken) {
          return response;
        }
        setFleetAlerts((current) =>
          applyFleetAlertStates(current, response.states),
        );
        setFleetAlertHistory((current) =>
          applyFleetAlertStates(current, response.states),
        );
        setFleetAlertEventReviewItems((current) =>
          applyFleetAlertStates(current, response.states),
        );
        return response;
      } catch (error) {
        if (apiTokenRef.current === apiToken) {
          try {
            await loadFleet(true);
          } catch {
            // Recovery is best effort; preserve the original mutation failure.
          }
        }
        throw error;
      }
    },
    [apiToken, loadFleet],
  );

  const resolveFleetAlert = useCallback(
    async (alertId: string, request: FleetAlertResolveRequest) => {
      const alert = await apiPost<FleetAlertRecord>(
        `/api/v1/fleet-alerts/${encodeURIComponent(alertId)}/resolve`,
        apiToken,
        request,
      );
      if (apiTokenRef.current !== apiToken) {
        return alert;
      }
      setFleetAlerts((current) =>
        current.filter((stored) => stored.id !== alert.id),
      );
      setFleetAlertEventReviewItems((current) =>
        current.filter((stored) => stored.id !== alert.id),
      );
      setFleetAlertHistory((current) => [
        alert,
        ...current.filter((stored) => stored.id !== alert.id),
      ]);
      await loadFleet(true);
      return alert;
    },
    [apiToken, loadFleet],
  );

  const loadOlderFleetAlertEvents = useCallback(async () => {
    if (
      fleetAlertEventReviewInFlight.current ||
      (fleetAlertEventReviewStarted && !fleetAlertEventReviewHasMore)
    ) {
      return;
    }
    fleetAlertEventReviewInFlight.current = true;
    setFleetAlertEventReviewLoading(true);
    setFleetAlertEventReviewError(null);
    let cursor = fleetAlertEventReviewStarted
      ? fleetAlertEventReviewCursor
      : null;
    const knownIds = new Set([
      ...fleetAlerts.map((alert) => alert.id),
      ...fleetAlertEventReviewItems.map((alert) => alert.id),
    ]);
    try {
      // The unified snapshot contains at most 200 rows. The first review page
      // can therefore be entirely duplicate; advance one more page in the
      // same explicit operator action so the first older incident is reachable.
      for (let pageIndex = 0; pageIndex < 2; pageIndex += 1) {
        const query = new URLSearchParams({
          include_muted: "true",
          limit: "200",
        });
        if (cursor) {
          query.set("cursor", cursor);
        }
        const requestedCursor = cursor;
        const page = await apiGet<FleetAlertEventPage>(
          `/api/v1/fleet-alert-events?${query.toString()}`,
          apiToken,
        );
        assertFleetAlertEventPage(page, requestedCursor);
        if (apiTokenRef.current !== apiToken) {
          return;
        }
        const newItems = page.items.filter((alert) => !knownIds.has(alert.id));
        for (const alert of page.items) {
          knownIds.add(alert.id);
        }
        setFleetAlertEventReviewItems((current) =>
          dedupeFleetAlertsById([...current, ...page.items]),
        );
        cursor = page.next_cursor;
        setFleetAlertEventReviewCursor(cursor);
        setFleetAlertEventReviewHasMore(page.has_more);
        setFleetAlertEventReviewStarted(true);
        if (newItems.length > 0 || !page.has_more) {
          break;
        }
      }
    } catch (error) {
      if (apiTokenRef.current === apiToken) {
        setFleetAlertEventReviewError(
          error instanceof Error
            ? error.message
            : "Older current incidents could not be loaded.",
        );
      }
    } finally {
      if (apiTokenRef.current === apiToken) {
        setFleetAlertEventReviewLoading(false);
      }
      fleetAlertEventReviewInFlight.current = false;
    }
  }, [
    apiToken,
    fleetAlertEventReviewCursor,
    fleetAlertEventReviewHasMore,
    fleetAlertEventReviewItems,
    fleetAlertEventReviewStarted,
    fleetAlerts,
  ]);

  const refreshFleetAlertEvents = useCallback(async () => {
    if (fleetAlertEventReviewInFlight.current) {
      return;
    }
    fleetAlertEventReviewInFlight.current = true;
    setFleetAlertEventReviewLoading(true);
    setFleetAlertEventReviewError(null);
    try {
      const page = await apiGet<FleetAlertEventPage>(
        "/api/v1/fleet-alert-events?include_muted=true&limit=200",
        apiToken,
      );
      assertFleetAlertEventPage(page, null);
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      // Replace the entire prior cursor walk only after a valid page arrives.
      // This atomically removes events resolved by another operator.
      setFleetAlertEventReviewItems(dedupeFleetAlertsById(page.items));
      setFleetAlertEventReviewCursor(page.next_cursor);
      setFleetAlertEventReviewHasMore(page.has_more);
      setFleetAlertEventReviewStarted(true);
    } catch (error) {
      if (apiTokenRef.current === apiToken) {
        setFleetAlertEventReviewError(
          error instanceof Error
            ? error.message
            : "Current incidents could not be refreshed.",
        );
      }
    } finally {
      if (apiTokenRef.current === apiToken) {
        setFleetAlertEventReviewLoading(false);
      }
      fleetAlertEventReviewInFlight.current = false;
    }
  }, [apiToken]);

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
      await loadFleet(true);
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
      await loadFleet(true);
    },
    [apiToken, loadFleet],
  );

  const dispatchFleetAlertNotifications = useCallback(
    async (request: FleetAlertNotificationDispatchRequest) => {
      const deliveries = await (request.dry_run ? apiPostPreview : apiPost)<
        FleetAlertNotificationDeliveryRecord[]
      >("/api/v1/fleet-alert-notifications/dispatch", apiToken, request);
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
        await loadFleet(true);
      }
      return deliveries;
    },
    [apiToken, loadFleet],
  );

  const processFleetAlertNotifications = useCallback(
    async (request: FleetAlertNotificationProcessRequest) => {
      const deliveries = await (request.dry_run ? apiPostPreview : apiPost)<
        FleetAlertNotificationDeliveryRecord[]
      >("/api/v1/fleet-alert-notifications/process", apiToken, request);
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
        await loadFleet(true);
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
      void loadFleet(true);
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
      await loadFleet(true);
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
      const deliveries = await (request.dry_run ? apiPostPreview : apiPost)<
        WebhookRuleDeliveryRecord[]
      >("/api/v1/webhook-rules/dispatch", apiToken, request);
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
        await loadFleet(true);
      }
      return deliveries;
    },
    [apiToken, loadFleet],
  );

  const processWebhookRuleDeliveries = useCallback(
    async (request: WebhookRuleProcessRequest) => {
      const deliveries = await (request.dry_run ? apiPostPreview : apiPost)<
        WebhookRuleDeliveryRecord[]
      >("/api/v1/webhook-deliveries/process", apiToken, request);
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
        await loadFleet(true);
      }
      return deliveries;
    },
    [apiToken, loadFleet],
  );

  const rotateWebhookDeliveryHistory = useCallback(
    async (request: WebhookDeliveryRotationRequest) => {
      const response = await (
        request.confirmed ? apiPost : apiPostPreview
      )<WebhookDeliveryRotationResponse>(
        "/api/v1/webhook-deliveries/rotate",
        apiToken,
        request,
      );
      if (request.confirmed) {
        await loadFleet(true);
      }
      return response;
    },
    [apiToken, loadFleet],
  );

  const clearFleet = useCallback(() => {
    apiTokenRef.current = "";
    fleetFullGeneration.current += 1;
    fleetCoreGeneration.current += 1;
    fleetTelemetryGeneration.current += 1;
    fleetTelemetryInFlight.current = null;
    fleetFullInFlight.current = null;
    fleetFullRefreshPending.current = false;
    fleetTelemetryRefreshPending.current = false;
    fleetAlertEventReviewInFlight.current = false;
    fleetSourceErrors.current = {};
    deletedClientIds.current.clear();
    setSummary(emptySummary);
    setAgents([]);
    setFleetAlerts([]);
    setFleetAlertsTruncated(false);
    setFleetAlertHistory([]);
    setFleetAlertHistoryTruncated(false);
    setFleetAlertEventReviewItems([]);
    setFleetAlertEventReviewCursor(null);
    setFleetAlertEventReviewHasMore(false);
    setFleetAlertEventReviewStarted(false);
    setFleetAlertEventReviewLoading(false);
    setFleetAlertEventReviewError(null);
    setFleetAlertPolicies([]);
    setVpsRuleValues([]);
    setTrafficAccounting([]);
    setPolicyAlerts([]);
    setPolicyAlertsTruncated(false);
    setCurrentPolicyAlerts([]);
    setCurrentPolicyAlertsTruncated(false);
    setFleetAlertNotificationChannels([]);
    setFleetAlertNotifications([]);
    setFleetAlertNotificationsTruncated(false);
    setWebhookRules([]);
    setWebhookRuleDeliveries([]);
    setWebhookRuleDeliveriesTruncated(false);
    setTelemetryRollups([]);
    setTelemetryNetworkRates([]);
    setTelemetryTunnels([]);
    setTelemetryUptimes([]);
    setFleetCoreEvidenceAvailable(false);
    setFleetAlertsEvidenceAvailable(false);
    setFleetAlertHistoryEvidenceAvailable(false);
    setPolicyAlertsEvidenceAvailable(false);
    setCurrentPolicyAlertsEvidenceAvailable(false);
    setConfigPolicyEvidenceAvailable(false);
    setVpsRuleEvidenceAvailable(false);
    setApiError(null);
  }, []);

  return {
    agents,
    apiError,
    beginHomeFleetHydration,
    clearFleet,
    configPolicyEvidenceAvailable,
    vpsRuleEvidenceAvailable,
    fleetAlertsEvidenceAvailable,
    fleetAlerts,
    fleetAlertsTruncated,
    fleetAlertHistory,
    fleetAlertHistoryTruncated,
    fleetAlertHistoryEvidenceAvailable,
    fleetAlertEventReviewItems,
    fleetAlertEventReviewHasMore,
    fleetAlertEventReviewStarted,
    fleetAlertEventReviewLoading,
    fleetAlertEventReviewError,
    hydrateHomeFleet,
    fleetAlertPolicies,
    vpsRuleValues,
    trafficAccounting,
    policyAlerts,
    policyAlertsTruncated,
    policyAlertsEvidenceAvailable,
    currentPolicyAlerts,
    currentPolicyAlertsTruncated,
    currentPolicyAlertsEvidenceAvailable,
    fleetAlertNotificationChannels,
    fleetAlertNotifications,
    fleetAlertNotificationsTruncated,
    webhookRules,
    webhookRuleDeliveries,
    webhookRuleDeliveriesTruncated,
    deleteAgents,
    loadFleet,
    loadOlderFleetAlertEvents,
    refreshFleetAlertEvents,
    loadFleetTelemetry,
    fleetCoreEvidenceAvailable,
    mutateAgentSuspensions,
    replaceFleetSnapshot,
    updateAgentAlias,
    resolveFleetAlert,
    summary,
    telemetryNetworkRates,
    telemetryRollups,
    telemetryTunnels,
    telemetryUptimes,
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
    bulkUpdateFleetAlertStates,
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

function assertFleetAlertEventPage(
  value: unknown,
  requestedCursor: string | null,
): asserts value is FleetAlertEventPage {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Incident review returned an invalid page.");
  }
  const page = value as Partial<FleetAlertEventPage>;
  if (
    !Array.isArray(page.items) ||
    typeof page.has_more !== "boolean" ||
    (page.next_cursor !== null && typeof page.next_cursor !== "string") ||
    (page.has_more && !page.next_cursor?.trim()) ||
    (!page.has_more && page.next_cursor !== null) ||
    (requestedCursor !== null && page.next_cursor === requestedCursor)
  ) {
    throw new Error("Incident review returned inconsistent cursor state.");
  }
  const ids = new Set<string>();
  for (const item of page.items) {
    if (
      !item ||
      typeof item !== "object" ||
      typeof item.id !== "string" ||
      !item.id.trim() ||
      ids.has(item.id) ||
      item.record_kind !== "event" ||
      !item.lifecycle ||
      item.lifecycle.resolved_at !== null
    ) {
      throw new Error(
        "Incident review returned a duplicate, non-event, or resolved record.",
      );
    }
    ids.add(item.id);
  }
}

function dedupeFleetAlertsById(alerts: FleetAlertRecord[]): FleetAlertRecord[] {
  const byId = new Map<string, FleetAlertRecord>();
  for (const alert of alerts) {
    byId.set(alert.id, alert);
  }
  return Array.from(byId.values());
}

export function applyFleetAlertStates(
  alerts: FleetAlertRecord[],
  states: FleetAlertStateRecord[],
): FleetAlertRecord[] {
  const stateByAlertId = latestFleetAlertStateById(states);
  return alerts.map((alert) =>
    applyFleetAlertState(alert, stateByAlertId.get(alert.id)),
  );
}

function applyFleetAlertState(
  alert: FleetAlertRecord,
  state: FleetAlertStateRecord | undefined,
): FleetAlertRecord {
  if (!state || state.revision < (alert.state_revision ?? 0)) {
    return alert;
  }
  return {
    ...alert,
    escalation_level: state.escalation_level,
    muted_until_unix: state.muted_until_unix,
    operator_state: state.state,
    state_actor_id: state.actor_id,
    state_reason: state.reason,
    state_revision: state.revision,
    state_updated_at: state.updated_at,
  };
}

function latestFleetAlertStateById(
  states: FleetAlertStateRecord[],
): Map<string, FleetAlertStateRecord> {
  const latest = new Map<string, FleetAlertStateRecord>();
  for (const state of states) {
    const existing = latest.get(state.alert_id);
    if (!existing || state.revision > existing.revision) {
      latest.set(state.alert_id, state);
    }
  }
  return latest;
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
