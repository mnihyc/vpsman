import { useCallback, useRef, useState } from "react";
import { apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import { FLEET_DETAIL_LIMIT } from "../constants";
import type {
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
  const [preferencesError, setPreferencesError] = useState<string | null>(null);
  const [preferencesSaving, setPreferencesSaving] = useState(false);
  const accessLoadGeneration = useRef(0);
  const preferencesMutationGeneration = useRef(0);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  function resetAccessRecords() {
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
    setPreferencesError(null);
    setPreferencesSaving(false);
  }

  const setAuthenticatedOperator = useCallback(
    (nextOperator: OperatorView | null) => {
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
    [],
  );

  const loadCurrentOperatorProfile = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = accessLoadGeneration.current + 1;
    accessLoadGeneration.current = generation;
    setAccessLoading(true);
    setAccessError(null);
    try {
      const nextOperator = await apiGet<OperatorView>(
        "/api/v1/auth/me",
        apiToken,
      );
      if (
        accessLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setAuthenticatedOperator(nextOperator);
    } catch (error) {
      if (
        accessLoadGeneration.current !== generation ||
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
      setAccessError(
        error instanceof Error ? error.message : "Operator profile unavailable",
      );
    } finally {
      if (
        accessLoadGeneration.current === generation &&
        currentApiToken.current === apiToken
      ) {
        setAccessLoading(false);
      }
    }
  }, [apiToken, onUnauthorized, setAuthenticatedOperator]);

  const beginHomeOperatorHydration = useCallback(
    () => {
      setAccessLoading(true);
      return ++accessLoadGeneration.current;
    },
    [],
  );

  const hydrateHomeOperator = useCallback(
    (
      generation: number,
      nextOperator: OperatorView | null,
      error: string | null = null,
    ) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (accessLoadGeneration.current !== generation) {
        return;
      }
      if (nextOperator) {
        setAuthenticatedOperator(nextOperator);
      }
      setAccessError(error);
      setAccessLoading(false);
    },
    [apiToken, setAuthenticatedOperator],
  );

  const loadCurrentOperator = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = accessLoadGeneration.current + 1;
    accessLoadGeneration.current = generation;
    setAccessLoading(true);
    setAccessError(null);
    try {
      const nextOperator = await apiGet<OperatorView>(
        "/api/v1/auth/me",
        apiToken,
      );
      if (
        accessLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setAuthenticatedOperator(nextOperator);
      const [
        gatewaySessionsResult,
        operatorsResult,
        operatorSessionsResult,
        operatorAuthEventsResult,
        clientKeyRevocationsResult,
        keyLifecycleReportResult,
      ] = await Promise.allSettled([
        apiGet<GatewaySessionRecord[]>(
          `/api/v1/gateway-sessions?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        nextOperator.role === "admin"
          ? apiGet<OperatorView[]>("/api/v1/operators", apiToken)
          : Promise.resolve([]),
        nextOperator.role === "admin"
          ? apiGet<OperatorSessionRecord[]>(
              `/api/v1/operator-sessions?limit=${FLEET_DETAIL_LIMIT}`,
              apiToken,
            )
          : Promise.resolve([]),
        nextOperator.role === "admin"
          ? apiGet<OperatorAuthEventRecord[]>(
              `/api/v1/operator-auth-events?limit=${FLEET_DETAIL_LIMIT}`,
              apiToken,
            )
          : Promise.resolve([]),
        nextOperator.role === "admin"
          ? apiGet<ClientKeyRevocationView[]>(
              `/api/v1/client-key-revocations?limit=${FLEET_DETAIL_LIMIT}`,
              apiToken,
            )
          : Promise.resolve([]),
        nextOperator.role === "admin"
          ? apiGet<KeyLifecycleReportView>(
              "/api/v1/key-lifecycle/report",
              apiToken,
            )
          : Promise.resolve(null),
      ]);
      if (
        accessLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      const failures: string[] = [];
      const unauthorized = [
        gatewaySessionsResult,
        operatorsResult,
        operatorSessionsResult,
        operatorAuthEventsResult,
        clientKeyRevocationsResult,
        keyLifecycleReportResult,
      ].some(
        (result) =>
          result.status === "rejected" && isApiUnauthorized(result.reason),
      );
      if (unauthorized) {
        onUnauthorized();
        resetAccessRecords();
        setAccessError("Operator login required");
        return;
      }
      setGatewaySessions(
        settledValue(gatewaySessionsResult, [], "gateway sessions", failures),
      );
      setOperators(settledValue(operatorsResult, [], "operators", failures));
      const nextOperatorSessions = settledValue(
        operatorSessionsResult,
        [],
        "operator sessions",
        failures,
      );
      setOperatorSessions(nextOperatorSessions);
      setOperatorSessionsTruncated(
        nextOperatorSessions.length >= FLEET_DETAIL_LIMIT,
      );
      const nextOperatorAuthEvents = settledValue(
        operatorAuthEventsResult,
        [],
        "auth history",
        failures,
      );
      setOperatorAuthEvents(nextOperatorAuthEvents);
      setOperatorAuthEventsTruncated(
        nextOperatorAuthEvents.length >= FLEET_DETAIL_LIMIT,
      );
      setClientKeyRevocations(
        settledValue(
          clientKeyRevocationsResult,
          [],
          "client revocations",
          failures,
        ),
      );
      setKeyLifecycleReport(
        settledValue(keyLifecycleReportResult, null, "key lifecycle", failures),
      );
      if (failures.length > 0) {
        setAccessError(
          `Some access records unavailable: ${failures.join(", ")}`,
        );
      }
    } catch (error) {
      if (
        accessLoadGeneration.current !== generation ||
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
      setAccessError(
        error instanceof Error ? error.message : "Operator session unavailable",
      );
    } finally {
      if (
        accessLoadGeneration.current === generation &&
        currentApiToken.current === apiToken
      ) {
        setAccessLoading(false);
      }
    }
  }, [apiToken, onUnauthorized, setAuthenticatedOperator]);

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
        await apiPost<OperatorView>("/api/v1/operators", apiToken, {
          username,
          role,
          password,
          scopes,
          session_refresh_ttl_secs: sessionRefreshTtlSecs,
          confirmed: true,
          admin_risk_acknowledged: adminRiskAcknowledged,
          privilege_assertion: privilegeAssertion,
        });
        if (currentApiToken.current !== apiToken) {
          return;
        }
        await loadCurrentOperator();
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
    [apiToken, loadCurrentOperator, onUnauthorized],
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
        await loadCurrentOperator();
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
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const revokeOperatorSession = useCallback(
    async (
      sessionId: string,
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      setAccessError(null);
      try {
        await apiPost<OperatorSessionRecord>(
          `/api/v1/operator-sessions/${encodeURIComponent(sessionId)}/revoke`,
          apiToken,
          {
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
            privilege_assertion: privilegeAssertion,
          },
        );
        if (currentApiToken.current !== apiToken) {
          return;
        }
        await loadCurrentOperator();
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
          error instanceof Error ? error.message : "Session revoke failed",
        );
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
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
        await apiPut<OperatorView>(
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
        await loadCurrentOperator();
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
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const setOperatorStatus = useCallback(
    async (
      operatorId: string,
      status: "active" | "disabled" | "deleted",
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      setAccessError(null);
      const action =
        status === "active"
          ? "enable"
          : status === "disabled"
            ? "disable"
            : "delete";
      try {
        await apiPost<OperatorView>(
          `/api/v1/operators/${encodeURIComponent(operatorId)}/${action}`,
          apiToken,
          {
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
            privilege_assertion: privilegeAssertion,
          },
        );
        if (currentApiToken.current !== apiToken) {
          return;
        }
        await loadCurrentOperator();
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
          error instanceof Error
            ? error.message
            : "Operator status change failed",
        );
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
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
        await apiPost<OperatorView>(
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
        await loadCurrentOperator();
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
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const clearOperatorTotp = useCallback(
    async (
      operatorId: string,
      adminRiskAcknowledged: boolean,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      setAccessError(null);
      try {
        await apiPost<OperatorView>(
          `/api/v1/operators/${encodeURIComponent(operatorId)}/totp-clear`,
          apiToken,
          {
            confirmed: true,
            admin_risk_acknowledged: adminRiskAcknowledged,
            privilege_assertion: privilegeAssertion,
          },
        );
        if (currentApiToken.current !== apiToken) {
          return;
        }
        await loadCurrentOperator();
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
          error instanceof Error ? error.message : "TOTP clear failed",
        );
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
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
        setAuthenticatedOperator(updated);
        await loadCurrentOperator();
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
    [apiToken, loadCurrentOperator, onUnauthorized, setAuthenticatedOperator],
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
        setAuthenticatedOperator(updated);
        await loadCurrentOperator();
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
    [apiToken, loadCurrentOperator, onUnauthorized, setAuthenticatedOperator],
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
        await loadCurrentOperator();
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
    [apiToken, loadCurrentOperator, onUnauthorized],
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
          return;
        }
        setAuthenticatedOperator(nextOperator);
        await loadCurrentOperatorProfile();
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
    [
      apiToken,
      loadCurrentOperatorProfile,
      onUnauthorized,
      setAuthenticatedOperator,
    ],
  );

  const clearOperator = useCallback(() => {
    accessLoadGeneration.current += 1;
    preferencesMutationGeneration.current += 1;
    currentApiToken.current = "";
    resetAccessRecords();
  }, []);

  return {
    accessError,
    accessLoading,
    beginHomeOperatorHydration,
    clearAccess: clearOperator,
    clearOperator,
    clientKeyRevocations,
    clearOperatorTotp,
    createOperator,
    upsertAgentIdentity,
    confirmTotp,
    disableTotp,
    gatewaySessions,
    hydrateHomeOperator,
    keyLifecycleReport,
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
    resetOperatorPassword,
    setAuthenticatedOperator,
    setOperatorStatus,
    setupTotp,
    updateOperator,
    updateOperatorPreferences,
  };
}

function settledValue<T>(
  result: PromiseSettledResult<T>,
  fallback: T,
  label: string,
  failures: string[],
): T {
  if (result.status === "fulfilled") {
    return result.value;
  }
  failures.push(label);
  return fallback;
}
