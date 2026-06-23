import { useCallback, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import type {
  AssignSourceTemplateRequest,
  AssignSourceTemplateResponse,
  BulkTagMutationRequest,
  BulkResolveResponse,
  RuntimeConfigPatchRequest,
  RuntimeConfigPatchResponse,
  CloneSourceTemplateRequest,
  CreateSourceTemplateRequest,
  TemplateRuntimeConfigResponse,
  SourceTemplateAssignmentRecord,
  SourceTemplateDiffRequest,
  SourceTemplateDiffResponse,
  SourceTemplateRecord,
  SourceTemplateTestRequest,
  SourceTemplateTestResponse,
  SourceStatusRecord,
  DeleteRuntimeConfigPatchGeneratorRequest,
  RuntimeConfigApplyStateRecord,
  RuntimeConfigPatchGeneratorRecord,
  RuntimeConfigPatchGeneratorRenderRequest,
  RuntimeConfigPatchGeneratorRenderResponse,
  JobTargetSelection,
  PrivilegeAssertion,
  TagMutationResponse,
  TagView,
  UpdateSourceTemplateRequest,
  UpdateSourceTemplateResponse,
  UpsertRuntimeConfigPatchGeneratorRequest,
} from "../types";

export function useInventoryData(apiToken: string, onUnauthorized: () => void, onFleetChanged: () => Promise<void>) {
  const [tags, setTags] = useState<TagView[]>([]);
  const [sourceTemplates, setSourceTemplates] = useState<SourceTemplateRecord[]>([]);
  const [sourceTemplateAssignments, setSourceTemplateAssignments] = useState<SourceTemplateAssignmentRecord[]>([]);
  const [sourceStatus, setSourceStatus] = useState<SourceStatusRecord[]>([]);
  const [runtimeConfigApplyStates, setRuntimeConfigApplyStates] = useState<RuntimeConfigApplyStateRecord[]>([]);
  const [runtimeConfigPatchGenerators, setRuntimeConfigPatchGenerators] = useState<RuntimeConfigPatchGeneratorRecord[]>([]);
  const [tagsError, setTagsError] = useState<string | null>(null);
  const [tagsLoading, setTagsLoading] = useState(false);

  const loadTagInventory = useCallback(async () => {
    setTagsLoading(true);
    setTagsError(null);
    try {
      const [
        nextTags,
        nextSourceTemplates,
        nextSourceTemplateAssignments,
        nextSourceStatus,
        nextRuntimeConfigApplyStates,
        nextPatchGenerators,
      ] = await Promise.all([
        apiGet<TagView[]>("/api/v1/tags", apiToken),
        apiGet<SourceTemplateRecord[]>("/api/v1/source-templates", apiToken),
        apiGet<SourceTemplateAssignmentRecord[]>("/api/v1/source-template-assignments", apiToken),
        apiGet<SourceStatusRecord[]>("/api/v1/source-status", apiToken),
        apiGet<RuntimeConfigApplyStateRecord[]>("/api/v1/runtime-config/apply-state", apiToken),
        apiGet<RuntimeConfigPatchGeneratorRecord[]>("/api/v1/runtime-config/patch-generators", apiToken),
      ]);
      setTags(nextTags);
      setSourceTemplates(nextSourceTemplates);
      setSourceTemplateAssignments(nextSourceTemplateAssignments);
      setSourceStatus(nextSourceStatus);
      setRuntimeConfigApplyStates(nextRuntimeConfigApplyStates);
      setRuntimeConfigPatchGenerators(nextPatchGenerators);
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setTags([]);
        setSourceTemplates([]);
        setSourceTemplateAssignments([]);
        setSourceStatus([]);
        setRuntimeConfigApplyStates([]);
        setRuntimeConfigPatchGenerators([]);
        setTagsError("Operator login required");
        return;
      }
      setTagsError(error instanceof Error ? error.message : "Tag inventory unavailable");
    } finally {
      setTagsLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const loadRuntimeConfigApplyStates = useCallback(async () => {
    try {
      setRuntimeConfigApplyStates(
        await apiGet<RuntimeConfigApplyStateRecord[]>("/api/v1/runtime-config/apply-state", apiToken),
      );
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setRuntimeConfigApplyStates([]);
      }
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

  const createSourceTemplate = useCallback(
    async (request: CreateSourceTemplateRequest) => {
      await apiPost("/api/v1/source-templates", apiToken, request);
      await loadTagInventory();
    },
    [apiToken, loadTagInventory],
  );

  const cloneSourceTemplate = useCallback(
    async (templateId: string, request: CloneSourceTemplateRequest) => {
      await apiPost(`/api/v1/source-templates/${encodeURIComponent(templateId)}/clone`, apiToken, request);
      await loadTagInventory();
    },
    [apiToken, loadTagInventory],
  );

  const diffSourceTemplate = useCallback(
    async (templateId: string, request: SourceTemplateDiffRequest) =>
      apiPost<SourceTemplateDiffResponse>(
        `/api/v1/source-templates/${encodeURIComponent(templateId)}/diff`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const testSourceTemplate = useCallback(
    async (templateId: string, request: SourceTemplateTestRequest) =>
      apiPost<SourceTemplateTestResponse>(
        `/api/v1/source-templates/${encodeURIComponent(templateId)}/test`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const updateSourceTemplate = useCallback(
    async (templateId: string, request: UpdateSourceTemplateRequest) => {
      const response = await apiPost<UpdateSourceTemplateResponse>(
        `/api/v1/source-templates/${encodeURIComponent(templateId)}/update`,
        apiToken,
        request,
      );
      await loadTagInventory();
      return response;
    },
    [apiToken, loadTagInventory],
  );

  const assignSourceTemplate = useCallback(
    async (request: AssignSourceTemplateRequest) => {
      const response = await apiPost<AssignSourceTemplateResponse>(
        "/api/v1/source-template-assignments",
        apiToken,
        request,
      );
      await loadTagInventory();
      return response;
    },
    [apiToken, loadTagInventory],
  );

  const renderTemplateRuntimeConfig = useCallback(
    async (clientId: string) =>
      apiGet<TemplateRuntimeConfigResponse>(
        `/api/v1/template-runtime-config?client_id=${encodeURIComponent(clientId)}`,
        apiToken,
      ),
    [apiToken],
  );

  const upsertRuntimeConfigPatchGenerator = useCallback(
    async (request: UpsertRuntimeConfigPatchGeneratorRequest) => {
      const response = await apiPost<RuntimeConfigPatchGeneratorRecord>(
        "/api/v1/runtime-config/patch-generators",
        apiToken,
        request,
      );
      await loadTagInventory();
      return response;
    },
    [apiToken, loadTagInventory],
  );

  const renderRuntimeConfigPatchGenerator = useCallback(
    async (generatorId: string, request: RuntimeConfigPatchGeneratorRenderRequest) =>
      apiPost<RuntimeConfigPatchGeneratorRenderResponse>(
        `/api/v1/runtime-config/patch-generators/${encodeURIComponent(generatorId)}/render`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const submitRuntimeConfigPatch = useCallback(
    async (request: RuntimeConfigPatchRequest) => {
      const response = await apiPost<RuntimeConfigPatchResponse>(
        "/api/v1/runtime-config/patch",
        apiToken,
        request,
      );
      await loadRuntimeConfigApplyStates();
      return response;
    },
    [apiToken, loadRuntimeConfigApplyStates],
  );

  const deleteRuntimeConfigPatchGenerator = useCallback(
    async (generatorId: string, request: DeleteRuntimeConfigPatchGeneratorRequest) => {
      await apiDelete(`/api/v1/runtime-config/patch-generators/${encodeURIComponent(generatorId)}`, apiToken, request);
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
    assignSourceTemplate,
    assignTag,
    bulkMutateTags,
    submitRuntimeConfigPatch,
    cloneSourceTemplate,
    createSourceTemplate,
    createTag,
    sourceTemplateAssignments,
    sourceTemplates,
    sourceStatus,
    deleteRuntimeConfigPatchGenerator,
    deleteTag,
    diffSourceTemplate,
    loadTagInventory,
    loadRuntimeConfigApplyStates,
    runtimeConfigApplyStates,
    runtimeConfigPatchGenerators,
    renderTemplateRuntimeConfig,
    renderRuntimeConfigPatchGenerator,
    resolveBulkPreview,
    resolveJobTargets,
    testSourceTemplate,
    tags,
    tagsError,
    tagsLoading,
    updateTagOrder,
    updateSourceTemplate,
    upsertRuntimeConfigPatchGenerator,
  };
}
