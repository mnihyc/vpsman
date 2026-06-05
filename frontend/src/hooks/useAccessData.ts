import { useCallback, useState } from "react";
import { apiDelete, apiGet, apiPost, isApiUnauthorized } from "../api";
import type {
  AuthProofRotationHistoryRecord,
  GatewaySessionRecord,
  OperatorSessionRecord,
  OperatorView,
  TotpSetupResponse,
} from "../types";
import type {
  ClientKeyRevocationView,
  CreateEnrollmentTokenRequest,
  CreateEnrollmentTokenResponse,
  EnrollmentTokenView,
  KeyLifecycleReportView,
} from "../typesAccess";

export function useAccessData(apiToken: string, onUnauthorized: () => void) {
  const [operator, setOperator] = useState<OperatorView | null>(null);
  const [operators, setOperators] = useState<OperatorView[]>([]);
  const [operatorSessions, setOperatorSessions] = useState<OperatorSessionRecord[]>([]);
  const [enrollmentTokens, setEnrollmentTokens] = useState<EnrollmentTokenView[]>([]);
  const [clientKeyRevocations, setClientKeyRevocations] = useState<ClientKeyRevocationView[]>([]);
  const [keyLifecycleReport, setKeyLifecycleReport] = useState<KeyLifecycleReportView | null>(null);
  const [gatewaySessions, setGatewaySessions] = useState<GatewaySessionRecord[]>([]);
  const [proofRotations, setProofRotations] = useState<AuthProofRotationHistoryRecord[]>([]);
  const [accessError, setAccessError] = useState<string | null>(null);
  const [accessLoading, setAccessLoading] = useState(false);

  function resetAccessRecords() {
    setOperator(null);
    setOperators([]);
    setOperatorSessions([]);
    setEnrollmentTokens([]);
    setClientKeyRevocations([]);
    setKeyLifecycleReport(null);
    setGatewaySessions([]);
    setProofRotations([]);
  }

  const loadCurrentOperator = useCallback(async () => {
    setAccessLoading(true);
    setAccessError(null);
    try {
      const nextOperator = await apiGet<OperatorView>("/api/v1/auth/me", apiToken);
      const [
        nextGatewaySessions,
        nextOperators,
        nextOperatorSessions,
        nextEnrollmentTokens,
        nextClientKeyRevocations,
        nextKeyLifecycleReport,
        nextProofRotations,
      ] = await Promise.all([
        apiGet<GatewaySessionRecord[]>("/api/v1/gateway-sessions?limit=200", apiToken),
        nextOperator.role === "admin" ? apiGet<OperatorView[]>("/api/v1/operators", apiToken) : Promise.resolve([]),
        nextOperator.role === "admin"
          ? apiGet<OperatorSessionRecord[]>("/api/v1/operator-sessions?limit=200", apiToken)
          : Promise.resolve([]),
        nextOperator.role === "admin" ? apiGet<EnrollmentTokenView[]>("/api/v1/enrollment-tokens", apiToken) : Promise.resolve([]),
        nextOperator.role === "admin"
          ? apiGet<ClientKeyRevocationView[]>("/api/v1/client-key-revocations?limit=200", apiToken)
          : Promise.resolve([]),
        nextOperator.role === "admin" ? apiGet<KeyLifecycleReportView>("/api/v1/key-lifecycle/report", apiToken) : Promise.resolve(null),
        nextOperator.role === "admin"
          ? apiGet<AuthProofRotationHistoryRecord[]>("/api/v1/auth/proof-rotations?limit=200", apiToken)
          : Promise.resolve([]),
      ]);
      setOperator(nextOperator);
      setOperators(nextOperators);
      setOperatorSessions(nextOperatorSessions);
      setEnrollmentTokens(nextEnrollmentTokens);
      setClientKeyRevocations(nextClientKeyRevocations);
      setKeyLifecycleReport(nextKeyLifecycleReport);
      setGatewaySessions(nextGatewaySessions);
      setProofRotations(nextProofRotations);
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        resetAccessRecords();
        setAccessError("Operator login required");
        return;
      }
      setAccessError(error instanceof Error ? error.message : "Operator session unavailable");
    } finally {
      setAccessLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const createOperator = useCallback(
    async (username: string, role: string, password: string, scopes: string[]) => {
      setAccessError(null);
      try {
        await apiPost<OperatorView>("/api/v1/operators", apiToken, { username, role, password, scopes });
        await loadCurrentOperator();
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(error instanceof Error ? error.message : "Operator creation failed");
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const createEnrollmentToken = useCallback(
    async (request: CreateEnrollmentTokenRequest): Promise<CreateEnrollmentTokenResponse> => {
      setAccessError(null);
      try {
        const response = await apiPost<CreateEnrollmentTokenResponse>("/api/v1/enrollment-tokens", apiToken, request);
        await loadCurrentOperator();
        return response;
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          throw error;
        }
        setAccessError(error instanceof Error ? error.message : "Enrollment token creation failed");
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const revokeOperatorSession = useCallback(
    async (sessionId: string) => {
      setAccessError(null);
      try {
        await apiDelete<OperatorSessionRecord>(`/api/v1/operator-sessions/${encodeURIComponent(sessionId)}`, apiToken);
        await loadCurrentOperator();
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(error instanceof Error ? error.message : "Session revoke failed");
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const setupTotp = useCallback(
    async (password: string) => {
      setAccessError(null);
      try {
        return await apiPost<TotpSetupResponse>("/api/v1/auth/totp/setup", apiToken, { password });
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return null;
        }
        setAccessError(error instanceof Error ? error.message : "TOTP setup failed");
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const confirmTotp = useCallback(
    async (password: string, code: string) => {
      setAccessError(null);
      try {
        await apiPost<OperatorView>("/api/v1/auth/totp/confirm", apiToken, { password, code });
        await loadCurrentOperator();
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(error instanceof Error ? error.message : "TOTP confirmation failed");
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
  );

  const disableTotp = useCallback(
    async (password: string, code: string) => {
      setAccessError(null);
      try {
        await apiPost<OperatorView>("/api/v1/auth/totp/disable", apiToken, { password, code });
        await loadCurrentOperator();
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          resetAccessRecords();
          setAccessError("Operator login required");
          return;
        }
        setAccessError(error instanceof Error ? error.message : "TOTP disable failed");
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
        setAccessError(error instanceof Error ? error.message : "Client key revoke failed");
        throw error;
      }
    },
    [apiToken, loadCurrentOperator, onUnauthorized],
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
    createEnrollmentToken,
    confirmTotp,
    disableTotp,
    enrollmentTokens,
    gatewaySessions,
    keyLifecycleReport,
    loadCurrentOperator,
    operator,
    operators,
    operatorSessions,
    proofRotations,
    revokeClientKey,
    revokeOperatorSession,
    setupTotp,
  };
}
