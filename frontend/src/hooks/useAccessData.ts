import { useCallback, useRef, useState } from "react";
import {
  apiGet,
  apiPost,
  apiPut,
  isApiUnauthorized,
  LatestReadConsumer,
} from "../api";
import { FLEET_DETAIL_LIMIT } from "../constants";
import type {
  BulkOperatorMutationItem,
  BulkOperatorMutationResponse,
  BulkOperatorSessionRevokeItem,
  BulkOperatorSessionRevokeResponse,
  GatewaySessionRecord,
  OperatorAuthEventRecord,
  OperatorPreferences,
  OperatorSessionRecord,
  OperatorView,
  TotpSetupResponse,
} from "../types";
import type {
  AgentIdentityMutationResponse,
  ClientKeyRevocationMutationResponse,
  ClientKeyRevocationView,
  KeyLifecycleReportView,
  UpsertAgentIdentityRequest,
} from "../typesAccess";
import type { PrivilegeAssertion } from "../privilege";
import { sanitizeOperatorPreferences } from "../utils";

const ACCESS_BULK_LIMIT = 1_000;

export type AccessProjection =
  | "profile"
  | "operators"
  | "operatorSessions"
  | "operatorAuthEvents"
  | "clientKeyRevocations"
  | "keyLifecycleReport"
  | "gatewaySessions";

type AccessProjectionGenerations = Record<AccessProjection, number>;
type AccessProjectionFailures = Record<AccessProjection, string | null>;
type AccessAggregateProjection = Exclude<AccessProjection, "profile">;

const ACCESS_PROJECTIONS: readonly AccessProjection[] = [
  "profile",
  "operators",
  "operatorSessions",
  "operatorAuthEvents",
  "clientKeyRevocations",
  "keyLifecycleReport",
  "gatewaySessions",
];

const ACCESS_AGGREGATE_PROJECTIONS = ACCESS_PROJECTIONS.filter(
  (projection): projection is AccessAggregateProjection =>
    projection !== "profile",
);

type HomeOperatorHydrationFence = {
  operationGeneration: number;
  profileGeneration: number;
};

function emptyAccessProjectionGenerations(): AccessProjectionGenerations {
  return {
    profile: 0,
    operators: 0,
    operatorSessions: 0,
    operatorAuthEvents: 0,
    clientKeyRevocations: 0,
    keyLifecycleReport: 0,
    gatewaySessions: 0,
  };
}

function emptyAccessProjectionFailures(): AccessProjectionFailures {
  return {
    profile: null,
    operators: null,
    operatorSessions: null,
    operatorAuthEvents: null,
    clientKeyRevocations: null,
    keyLifecycleReport: null,
    gatewaySessions: null,
  };
}

export function useAccessData(apiToken: string, onUnauthorized: () => void) {
  const [operator, setOperator] = useState<OperatorView | null>(null);
  const [operators, setOperators] = useState<OperatorView[]>([]);
  const [operatorSessions, setOperatorSessions] = useState<
    OperatorSessionRecord[]
  >([]);
  const [operatorAuthEvents, setOperatorAuthEvents] = useState<
    OperatorAuthEventRecord[]
  >([]);
  const [operatorSessionsTruncated, setOperatorSessionsTruncated] =
    useState(false);
  const [operatorAuthEventsTruncated, setOperatorAuthEventsTruncated] =
    useState(false);
  const [clientKeyRevocations, setClientKeyRevocations] = useState<
    ClientKeyRevocationView[]
  >([]);
  const [keyLifecycleReport, setKeyLifecycleReport] =
    useState<KeyLifecycleReportView | null>(null);
  const [gatewaySessions, setGatewaySessions] = useState<
    GatewaySessionRecord[]
  >([]);
  const [accessError, setAccessError] = useState<string | null>(null);
  const [accessLoading, setAccessLoading] = useState(false);
  const [accessSourceLoadingVersion, setAccessSourceLoadingVersion] =
    useState(0);
  const [preferencesError, setPreferencesError] = useState<string | null>(null);
  const [preferencesSaving, setPreferencesSaving] = useState(false);
  // Loading is presentation-global, but reads and mutations are projection-local.
  // A newer session read must not cancel an operator-list read, and an older
  // aggregate response must not restore a row changed by an exact mutation.
  const accessLoadOperationGeneration = useRef(0);
  const accessProjectionGenerations = useRef<AccessProjectionGenerations>(
    emptyAccessProjectionGenerations(),
  );
  const accessProjectionFailures = useRef<AccessProjectionFailures>(
    emptyAccessProjectionFailures(),
  );
  const accessSourceLoadTokens = useRef(
    new Map<AccessProjection, Set<number>>(),
  );
  const nextAccessSourceLoadToken = useRef(0);
  const accessReadConsumers = useRef({
    profile: new LatestReadConsumer<OperatorView>(),
    operators: new LatestReadConsumer<OperatorView[]>(),
    operatorSessions: new LatestReadConsumer<OperatorSessionRecord[]>(),
    operatorAuthEvents: new LatestReadConsumer<OperatorAuthEventRecord[]>(),
    clientKeyRevocations:
      new LatestReadConsumer<ClientKeyRevocationView[]>(),
    keyLifecycleReport: new LatestReadConsumer<KeyLifecycleReportView>(),
    gatewaySessions: new LatestReadConsumer<GatewaySessionRecord[]>(),
  });
  const preferencesMutationGeneration = useRef(0);
  const currentApiToken = useRef(apiToken);
  const operatorRef = useRef<OperatorView | null>(operator);
  currentApiToken.current = apiToken;
  operatorRef.current = operator;

  const publishAccessSourceErrors = useCallback(() => {
    setAccessError(
      formatAccessProjectionFailures(accessProjectionFailures.current),
    );
  }, []);

  const trackAccessSourceLoad = useCallback(
    async <T,>(
      sources: readonly AccessProjection[],
      load: () => Promise<T>,
    ): Promise<T> => {
      const token = ++nextAccessSourceLoadToken.current;
      for (const source of sources) {
        const sourceTokens =
          accessSourceLoadTokens.current.get(source) ?? new Set<number>();
        sourceTokens.add(token);
        accessSourceLoadTokens.current.set(source, sourceTokens);
      }
      setAccessSourceLoadingVersion((version) => version + 1);
      try {
        return await load();
      } finally {
        for (const source of sources) {
          const currentTokens = accessSourceLoadTokens.current.get(source);
          currentTokens?.delete(token);
          if (currentTokens?.size === 0) {
            accessSourceLoadTokens.current.delete(source);
          }
        }
        setAccessSourceLoadingVersion((version) => version + 1);
      }
    },
    [],
  );

  const accessSourcesLoading = useCallback(
    (sources: readonly AccessProjection[]) => {
      void accessSourceLoadingVersion;
      return sources.some(
        (source) =>
          (accessSourceLoadTokens.current.get(source)?.size ?? 0) > 0,
      );
    },
    [accessSourceLoadingVersion],
  );

  const accessSourcesError = useCallback(
    (sources: readonly AccessProjection[]) => {
      const allSourceError = formatAccessProjectionFailures(
        accessProjectionFailures.current,
      );
      if (accessError && accessError !== allSourceError) {
        return accessError;
      }
      const sourceFailures = emptyAccessProjectionFailures();
      for (const source of sources) {
        sourceFailures[source] = accessProjectionFailures.current[source];
      }
      return formatAccessProjectionFailures(sourceFailures);
    },
    [accessError],
  );

  function resetAccessRecords() {
    accessLoadOperationGeneration.current += 1;
    for (const projection of ACCESS_PROJECTIONS) {
      accessProjectionGenerations.current[projection] += 1;
    }
    accessReadConsumers.current.profile.discardPending();
    accessReadConsumers.current.operators.discardPending([]);
    accessReadConsumers.current.operatorSessions.discardPending([]);
    accessReadConsumers.current.operatorAuthEvents.discardPending([]);
    accessReadConsumers.current.clientKeyRevocations.discardPending([]);
    accessReadConsumers.current.keyLifecycleReport.discardPending();
    accessReadConsumers.current.gatewaySessions.discardPending([]);
    accessProjectionFailures.current = emptyAccessProjectionFailures();
    accessSourceLoadTokens.current.clear();
    operatorRef.current = null;
    setOperator(null);
    setOperators([]);
    setOperatorSessions([]);
    setOperatorAuthEvents([]);
    setOperatorSessionsTruncated(false);
    setOperatorAuthEventsTruncated(false);
    setClientKeyRevocations([]);
    setKeyLifecycleReport(null);
    setGatewaySessions([]);
    setAccessError(null);
    setAccessLoading(false);
    setAccessSourceLoadingVersion((version) => version + 1);
    setPreferencesError(null);
    setPreferencesSaving(false);
  }

  const setAuthenticatedOperator = useCallback(
    (nextOperator: OperatorView | null) => {
      accessProjectionGenerations.current.profile += 1;
      accessProjectionFailures.current.profile = null;
      publishAccessSourceErrors();
      if (nextOperator) {
        accessProjectionGenerations.current.operators += 1;
      }
      operatorRef.current = nextOperator;
      setOperator(nextOperator);
      if (!nextOperator) {
        return;
      }
      setOperators((current) => {
        if (current.length === 0) {
          return current;
        }
        return current.map((existing) =>
          existing.id === nextOperator.id ? nextOperator : existing,
        );
      });
    },
    [publishAccessSourceErrors],
  );

  const storeOperators = useCallback((records: OperatorView[]) => {
    if (records.length === 0) {
      return;
    }
    accessProjectionGenerations.current.operators += 1;
    const byId = new Map(records.map((record) => [record.id, record]));
    const currentOperator = operatorRef.current;
    const nextOperator = currentOperator
      ? (byId.get(currentOperator.id) ?? currentOperator)
      : null;
    if (currentOperator && nextOperator !== currentOperator) {
      accessProjectionGenerations.current.profile += 1;
      operatorRef.current = nextOperator;
      setOperator(nextOperator);
    }
    setOperators((current) => {
      const currentIds = new Set(current.map((record) => record.id));
      const added = records.filter((record) => !currentIds.has(record.id));
      return [
        ...added,
        ...current.map((record) => byId.get(record.id) ?? record),
      ];
    });
    // Operator identity is a denormalized part of each session row. Fence the
    // session projection before applying that derived patch so an older session
    // read cannot restore the previous username or role.
    accessProjectionGenerations.current.operatorSessions += 1;
    setOperatorSessions((current) =>
      current.map((session) => {
        const updated = byId.get(session.operator_id);
        return updated
          ? {
              ...session,
              operator_role: updated.role,
              operator_username: updated.username,
            }
          : session;
      }),
    );
  }, []);

  const storeOperatorSessions = useCallback(
    (records: OperatorSessionRecord[]) => {
      if (records.length === 0) {
        return;
      }
      accessProjectionGenerations.current.operatorSessions += 1;
      const byId = new Map(records.map((record) => [record.id, record]));
      setOperatorSessions((current) => {
        const currentIds = new Set(current.map((record) => record.id));
        const added = records.filter((record) => !currentIds.has(record.id));
        return [
          ...added,
          ...current.map((record) => byId.get(record.id) ?? record),
        ];
      });
    },
    [],
  );

  const loadOperatorSessions = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const operationGeneration = ++accessLoadOperationGeneration.current;
    const generation =
      ++accessProjectionGenerations.current.operatorSessions;
    accessProjectionFailures.current.operatorSessions = null;
    setAccessLoading(true);
    publishAccessSourceErrors();
    try {
      const records = await accessReadConsumers.current.operatorSessions.enqueue(
        () =>
          apiGet<OperatorSessionRecord[]>(
            `/api/v1/operator-sessions?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
      );
      if (
        accessProjectionGenerations.current.operatorSessions !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setOperatorSessions(records);
      setOperatorSessionsTruncated(records.length >= FLEET_DETAIL_LIMIT);
      accessProjectionFailures.current.operatorSessions = null;
      publishAccessSourceErrors();
    } catch (error) {
      if (
        accessProjectionGenerations.current.operatorSessions !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        resetAccessRecords();
        setAccessError("Operator login required");
        return;
      }
      accessProjectionFailures.current.operatorSessions = accessSourceFailure(
        "Operator sessions",
        error,
      );
      publishAccessSourceErrors();
    } finally {
      if (
        accessLoadOperationGeneration.current === operationGeneration &&
        currentApiToken.current === apiToken
      ) {
        setAccessLoading(false);
      }
    }
  }, [apiToken, onUnauthorized, publishAccessSourceErrors]);

  const loadKeyLifecycleMutationSources = useCallback(
    async (includeRotatedKeySources: boolean) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      const operationGeneration = ++accessLoadOperationGeneration.current;
      const reportGeneration =
        ++accessProjectionGenerations.current.keyLifecycleReport;
      const gatewayGeneration = includeRotatedKeySources
        ? ++accessProjectionGenerations.current.gatewaySessions
        : null;
      const revocationGeneration = includeRotatedKeySources
        ? ++accessProjectionGenerations.current.clientKeyRevocations
        : null;
      accessProjectionFailures.current.keyLifecycleReport = null;
      if (includeRotatedKeySources) {
        accessProjectionFailures.current.gatewaySessions = null;
        accessProjectionFailures.current.clientKeyRevocations = null;
      }
      setAccessLoading(true);
      publishAccessSourceErrors();
      try {
        const [reportResult, gatewayResult, revocationsResult] =
          await Promise.allSettled([
            accessReadConsumers.current.keyLifecycleReport.enqueue(() =>
              apiGet<KeyLifecycleReportView>(
                "/api/v1/key-lifecycle/report",
                apiToken,
              ),
            ),
            includeRotatedKeySources
              ? accessReadConsumers.current.gatewaySessions.enqueue(() =>
                  apiGet<GatewaySessionRecord[]>(
                    `/api/v1/gateway-sessions?limit=${FLEET_DETAIL_LIMIT}`,
                    apiToken,
                  ),
                )
              : Promise.resolve<GatewaySessionRecord[] | null>(null),
            includeRotatedKeySources
              ? accessReadConsumers.current.clientKeyRevocations.enqueue(() =>
                  apiGet<ClientKeyRevocationView[]>(
                    `/api/v1/client-key-revocations?limit=${FLEET_DETAIL_LIMIT}`,
                    apiToken,
                  ),
                )
              : Promise.resolve<ClientKeyRevocationView[] | null>(null),
          ]);
        if (currentApiToken.current !== apiToken) {
          return;
        }
        const reportIsCurrent =
          accessProjectionGenerations.current.keyLifecycleReport ===
          reportGeneration;
        const gatewayIsCurrent =
          gatewayGeneration !== null &&
          accessProjectionGenerations.current.gatewaySessions ===
            gatewayGeneration;
        const revocationsAreCurrent =
          revocationGeneration !== null &&
          accessProjectionGenerations.current.clientKeyRevocations ===
            revocationGeneration;
        if (
          (reportIsCurrent &&
            reportResult.status === "rejected" &&
            isApiUnauthorized(reportResult.reason)) ||
          (gatewayIsCurrent &&
            gatewayResult.status === "rejected" &&
            isApiUnauthorized(gatewayResult.reason)) ||
          (revocationsAreCurrent &&
            revocationsResult.status === "rejected" &&
            isApiUnauthorized(revocationsResult.reason))
        ) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        if (reportIsCurrent) {
          if (reportResult.status === "fulfilled") {
            setKeyLifecycleReport(reportResult.value);
            accessProjectionFailures.current.keyLifecycleReport = null;
          } else {
            accessProjectionFailures.current.keyLifecycleReport =
              accessSourceFailure("Key lifecycle", reportResult.reason);
          }
        }
        if (gatewayIsCurrent) {
          if (gatewayResult.status === "fulfilled" && gatewayResult.value) {
            setGatewaySessions(gatewayResult.value);
            accessProjectionFailures.current.gatewaySessions = null;
          } else if (gatewayResult.status === "rejected") {
            accessProjectionFailures.current.gatewaySessions =
              accessSourceFailure("Gateway sessions", gatewayResult.reason);
          }
        }
        if (revocationsAreCurrent) {
          if (
            revocationsResult.status === "fulfilled" &&
            revocationsResult.value
          ) {
            setClientKeyRevocations(revocationsResult.value);
            accessProjectionFailures.current.clientKeyRevocations = null;
          } else if (revocationsResult.status === "rejected") {
            accessProjectionFailures.current.clientKeyRevocations =
              accessSourceFailure(
                "Client key revocations",
                revocationsResult.reason,
              );
          }
        }
        publishAccessSourceErrors();
      } finally {
        if (
          accessLoadOperationGeneration.current === operationGeneration &&
          currentApiToken.current === apiToken
        ) {
          setAccessLoading(false);
        }
      }
    },
    [apiToken, onUnauthorized, publishAccessSourceErrors],
  );

  const loadCurrentOperatorProfileUntracked = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const operationGeneration = ++accessLoadOperationGeneration.current;
    const generation = ++accessProjectionGenerations.current.profile;
    accessProjectionFailures.current.profile = null;
    setAccessLoading(true);
    publishAccessSourceErrors();
    try {
      const nextOperator = await accessReadConsumers.current.profile.enqueue(
        () => apiGet<OperatorView>("/api/v1/auth/me", apiToken),
      );
      if (
        accessProjectionGenerations.current.profile !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      accessProjectionGenerations.current.operators += 1;
      operatorRef.current = nextOperator;
      setOperator(nextOperator);
      setOperators((current) =>
        current.length === 0
          ? current
          : current.map((existing) =>
              existing.id === nextOperator.id ? nextOperator : existing,
            ),
      );
      accessProjectionFailures.current.profile = null;
      publishAccessSourceErrors();
    } catch (error) {
      if (
        accessProjectionGenerations.current.profile !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        resetAccessRecords();
        setAccessError("Operator login required");
        return;
      }
      accessProjectionFailures.current.profile = accessSourceFailure(
        "Operator profile",
        error,
      );
      publishAccessSourceErrors();
    } finally {
      if (
        accessLoadOperationGeneration.current === operationGeneration &&
        currentApiToken.current === apiToken
      ) {
        setAccessLoading(false);
      }
    }
  }, [apiToken, onUnauthorized, publishAccessSourceErrors]);

  const loadCurrentOperatorProfile = useCallback(
    () =>
      trackAccessSourceLoad(
        ["profile"],
        loadCurrentOperatorProfileUntracked,
      ),
    [loadCurrentOperatorProfileUntracked, trackAccessSourceLoad],
  );

  const beginHomeOperatorHydration = useCallback((): HomeOperatorHydrationFence => {
    setAccessLoading(true);
    accessProjectionFailures.current.profile = null;
    publishAccessSourceErrors();
    return {
      operationGeneration: ++accessLoadOperationGeneration.current,
      profileGeneration: ++accessProjectionGenerations.current.profile,
    };
  }, [publishAccessSourceErrors]);

  const hydrateHomeOperator = useCallback(
    (
      fence: HomeOperatorHydrationFence,
      nextOperator: OperatorView | null,
      error: string | null = null,
    ) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (
        accessProjectionGenerations.current.profile !==
        fence.profileGeneration
      ) {
        return;
      }
      if (nextOperator) {
        accessProjectionGenerations.current.operators += 1;
        operatorRef.current = nextOperator;
        setOperator(nextOperator);
        setOperators((current) =>
          current.length === 0
            ? current
            : current.map((existing) =>
                existing.id === nextOperator.id ? nextOperator : existing,
              ),
        );
      }
      accessProjectionFailures.current.profile = error;
      publishAccessSourceErrors();
      if (
        accessLoadOperationGeneration.current === fence.operationGeneration
      ) {
        setAccessLoading(false);
      }
    },
    [apiToken, publishAccessSourceErrors],
  );

  const loadCurrentOperatorSources = useCallback(
    (requestedSources: readonly AccessAggregateProjection[]) =>
      trackAccessSourceLoad(["profile", ...requestedSources], async () => {
        if (currentApiToken.current !== apiToken) {
          return;
        }
        const requested = new Set(requestedSources);
        const operationGeneration = ++accessLoadOperationGeneration.current;
        const profileGeneration =
          ++accessProjectionGenerations.current.profile;
        accessProjectionFailures.current.profile = null;
        setAccessLoading(true);
        publishAccessSourceErrors();
        try {
      const nextOperator = await accessReadConsumers.current.profile.enqueue(
        () => apiGet<OperatorView>("/api/v1/auth/me", apiToken),
      );
      if (currentApiToken.current !== apiToken) {
        return;
      }
      // A newer exact profile owner may have changed the role while /auth/me was
      // in flight. Stop this aggregate instead of using stale authorization to
      // decide which admin-only sources to request.
      if (
        accessProjectionGenerations.current.profile !== profileGeneration
      ) {
        return;
      }
      accessProjectionGenerations.current.operators += 1;
      operatorRef.current = nextOperator;
      setOperator(nextOperator);
      setOperators((current) =>
        current.length === 0
          ? current
          : current.map((existing) =>
              existing.id === nextOperator.id ? nextOperator : existing,
            ),
      );
      accessProjectionFailures.current.profile = null;
      // Start each aggregate source under its own current revision. A mutation
      // that lands while these requests are active advances only its affected
      // projection; unaffected aggregate results still commit.
      const generations = accessProjectionGenerations.current;
      const operatorGeneration = requested.has("operators")
        ? ++generations.operators
        : null;
      const sessionGeneration = requested.has("operatorSessions")
        ? ++generations.operatorSessions
        : null;
      const authEventGeneration = requested.has("operatorAuthEvents")
        ? ++generations.operatorAuthEvents
        : null;
      const revocationGeneration = requested.has("clientKeyRevocations")
        ? ++generations.clientKeyRevocations
        : null;
      const keyLifecycleGeneration = requested.has("keyLifecycleReport")
        ? ++generations.keyLifecycleReport
        : null;
      const gatewayGeneration = requested.has("gatewaySessions")
        ? ++generations.gatewaySessions
        : null;
      for (const projection of requestedSources) {
        accessProjectionFailures.current[projection] = null;
      }
      publishAccessSourceErrors();
      const [
        gatewaySessionsResult,
        operatorsResult,
        operatorSessionsResult,
        operatorAuthEventsResult,
        clientKeyRevocationsResult,
        keyLifecycleReportResult,
      ] = await Promise.allSettled([
        requested.has("gatewaySessions")
          ? accessReadConsumers.current.gatewaySessions.enqueue(() =>
              apiGet<GatewaySessionRecord[]>(
                `/api/v1/gateway-sessions?limit=${FLEET_DETAIL_LIMIT}`,
                apiToken,
              ),
            )
          : Promise.resolve([]),
        requested.has("operators") && nextOperator.role === "admin"
          ? accessReadConsumers.current.operators.enqueue(() =>
              apiGet<OperatorView[]>("/api/v1/operators", apiToken),
            )
          : Promise.resolve([]),
        requested.has("operatorSessions") && nextOperator.role === "admin"
          ? accessReadConsumers.current.operatorSessions.enqueue(() =>
              apiGet<OperatorSessionRecord[]>(
                `/api/v1/operator-sessions?limit=${FLEET_DETAIL_LIMIT}`,
                apiToken,
              ),
            )
          : Promise.resolve([]),
        requested.has("operatorAuthEvents") && nextOperator.role === "admin"
          ? accessReadConsumers.current.operatorAuthEvents.enqueue(() =>
              apiGet<OperatorAuthEventRecord[]>(
                `/api/v1/operator-auth-events?limit=${FLEET_DETAIL_LIMIT}`,
                apiToken,
              ),
            )
          : Promise.resolve([]),
        requested.has("clientKeyRevocations") &&
        nextOperator.role === "admin"
          ? accessReadConsumers.current.clientKeyRevocations.enqueue(() =>
              apiGet<ClientKeyRevocationView[]>(
                `/api/v1/client-key-revocations?limit=${FLEET_DETAIL_LIMIT}`,
                apiToken,
              ),
            )
          : Promise.resolve([]),
        requested.has("keyLifecycleReport") && nextOperator.role === "admin"
          ? accessReadConsumers.current.keyLifecycleReport.enqueue(() =>
              apiGet<KeyLifecycleReportView>(
                "/api/v1/key-lifecycle/report",
                apiToken,
              ),
            )
          : Promise.resolve(null),
      ]);
      if (currentApiToken.current !== apiToken) {
        return;
      }
      const sourceIsCurrent = {
        operators:
          operatorGeneration !== null &&
          accessProjectionGenerations.current.operators === operatorGeneration,
        operatorSessions:
          sessionGeneration !== null &&
          accessProjectionGenerations.current.operatorSessions ===
          sessionGeneration,
        operatorAuthEvents:
          authEventGeneration !== null &&
          accessProjectionGenerations.current.operatorAuthEvents ===
          authEventGeneration,
        clientKeyRevocations:
          revocationGeneration !== null &&
          accessProjectionGenerations.current.clientKeyRevocations ===
          revocationGeneration,
        keyLifecycleReport:
          keyLifecycleGeneration !== null &&
          accessProjectionGenerations.current.keyLifecycleReport ===
          keyLifecycleGeneration,
        gatewaySessions:
          gatewayGeneration !== null &&
          accessProjectionGenerations.current.gatewaySessions ===
          gatewayGeneration,
      };
      const unauthorized = (
        [
          ["gatewaySessions", gatewaySessionsResult],
          ["operators", operatorsResult],
          ["operatorSessions", operatorSessionsResult],
          ["operatorAuthEvents", operatorAuthEventsResult],
          ["clientKeyRevocations", clientKeyRevocationsResult],
          ["keyLifecycleReport", keyLifecycleReportResult],
        ] as const
      ).some(
        ([projection, result]) =>
          sourceIsCurrent[projection] &&
          result.status === "rejected" &&
          isApiUnauthorized(result.reason),
      );
      if (unauthorized) {
        onUnauthorized();
        resetAccessRecords();
        setAccessError("Operator login required");
        return;
      }
      if (sourceIsCurrent.gatewaySessions) {
        commitAccessArraySource(
          "gatewaySessions",
          "Gateway sessions",
          gatewaySessionsResult,
          setGatewaySessions,
          accessProjectionFailures.current,
        );
      }
      if (sourceIsCurrent.operatorSessions) {
        if (operatorSessionsResult.status === "fulfilled") {
          setOperatorSessions(operatorSessionsResult.value);
          setOperatorSessionsTruncated(
            operatorSessionsResult.value.length >= FLEET_DETAIL_LIMIT,
          );
          accessProjectionFailures.current.operatorSessions = null;
        } else {
          accessProjectionFailures.current.operatorSessions =
            accessSourceFailure(
              "Operator sessions",
              operatorSessionsResult.reason,
            );
        }
      }
      if (sourceIsCurrent.operators) {
        if (operatorsResult.status === "fulfilled") {
          setOperators(operatorsResult.value);
          const byId = new Map(
            operatorsResult.value.map((record) => [record.id, record]),
          );
          const currentOperator = operatorRef.current;
          const updatedCurrent = currentOperator
            ? byId.get(currentOperator.id)
            : undefined;
          if (updatedCurrent) {
            operatorRef.current = updatedCurrent;
            setOperator(updatedCurrent);
          }
          if (sourceIsCurrent.operatorSessions) {
            // Commit the same-wave session rows first, then apply the operator
            // projection's denormalized identity fields. A newer independent
            // session owner is left untouched.
            accessProjectionGenerations.current.operatorSessions += 1;
            setOperatorSessions((current) =>
              current.map((session) => {
                const updated = byId.get(session.operator_id);
                return updated
                  ? {
                      ...session,
                      operator_role: updated.role,
                      operator_username: updated.username,
                    }
                  : session;
              }),
            );
          }
          accessProjectionFailures.current.operators = null;
        } else {
          accessProjectionFailures.current.operators = accessSourceFailure(
            "Operators",
            operatorsResult.reason,
          );
        }
      }
      if (sourceIsCurrent.operatorAuthEvents) {
        if (operatorAuthEventsResult.status === "fulfilled") {
          setOperatorAuthEvents(operatorAuthEventsResult.value);
          setOperatorAuthEventsTruncated(
            operatorAuthEventsResult.value.length >= FLEET_DETAIL_LIMIT,
          );
          accessProjectionFailures.current.operatorAuthEvents = null;
        } else {
          accessProjectionFailures.current.operatorAuthEvents =
            accessSourceFailure(
              "Operator auth history",
              operatorAuthEventsResult.reason,
            );
        }
      }
      if (sourceIsCurrent.clientKeyRevocations) {
        commitAccessArraySource(
          "clientKeyRevocations",
          "Client key revocations",
          clientKeyRevocationsResult,
          setClientKeyRevocations,
          accessProjectionFailures.current,
        );
      }
      if (sourceIsCurrent.keyLifecycleReport) {
        if (keyLifecycleReportResult.status === "fulfilled") {
          setKeyLifecycleReport(keyLifecycleReportResult.value);
          accessProjectionFailures.current.keyLifecycleReport = null;
        } else {
          accessProjectionFailures.current.keyLifecycleReport =
            accessSourceFailure(
              "Key lifecycle",
              keyLifecycleReportResult.reason,
            );
        }
      }
      publishAccessSourceErrors();
    } catch (error) {
      if (
        accessProjectionGenerations.current.profile !== profileGeneration ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        resetAccessRecords();
        setAccessError("Operator login required");
        return;
      }
      accessProjectionFailures.current.profile = accessSourceFailure(
        "Operator profile",
        error,
      );
      publishAccessSourceErrors();
    } finally {
      if (
        accessLoadOperationGeneration.current === operationGeneration &&
        currentApiToken.current === apiToken
      ) {
        setAccessLoading(false);
      }
        }
      }),
    [
      apiToken,
      onUnauthorized,
      publishAccessSourceErrors,
      trackAccessSourceLoad,
    ],
  );

  const loadCurrentOperator = useCallback(
    () => loadCurrentOperatorSources(ACCESS_AGGREGATE_PROJECTIONS),
    [loadCurrentOperatorSources],
  );

  const loadAccessOverview = useCallback(
    () =>
      loadCurrentOperatorSources([
        "operators",
        "operatorSessions",
        "clientKeyRevocations",
        "keyLifecycleReport",
        "gatewaySessions",
      ]),
    [loadCurrentOperatorSources],
  );

  const loadAccessOperators = useCallback(
    () =>
      loadCurrentOperatorSources([
        "operators",
        "operatorSessions",
        "operatorAuthEvents",
      ]),
    [loadCurrentOperatorSources],
  );

  const loadAccessVpsIdentities = useCallback(
    () =>
      loadCurrentOperatorSources([
        "clientKeyRevocations",
        "keyLifecycleReport",
      ]),
    [loadCurrentOperatorSources],
  );

  const loadAccessGatewaySessions = useCallback(
    () =>
      loadCurrentOperatorSources(["gatewaySessions", "keyLifecycleReport"]),
    [loadCurrentOperatorSources],
  );

  const loadAccessAuditSessions = useCallback(
    () =>
      loadCurrentOperatorSources([
        "operatorSessions",
        "operatorAuthEvents",
      ]),
    [loadCurrentOperatorSources],
  );

  const createOperator = useCallback(
    async (
      username: string,
      role: string,
      password: string,
      scopes: string[],
      sessionRefreshTtlSecs: number,
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      setAccessError(null);
      try {
        const created = await apiPost<OperatorView>(
          "/api/v1/operators",
          apiToken,
          {
            username,
            role,
            password,
            scopes,
            session_refresh_ttl_secs: sessionRefreshTtlSecs,
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
            privilege_assertion: privilegeAssertion,
          },
        );
        if (currentApiToken.current !== apiToken) {
          return;
        }
        storeOperators([created]);
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          return;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(
          error instanceof Error ? error.message : "Operator creation failed",
        );
        throw error;
      }
    },
    [apiToken, onUnauthorized, storeOperators],
  );

  const upsertAgentIdentity = useCallback(
    async (
      request: UpsertAgentIdentityRequest,
    ): Promise<AgentIdentityMutationResponse> => {
      setAccessError(null);
      try {
        const response = await apiPost<AgentIdentityMutationResponse>(
          "/api/v1/agent-identities",
          apiToken,
          request,
        );
        if (currentApiToken.current !== apiToken) {
          return response;
        }
        await loadKeyLifecycleMutationSources(request.replace_existing_key);
        return response;
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          throw error;
        }
        setAccessError(
          error instanceof Error
            ? error.message
            : "Agent identity import failed",
        );
        throw error;
      }
    },
    [apiToken, loadKeyLifecycleMutationSources, onUnauthorized],
  );

  const revokeOperatorSessions = useCallback(
    async (
      items: BulkOperatorSessionRevokeItem[],
      adminRiskAcknowledged: boolean,
    ): Promise<BulkOperatorSessionRevokeResponse> => {
      validateAccessBulkIds(
        items.map((item) => item.session_id),
        "operator session",
      );
      setAccessError(null);
      try {
        const response = await apiPost<BulkOperatorSessionRevokeResponse>(
          "/api/v1/operator-sessions/revocations",
          apiToken,
          {
            items,
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
          },
        );
        validateAccessBulkOutcomes(
          items.map((item) => item.session_id),
          response.outcomes.map((outcome) => outcome.session_id),
          "operator session",
        );
        if (currentApiToken.current === apiToken) {
          storeOperatorSessions(
            response.outcomes.flatMap((outcome) =>
              outcome.status === "succeeded" && outcome.result
                ? [outcome.result]
                : [],
            ),
          );
        }
        return response;
      } catch (error) {
        if (currentApiToken.current === apiToken) {
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            resetAccessRecords();
            setAccessError("Operator login required");
          } else {
            setAccessError(
              error instanceof Error ? error.message : "Session revoke failed",
            );
          }
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized, storeOperatorSessions],
  );

  const revokeOperatorSession = useCallback(
    async (
      sessionId: string,
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      const response = await revokeOperatorSessions(
        [
          {
            session_id: sessionId,
            privilege_assertion: privilegeAssertion,
          },
        ],
        adminRiskAcknowledged,
      );
      requireSingleAccessOutcome(response.outcomes[0], "Session revoke failed");
    },
    [revokeOperatorSessions],
  );

  const updateOperator = useCallback(
    async (
      operatorId: string,
      role: string,
      scopes: string[],
      sessionRefreshTtlSecs: number,
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      setAccessError(null);
      try {
        const updated = await apiPut<OperatorView>(
          `/api/v1/operators/${encodeURIComponent(operatorId)}`,
          apiToken,
          {
            role,
            scopes,
            session_refresh_ttl_secs: sessionRefreshTtlSecs,
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
            privilege_assertion: privilegeAssertion,
          },
        );
        if (currentApiToken.current !== apiToken) {
          return;
        }
        storeOperators([updated]);
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          return;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(
          error instanceof Error ? error.message : "Operator update failed",
        );
        throw error;
      }
    },
    [apiToken, onUnauthorized, storeOperators],
  );

  const setOperatorStatuses = useCallback(
    async (
      items: BulkOperatorMutationItem[],
      status: "active" | "disabled" | "deleted",
      adminRiskAcknowledged: boolean,
    ): Promise<BulkOperatorMutationResponse> => {
      validateAccessBulkIds(
        items.map((item) => item.operator_id),
        "operator",
      );
      setAccessError(null);
      try {
        const response = await apiPost<BulkOperatorMutationResponse>(
          "/api/v1/operators/statuses",
          apiToken,
          {
            status,
            items,
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
          },
        );
        validateAccessBulkOutcomes(
          items.map((item) => item.operator_id),
          response.outcomes.map((outcome) => outcome.operator_id),
          "operator",
        );
        const updated = response.outcomes.flatMap((outcome) =>
          outcome.status === "succeeded" && outcome.result
            ? [outcome.result]
            : [],
        );
        if (currentApiToken.current === apiToken) {
          storeOperators(updated);
          if (status !== "active" && updated.length > 0) {
            await loadOperatorSessions();
          }
        }
        return response;
      } catch (error) {
        if (currentApiToken.current === apiToken) {
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            resetAccessRecords();
            setAccessError("Operator login required");
          } else {
            setAccessError(
              error instanceof Error
                ? error.message
                : "Operator status change failed",
            );
          }
        }
        throw error;
      }
    },
    [apiToken, loadOperatorSessions, onUnauthorized, storeOperators],
  );

  const setOperatorStatus = useCallback(
    async (
      operatorId: string,
      status: "active" | "disabled" | "deleted",
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      const response = await setOperatorStatuses(
        [
          {
            operator_id: operatorId,
            privilege_assertion: privilegeAssertion,
          },
        ],
        status,
        adminRiskAcknowledged,
      );
      requireSingleAccessOutcome(
        response.outcomes[0],
        "Operator status change failed",
      );
    },
    [setOperatorStatuses],
  );

  const resetOperatorPassword = useCallback(
    async (
      operatorId: string,
      password: string,
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      setAccessError(null);
      try {
        const updated = await apiPost<OperatorView>(
          `/api/v1/operators/${encodeURIComponent(operatorId)}/password-reset`,
          apiToken,
          {
            password,
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
            privilege_assertion: privilegeAssertion,
          },
        );
        if (currentApiToken.current !== apiToken) {
          return;
        }
        storeOperators([updated]);
        await loadOperatorSessions();
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          return;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(
          error instanceof Error ? error.message : "Password reset failed",
        );
        throw error;
      }
    },
    [apiToken, loadOperatorSessions, onUnauthorized, storeOperators],
  );

  const clearOperatorTotps = useCallback(
    async (
      items: BulkOperatorMutationItem[],
      adminRiskAcknowledged: boolean,
    ): Promise<BulkOperatorMutationResponse> => {
      validateAccessBulkIds(
        items.map((item) => item.operator_id),
        "operator",
      );
      setAccessError(null);
      try {
        const response = await apiPost<BulkOperatorMutationResponse>(
          "/api/v1/operators/totp-clears",
          apiToken,
          {
            items,
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
          },
        );
        validateAccessBulkOutcomes(
          items.map((item) => item.operator_id),
          response.outcomes.map((outcome) => outcome.operator_id),
          "operator",
        );
        const updated = response.outcomes.flatMap((outcome) =>
          outcome.status === "succeeded" && outcome.result
            ? [outcome.result]
            : [],
        );
        if (currentApiToken.current === apiToken) {
          storeOperators(updated);
          if (updated.length > 0) {
            await loadOperatorSessions();
          }
        }
        return response;
      } catch (error) {
        if (currentApiToken.current === apiToken) {
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            resetAccessRecords();
            setAccessError("Operator login required");
          } else {
            setAccessError(
              error instanceof Error ? error.message : "TOTP clear failed",
            );
          }
        }
        throw error;
      }
    },
    [apiToken, loadOperatorSessions, onUnauthorized, storeOperators],
  );

  const clearOperatorTotp = useCallback(
    async (
      operatorId: string,
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      const response = await clearOperatorTotps(
        [
          {
            operator_id: operatorId,
            privilege_assertion: privilegeAssertion,
          },
        ],
        adminRiskAcknowledged,
      );
      requireSingleAccessOutcome(response.outcomes[0], "TOTP clear failed");
    },
    [clearOperatorTotps],
  );

  const setupTotp = useCallback(
    async (password: string) => {
      setAccessError(null);
      try {
        const response = await apiPost<TotpSetupResponse>(
          "/api/v1/auth/totp/setup",
          apiToken,
          { password },
        );
        if (currentApiToken.current !== apiToken) {
          throw new Error(
            "The operator session changed before TOTP setup could be confirmed. Retry setup from the current session.",
          );
        }
        return response;
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          throw error;
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const confirmTotp = useCallback(
    async (password: string, code: string) => {
      setAccessError(null);
      try {
        const updated = await apiPost<OperatorView>(
          "/api/v1/auth/totp/confirm",
          apiToken,
          { password, code },
        );
        if (currentApiToken.current !== apiToken) {
          throw new Error(
            "The operator session changed before TOTP confirmation could be reflected. Refresh current access state before retrying.",
          );
        }
        storeOperators([updated]);
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          throw error;
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized, storeOperators],
  );

  const disableTotp = useCallback(
    async (password: string, code: string) => {
      setAccessError(null);
      try {
        const updated = await apiPost<OperatorView>(
          "/api/v1/auth/totp/disable",
          apiToken,
          { password, code },
        );
        if (currentApiToken.current !== apiToken) {
          throw new Error(
            "The operator session changed before the TOTP disable result could be reflected. Refresh current access state before retrying.",
          );
        }
        storeOperators([updated]);
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          throw error;
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized, storeOperators],
  );

  const revokeClientKey = useCallback(
    async (
      clientId: string,
      reason: string | null,
      confirmed: boolean,
      privilegeAssertion: PrivilegeAssertion | null,
    ) => {
      setAccessError(null);
      try {
        const response = await apiPost<ClientKeyRevocationMutationResponse>(
          `/api/v1/clients/${encodeURIComponent(clientId)}/key-revocations`,
          apiToken,
          { confirmed, reason, privilege_assertion: privilegeAssertion },
        );
        if (currentApiToken.current !== apiToken) {
          return response;
        }
        accessProjectionGenerations.current.clientKeyRevocations += 1;
        accessProjectionFailures.current.clientKeyRevocations = null;
        setClientKeyRevocations((current) =>
          [
            response.revocation,
            ...current.filter(
              (record) => record.id !== response.revocation.id,
            ),
          ].slice(0, FLEET_DETAIL_LIMIT),
        );
        publishAccessSourceErrors();
        await loadKeyLifecycleMutationSources(true);
        return response;
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          throw error;
        }
        setAccessError(
          error instanceof Error ? error.message : "Client key revoke failed",
        );
        throw error;
      }
    },
    [
      apiToken,
      loadKeyLifecycleMutationSources,
      onUnauthorized,
      publishAccessSourceErrors,
    ],
  );

  const updateOperatorPreferences = useCallback(
    async (preferences: OperatorPreferences) => {
      const operationGeneration = preferencesMutationGeneration.current + 1;
      preferencesMutationGeneration.current = operationGeneration;
      setPreferencesSaving(true);
      setPreferencesError(null);
      setAccessError(null);
      try {
        const nextOperator = await apiPut<OperatorView>(
          "/api/v1/auth/preferences",
          apiToken,
          preferences,
        );
        if (
          currentApiToken.current !== apiToken ||
          preferencesMutationGeneration.current !== operationGeneration
        ) {
          return null;
        }
        storeOperators([nextOperator]);
        return sanitizeOperatorPreferences(nextOperator.preferences);
      } catch (error) {
        if (
          currentApiToken.current !== apiToken ||
          preferencesMutationGeneration.current !== operationGeneration
        ) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setPreferencesError("Operator login required");
          throw error;
        }
        const message =
          error instanceof Error ? error.message : "Preference update failed";
        setPreferencesError(message);
        throw error;
      } finally {
        if (
          currentApiToken.current === apiToken &&
          preferencesMutationGeneration.current === operationGeneration
        ) {
          setPreferencesSaving(false);
        }
      }
    },
    [apiToken, onUnauthorized, storeOperators],
  );

  const clearOperator = useCallback(() => {
    preferencesMutationGeneration.current += 1;
    currentApiToken.current = "";
    resetAccessRecords();
  }, []);

  return {
    accessError,
    accessLoading,
    accessSourcesError,
    accessSourcesLoading,
    beginHomeOperatorHydration,
    clearAccess: clearOperator,
    clearOperator,
    clientKeyRevocations,
    clearOperatorTotp,
    clearOperatorTotps,
    createOperator,
    upsertAgentIdentity,
    confirmTotp,
    disableTotp,
    gatewaySessions,
    hydrateHomeOperator,
    keyLifecycleReport,
    loadAccessAuditSessions,
    loadAccessGatewaySessions,
    loadAccessOperators,
    loadAccessOverview,
    loadAccessVpsIdentities,
    loadCurrentOperatorProfile,
    loadCurrentOperator,
    operator,
    operatorAuthEvents,
    operatorAuthEventsTruncated,
    operators,
    operatorSessions,
    operatorSessionsTruncated,
    preferencesError,
    preferencesSaving,
    revokeClientKey,
    revokeOperatorSession,
    revokeOperatorSessions,
    resetOperatorPassword,
    setAuthenticatedOperator,
    setOperatorStatus,
    setOperatorStatuses,
    setupTotp,
    updateOperator,
    updateOperatorPreferences,
  };
}

function validateAccessBulkIds(ids: string[], label: string): void {
  if (ids.length < 1 || ids.length > ACCESS_BULK_LIMIT) {
    throw new Error(
      `${label} bulk action requires 1 to ${ACCESS_BULK_LIMIT} targets`,
    );
  }
  if (ids.some((id) => !id.trim()) || new Set(ids).size !== ids.length) {
    throw new Error(`${label} bulk action targets must be non-empty and unique`);
  }
}

function validateAccessBulkOutcomes(
  requestedIds: string[],
  outcomeIds: string[],
  label: string,
): void {
  if (
    requestedIds.length !== outcomeIds.length ||
    requestedIds.some((id, index) => id !== outcomeIds[index])
  ) {
    throw new Error(
      `${label} bulk action returned an invalid ordered result set`,
    );
  }
}

function requireSingleAccessOutcome(
  outcome:
    | BulkOperatorMutationResponse["outcomes"][number]
    | BulkOperatorSessionRevokeResponse["outcomes"][number]
    | undefined,
  fallback: string,
): void {
  if (outcome?.status === "succeeded" && outcome.result) {
    return;
  }
  throw new Error(outcome?.error_message || outcome?.error_code || fallback);
}

function commitAccessArraySource<T>(
  projection: AccessProjection,
  label: string,
  result: PromiseSettledResult<T>,
  commit: (records: T) => void,
  failures: AccessProjectionFailures,
): void {
  if (result.status === "fulfilled") {
    commit(result.value);
    failures[projection] = null;
    return;
  }
  failures[projection] = accessSourceFailure(label, result.reason);
}

function accessSourceFailure(label: string, error: unknown): string {
  return `${label}: ${
    error instanceof Error ? error.message : "source unavailable"
  }`;
}

function formatAccessProjectionFailures(
  failures: AccessProjectionFailures,
): string | null {
  const messages = ACCESS_PROJECTIONS.flatMap((projection) => {
    const message = failures[projection];
    return message ? [message] : [];
  });
  return messages.length > 0 ? messages.join("; ") : null;
}
