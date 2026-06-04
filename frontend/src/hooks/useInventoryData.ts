import { useCallback, useState } from "react";
import { apiGet, apiPost, isApiUnauthorized } from "../api";
import type {
  AssignDataSourcePresetRequest,
  AssignDataSourcePresetResponse,
  BulkResolveResponse,
  CloneDataSourcePresetRequest,
  CreateDataSourcePresetRequest,
  DataSourceHotConfigResponse,
  DataSourcePresetAssignmentRecord,
  DataSourcePresetDiffRequest,
  DataSourcePresetDiffResponse,
  DataSourcePresetRecord,
  DataSourcePresetTestRequest,
  DataSourcePresetTestResponse,
  DataSourceStatusRecord,
  JobTargetSelection,
  ResourcePoolView,
  TagView,
  UpdateDataSourcePresetRequest,
  UpdateDataSourcePresetResponse,
} from "../types";

export function useInventoryData(apiToken: string, onUnauthorized: () => void, onFleetChanged: () => Promise<void>) {
  const [pools, setPools] = useState<ResourcePoolView[]>([]);
  const [tags, setTags] = useState<TagView[]>([]);
  const [dataSourcePresets, setDataSourcePresets] = useState<DataSourcePresetRecord[]>([]);
  const [dataSourceAssignments, setDataSourceAssignments] = useState<DataSourcePresetAssignmentRecord[]>([]);
  const [dataSourceStatus, setDataSourceStatus] = useState<DataSourceStatusRecord[]>([]);
  const [poolsError, setPoolsError] = useState<string | null>(null);
  const [poolsLoading, setPoolsLoading] = useState(false);

  const loadPoolsAndTags = useCallback(async () => {
    setPoolsLoading(true);
    setPoolsError(null);
    try {
      const [nextPools, nextTags, nextDataSourcePresets, nextDataSourceAssignments, nextDataSourceStatus] = await Promise.all([
        apiGet<ResourcePoolView[]>("/api/v1/pools", apiToken),
        apiGet<TagView[]>("/api/v1/tags", apiToken),
        apiGet<DataSourcePresetRecord[]>("/api/v1/data-source-presets", apiToken),
        apiGet<DataSourcePresetAssignmentRecord[]>("/api/v1/data-source-assignments", apiToken),
        apiGet<DataSourceStatusRecord[]>("/api/v1/data-source-status", apiToken),
      ]);
      setPools(nextPools);
      setTags(nextTags);
      setDataSourcePresets(nextDataSourcePresets);
      setDataSourceAssignments(nextDataSourceAssignments);
      setDataSourceStatus(nextDataSourceStatus);
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setPools([]);
        setTags([]);
        setDataSourcePresets([]);
        setDataSourceAssignments([]);
        setDataSourceStatus([]);
        setPoolsError("Operator login required");
        return;
      }
      setPoolsError(error instanceof Error ? error.message : "Pool/tag data unavailable");
    } finally {
      setPoolsLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const createPool = useCallback(
    async (name: string, provider: string, region: string) => {
      await apiPost("/api/v1/pools", apiToken, {
        name,
        provider: provider || null,
        region: region || null,
      });
      await loadPoolsAndTags();
    },
    [apiToken, loadPoolsAndTags],
  );

  const createTag = useCallback(
    async (name: string) => {
      await apiPost("/api/v1/tags", apiToken, { name });
      await loadPoolsAndTags();
    },
    [apiToken, loadPoolsAndTags],
  );

  const assignPool = useCallback(
    async (clientId: string, poolId: string) => {
      await apiPost(`/api/v1/agents/${encodeURIComponent(clientId)}/pool`, apiToken, { pool_id: poolId });
      await Promise.all([onFleetChanged(), loadPoolsAndTags()]);
    },
    [apiToken, loadPoolsAndTags, onFleetChanged],
  );

  const assignTag = useCallback(
    async (clientId: string, tag: string) => {
      await apiPost(`/api/v1/agents/${encodeURIComponent(clientId)}/tags`, apiToken, { tag });
      await Promise.all([onFleetChanged(), loadPoolsAndTags()]);
    },
    [apiToken, loadPoolsAndTags, onFleetChanged],
  );

  const createDataSourcePreset = useCallback(
    async (request: CreateDataSourcePresetRequest) => {
      await apiPost("/api/v1/data-source-presets", apiToken, request);
      await loadPoolsAndTags();
    },
    [apiToken, loadPoolsAndTags],
  );

  const cloneDataSourcePreset = useCallback(
    async (presetId: string, request: CloneDataSourcePresetRequest) => {
      await apiPost(`/api/v1/data-source-presets/${encodeURIComponent(presetId)}/clone`, apiToken, request);
      await loadPoolsAndTags();
    },
    [apiToken, loadPoolsAndTags],
  );

  const diffDataSourcePreset = useCallback(
    async (presetId: string, request: DataSourcePresetDiffRequest) =>
      apiPost<DataSourcePresetDiffResponse>(
        `/api/v1/data-source-presets/${encodeURIComponent(presetId)}/diff`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const testDataSourcePreset = useCallback(
    async (presetId: string, request: DataSourcePresetTestRequest) =>
      apiPost<DataSourcePresetTestResponse>(
        `/api/v1/data-source-presets/${encodeURIComponent(presetId)}/test`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const updateDataSourcePreset = useCallback(
    async (presetId: string, request: UpdateDataSourcePresetRequest) => {
      const response = await apiPost<UpdateDataSourcePresetResponse>(
        `/api/v1/data-source-presets/${encodeURIComponent(presetId)}/update`,
        apiToken,
        request,
      );
      await loadPoolsAndTags();
      return response;
    },
    [apiToken, loadPoolsAndTags],
  );

  const assignDataSourcePreset = useCallback(
    async (request: AssignDataSourcePresetRequest) => {
      const response = await apiPost<AssignDataSourcePresetResponse>(
        "/api/v1/data-source-assignments",
        apiToken,
        request,
      );
      await loadPoolsAndTags();
      return response;
    },
    [apiToken, loadPoolsAndTags],
  );

  const renderDataSourceHotConfig = useCallback(
    async (clientId: string) =>
      apiGet<DataSourceHotConfigResponse>(
        `/api/v1/data-source-hot-config?client_id=${encodeURIComponent(clientId)}`,
        apiToken,
      ),
    [apiToken],
  );

  const resolveBulkPreview = useCallback(
    async (poolIds: string[], tagNames: string[], destructive: boolean, tagMode: "any" | "all" = "any") =>
      apiPost<BulkResolveResponse>("/api/v1/bulk/resolve", apiToken, {
        clients: [],
        pools: poolIds,
        tags: tagNames,
        tag_mode: tagMode,
        destructive,
        confirmed: false,
      }),
    [apiToken],
  );

  const resolveJobTargets = useCallback(
    async (selection: JobTargetSelection) =>
      apiPost<BulkResolveResponse>("/api/v1/bulk/resolve", apiToken, selection),
    [apiToken],
  );

  return {
    assignDataSourcePreset,
    assignPool,
    assignTag,
    cloneDataSourcePreset,
    createDataSourcePreset,
    createPool,
    createTag,
    dataSourceAssignments,
    dataSourcePresets,
    dataSourceStatus,
    diffDataSourcePreset,
    loadPoolsAndTags,
    pools,
    poolsError,
    poolsLoading,
    renderDataSourceHotConfig,
    resolveBulkPreview,
    resolveJobTargets,
    testDataSourcePreset,
    tags,
    updateDataSourcePreset,
  };
}
