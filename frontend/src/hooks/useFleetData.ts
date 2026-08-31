import { useCallback, useRef, useState } from "react";
import { apiGet, apiPost, apiPostPreview, isApiUnauthorized } from "../api";
import { emptySummary } from "../constants";
import type { SnapshotSource } from "../homeSnapshot";
import type {
  AlertConfigurationBulkRequest,
  AlertConfigurationBulkResponse,
  AlertConfigurationRecord,
  AgentSuspensionBatchOutcome,
  AgentSuspensionBatchTarget,
  AgentView,
  BulkAgentSuspensionRequest,
  BulkAgentSuspensionResponse,
  BulkDeleteAgentsRequest,
  BulkDeleteAgentsResponse,
  FleetAlertPolicyRecord,
  FleetAlertPolicyRequest,
  FleetAlertRecord,
  FleetAlertEventPage,
  FleetAlertEventSyncResponse,
  FleetAlertBulkResolveRequest,
  FleetAlertBulkResolveResponse,
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
  TagMutationResponse,
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

const FLEET_ERROR_SOURCE_ORDER = [
  "core",
  "detail",
  "detail-list",
  "telemetry",
] as const;
const FLEET_DETAIL_LIST_ERROR_SOURCE_ORDER = [
  "fleet alert policies",
  "VPS rules",
  "notification channels",
  "webhook rules",
  "webhook deliveries",
] as const;
const FLEET_DETAIL_ERROR_SOURCE_ORDER = [
  "fleet alerts",
  "fleet alert history",
  "traffic accounting",
  "policy alert history",
  "current policy alerts",
  "notification deliveries",
] as const;
const FLEET_ALERT_EVENT_PAGE_LIMIT = 200;
const FLEET_ALERT_EVENT_SYNC_ID_LIMIT = 5_000;
const FLEET_DELIVERY_LIST_LIMIT = 200;
type FleetErrorSource = (typeof FLEET_ERROR_SOURCE_ORDER)[number];
type FleetDetailListErrorSource =
  (typeof FLEET_DETAIL_LIST_ERROR_SOURCE_ORDER)[number];
type FleetDetailErrorSource = (typeof FLEET_DETAIL_ERROR_SOURCE_ORDER)[number];

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
  alerts: number;
  telemetry: number;
};

type FleetDetailProjectionFence = {
  alerts: number;
  alertPolicies: number;
  policyEpisodes: number;
  vpsRules: number;
  notificationChannels: number;
  notificationDeliveries: number;
  webhookRules: number;
  webhookDeliveries: number;
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
  const fleetAlertProjectionGeneration = useRef(0);
  const fleetAlertPolicyListGeneration = useRef(0);
  const policyAlertEpisodeGeneration = useRef(0);
  const vpsRuleListGeneration = useRef(0);
  const fleetAlertNotificationChannelListGeneration = useRef(0);
  const fleetAlertNotificationDeliveryGeneration = useRef(0);
  const webhookRuleListGeneration = useRef(0);
  const webhookDeliveryListGeneration = useRef(0);
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
  const fleetAlertEventReviewInFlight = useRef<Promise<void> | null>(null);
  const fleetAlertEventReviewInFlightKind = useRef<
    "sync" | "older" | "search" | null
  >(null);
  const fleetAlertEventReviewActive = useRef(false);
  const fleetAlertEventSyncPending = useRef(false);
  const fleetAlertEventProjectionGeneration = useRef(0);
  const syncFleetAlertEventsRef = useRef<
    (queueTrailing?: boolean) => Promise<void>
  >(() => Promise.resolve());
  const deletedClientIds = useRef(new Set<string>());
  const fleetSourceErrors = useRef<Partial<Record<FleetErrorSource, string>>>(
    {},
  );
  const fleetDetailListErrors = useRef<
    Partial<Record<FleetDetailListErrorSource, string>>
  >({});
  const fleetDetailErrors = useRef<
    Partial<Record<FleetDetailErrorSource, string>>
  >({});
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
  const [
    fleetAlertEventReviewLimitNotice,
    setFleetAlertEventReviewLimitNotice,
  ] = useState<string | null>(null);
  const [fleetAlertEventReviewVerified, setFleetAlertEventReviewVerified] =
    useState(false);
  const fleetAlertEventReviewItemsRef = useRef<FleetAlertRecord[]>([]);
  const fleetAlertEventReviewCursorRef = useRef<string | null>(null);
  const fleetAlertEventReviewHasMoreRef = useRef(false);
  const fleetAlertEventReviewStartedRef = useRef(false);
  const fleetAlertEventSearchBaseCursorRef = useRef<string | null>(null);
  const fleetAlertEventSearchBaseHasMoreRef = useRef(false);
  const fleetAlertEventSearchCursorRef = useRef<string | null>(null);
  const fleetAlertEventSearchHasMoreRef = useRef(false);
  const fleetAlertEventSearchQueryRef = useRef("");
  fleetAlertEventReviewItemsRef.current = fleetAlertEventReviewItems;
  fleetAlertEventReviewCursorRef.current = fleetAlertEventReviewCursor;
  fleetAlertEventReviewHasMoreRef.current = fleetAlertEventReviewHasMore;
  fleetAlertEventReviewStartedRef.current = fleetAlertEventReviewStarted;
  const [fleetAlertEventSearchHasMore, setFleetAlertEventSearchHasMore] =
    useState(false);
  const [fleetAlertEventSearchQuery, setFleetAlertEventSearchQuery] =
    useState("");
  const [
    fleetAlertEventSearchScannedCount,
    setFleetAlertEventSearchScannedCount,
  ] = useState(0);
  const [fleetAlertEventSearchItems, setFleetAlertEventSearchItems] = useState<
    FleetAlertRecord[]
  >([]);
  const fleetAlertEventSearchItemsRef = useRef<FleetAlertRecord[]>([]);
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
  const [
    fleetAlertNotificationsTruncated,
    setFleetAlertNotificationsTruncated,
  ] = useState(false);
  const [webhookRules, setWebhookRules] = useState<WebhookRuleRecord[]>([]);
  const [webhookRuleDeliveries, setWebhookRuleDeliveries] = useState<
    WebhookRuleDeliveryRecord[]
  >([]);
  const [webhookRuleDeliveriesTruncated, setWebhookRuleDeliveriesTruncated] =
    useState(false);
  const fleetAlertNotificationsRef = useRef<
    FleetAlertNotificationDeliveryRecord[]
  >([]);
  const fleetAlertNotificationsTruncatedRef = useRef(false);
  const webhookRuleDeliveriesRef = useRef<WebhookRuleDeliveryRecord[]>([]);
  const webhookRuleDeliveriesTruncatedRef = useRef(false);
  fleetAlertNotificationsRef.current = fleetAlertNotifications;
  fleetAlertNotificationsTruncatedRef.current =
    fleetAlertNotificationsTruncated;
  webhookRuleDeliveriesRef.current = webhookRuleDeliveries;
  webhookRuleDeliveriesTruncatedRef.current = webhookRuleDeliveriesTruncated;
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

  const publishFleetDetailListError = useCallback(
    (source: FleetDetailListErrorSource, error: string | null) => {
      if (error) {
        fleetDetailListErrors.current[source] = error;
      } else {
        delete fleetDetailListErrors.current[source];
      }
      const errors = FLEET_DETAIL_LIST_ERROR_SOURCE_ORDER.flatMap((key) => {
        const current = fleetDetailListErrors.current[key];
        return current ? [`${key}: ${current}`] : [];
      });
      publishFleetError(
        "detail-list",
        errors.length > 0 ? errors.join("; ") : null,
      );
    },
    [publishFleetError],
  );

  const publishFleetDetailError = useCallback(
    (source: FleetDetailErrorSource, error: string | null) => {
      if (error) {
        fleetDetailErrors.current[source] = error;
      } else {
        delete fleetDetailErrors.current[source];
      }
      const errors = FLEET_DETAIL_ERROR_SOURCE_ORDER.flatMap((key) => {
        const current = fleetDetailErrors.current[key];
        return current ? [`${key}: ${current}`] : [];
      });
      publishFleetError(
        "detail",
        errors.length > 0
          ? `Some fleet detail sources are unavailable: ${errors.join("; ")}`
          : null,
      );
    },
    [publishFleetError],
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
    (snapshot: FleetSnapshotRecord, fence: FleetDetailProjectionFence) => {
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

      const alertsAreCurrent =
        fence.alerts === fleetAlertProjectionGeneration.current;
      const alertPoliciesAreCurrent =
        fence.alertPolicies === fleetAlertPolicyListGeneration.current;
      const policyEpisodesAreCurrent =
        fence.policyEpisodes === policyAlertEpisodeGeneration.current;
      const vpsRulesAreCurrent =
        fence.vpsRules === vpsRuleListGeneration.current;
      const notificationChannelsAreCurrent =
        fence.notificationChannels ===
        fleetAlertNotificationChannelListGeneration.current;
      const notificationDeliveriesAreCurrent =
        fence.notificationDeliveries ===
        fleetAlertNotificationDeliveryGeneration.current;
      const webhookRulesAreCurrent =
        fence.webhookRules === webhookRuleListGeneration.current;
      const webhookDeliveriesAreCurrent =
        fence.webhookDeliveries === webhookDeliveryListGeneration.current;

      if (alertsAreCurrent) {
        setFleetAlertsEvidenceAvailable(sourceAvailable(fleetAlertsSource));
        setFleetAlertHistoryEvidenceAvailable(
          sourceAvailable(fleetAlertHistorySource),
        );
      }
      if (policyEpisodesAreCurrent) {
        setPolicyAlertsEvidenceAvailable(sourceAvailable(policyAlertsSource));
        setCurrentPolicyAlertsEvidenceAvailable(
          sourceAvailable(currentPolicyAlertsSource),
        );
      }
      if (vpsRulesAreCurrent) {
        setVpsRuleEvidenceAvailable(sourceAvailable(vpsRuleValuesSource));
      }
      if (alertPoliciesAreCurrent && vpsRulesAreCurrent) {
        setConfigPolicyEvidenceAvailable(
          [
            fleetAlertPoliciesSource,
            vpsRuleValuesSource,
            trafficAccountingSource,
          ].every(sourceAvailable),
        );
      }

      if (alertsAreCurrent) {
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
      }
      if (alertPoliciesAreCurrent) {
        if (fleetAlertPoliciesSource.data) {
          setFleetAlertPolicies(fleetAlertPoliciesSource.data);
        }
        publishFleetDetailListError(
          "fleet alert policies",
          snapshotSourceFailure(fleetAlertPoliciesSource),
        );
      }
      if (vpsRulesAreCurrent) {
        if (vpsRuleValuesSource.data) {
          setVpsRuleValues(
            withoutDeletedClients(
              vpsRuleValuesSource.data,
              deletedClientIds.current,
            ),
          );
        }
        publishFleetDetailListError(
          "VPS rules",
          snapshotSourceFailure(vpsRuleValuesSource),
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
      if (policyEpisodesAreCurrent) {
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
      }
      if (notificationChannelsAreCurrent) {
        if (notificationChannelsSource.data) {
          setFleetAlertNotificationChannels(notificationChannelsSource.data);
        }
        publishFleetDetailListError(
          "notification channels",
          snapshotSourceFailure(notificationChannelsSource),
        );
      }
      if (notificationDeliveriesAreCurrent && notificationsSource.data) {
        fleetAlertNotificationsRef.current = notificationsSource.data;
        fleetAlertNotificationsTruncatedRef.current =
          snapshot.fleet_alert_notifications_truncated !== false;
        setFleetAlertNotifications(notificationsSource.data);
        setFleetAlertNotificationsTruncated(
          snapshot.fleet_alert_notifications_truncated !== false,
        );
      }
      if (webhookRulesAreCurrent) {
        if (webhookRulesSource.data) {
          setWebhookRules(webhookRulesSource.data);
        }
        publishFleetDetailListError(
          "webhook rules",
          snapshotSourceFailure(webhookRulesSource),
        );
      }
      if (webhookDeliveriesAreCurrent) {
        if (webhookDeliveriesSource.data) {
          webhookRuleDeliveriesRef.current = webhookDeliveriesSource.data;
          webhookRuleDeliveriesTruncatedRef.current =
            snapshot.webhook_rule_deliveries_truncated !== false;
          setWebhookRuleDeliveries(webhookDeliveriesSource.data);
          setWebhookRuleDeliveriesTruncated(
            snapshot.webhook_rule_deliveries_truncated !== false,
          );
        }
        publishFleetDetailListError(
          "webhook deliveries",
          snapshotSourceFailure(webhookDeliveriesSource),
        );
      }
      publishFleetDetailError(
        "traffic accounting",
        snapshotSourceFailure(trafficAccountingSource),
      );
      if (policyEpisodesAreCurrent) {
        publishFleetDetailError(
          "policy alert history",
          snapshotSourceFailure(policyAlertsSource),
        );
        publishFleetDetailError(
          "current policy alerts",
          snapshotSourceFailure(currentPolicyAlertsSource),
        );
      }
      if (alertsAreCurrent) {
        publishFleetDetailError(
          "fleet alerts",
          snapshotSourceFailure(fleetAlertsSource),
        );
        publishFleetDetailError(
          "fleet alert history",
          snapshotSourceFailure(fleetAlertHistorySource),
        );
      }
      if (notificationDeliveriesAreCurrent) {
        publishFleetDetailError(
          "notification deliveries",
          snapshotSourceFailure(notificationsSource),
        );
      }
    },
    [publishFleetDetailError, publishFleetDetailListError],
  );

  const beginHomeFleetHydration = useCallback(
    (): HomeFleetHydrationFence => ({
      core: ++fleetCoreGeneration.current,
      // Home owns only the alert slice of the full-detail projection. Claim
      // that source without invalidating unrelated configuration/detail reads.
      detail: fleetFullGeneration.current,
      alerts: ++fleetAlertProjectionGeneration.current,
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

      if (
        fence.detail === fleetFullGeneration.current &&
        fence.alerts === fleetAlertProjectionGeneration.current
      ) {
        setFleetAlertsEvidenceAvailable(sourceAvailable(snapshot.fleet_alerts));
        if (snapshot.fleet_alerts.data) {
          setFleetAlerts(snapshot.fleet_alerts.data);
          setFleetAlertsTruncated(snapshot.fleet_alerts_truncated !== false);
        } else {
          setFleetAlerts([]);
          setFleetAlertsTruncated(false);
        }
        publishFleetDetailError(
          "fleet alerts",
          snapshotSourceFailure(snapshot.fleet_alerts),
        );
      }
    },
    [apiToken, publishFleetDetailError, publishFleetError],
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
        // The full snapshot is one transport request, not one mutation owner.
        // Capture each locally mutable detail projection independently so a
        // later exact read or mutation fences only its own response field.
        const detailFence: FleetDetailProjectionFence = {
          // Claim every source when the aggregate request starts. Capturing
          // the current value would let an older exact read commit after this
          // newer aggregate response; independent increments preserve true
          // request-start ordering without coupling unrelated projections.
          alerts: ++fleetAlertProjectionGeneration.current,
          alertPolicies: ++fleetAlertPolicyListGeneration.current,
          policyEpisodes: ++policyAlertEpisodeGeneration.current,
          vpsRules: ++vpsRuleListGeneration.current,
          notificationChannels:
            ++fleetAlertNotificationChannelListGeneration.current,
          notificationDeliveries:
            ++fleetAlertNotificationDeliveryGeneration.current,
          webhookRules: ++webhookRuleListGeneration.current,
          webhookDeliveries: ++webhookDeliveryListGeneration.current,
        };
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
            applyFleetDetailSnapshot(snapshot, detailFence);
            if (fleetAlertEventReviewActive.current) {
              await syncFleetAlertEventsRef.current();
            }
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
            fleetAlertNotificationsRef.current = [];
            fleetAlertNotificationsTruncatedRef.current = false;
            setFleetAlertNotifications([]);
            setFleetAlertNotificationsTruncated(false);
            setWebhookRules([]);
            webhookRuleDeliveriesRef.current = [];
            webhookRuleDeliveriesTruncatedRef.current = false;
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
            fleetDetailErrors.current = {};
            fleetDetailListErrors.current = {};
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
            if (detailFence.alerts === fleetAlertProjectionGeneration.current) {
              setFleetAlertsEvidenceAvailable(false);
              setFleetAlerts([]);
              setFleetAlertsTruncated(false);
              setFleetAlertHistoryEvidenceAvailable(false);
              publishFleetDetailError("fleet alerts", message);
              publishFleetDetailError("fleet alert history", message);
            }
            if (
              detailFence.policyEpisodes ===
              policyAlertEpisodeGeneration.current
            ) {
              setPolicyAlertsEvidenceAvailable(false);
              setCurrentPolicyAlertsEvidenceAvailable(false);
              setCurrentPolicyAlerts([]);
              setCurrentPolicyAlertsTruncated(false);
              publishFleetDetailError("policy alert history", message);
              publishFleetDetailError("current policy alerts", message);
            }
            setConfigPolicyEvidenceAvailable(false);
            if (detailFence.vpsRules === vpsRuleListGeneration.current) {
              setVpsRuleEvidenceAvailable(false);
            }
            publishFleetDetailError("traffic accounting", message);
            if (
              detailFence.notificationDeliveries ===
              fleetAlertNotificationDeliveryGeneration.current
            ) {
              publishFleetDetailError("notification deliveries", message);
            }
            if (
              detailFence.alertPolicies ===
              fleetAlertPolicyListGeneration.current
            ) {
              publishFleetDetailListError("fleet alert policies", message);
            }
            if (detailFence.vpsRules === vpsRuleListGeneration.current) {
              publishFleetDetailListError("VPS rules", message);
            }
            if (
              detailFence.notificationChannels ===
              fleetAlertNotificationChannelListGeneration.current
            ) {
              publishFleetDetailListError("notification channels", message);
            }
            if (
              detailFence.webhookRules === webhookRuleListGeneration.current
            ) {
              publishFleetDetailListError("webhook rules", message);
            }
            if (
              detailFence.webhookDeliveries ===
              webhookDeliveryListGeneration.current
            ) {
              publishFleetDetailListError("webhook deliveries", message);
            }
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
      publishFleetDetailError,
      publishFleetDetailListError,
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
            fleetDetailErrors.current = {};
            fleetDetailListErrors.current = {};
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

  const publishFleetDetailListFailure = useCallback(
    (source: FleetDetailListErrorSource, error: unknown, fallback: string) => {
      if (isApiUnauthorized(error)) {
        publishFleetDetailListError(source, "Operator login required");
        onUnauthorized();
        return;
      }
      publishFleetDetailListError(
        source,
        error instanceof Error ? error.message : fallback,
      );
    },
    [onUnauthorized, publishFleetDetailListError],
  );

  const loadWebhookDeliveryList = useCallback(async () => {
    if (apiTokenRef.current !== apiToken) {
      return;
    }
    const generation = ++webhookDeliveryListGeneration.current;
    try {
      const deliveries = await apiGet<WebhookRuleDeliveryRecord[]>(
        `/api/v1/webhook-deliveries?limit=${FLEET_DELIVERY_LIST_LIMIT + 1}`,
        apiToken,
      );
      if (
        apiTokenRef.current !== apiToken ||
        webhookDeliveryListGeneration.current !== generation
      ) {
        return;
      }
      setWebhookRuleDeliveries(deliveries.slice(0, FLEET_DELIVERY_LIST_LIMIT));
      const truncated = deliveries.length > FLEET_DELIVERY_LIST_LIMIT;
      webhookRuleDeliveriesRef.current = deliveries.slice(
        0,
        FLEET_DELIVERY_LIST_LIMIT,
      );
      webhookRuleDeliveriesTruncatedRef.current = truncated;
      setWebhookRuleDeliveriesTruncated(truncated);
      publishFleetDetailListError("webhook deliveries", null);
    } catch (error) {
      if (
        apiTokenRef.current === apiToken &&
        webhookDeliveryListGeneration.current === generation
      ) {
        publishFleetDetailListFailure(
          "webhook deliveries",
          error,
          "Webhook deliveries could not be refreshed.",
        );
      }
    }
  }, [apiToken, publishFleetDetailListError, publishFleetDetailListFailure]);

  const commitAgentProjection = useCallback(
    (update: (current: AgentView[]) => AgentView[]) => {
      // Core reads own summary and AgentView as one projection. Advance that
      // owner synchronously before scheduling the local state patch so an
      // older live, full, or Home response cannot restore pre-mutation rows.
      ++fleetCoreGeneration.current;
      setAgents(update);
    },
    [],
  );

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
      commitAgentProjection((current) =>
        current.map((stored) => (stored.id === agent.id ? agent : stored)),
      );
      return agent;
    },
    [apiToken, commitAgentProjection],
  );

  const reconcileAgentTagMutation = useCallback(
    (response: TagMutationResponse) => {
      if (apiTokenRef.current !== apiToken || response.confirmation_required) {
        return;
      }
      const addTag = response.action === "add" || response.action === "assign";
      const removeTag =
        response.action === "remove" || response.action === "delete";
      if (!addTag && !removeTag) {
        return;
      }
      const clientIds = new Set(response.affected.map((agent) => agent.id));
      if (clientIds.size === 0) {
        return;
      }
      commitAgentProjection((current) =>
        current.map((agent) => {
          if (!clientIds.has(agent.id)) {
            return agent;
          }
          if (addTag) {
            return agent.tags.includes(response.tag)
              ? agent
              : { ...agent, tags: [...agent.tags, response.tag] };
          }
          if (!agent.tags.includes(response.tag)) {
            return agent;
          }
          return {
            ...agent,
            tags: agent.tags.filter((tag) => tag !== response.tag),
          };
        }),
      );
    },
    [apiToken, commitAgentProjection],
  );

  const mutateAgentSuspensions = useCallback(
    async (
      targets: AgentSuspensionBatchTarget[],
    ): Promise<AgentSuspensionBatchOutcome[]> => {
      if (targets.length === 0) {
        return [];
      }
      const action = targets[0].action;
      if (targets.some((target) => target.action !== action)) {
        throw new Error("A suspension batch must use one reviewed action.");
      }
      const clientIds = targets.map((target) => target.client_id);
      requireUniqueFleetMutationTargets(clientIds, "suspension");
      const reasons = new Set(
        targets.map((target) => target.reason?.trim() || null),
      );
      if (reasons.size !== 1) {
        throw new Error("A suspension batch must use one reviewed reason.");
      }
      const reason =
        action === "suspend" ? reasons.values().next().value : null;
      const request: BulkAgentSuspensionRequest = {
        action,
        client_ids: clientIds,
        confirmed: true,
        reason: reason ?? null,
      };
      let response: BulkAgentSuspensionResponse;
      try {
        response = await apiPost<BulkAgentSuspensionResponse>(
          "/api/v1/agents/suspensions",
          apiToken,
          request,
        );
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
        }
        throw error;
      }
      assertOrderedFleetMutationEnvelope(response, clientIds, "suspension");
      const outcomes = response.outcomes.map((outcome) => ({
        action,
        client_id: outcome.client_id,
        response: outcome.result ?? null,
        error:
          outcome.status === "rejected"
            ? (outcome.error_message ??
              outcome.error_code ??
              "The suspension change was rejected.")
            : null,
      }));
      if (apiTokenRef.current === apiToken) {
        const changedAgents = new Map(
          outcomes.flatMap((outcome) =>
            outcome.response
              ? [[outcome.response.agent.id, outcome.response.agent] as const]
              : [],
          ),
        );
        if (changedAgents.size > 0) {
          commitAgentProjection((current) =>
            current.map((agent) => changedAgents.get(agent.id) ?? agent),
          );
        }
      }
      return outcomes;
    },
    [apiToken, commitAgentProjection, onUnauthorized],
  );

  const deleteAgents = useCallback(
    async (
      targets: DeleteAgentBatchTarget[],
    ): Promise<DeleteAgentBatchOutcome[]> => {
      if (targets.length === 0) {
        return [];
      }
      const clientIds = targets.map((target) => target.client_id);
      requireUniqueFleetMutationTargets(clientIds, "deletion");
      if (targets.some((target) => !target.request.confirmed)) {
        throw new Error("A deletion batch requires one completed review.");
      }
      const reasons = new Set(
        targets.map((target) => target.request.reason?.trim() || null),
      );
      if (reasons.size !== 1) {
        throw new Error("A deletion batch must use one reviewed reason.");
      }
      const request: BulkDeleteAgentsRequest = {
        confirmed: true,
        items: targets.map((target) => ({
          client_id: target.client_id,
          privilege_assertion: target.request.privilege_assertion ?? null,
        })),
        reason: reasons.values().next().value ?? null,
      };
      let response: BulkDeleteAgentsResponse;
      try {
        response = await apiPost<BulkDeleteAgentsResponse>(
          "/api/v1/agents/deletions",
          apiToken,
          request,
        );
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
        }
        throw error;
      }
      assertOrderedFleetMutationEnvelope(response, clientIds, "deletion");
      const outcomes = response.outcomes.map((outcome) => ({
        client_id: outcome.client_id,
        response: outcome.result ?? null,
        error:
          outcome.status === "rejected"
            ? (outcome.error_message ??
              outcome.error_code ??
              "The VPS deletion was rejected.")
            : null,
      }));
      const deletedIds = new Set(
        outcomes.flatMap((outcome) =>
          outcome.response ? [outcome.response.client_id] : [],
        ),
      );
      if (apiTokenRef.current === apiToken && deletedIds.size > 0) {
        for (const clientId of deletedIds) {
          deletedClientIds.current.add(clientId);
        }
        commitAgentProjection((current) =>
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
      }
      return outcomes;
    },
    [apiToken, commitAgentProjection, onUnauthorized],
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
      ++fleetAlertPolicyListGeneration.current;
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
      return policy;
    },
    [apiToken],
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
      if (request.confirmed && apiTokenRef.current === apiToken) {
        assertCommittedVpsRuleProjection(preview);
        ++vpsRuleListGeneration.current;
        setVpsRuleValues((current) =>
          withoutDeletedClients(
            applyCommittedVpsRuleProjection(current, preview),
            deletedClientIds.current,
          ),
        );
      }
      return preview;
    },
    [apiToken],
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
      if (request.confirmed && apiTokenRef.current === apiToken) {
        assertCommittedVpsRuleProjection(preview);
        ++vpsRuleListGeneration.current;
        setVpsRuleValues((current) =>
          withoutDeletedClients(
            applyCommittedVpsRuleProjection(current, preview),
            deletedClientIds.current,
          ),
        );
      }
      return preview;
    },
    [apiToken],
  );

  const bulkMutateFleetAlertPolicies = useCallback(
    async (request: AlertConfigurationBulkRequest) => {
      const response = await apiPost<
        AlertConfigurationBulkResponse<FleetAlertPolicyRecord>
      >("/api/v1/fleet-alert-policies/bulk-mutate", apiToken, request);
      assertOrderedAlertConfigurationMutation(request, response, "policy");
      if (apiTokenRef.current === apiToken) {
        ++fleetAlertPolicyListGeneration.current;
        setFleetAlertPolicies((current) =>
          applyAlertConfigurationMutation(
            current,
            response,
            (left, right) =>
              Number(right.enabled) - Number(left.enabled) ||
              left.name.localeCompare(right.name),
          ),
        );
      }
      return response;
    },
    [apiToken],
  );

  const invalidateFleetAlertEventReads = useCallback(() => {
    ++fleetAlertEventProjectionGeneration.current;
    if (fleetAlertEventReviewInFlight.current) {
      // The successful mutation below owns the affected rows. Discard the
      // older read, then run one authoritative sync to recover unrelated
      // occurrence changes that may have shared that discarded response.
      fleetAlertEventSyncPending.current = true;
    }
  }, []);

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
        ++fleetAlertProjectionGeneration.current;
        invalidateFleetAlertEventReads();
        setFleetAlerts((current) =>
          applyFleetAlertStates(current, response.states),
        );
        setFleetAlertHistory((current) =>
          applyFleetAlertStates(current, response.states),
        );
        setFleetAlertEventReviewItems((current) => {
          const next = applyFleetAlertStates(current, response.states);
          fleetAlertEventReviewItemsRef.current = next;
          return next;
        });
        setFleetAlertEventSearchItems((current) => {
          const next = applyFleetAlertStates(current, response.states);
          fleetAlertEventSearchItemsRef.current = next;
          return next;
        });
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
    [apiToken, invalidateFleetAlertEventReads, loadFleet],
  );

  const resolveFleetAlert = useCallback(
    async (request: FleetAlertBulkResolveRequest) => {
      const response = await apiPost<FleetAlertBulkResolveResponse>(
        "/api/v1/fleet-alerts/resolve",
        apiToken,
        request,
      );
      if (apiTokenRef.current !== apiToken) {
        return response;
      }
      ++fleetAlertProjectionGeneration.current;
      ++policyAlertEpisodeGeneration.current;
      invalidateFleetAlertEventReads();
      const resolvedIds = new Set(response.alerts.map((alert) => alert.id));
      setFleetAlerts((current) =>
        current.filter((stored) => !resolvedIds.has(stored.id)),
      );
      setFleetAlertEventReviewItems((current) => {
        const next = current.filter((stored) => !resolvedIds.has(stored.id));
        fleetAlertEventReviewItemsRef.current = next;
        return next;
      });
      setFleetAlertEventSearchItems((current) => {
        const next = current.filter((stored) => !resolvedIds.has(stored.id));
        fleetAlertEventSearchItemsRef.current = next;
        return next;
      });
      setFleetAlertHistory((current) => [
        ...response.alerts,
        ...current.filter((stored) => !resolvedIds.has(stored.id)),
      ]);
      setCurrentPolicyAlerts((current) =>
        withoutResolvedPolicyAlertEpisodes(current, response.alerts),
      );
      setPolicyAlerts((current) =>
        applyResolvedPolicyAlertHistory(current, response.alerts),
      );
      return response;
    },
    [apiToken, invalidateFleetAlertEventReads],
  );

  const finishFleetAlertEventReviewRequest = useCallback(
    (request: Promise<void>, kind: "sync" | "older" | "search") => {
      const tracked = request.finally(() => {
        if (fleetAlertEventReviewInFlight.current !== tracked) {
          return;
        }
        fleetAlertEventReviewInFlight.current = null;
        fleetAlertEventReviewInFlightKind.current = null;
        if (apiTokenRef.current === apiToken) {
          setFleetAlertEventReviewLoading(false);
        }
        if (
          fleetAlertEventSyncPending.current &&
          fleetAlertEventReviewActive.current &&
          apiTokenRef.current === apiToken
        ) {
          fleetAlertEventSyncPending.current = false;
          queueMicrotask(() => void syncFleetAlertEventsRef.current());
        }
      });
      fleetAlertEventReviewInFlight.current = tracked;
      fleetAlertEventReviewInFlightKind.current = kind;
      return tracked;
    },
    [apiToken],
  );

  const syncFleetAlertEvents = useCallback(
    (queueTrailing = true) => {
      if (apiTokenRef.current !== apiToken) {
        return Promise.resolve();
      }
      const inFlight = fleetAlertEventReviewInFlight.current;
      if (inFlight) {
        if (queueTrailing) {
          fleetAlertEventSyncPending.current = true;
        }
        return inFlight;
      }
      const knownAlertIds = fleetAlertEventReviewItemsRef.current.map(
        (alert) => alert.id,
      );
      if (knownAlertIds.length > FLEET_ALERT_EVENT_SYNC_ID_LIMIT) {
        setFleetAlertEventReviewVerified(false);
        setFleetAlertEventReviewError(
          `Current occurrence synchronization supports ${FLEET_ALERT_EVENT_SYNC_ID_LIMIT.toLocaleString()} retained rows. Narrow the search before loading more.`,
        );
        return Promise.resolve();
      }
      setFleetAlertEventReviewLoading(true);
      setFleetAlertEventReviewError(null);
      setFleetAlertEventReviewLimitNotice(null);
      const projectionGeneration =
        fleetAlertEventProjectionGeneration.current;
      const request = (async () => {
        try {
          const response = await apiPost<FleetAlertEventSyncResponse>(
            "/api/v1/fleet-alert-events/sync",
            apiToken,
            { known_alert_ids: knownAlertIds },
          );
          assertFleetAlertEventSyncResponse(response, knownAlertIds);
          if (
            apiTokenRef.current !== apiToken ||
            projectionGeneration !==
              fleetAlertEventProjectionGeneration.current
          ) {
            return;
          }
          const reconciled = reconcileFleetAlertEventReviewItems(
            response.head.items,
            response.current_items,
          );
          const nextItems = reconciled.items;
          setFleetAlertEventReviewItems(nextItems);
          fleetAlertEventReviewItemsRef.current = nextItems;
          if (
            !fleetAlertEventReviewStartedRef.current ||
            reconciled.resetTail
          ) {
            setFleetAlertEventReviewCursor(response.head.next_cursor);
            setFleetAlertEventReviewHasMore(response.head.has_more);
            fleetAlertEventReviewCursorRef.current = response.head.next_cursor;
            fleetAlertEventReviewHasMoreRef.current = response.head.has_more;
          }
          if (reconciled.resetTail) {
            setFleetAlertEventReviewLimitNotice(
              `New current occurrences exceeded the ${FLEET_ALERT_EVENT_SYNC_ID_LIMIT.toLocaleString()}-row explicit-review reconciliation bound. Older retained rows were replaced by the verified head so Load older can continue from a contiguous cursor; valid search remains exhaustive.`,
            );
          }
          fleetAlertEventSearchBaseCursorRef.current =
            response.head.next_cursor;
          fleetAlertEventSearchBaseHasMoreRef.current = response.head.has_more;
          fleetAlertEventSearchCursorRef.current = response.head.next_cursor;
          fleetAlertEventSearchHasMoreRef.current = response.head.has_more;
          fleetAlertEventSearchQueryRef.current = "";
          setFleetAlertEventSearchHasMore(response.head.has_more);
          setFleetAlertEventSearchQuery("");
          setFleetAlertEventSearchScannedCount(0);
          setFleetAlertEventSearchItems([]);
          fleetAlertEventSearchItemsRef.current = [];
          setFleetAlertEventReviewStarted(true);
          fleetAlertEventReviewStartedRef.current = true;
          setFleetAlertEventReviewVerified(true);
        } catch (error) {
          if (
            apiTokenRef.current === apiToken &&
            projectionGeneration ===
              fleetAlertEventProjectionGeneration.current
          ) {
            setFleetAlertEventReviewVerified(false);
            setFleetAlertEventReviewError(
              error instanceof Error
                ? error.message
                : "Current occurrences could not be synchronized.",
            );
          }
        }
      })();
      return finishFleetAlertEventReviewRequest(request, "sync");
    },
    [apiToken, finishFleetAlertEventReviewRequest],
  );
  syncFleetAlertEventsRef.current = syncFleetAlertEvents;

  const loadOlderFleetAlertEvents = useCallback(() => {
    if (apiTokenRef.current !== apiToken) {
      return Promise.resolve();
    }
    const inFlight = fleetAlertEventReviewInFlight.current;
    if (inFlight) {
      return inFlight;
    }
    const cursor = fleetAlertEventReviewCursorRef.current;
    if (
      !fleetAlertEventReviewStartedRef.current ||
      !fleetAlertEventReviewHasMoreRef.current ||
      !cursor
    ) {
      return Promise.resolve();
    }
    if (
      fleetAlertEventReviewItemsRef.current.length >=
      FLEET_ALERT_EVENT_SYNC_ID_LIMIT
    ) {
      setFleetAlertEventReviewLimitNotice(
        `Explicit occurrence review reached its ${FLEET_ALERT_EVENT_SYNC_ID_LIMIT.toLocaleString()}-row reconciliation bound. Narrow the view or use alert export before retaining more rows; valid search can still scan to an authoritative end.`,
      );
      return Promise.resolve();
    }
    setFleetAlertEventReviewLoading(true);
    setFleetAlertEventReviewError(null);
    setFleetAlertEventReviewLimitNotice(null);
    const projectionGeneration = fleetAlertEventProjectionGeneration.current;
    const pageLimit = Math.min(
      FLEET_ALERT_EVENT_PAGE_LIMIT,
      FLEET_ALERT_EVENT_SYNC_ID_LIMIT -
        fleetAlertEventReviewItemsRef.current.length,
    );
    const request = (async () => {
      try {
        const query = new URLSearchParams({
          cursor,
          include_muted: "true",
          limit: String(pageLimit),
        });
        const page = await apiGet<FleetAlertEventPage>(
          `/api/v1/fleet-alert-events?${query.toString()}`,
          apiToken,
        );
        assertFleetAlertEventPage(page, cursor);
        if (
          apiTokenRef.current !== apiToken ||
          projectionGeneration !== fleetAlertEventProjectionGeneration.current
        ) {
          return;
        }
        const nextItems = dedupeFleetAlertsById([
          ...fleetAlertEventReviewItemsRef.current,
          ...page.items,
        ]);
        setFleetAlertEventReviewItems(nextItems);
        fleetAlertEventReviewItemsRef.current = nextItems;
        setFleetAlertEventReviewCursor(page.next_cursor);
        setFleetAlertEventReviewHasMore(page.has_more);
        fleetAlertEventReviewCursorRef.current = page.next_cursor;
        fleetAlertEventReviewHasMoreRef.current = page.has_more;
        if (
          nextItems.length >= FLEET_ALERT_EVENT_SYNC_ID_LIMIT &&
          page.has_more
        ) {
          setFleetAlertEventReviewLimitNotice(
            `Explicit occurrence review reached its ${FLEET_ALERT_EVENT_SYNC_ID_LIMIT.toLocaleString()}-row reconciliation bound. Narrow the view or use alert export before retaining more rows; valid search can still scan to an authoritative end.`,
          );
        }
        setFleetAlertEventReviewVerified(true);
      } catch (error) {
        if (
          apiTokenRef.current === apiToken &&
          projectionGeneration === fleetAlertEventProjectionGeneration.current
        ) {
          setFleetAlertEventReviewVerified(false);
          setFleetAlertEventReviewError(
            error instanceof Error
              ? error.message
              : "Older current occurrences could not be loaded.",
          );
        }
      }
    })();
    return finishFleetAlertEventReviewRequest(request, "older");
  }, [apiToken, finishFleetAlertEventReviewRequest]);

  const searchOlderFleetAlertEvents = useCallback(
    (
      query: string,
      matchingItems: (items: FleetAlertRecord[]) => FleetAlertRecord[],
      scan: boolean,
    ) => {
      if (apiTokenRef.current !== apiToken) {
        return Promise.resolve();
      }
      const normalizedQuery = query.trim();
      const startingNewSearch =
        fleetAlertEventSearchQueryRef.current !== normalizedQuery;
      if (startingNewSearch) {
        fleetAlertEventSearchQueryRef.current = normalizedQuery;
        fleetAlertEventSearchCursorRef.current =
          fleetAlertEventSearchBaseCursorRef.current;
        fleetAlertEventSearchHasMoreRef.current =
          fleetAlertEventSearchBaseHasMoreRef.current;
        setFleetAlertEventSearchQuery(normalizedQuery);
        setFleetAlertEventSearchHasMore(
          fleetAlertEventSearchBaseHasMoreRef.current,
        );
        setFleetAlertEventSearchScannedCount(0);
        setFleetAlertEventSearchItems([]);
        fleetAlertEventSearchItemsRef.current = [];
      }
      if (!normalizedQuery || !scan) {
        return Promise.resolve();
      }
      const inFlight = fleetAlertEventReviewInFlight.current;
      if (inFlight) {
        return inFlight;
      }
      const cursor = fleetAlertEventSearchCursorRef.current;
      if (!fleetAlertEventSearchHasMoreRef.current || !cursor) {
        return Promise.resolve();
      }
      setFleetAlertEventReviewLoading(true);
      setFleetAlertEventReviewError(null);
      const projectionGeneration =
        fleetAlertEventProjectionGeneration.current;
      const request = (async () => {
        try {
          const requestQuery = new URLSearchParams({
            cursor,
            include_muted: "true",
            limit: String(FLEET_ALERT_EVENT_PAGE_LIMIT),
          });
          const page = await apiGet<FleetAlertEventPage>(
            `/api/v1/fleet-alert-events?${requestQuery.toString()}`,
            apiToken,
          );
          assertFleetAlertEventPage(page, cursor);
          if (
            apiTokenRef.current !== apiToken ||
            projectionGeneration !==
              fleetAlertEventProjectionGeneration.current ||
            fleetAlertEventSearchQueryRef.current !== normalizedQuery
          ) {
            return;
          }
          const pageIds = new Set(page.items.map((item) => item.id));
          const matches = matchingItems(page.items);
          const matchIds = new Set<string>();
          let invalidMatchSubset = false;
          for (const item of matches) {
            if (!pageIds.has(item.id) || matchIds.has(item.id)) {
              invalidMatchSubset = true;
              break;
            }
            matchIds.add(item.id);
          }
          if (invalidMatchSubset) {
            throw new Error(
              "Current occurrence search returned an invalid retained subset.",
            );
          }
          const nextItems = dedupeFleetAlertsById([
            ...fleetAlertEventSearchItemsRef.current,
            ...matches,
          ]);
          setFleetAlertEventSearchItems(nextItems);
          fleetAlertEventSearchItemsRef.current = nextItems;
          fleetAlertEventSearchCursorRef.current = page.next_cursor;
          fleetAlertEventSearchHasMoreRef.current = page.has_more;
          setFleetAlertEventSearchHasMore(page.has_more);
          setFleetAlertEventSearchScannedCount(
            (current) => current + page.items.length,
          );
          setFleetAlertEventReviewVerified(true);
        } catch (error) {
          if (
            apiTokenRef.current === apiToken &&
            projectionGeneration ===
              fleetAlertEventProjectionGeneration.current
          ) {
            setFleetAlertEventReviewVerified(false);
            setFleetAlertEventReviewError(
              error instanceof Error
                ? error.message
                : "Older current occurrences could not be searched.",
            );
          }
        }
      })();
      return finishFleetAlertEventReviewRequest(request, "search");
    },
    [apiToken, finishFleetAlertEventReviewRequest],
  );

  const activateFleetAlertEventReview = useCallback(() => {
    fleetAlertEventReviewActive.current = true;
    // React StrictMode remounts panel effects in development. The remount
    // shares an already-running head sync, while entry during an older/search
    // request queues one authoritative head sync after that request completes.
    const inFlight = fleetAlertEventReviewInFlight.current;
    if (!inFlight) {
      // A mutation may have invalidated a request after this panel unmounted.
      // This activation sync itself satisfies that pending freshness signal.
      fleetAlertEventSyncPending.current = false;
    }
    return syncFleetAlertEventsRef.current(
      fleetAlertEventReviewInFlightKind.current !== "sync" ||
        fleetAlertEventSyncPending.current,
    );
  }, []);

  const deactivateFleetAlertEventReview = useCallback(() => {
    fleetAlertEventReviewActive.current = false;
  }, []);

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
      ++fleetAlertNotificationChannelListGeneration.current;
      setFleetAlertNotificationChannels((current) => {
        const withoutChannel = current.filter(
          (stored) => stored.id !== channel.id && stored.name !== channel.name,
        );
        return [channel, ...withoutChannel].sort((left, right) =>
          left.name.localeCompare(right.name),
        );
      });
      return channel;
    },
    [apiToken],
  );

  const bulkMutateFleetAlertNotificationChannels = useCallback(
    async (request: AlertConfigurationBulkRequest) => {
      const response = await apiPost<
        AlertConfigurationBulkResponse<FleetAlertNotificationChannelRecord>
      >(
        "/api/v1/fleet-alert-notification-channels/bulk-mutate",
        apiToken,
        request,
      );
      assertOrderedAlertConfigurationMutation(request, response, "channel");
      if (apiTokenRef.current === apiToken) {
        ++fleetAlertNotificationChannelListGeneration.current;
        setFleetAlertNotificationChannels((current) =>
          applyAlertConfigurationMutation(current, response, (left, right) =>
            left.name.localeCompare(right.name),
          ),
        );
      }
      return response;
    },
    [apiToken],
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
        ++fleetAlertNotificationDeliveryGeneration.current;
        const projection = mergeBoundedDeliveries(
          fleetAlertNotificationsRef.current,
          deliveries,
          fleetAlertNotificationsTruncatedRef.current,
        );
        fleetAlertNotificationsRef.current = projection.items;
        fleetAlertNotificationsTruncatedRef.current = projection.truncated;
        setFleetAlertNotifications(projection.items);
        setFleetAlertNotificationsTruncated(projection.truncated);
      }
      return deliveries;
    },
    [apiToken],
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
        ++fleetAlertNotificationDeliveryGeneration.current;
        const projection = mergeBoundedDeliveries(
          fleetAlertNotificationsRef.current,
          deliveries,
          fleetAlertNotificationsTruncatedRef.current,
        );
        fleetAlertNotificationsRef.current = projection.items;
        fleetAlertNotificationsTruncatedRef.current = projection.truncated;
        setFleetAlertNotifications(projection.items);
        setFleetAlertNotificationsTruncated(projection.truncated);
      }
      return deliveries;
    },
    [apiToken],
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
      ++webhookRuleListGeneration.current;
      setWebhookRules((current) => {
        const withoutRule = current.filter(
          (stored) => stored.id !== rule.id && stored.name !== rule.name,
        );
        return [rule, ...withoutRule].sort((left, right) =>
          left.name.localeCompare(right.name),
        );
      });
      return rule;
    },
    [apiToken],
  );

  const bulkMutateWebhookRules = useCallback(
    async (request: AlertConfigurationBulkRequest) => {
      const response = await apiPost<
        AlertConfigurationBulkResponse<WebhookRuleRecord>
      >("/api/v1/webhook-rules/bulk-mutate", apiToken, request);
      assertOrderedAlertConfigurationMutation(
        request,
        response,
        "webhook rule",
      );
      if (apiTokenRef.current === apiToken) {
        ++webhookRuleListGeneration.current;
        setWebhookRules((current) =>
          applyAlertConfigurationMutation(current, response, (left, right) =>
            left.name.localeCompare(right.name),
          ),
        );
      }
      return response;
    },
    [apiToken],
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
        ++webhookDeliveryListGeneration.current;
        const projection = mergeBoundedDeliveries(
          webhookRuleDeliveriesRef.current,
          deliveries,
          webhookRuleDeliveriesTruncatedRef.current,
        );
        webhookRuleDeliveriesRef.current = projection.items;
        webhookRuleDeliveriesTruncatedRef.current = projection.truncated;
        setWebhookRuleDeliveries(projection.items);
        setWebhookRuleDeliveriesTruncated(projection.truncated);
      }
      return deliveries;
    },
    [apiToken],
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
        ++webhookDeliveryListGeneration.current;
        const projection = mergeBoundedDeliveries(
          webhookRuleDeliveriesRef.current,
          deliveries,
          webhookRuleDeliveriesTruncatedRef.current,
        );
        webhookRuleDeliveriesRef.current = projection.items;
        webhookRuleDeliveriesTruncatedRef.current = projection.truncated;
        setWebhookRuleDeliveries(projection.items);
        setWebhookRuleDeliveriesTruncated(projection.truncated);
      }
      return deliveries;
    },
    [apiToken],
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
      if (request.confirmed && apiTokenRef.current === apiToken) {
        await loadWebhookDeliveryList();
      }
      return response;
    },
    [apiToken, loadWebhookDeliveryList],
  );

  const clearFleet = useCallback(() => {
    apiTokenRef.current = "";
    fleetFullGeneration.current += 1;
    fleetCoreGeneration.current += 1;
    fleetTelemetryGeneration.current += 1;
    fleetAlertProjectionGeneration.current += 1;
    policyAlertEpisodeGeneration.current += 1;
    fleetAlertPolicyListGeneration.current += 1;
    vpsRuleListGeneration.current += 1;
    fleetAlertNotificationChannelListGeneration.current += 1;
    fleetAlertNotificationDeliveryGeneration.current += 1;
    webhookRuleListGeneration.current += 1;
    webhookDeliveryListGeneration.current += 1;
    fleetTelemetryInFlight.current = null;
    fleetFullInFlight.current = null;
    fleetFullRefreshPending.current = false;
    fleetTelemetryRefreshPending.current = false;
    fleetAlertEventReviewInFlight.current = null;
    fleetAlertEventReviewInFlightKind.current = null;
    fleetAlertEventReviewActive.current = false;
    fleetAlertEventSyncPending.current = false;
    fleetAlertEventProjectionGeneration.current += 1;
    fleetAlertEventReviewItemsRef.current = [];
    fleetAlertEventReviewCursorRef.current = null;
    fleetAlertEventReviewHasMoreRef.current = false;
    fleetAlertEventReviewStartedRef.current = false;
    fleetAlertEventSearchBaseCursorRef.current = null;
    fleetAlertEventSearchBaseHasMoreRef.current = false;
    fleetAlertEventSearchCursorRef.current = null;
    fleetAlertEventSearchHasMoreRef.current = false;
    fleetAlertEventSearchQueryRef.current = "";
    fleetAlertEventSearchItemsRef.current = [];
    fleetSourceErrors.current = {};
    fleetDetailErrors.current = {};
    fleetDetailListErrors.current = {};
    deletedClientIds.current.clear();
    fleetAlertNotificationsRef.current = [];
    fleetAlertNotificationsTruncatedRef.current = false;
    webhookRuleDeliveriesRef.current = [];
    webhookRuleDeliveriesTruncatedRef.current = false;
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
    setFleetAlertEventReviewLimitNotice(null);
    setFleetAlertEventReviewVerified(false);
    setFleetAlertEventSearchHasMore(false);
    setFleetAlertEventSearchQuery("");
    setFleetAlertEventSearchScannedCount(0);
    setFleetAlertEventSearchItems([]);
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
    fleetAlertEventReviewLimitNotice,
    fleetAlertEventReviewVerified,
    fleetAlertEventSearchHasMore,
    fleetAlertEventSearchQuery,
    fleetAlertEventSearchScannedCount,
    fleetAlertEventSearchItems,
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
    activateFleetAlertEventReview,
    deactivateFleetAlertEventReview,
    loadOlderFleetAlertEvents,
    searchOlderFleetAlertEvents,
    syncFleetAlertEvents,
    loadFleetTelemetry,
    fleetCoreEvidenceAvailable,
    mutateAgentSuspensions,
    reconcileAgentTagMutation,
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
    bulkMutateFleetAlertPolicies,
    loadEffectiveVpsRules,
    dryRunVpsRules,
    bulkUpsertVpsRules,
    bulkUnsetVpsRules,
    upsertFleetAlertNotificationChannel,
    bulkMutateFleetAlertNotificationChannels,
    dispatchFleetAlertNotifications,
    processFleetAlertNotifications,
    upsertWebhookRule,
    bulkMutateWebhookRules,
    dryRunWebhookRule,
    dispatchWebhookRules,
    processWebhookRuleDeliveries,
    rotateWebhookDeliveryHistory,
    bulkUpdateFleetAlertStates,
  };
}

function requireUniqueFleetMutationTargets(
  clientIds: string[],
  operation: string,
): void {
  if (new Set(clientIds).size !== clientIds.length) {
    throw new Error(`A ${operation} batch cannot contain duplicate VPSs.`);
  }
}

function assertOrderedAlertConfigurationMutation<
  TRecord extends AlertConfigurationRecord,
>(
  request: AlertConfigurationBulkRequest,
  response: AlertConfigurationBulkResponse<TRecord>,
  resourceLabel: string,
): void {
  const expectedResult =
    request.action === "enable"
      ? "enabled"
      : request.action === "disable"
        ? "disabled"
        : "deleted";
  if (
    response.action !== request.action ||
    response.outcomes.length !== request.items.length ||
    response.outcomes.some((outcome, index) => {
      const item = request.items[index];
      const record = outcome.record;
      return (
        outcome.id !== item?.id ||
        outcome.name !== item.reviewed_name.trim() ||
        outcome.result !== expectedResult ||
        (request.action === "delete"
          ? record !== null
          : record === null ||
            record.id !== outcome.id ||
            record.name !== outcome.name ||
            record.enabled !== (request.action === "enable") ||
            record.updated_at === item.expected_updated_at)
      );
    })
  ) {
    throw new Error(
      `The ${resourceLabel} bulk API returned an invalid ordered result set.`,
    );
  }
}

function applyAlertConfigurationMutation<
  TRecord extends AlertConfigurationRecord,
>(
  current: TRecord[],
  response: AlertConfigurationBulkResponse<TRecord>,
  compare: (left: TRecord, right: TRecord) => number,
): TRecord[] {
  const affectedIds = new Set(response.outcomes.map((outcome) => outcome.id));
  return [
    ...current.filter((record) => !affectedIds.has(record.id)),
    ...response.outcomes.flatMap((outcome) =>
      outcome.record === null ? [] : [outcome.record],
    ),
  ].sort(compare);
}

function vpsRuleProjectionIdentity(record: {
  client_id: string;
  key: string;
}): string {
  return JSON.stringify([record.client_id, record.key]);
}

function assertCommittedVpsRuleProjection(
  response: VpsRulesDryRunResponse,
): void {
  const identities = new Map(
    response.changes.map((change) => [
      vpsRuleProjectionIdentity(change),
      change,
    ]),
  );
  const changedCount = response.changes.filter((change) =>
    matchesVpsRuleChangeAction(change.action, "set", "unset"),
  ).length;
  const invalidCount = response.changes.filter(
    (change) => change.action === "invalid",
  ).length;
  const expectedRecordIds = new Set(
    response.changes
      .filter(
        (change) =>
          change.action === "set" ||
          (change.action === "unchanged" && change.after !== null),
      )
      .map(vpsRuleProjectionIdentity),
  );
  const returnedRecordIds = new Set<string>();
  let previousRecord: VpsRuleValueRecord | null = null;
  const recordsValid = response.committed_records.every((record) => {
    const identity = vpsRuleProjectionIdentity(record);
    const change = identities.get(identity);
    const ordered =
      previousRecord === null || compareVpsRules(previousRecord, record) <= 0;
    previousRecord = record;
    if (
      change === undefined ||
      returnedRecordIds.has(identity) ||
      !expectedRecordIds.has(identity) ||
      change.after !== record.value_raw ||
      !ordered
    ) {
      return false;
    }
    returnedRecordIds.add(identity);
    return true;
  });
  if (
    identities.size !== response.changes.length ||
    changedCount !== response.changed_row_count ||
    invalidCount !== response.invalid_row_count ||
    invalidCount !== 0 ||
    response.changes.some(
      (change) =>
        !matchesVpsRuleChangeAction(change.action, "set", "unset", "unchanged"),
    ) ||
    !recordsValid ||
    returnedRecordIds.size !== expectedRecordIds.size
  ) {
    throw new Error("The VPS rule bulk API returned an invalid committed set.");
  }
}

function matchesVpsRuleChangeAction(
  action: string,
  ...expected: string[]
): boolean {
  return expected.includes(action);
}

function compareVpsRules(
  left: VpsRuleValueRecord,
  right: VpsRuleValueRecord,
): number {
  return (
    left.client_id.localeCompare(right.client_id) ||
    left.key.localeCompare(right.key)
  );
}

function applyCommittedVpsRuleProjection(
  current: VpsRuleValueRecord[],
  response: VpsRulesDryRunResponse,
): VpsRuleValueRecord[] {
  const affectedIds = new Set(response.changes.map(vpsRuleProjectionIdentity));
  return [
    ...current.filter(
      (record) => !affectedIds.has(vpsRuleProjectionIdentity(record)),
    ),
    ...response.committed_records,
  ].sort(compareVpsRules);
}

function assertOrderedFleetMutationEnvelope(
  response: {
    outcomes: Array<{
      client_id: string;
      status: string;
      result?: unknown;
      error_code?: string;
      error_message?: string;
    }>;
  },
  expectedClientIds: string[],
  operation: string,
): void {
  if (
    !Array.isArray(response.outcomes) ||
    response.outcomes.length !== expectedClientIds.length
  ) {
    throw new Error(`The ${operation} API returned an incomplete result set.`);
  }
  response.outcomes.forEach((outcome, index) => {
    const succeeded = outcome.status === "succeeded";
    const rejected = outcome.status === "rejected";
    if (
      outcome.client_id !== expectedClientIds[index] ||
      (!succeeded && !rejected) ||
      (succeeded && outcome.result == null) ||
      (rejected && outcome.result != null)
    ) {
      throw new Error(
        `The ${operation} API returned an invalid ordered outcome.`,
      );
    }
  });
}

function sourceAvailable(source: FleetSnapshotSource<unknown>): boolean {
  return source.data !== null && source.error === null;
}

function snapshotSourceFailure(
  source: FleetSnapshotSource<unknown>,
): string | null {
  if (sourceAvailable(source)) {
    return null;
  }
  return source.error ?? "Snapshot source unavailable";
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

function assertFleetAlertEventSyncResponse(
  value: unknown,
  knownAlertIds: string[],
): asserts value is FleetAlertEventSyncResponse {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(
      "Current occurrence synchronization returned invalid data.",
    );
  }
  const response = value as Partial<FleetAlertEventSyncResponse>;
  assertFleetAlertEventPage(response.head, null);
  if (!Array.isArray(response.current_items)) {
    throw new Error(
      "Current occurrence synchronization returned invalid data.",
    );
  }
  const known = new Set(knownAlertIds);
  const returned = new Set<string>();
  for (const item of response.current_items) {
    if (
      !item ||
      typeof item !== "object" ||
      typeof item.id !== "string" ||
      !known.has(item.id) ||
      returned.has(item.id) ||
      item.record_kind !== "event" ||
      !item.lifecycle ||
      item.lifecycle.resolved_at !== null
    ) {
      throw new Error(
        "Current occurrence synchronization returned an unknown, duplicate, non-event, or resolved retained record.",
      );
    }
    returned.add(item.id);
  }
}

function dedupeFleetAlertsById(alerts: FleetAlertRecord[]): FleetAlertRecord[] {
  const byId = new Map<string, FleetAlertRecord>();
  for (const alert of alerts) {
    byId.set(alert.id, alert);
  }
  return Array.from(byId.values());
}

export function policyAlertEpisodeId(alertId: string): string | null {
  const prefix = "policy-alert:";
  if (!alertId.startsWith(prefix) || alertId.length === prefix.length) {
    return null;
  }
  return alertId.slice(prefix.length);
}

function resolvedPolicyAlertsByEpisode(
  alerts: FleetAlertRecord[],
): Map<string, FleetAlertRecord> {
  return new Map(
    alerts.flatMap((alert) => {
      const episodeId = policyAlertEpisodeId(alert.id);
      return episodeId ? [[episodeId, alert] as const] : [];
    }),
  );
}

export function withoutResolvedPolicyAlertEpisodes(
  current: PolicyAlertRecord[],
  resolvedAlerts: FleetAlertRecord[],
): PolicyAlertRecord[] {
  const resolved = resolvedPolicyAlertsByEpisode(resolvedAlerts);
  return resolved.size === 0
    ? current
    : current.filter((stored) => !resolved.has(stored.id));
}

export function applyResolvedPolicyAlertHistory(
  current: PolicyAlertRecord[],
  resolvedAlerts: FleetAlertRecord[],
): PolicyAlertRecord[] {
  const resolved = resolvedPolicyAlertsByEpisode(resolvedAlerts);
  if (resolved.size === 0) {
    return current;
  }
  return current.map((stored) => {
    const alert = resolved.get(stored.id);
    return alert
      ? {
          ...stored,
          last_confirmed_at: alert.lifecycle.last_confirmed_at,
          lifecycle_state: alert.lifecycle.state,
          observed_at: alert.observed_at,
          resolved_at: alert.lifecycle.resolved_at,
          resolution_reason: alert.lifecycle.resolution_reason,
        }
      : stored;
  });
}

function mergeBoundedDeliveries<T extends { created_at: string; id: string }>(
  current: T[],
  incoming: T[],
  alreadyTruncated: boolean,
): { items: T[]; truncated: boolean } {
  const byId = new Map(current.map((delivery) => [delivery.id, delivery]));
  for (const delivery of incoming) {
    byId.set(delivery.id, delivery);
  }
  const merged = Array.from(byId.values()).sort((left, right) =>
    right.created_at.localeCompare(left.created_at),
  );
  return {
    items: merged.slice(0, FLEET_DELIVERY_LIST_LIMIT),
    truncated: alreadyTruncated || merged.length > FLEET_DELIVERY_LIST_LIMIT,
  };
}

export function reconcileFleetAlertEventReviewItems(
  head: FleetAlertRecord[],
  currentRetained: FleetAlertRecord[],
  limit = FLEET_ALERT_EVENT_SYNC_ID_LIMIT,
): { items: FleetAlertRecord[]; resetTail: boolean } {
  const merged = dedupeFleetAlertsById([...head, ...currentRetained]);
  if (merged.length <= limit) {
    return { items: merged, resetTail: false };
  }
  return {
    items: dedupeFleetAlertsById(head),
    resetTail: true,
  };
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
