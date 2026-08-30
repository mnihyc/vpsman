import { useCallback, useRef, useState } from "react";
import {
  apiGet,
  apiPost,
  apiPut,
  isApiUnauthorized,
  LatestReadConsumer,
} from "../api";
import type {
  CreatePortForwardRuleRequest,
  PortForwardBulkAction,
  PortForwardBulkResponse,
  PortForwardMutationRequest,
  PortForwardMutationResponse,
  PortForwardRuleListItem,
  ResolveHostnameResponse,
  UpdatePortForwardRuleRequest,
} from "../types";

export function usePortForwardingData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
) {
  const [portForwardRules, setPortForwardRules] = useState<
    PortForwardRuleListItem[]
  >([]);
  const [portForwardError, setPortForwardError] = useState<string | null>(null);
  const [portForwardLoading, setPortForwardLoading] = useState(false);
  const portForwardLoadGeneration = useRef(0);
  const portForwardLoadConsumer = useRef(
    new LatestReadConsumer<string | null>(),
  );
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const loadPortForwardRules = useCallback((): Promise<string | null> => {
    if (currentApiToken.current !== apiToken) {
      return Promise.resolve(
        "The operator session changed before port-forward rules could be loaded.",
      );
    }
    const generation = portForwardLoadGeneration.current + 1;
    portForwardLoadGeneration.current = generation;
    setPortForwardLoading(true);
    setPortForwardError(null);
    return portForwardLoadConsumer.current.enqueue(async () => {
      try {
        const records = await apiGet<PortForwardRuleListItem[]>(
          "/api/v1/port-forward-rules",
          apiToken,
        );
        if (
          portForwardLoadGeneration.current !== generation ||
          currentApiToken.current !== apiToken
        ) {
          return "A newer port-forward refresh superseded this request.";
        }
        setPortForwardRules(records);
        return null;
      } catch (error) {
        if (
          portForwardLoadGeneration.current !== generation ||
          currentApiToken.current !== apiToken
        ) {
          return "A newer port-forward refresh superseded this request.";
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          setPortForwardRules([]);
          setPortForwardError("Operator login required");
          return "Operator login required";
        }
        const message =
          error instanceof Error
            ? error.message
            : "Port-forward rules unavailable";
        setPortForwardError(message);
        return message;
      } finally {
        if (
          portForwardLoadGeneration.current === generation &&
          currentApiToken.current === apiToken
        ) {
          setPortForwardLoading(false);
        }
      }
    });
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
      if (currentApiToken.current !== apiToken) {
        return response;
      }
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
      if (currentApiToken.current !== apiToken) {
        return response;
      }
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
      if (currentApiToken.current !== apiToken) {
        return response;
      }
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
      if (currentApiToken.current !== apiToken) {
        return response;
      }
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

  const clearPortForwarding = useCallback(() => {
    portForwardLoadGeneration.current += 1;
    portForwardLoadConsumer.current.discardPending(null);
    currentApiToken.current = "";
    setPortForwardRules([]);
    setPortForwardError(null);
    setPortForwardLoading(false);
  }, []);

  return {
    bulkMutatePortForwardRules,
    clearPortForwarding,
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
