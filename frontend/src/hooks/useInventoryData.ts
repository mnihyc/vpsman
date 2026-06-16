import { useCallback, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import type {
  AssignDataSourcePresetRequest,
  AssignDataSourcePresetResponse,
  BulkTagMutationRequest,
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
  HotConfigRuleTemplateRecord,
  HotConfigRuleTemplateRenderRequest,
  HotConfigRuleTemplateRenderResponse,
  JobTargetSelection,
  PrivilegeAssertion,
  TagMutationResponse,
  TagView,
  UpdateDataSourcePresetRequest,
  UpdateDataSourcePresetResponse,
  UpsertHotConfigRuleTemplateRequest,
} from "../types";

export function useInventoryData(apiToken: string, onUnauthorized: () => void, onFleetChanged: () => Promise<void>) {
  const [tags, setTags] = useState<TagView[]>([]);
  const [dataSourcePresets, setDataSourcePresets] = useState<DataSourcePresetRecord[]>([]);
  const [dataSourceAssignments, setDataSourceAssignments] = useState<DataSourcePresetAssignmentRecord[]>([]);
  const [dataSourceStatus, setDataSourceStatus] = useState<DataSourceStatusRecord[]>([]);
  const [hotConfigRuleTemplates, setHotConfigRuleTemplates] = useState<HotConfigRuleTemplateRecord[]>([]);
  const [tagsError, setTagsError] = useState<string | null>(null);
  const [tagsLoading, setTagsLoading] = useState(false);

  const loadTagInventory = useCallback(async () => {
    setTagsLoading(true);
    setTagsError(null);
    try {
      const [
        nextTags,
        nextDataSourcePresets,
        nextDataSourceAssignments,
        nextDataSourceStatus,
        nextRuleTemplates,
      ] = await Promise.all([
        apiGet<TagView[]>("/api/v1/tags", apiToken),
        apiGet<DataSourcePresetRecord[]>("/api/v1/data-source-presets", apiToken),
        apiGet<DataSourcePresetAssignmentRecord[]>("/api/v1/data-source-assignments", apiToken),
        apiGet<DataSourceStatusRecord[]>("/api/v1/data-source-status", apiToken),
        apiGet<HotConfigRuleTemplateRecord[]>("/api/v1/hot-config/rule-templates", apiToken),
      ]);
      setTags(nextTags);
      setDataSourcePresets(nextDataSourcePresets);
      setDataSourceAssignments(nextDataSourceAssignments);
      setDataSourceStatus(nextDataSourceStatus);
      setHotConfigRuleTemplates(nextRuleTemplates);
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setTags([]);
        setDataSourcePresets([]);
        setDataSourceAssignments([]);
        setDataSourceStatus([]);
        setHotConfigRuleTemplates([]);
        setTagsError("Operator login required");
        return;
      }
      setTagsError(error instanceof Error ? error.message : "Tag inventory unavailable");
    } finally {
      setTagsLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const createTag = useCallback(
    async (name: string, privilegeAssertion: PrivilegeAssertion) => {
      await apiPost("/api/v1/tags", apiToken, { confirmed: true, name, privilege_assertion: privilegeAssertion });
      await loadTagInventory();
    },
    [apiToken, loadTagInventory],
  );

  const updateTagOrder = useCallback(
    async (orderedTags: string[]) => {
      const response = await apiPut<TagView[]>("/api/v1/tags/order", apiToken, {
        ordered_tags: orderedTags,
      });
      setTags(response);
      return response;
    },
    [apiToken],
  );

  const assignTag = useCallback(
    async (clientId: string, tag: string, privilegeAssertion: PrivilegeAssertion) => {
      const response = await apiPost<TagMutationResponse>(`/api/v1/agents/${encodeURIComponent(clientId)}/tags`, apiToken, {
        confirmed: true,
        privilege_assertion: privilegeAssertion,
        tag,
      });
      await Promise.all([onFleetChanged(), loadTagInventory()]);
      return response;
    },
    [apiToken, loadTagInventory, onFleetChanged],
  );

  const bulkMutateTags = useCallback(
    async (request: BulkTagMutationRequest) => {
      const response = await apiPost<TagMutationResponse>("/api/v1/tags/bulk", apiToken, request);
      if (!response.confirmation_required) {
        await Promise.all([onFleetChanged(), loadTagInventory()]);
      }
      return response;
    },
    [apiToken, loadTagInventory, onFleetChanged],
  );

  const deleteTag = useCallback(
    async (tag: string, confirmed: boolean, privilegeAssertion?: PrivilegeAssertion | null) => {
      const response = await apiDelete<TagMutationResponse>(`/api/v1/tags/${encodeURIComponent(tag)}`, apiToken, {
        confirmed,
        privilege_assertion: privilegeAssertion,
      });
      if (!response.confirmation_required) {
        await Promise.all([onFleetChanged(), loadTagInventory()]);
      }
      return response;
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

  const upsertHotConfigRuleTemplate = useCallback(
    async (request: UpsertHotConfigRuleTemplateRequest) => {
      const response = await apiPost<HotConfigRuleTemplateRecord>(
        "/api/v1/hot-config/rule-templates",
        apiToken,
        request,
      );
      await loadTagInventory();
      return response;
    },
    [apiToken, loadTagInventory],
  );

  const renderHotConfigRuleTemplate = useCallback(
    async (templateId: string, request: HotConfigRuleTemplateRenderRequest) =>
      apiPost<HotConfigRuleTemplateRenderResponse>(
        `/api/v1/hot-config/rule-templates/${encodeURIComponent(templateId)}/render`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const deleteHotConfigRuleTemplate = useCallback(
    async (templateId: string) => {
      await apiDelete(`/api/v1/hot-config/rule-templates/${encodeURIComponent(templateId)}`, apiToken);
      await loadTagInventory();
    },
    [apiToken, loadTagInventory],
  );

  const resolveBulkPreview = useCallback(
    async (selectorExpression: string) =>
      apiPost<BulkResolveResponse>("/api/v1/bulk/resolve", apiToken, {
        selector_expression: selectorExpression,
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
    bulkMutateTags,
    cloneDataSourcePreset,
    createDataSourcePreset,
    createTag,
    dataSourceAssignments,
    dataSourcePresets,
    dataSourceStatus,
    deleteHotConfigRuleTemplate,
    deleteTag,
    diffDataSourcePreset,
    loadTagInventory,
    hotConfigRuleTemplates,
    renderDataSourceHotConfig,
    renderHotConfigRuleTemplate,
    resolveBulkPreview,
    resolveJobTargets,
    testDataSourcePreset,
    tags,
    tagsError,
    tagsLoading,
    updateTagOrder,
    updateDataSourcePreset,
    upsertHotConfigRuleTemplate,
  };
}
