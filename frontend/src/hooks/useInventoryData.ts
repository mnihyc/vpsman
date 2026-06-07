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
  TagView,
  UpdateDataSourcePresetRequest,
  UpdateDataSourcePresetResponse,
} from "../types";

export function useInventoryData(apiToken: string, onUnauthorized: () => void, onFleetChanged: () => Promise<void>) {
  const [tags, setTags] = useState<TagView[]>([]);
  const [dataSourcePresets, setDataSourcePresets] = useState<DataSourcePresetRecord[]>([]);
  const [dataSourceAssignments, setDataSourceAssignments] = useState<DataSourcePresetAssignmentRecord[]>([]);
  const [dataSourceStatus, setDataSourceStatus] = useState<DataSourceStatusRecord[]>([]);
  const [tagsError, setTagsError] = useState<string | null>(null);
  const [tagsLoading, setTagsLoading] = useState(false);

  const loadTagInventory = useCallback(async () => {
    setTagsLoading(true);
    setTagsError(null);
    try {
      const [nextTags, nextDataSourcePresets, nextDataSourceAssignments, nextDataSourceStatus] = await Promise.all([
        apiGet<TagView[]>("/api/v1/tags", apiToken),
        apiGet<DataSourcePresetRecord[]>("/api/v1/data-source-presets", apiToken),
        apiGet<DataSourcePresetAssignmentRecord[]>("/api/v1/data-source-assignments", apiToken),
        apiGet<DataSourceStatusRecord[]>("/api/v1/data-source-status", apiToken),
      ]);
      setTags(nextTags);
      setDataSourcePresets(nextDataSourcePresets);
      setDataSourceAssignments(nextDataSourceAssignments);
      setDataSourceStatus(nextDataSourceStatus);
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setTags([]);
        setDataSourcePresets([]);
        setDataSourceAssignments([]);
        setDataSourceStatus([]);
        setTagsError("Operator login required");
        return;
      }
      setTagsError(error instanceof Error ? error.message : "Tag inventory unavailable");
    } finally {
      setTagsLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const createTag = useCallback(
    async (name: string) => {
      await apiPost("/api/v1/tags", apiToken, { name });
      await loadTagInventory();
    },
    [apiToken, loadTagInventory],
  );

  const assignTag = useCallback(
    async (clientId: string, tag: string) => {
      await apiPost(`/api/v1/agents/${encodeURIComponent(clientId)}/tags`, apiToken, { tag });
      await Promise.all([onFleetChanged(), loadTagInventory()]);
    },
    [apiToken, loadTagInventory, onFleetChanged],
  );

  const createDataSourcePreset = useCallback(
    async (request: CreateDataSourcePresetRequest) => {
      await apiPost("/api/v1/data-source-presets", apiToken, request);
      await loadTagInventory();
    },
    [apiToken, loadTagInventory],
  );

  const cloneDataSourcePreset = useCallback(
    async (presetId: string, request: CloneDataSourcePresetRequest) => {
      await apiPost(`/api/v1/data-source-presets/${encodeURIComponent(presetId)}/clone`, apiToken, request);
      await loadTagInventory();
    },
    [apiToken, loadTagInventory],
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
      await loadTagInventory();
      return response;
    },
    [apiToken, loadTagInventory],
  );

  const assignDataSourcePreset = useCallback(
    async (request: AssignDataSourcePresetRequest) => {
      const response = await apiPost<AssignDataSourcePresetResponse>(
        "/api/v1/data-source-assignments",
        apiToken,
        request,
      );
      await loadTagInventory();
      return response;
    },
    [apiToken, loadTagInventory],
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
    async (selectorExpression: string, destructive: boolean, confirmed = false) =>
      apiPost<BulkResolveResponse>("/api/v1/bulk/resolve", apiToken, {
        selector_expression: selectorExpression,
        destructive,
        confirmed,
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
    assignTag,
    cloneDataSourcePreset,
    createDataSourcePreset,
    createTag,
    dataSourceAssignments,
    dataSourcePresets,
    dataSourceStatus,
    diffDataSourcePreset,
    loadTagInventory,
    renderDataSourceHotConfig,
    resolveBulkPreview,
    resolveJobTargets,
    testDataSourcePreset,
    tags,
    tagsError,
    tagsLoading,
    updateDataSourcePreset,
  };
}
