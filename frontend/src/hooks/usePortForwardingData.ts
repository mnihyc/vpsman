import { useCallback, useState } from "react";
import { apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import type {
  CreatePortForwardRuleRequest,
  PortForwardBulkAction,
  PortForwardBulkResponse,
  PortForwardMutationRequest,
  PortForwardMutationResponse,
  PortForwardRuleRecord,
  ResolveHostnameResponse,
  UpdatePortForwardRuleRequest,
} from "../types";

export function usePortForwardingData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
) {
  const [portForwardRules, setPortForwardRules] = useState<PortForwardRuleRecord[]>([]);
  const [portForwardError, setPortForwardError] = useState<string | null>(null);
  const [portForwardLoading, setPortForwardLoading] = useState(false);

  const loadPortForwardRules = useCallback(async () => {
    setPortForwardLoading(true);
    setPortForwardError(null);
    try {
      setPortForwardRules(
        await apiGet<PortForwardRuleRecord[]>("/api/v1/port-forward-rules", apiToken),
      );
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setPortForwardRules([]);
        setPortForwardError("Operator login required");
        return;
      }
      setPortForwardError(
        error instanceof Error ? error.message : "Port-forward rules unavailable",
      );
    } finally {
      setPortForwardLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const refreshAfterMutation = useCallback(async () => {
    await Promise.allSettled([loadPortForwardRules(), onAuditChanged()]);
  }, [loadPortForwardRules, onAuditChanged]);

  const createPortForwardRule = useCallback(
    async (request: CreatePortForwardRuleRequest) => {
      const response = await apiPost<PortForwardMutationResponse>(
        "/api/v1/port-forward-rules",
        apiToken,
        request,
      );
      await refreshAfterMutation();
      return response;
    },
    [apiToken, refreshAfterMutation],
  );

  const updatePortForwardRule = useCallback(
    async (ruleId: string, request: UpdatePortForwardRuleRequest) => {
      const response = await apiPut<PortForwardMutationResponse>(
        `/api/v1/port-forward-rules/${encodeURIComponent(ruleId)}`,
        apiToken,
        request,
      );
      await refreshAfterMutation();
      return response;
    },
    [apiToken, refreshAfterMutation],
  );

  const mutatePortForwardRule = useCallback(
    async (
      ruleId: string,
      operation: "enable" | "disable" | "delete" | "forget" | "reapply",
      request: PortForwardMutationRequest,
    ) => {
      const response = await apiPost<PortForwardMutationResponse>(
        `/api/v1/port-forward-rules/${encodeURIComponent(ruleId)}/${operation}`,
        apiToken,
        request,
      );
      await refreshAfterMutation();
      return response;
    },
    [apiToken, refreshAfterMutation],
  );

  const bulkMutatePortForwardRules = useCallback(
    async (
      action: PortForwardBulkAction,
      items: Array<{ id: string; expected_revision: number }>,
      reason?: string,
    ) => {
      const response = await apiPost<PortForwardBulkResponse>(
        "/api/v1/port-forward-rules/bulk",
        apiToken,
        { action, confirmed: true, items, reason: reason || null },
      );
      await refreshAfterMutation();
      return response;
    },
    [apiToken, refreshAfterMutation],
  );

  const resolvePortForwardHostname = useCallback(
    (hostname: string) =>
      apiPost<ResolveHostnameResponse>(
        "/api/v1/network/resolve-hostname",
        apiToken,
        { hostname },
      ),
    [apiToken],
  );

  return {
    bulkMutatePortForwardRules,
    createPortForwardRule,
    loadPortForwardRules,
    mutatePortForwardRule,
    portForwardError,
    portForwardLoading,
    portForwardRules,
    resolvePortForwardHostname,
    updatePortForwardRule,
  };
}
