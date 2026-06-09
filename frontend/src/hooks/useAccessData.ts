import { useCallback, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import type {
  GatewaySessionRecord,
  OperatorPreferences,
  OperatorSessionRecord,
  OperatorView,
  TotpSetupResponse,
} from "../types";
import type {
  AgentIdentityView,
  ClientKeyRevocationView,
  KeyLifecycleReportView,
  UpsertAgentIdentityRequest,
} from "../typesAccess";

export function useAccessData(apiToken: string, onUnauthorized: () => void) {
  const [operator, setOperator] = useState<OperatorView | null>(null);
  const [operators, setOperators] = useState<OperatorView[]>([]);
  const [operatorSessions, setOperatorSessions] = useState<
    OperatorSessionRecord[]
  >([]);
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

  function resetAccessRecords() {
    setOperator(null);
    setOperators([]);
    setOperatorSessions([]);
    setClientKeyRevocations([]);
    setKeyLifecycleReport(null);
    setGatewaySessions([]);
    setPreferencesError(null);
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
    setAccessLoading(true);
    setAccessError(null);
    try {
      const nextOperator = await apiGet<OperatorView>(
        "/api/v1/auth/me",
        apiToken,
      );
      setAuthenticatedOperator(nextOperator);
    } catch (error) {
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
      setAccessLoading(false);
    }
  }, [apiToken, onUnauthorized, setAuthenticatedOperator]);

  const loadCurrentOperator = useCallback(async () => {
    setAccessLoading(true);
    setAccessError(null);
    try {
      const nextOperator = await apiGet<OperatorView>(
        "/api/v1/auth/me",
        apiToken,
      );
      setAuthenticatedOperator(nextOperator);
      const [
        gatewaySessionsResult,
        operatorsResult,
        operatorSessionsResult,
        clientKeyRevocationsResult,
        keyLifecycleReportResult,
      ] = await Promise.allSettled([
        apiGet<GatewaySessionRecord[]>(
          "/api/v1/gateway-sessions?limit=200",
          apiToken,
        ),
        nextOperator.role === "admin"
          ? apiGet<OperatorView[]>("/api/v1/operators", apiToken)
          : Promise.resolve([]),
        nextOperator.role === "admin"
          ? apiGet<OperatorSessionRecord[]>(
              "/api/v1/operator-sessions?limit=200",
              apiToken,
            )
          : Promise.resolve([]),
        nextOperator.role === "admin"
          ? apiGet<ClientKeyRevocationView[]>(
              "/api/v1/client-key-revocations?limit=200",
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
      const failures: string[] = [];
      const unauthorized = [
        gatewaySessionsResult,
        operatorsResult,
        operatorSessionsResult,
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
      setOperatorSessions(
        settledValue(operatorSessionsResult, [], "operator sessions", failures),
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
      setAccessLoading(false);
    }
  }, [apiToken, onUnauthorized, setAuthenticatedOperator]);

  const createOperator = useCallback(
    async (
      username: string,
      role: string,
      password: string,
      scopes: string[],
    ) => {
      setAccessError(null);
      try {
        await apiPost<OperatorView>("/api/v1/operators", apiToken, {
          username,
          role,
          password,
          scopes,
        });
        await loadCurrentOperator();
      } catch (error) {
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
    async (request: UpsertAgentIdentityRequest): Promise<AgentIdentityView> => {
      setAccessError(null);
      try {
        const response = await apiPost<AgentIdentityView>(
          "/api/v1/agent-identities",
          apiToken,
          request,
        );
        await loadCurrentOperator();
        return response;
      } catch (error) {
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
    async (sessionId: string) => {
      setAccessError(null);
      try {
        await apiDelete<OperatorSessionRecord>(
          `/api/v1/operator-sessions/${encodeURIComponent(sessionId)}`,
          apiToken,
        );
        await loadCurrentOperator();
      } catch (error) {
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

  const setupTotp = useCallback(
    async (password: string) => {
      setAccessError(null);
      try {
        return await apiPost<TotpSetupResponse>(
          "/api/v1/auth/totp/setup",
          apiToken,
          { password },
        );
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return null;
        }
        setAccessError(
          error instanceof Error ? error.message : "TOTP setup failed",
        );
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const confirmTotp = useCallback(
    async (password: string, code: string) => {
      setAccessError(null);
      try {
        await apiPost<OperatorView>("/api/v1/auth/totp/confirm", apiToken, {
          password,
          code,
        });
        await loadCurrentOperator();
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(
          error instanceof Error ? error.message : "TOTP confirmation failed",
        );
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const disableTotp = useCallback(
    async (password: string, code: string) => {
      setAccessError(null);
      try {
        await apiPost<OperatorView>("/api/v1/auth/totp/disable", apiToken, {
          password,
          code,
        });
        await loadCurrentOperator();
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(
          error instanceof Error ? error.message : "TOTP disable failed",
        );
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const revokeClientKey = useCallback(
    async (clientId: string, reason: string | null, confirmed: boolean) => {
      setAccessError(null);
      try {
        await apiPost<ClientKeyRevocationView>(
          `/api/v1/clients/${encodeURIComponent(clientId)}/key-revocations`,
          apiToken,
          { confirmed, reason },
        );
        await loadCurrentOperator();
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
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
      setPreferencesSaving(true);
      setPreferencesError(null);
      setAccessError(null);
      try {
        const nextOperator = await apiPut<OperatorView>(
          "/api/v1/auth/preferences",
          apiToken,
          preferences,
        );
        setAuthenticatedOperator(nextOperator);
      } catch (error) {
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
        setPreferencesSaving(false);
      }
    },
    [apiToken, onUnauthorized, setAuthenticatedOperator],
  );

  const clearOperator = useCallback(() => {
    resetAccessRecords();
  }, []);

  return {
    accessError,
    accessLoading,
    clearOperator,
    clientKeyRevocations,
    createOperator,
    upsertAgentIdentity,
    confirmTotp,
    disableTotp,
    gatewaySessions,
    keyLifecycleReport,
    loadCurrentOperatorProfile,
    loadCurrentOperator,
    operator,
    operators,
    operatorSessions,
    preferencesError,
    preferencesSaving,
    revokeClientKey,
    revokeOperatorSession,
    setAuthenticatedOperator,
    setupTotp,
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
