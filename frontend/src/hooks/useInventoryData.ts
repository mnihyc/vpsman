import { useCallback, useRef, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPostPreview, apiPut, isApiUnauthorized } from "../api";
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
  const [runtimeConfigApplyError, setRuntimeConfigApplyError] =
    useState<string | null>(null);
  const [tagsLoading, setTagsLoading] = useState(false);
  const [runtimeConfigApplyLoading, setRuntimeConfigApplyLoading] =
    useState(false);
  const [
    tagInventoryEvidenceAvailable,
    setTagInventoryEvidenceAvailable,
  ] = useState(false);
  const [
    runtimeConfigApplyEvidenceAvailable,
    setRuntimeConfigApplyEvidenceAvailable,
  ] = useState(false);
  const loadTagInventoryInFlight = useRef<{
    request: Promise<void>;
    token: string;
  } | null>(null);
  const tagInventoryLoadGeneration = useRef(0);
  const sourceTemplatesLoadGeneration = useRef(0);
  const runtimeConfigApplyLoadGeneration = useRef(0);
  const tagOrderMutationGeneration = useRef(0);
  const tagInventoryError = useRef<string | null>(null);
  const sourceTemplatesError = useRef<string | null>(null);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const publishTagsError = useCallback(() => {
    const errors = [
      tagInventoryError.current,
      sourceTemplatesError.current,
    ].filter((message): message is string => Boolean(message));
    setTagsError(errors.length > 0 ? errors.join("; ") : null);
  }, []);

  const loadTagInventory = useCallback(async (forceFresh = false) => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    if (
      !forceFresh &&
      loadTagInventoryInFlight.current?.token === apiToken
    ) {
      return loadTagInventoryInFlight.current.request;
    }
    const generation = tagInventoryLoadGeneration.current + 1;
    tagInventoryLoadGeneration.current = generation;
    const sourceTemplatesGeneration =
      sourceTemplatesLoadGeneration.current + 1;
    sourceTemplatesLoadGeneration.current = sourceTemplatesGeneration;
    const runtimeApplyGeneration =
      runtimeConfigApplyLoadGeneration.current + 1;
    runtimeConfigApplyLoadGeneration.current = runtimeApplyGeneration;
    const request = (async () => {
      setTagsLoading(true);
      tagInventoryError.current = null;
      sourceTemplatesError.current = null;
      publishTagsError();
      setRuntimeConfigApplyError(null);
      setRuntimeConfigApplyLoading(true);
      setTagInventoryEvidenceAvailable(false);
      try {
        const [
          tagsResult,
          sourceTemplatesResult,
          sourceTemplateAssignmentsResult,
          sourceStatusResult,
          runtimeConfigApplyStatesResult,
          patchGeneratorsResult,
        ] = await Promise.allSettled([
          apiGet<TagView[]>("/api/v1/tags", apiToken),
          apiGet<SourceTemplateRecord[]>("/api/v1/source-templates", apiToken),
          apiGet<SourceTemplateAssignmentRecord[]>("/api/v1/source-template-assignments", apiToken),
          apiGet<SourceStatusRecord[]>("/api/v1/source-status", apiToken),
          apiGet<RuntimeConfigApplyStateRecord[]>("/api/v1/runtime-config/apply-state", apiToken),
          apiGet<RuntimeConfigPatchGeneratorRecord[]>("/api/v1/runtime-config/patch-generators", apiToken),
        ]);
        if (
          tagInventoryLoadGeneration.current !== generation ||
          currentApiToken.current !== apiToken
        ) {
          return;
        }
        const results = [
          tagsResult,
          sourceTemplatesResult,
          sourceTemplateAssignmentsResult,
          sourceStatusResult,
          runtimeConfigApplyStatesResult,
          patchGeneratorsResult,
        ];
        if (
          results.some(
            (result) =>
              result.status === "rejected" &&
              isApiUnauthorized(result.reason),
          )
        ) {
          onUnauthorized();
          setTags([]);
          setSourceTemplates([]);
          setSourceTemplateAssignments([]);
          setSourceStatus([]);
          setRuntimeConfigApplyStates([]);
          setRuntimeConfigPatchGenerators([]);
          setTagInventoryEvidenceAvailable(false);
          setRuntimeConfigApplyEvidenceAvailable(false);
          tagInventoryError.current = "Operator login required";
          sourceTemplatesError.current = null;
          publishTagsError();
          setRuntimeConfigApplyError("Operator login required");
          return;
        }
        if (tagsResult.status === "fulfilled") {
          setTags(tagsResult.value);
        }
        if (
          sourceTemplatesLoadGeneration.current ===
            sourceTemplatesGeneration &&
          sourceTemplatesResult.status === "fulfilled"
        ) {
          setSourceTemplates(sourceTemplatesResult.value);
        }
        if (sourceTemplateAssignmentsResult.status === "fulfilled") {
          setSourceTemplateAssignments(sourceTemplateAssignmentsResult.value);
        }
        if (sourceStatusResult.status === "fulfilled") {
          setSourceStatus(sourceStatusResult.value);
        }
        if (runtimeConfigApplyStatesResult.status === "fulfilled") {
          if (
            runtimeConfigApplyLoadGeneration.current ===
            runtimeApplyGeneration
          ) {
            setRuntimeConfigApplyStates(runtimeConfigApplyStatesResult.value);
            setRuntimeConfigApplyEvidenceAvailable(true);
          }
        }
        if (patchGeneratorsResult.status === "fulfilled") {
          setRuntimeConfigPatchGenerators(patchGeneratorsResult.value);
        }
        setTagInventoryEvidenceAvailable(
          [
            tagsResult,
            sourceTemplatesResult,
            sourceTemplateAssignmentsResult,
            sourceStatusResult,
            patchGeneratorsResult,
          ].every((result) => result.status === "fulfilled"),
        );
        tagInventoryError.current = unavailableSourceSummary(
          "Some inventory sources are unavailable",
          [
            tagsResult,
            sourceTemplateAssignmentsResult,
            sourceStatusResult,
            patchGeneratorsResult,
          ],
          [
            "tags",
            "source template assignments",
            "source status",
            "runtime configuration patch generators",
          ],
        );
        if (
          sourceTemplatesLoadGeneration.current === sourceTemplatesGeneration
        ) {
          sourceTemplatesError.current = settledSourceError(
            "Source templates",
            sourceTemplatesResult,
          );
        }
        publishTagsError();
        if (
          runtimeConfigApplyLoadGeneration.current === runtimeApplyGeneration
        ) {
          if (runtimeConfigApplyStatesResult.status === "rejected") {
            setRuntimeConfigApplyEvidenceAvailable(false);
          }
          setRuntimeConfigApplyError(
            unavailableSourceSummary(
              "Runtime configuration source unavailable",
              [runtimeConfigApplyStatesResult],
              ["apply state"],
            ),
          );
        }
      } finally {
        if (
          tagInventoryLoadGeneration.current === generation &&
          currentApiToken.current === apiToken
        ) {
          setTagsLoading(false);
        }
        if (
          runtimeConfigApplyLoadGeneration.current === runtimeApplyGeneration &&
          currentApiToken.current === apiToken
        ) {
          setRuntimeConfigApplyLoading(false);
        }
      }
    })();
    loadTagInventoryInFlight.current = { request, token: apiToken };
    try {
      await request;
    } finally {
      if (loadTagInventoryInFlight.current?.request === request) {
        loadTagInventoryInFlight.current = null;
      }
    }
  }, [apiToken, onUnauthorized, publishTagsError]);

  const refreshTagInventoryAfterMutation = useCallback(
    () => loadTagInventory(true),
    [loadTagInventory],
  );

  const loadSourceTemplates = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = sourceTemplatesLoadGeneration.current + 1;
    sourceTemplatesLoadGeneration.current = generation;
    sourceTemplatesError.current = null;
    publishTagsError();
    try {
      const records = await apiGet<SourceTemplateRecord[]>(
        "/api/v1/source-templates",
        apiToken,
      );
      if (
        sourceTemplatesLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setSourceTemplates(records);
      sourceTemplatesError.current = null;
      publishTagsError();
    } catch (error) {
      if (
        sourceTemplatesLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setSourceTemplates([]);
        sourceTemplatesError.current = "Operator login required";
        publishTagsError();
        throw new Error("Operator login required");
      }
      sourceTemplatesError.current =
        error instanceof Error
          ? `Source templates: ${error.message}`
          : "Source templates unavailable";
      publishTagsError();
      throw error;
    }
  }, [apiToken, onUnauthorized, publishTagsError]);

  const loadRuntimeConfigApplyStates = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = runtimeConfigApplyLoadGeneration.current + 1;
    runtimeConfigApplyLoadGeneration.current = generation;
    setRuntimeConfigApplyError(null);
    setRuntimeConfigApplyLoading(true);
    try {
      const records = await apiGet<RuntimeConfigApplyStateRecord[]>(
        "/api/v1/runtime-config/apply-state",
        apiToken,
      );
      if (
        runtimeConfigApplyLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setRuntimeConfigApplyStates(records);
      setRuntimeConfigApplyEvidenceAvailable(true);
      setRuntimeConfigApplyError(null);
    } catch (error) {
      if (
        runtimeConfigApplyLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setRuntimeConfigApplyStates([]);
        setRuntimeConfigApplyEvidenceAvailable(false);
        setRuntimeConfigApplyError("Operator login required");
        return;
      }
      setRuntimeConfigApplyEvidenceAvailable(false);
      setRuntimeConfigApplyError(
        error instanceof Error
          ? error.message
          : "Runtime configuration apply state unavailable",
      );
    } finally {
      if (
        runtimeConfigApplyLoadGeneration.current === generation &&
        currentApiToken.current === apiToken
      ) {
        setRuntimeConfigApplyLoading(false);
      }
    }
  }, [apiToken, onUnauthorized]);

  const createTag = useCallback(
    async (name: string, privilegeAssertion: PrivilegeAssertion) => {
      await apiPost("/api/v1/tags", apiToken, { confirmed: true, name, privilege_assertion: privilegeAssertion });
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await refreshTagInventoryAfterMutation();
    },
    [apiToken, refreshTagInventoryAfterMutation],
  );

  const updateTagOrder = useCallback(
    async (orderedTags: string[]) => {
      const operationGeneration = tagOrderMutationGeneration.current + 1;
      tagOrderMutationGeneration.current = operationGeneration;
      const response = await apiPut<TagView[]>("/api/v1/tags/order", apiToken, {
        ordered_tags: orderedTags,
      });
      if (
        currentApiToken.current !== apiToken ||
        tagOrderMutationGeneration.current !== operationGeneration
      ) {
        return response;
      }
      setTags(response);
      await refreshTagInventoryAfterMutation();
      return response;
    },
    [apiToken, refreshTagInventoryAfterMutation],
  );

  const assignTag = useCallback(
    async (clientId: string, tag: string, privilegeAssertion: PrivilegeAssertion) => {
      const response = await apiPost<TagMutationResponse>(`/api/v1/agents/${encodeURIComponent(clientId)}/tags`, apiToken, {
        confirmed: true,
        privilege_assertion: privilegeAssertion,
        tag,
      });
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await Promise.all([onFleetChanged(), refreshTagInventoryAfterMutation()]);
      return response;
    },
    [apiToken, onFleetChanged, refreshTagInventoryAfterMutation],
  );

  const bulkMutateTags = useCallback(
    async (request: BulkTagMutationRequest) => {
      const response = await (request.confirmed ? apiPost : apiPostPreview)<TagMutationResponse>(
        "/api/v1/tags/bulk",
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      if (!response.confirmation_required) {
        await Promise.all([onFleetChanged(), refreshTagInventoryAfterMutation()]);
      }
      return response;
    },
    [apiToken, onFleetChanged, refreshTagInventoryAfterMutation],
  );

  const deleteTag = useCallback(
    async (
      tag: string,
      confirmed: boolean,
      privilegeAssertion?: PrivilegeAssertion | null,
      previewHash?: string | null,
    ) => {
      const response = await apiDelete<TagMutationResponse>(`/api/v1/tags/${encodeURIComponent(tag)}`, apiToken, {
        confirmed,
        preview_hash: previewHash ?? null,
        privilege_assertion: privilegeAssertion,
      });
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      if (!response.confirmation_required) {
        await Promise.all([onFleetChanged(), refreshTagInventoryAfterMutation()]);
      }
      return response;
    },
    [apiToken, onFleetChanged, refreshTagInventoryAfterMutation],
  );

  const createSourceTemplate = useCallback(
    async (request: CreateSourceTemplateRequest) => {
      await apiPost("/api/v1/source-templates", apiToken, request);
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await refreshTagInventoryAfterMutation();
    },
    [apiToken, refreshTagInventoryAfterMutation],
  );

  const cloneSourceTemplate = useCallback(
    async (templateId: string, request: CloneSourceTemplateRequest) => {
      await apiPost(`/api/v1/source-templates/${encodeURIComponent(templateId)}/clone`, apiToken, request);
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await refreshTagInventoryAfterMutation();
    },
    [apiToken, refreshTagInventoryAfterMutation],
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
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await refreshTagInventoryAfterMutation();
      return response;
    },
    [apiToken, refreshTagInventoryAfterMutation],
  );

  const assignSourceTemplate = useCallback(
    async (request: AssignSourceTemplateRequest) => {
      const response = await apiPost<AssignSourceTemplateResponse>(
        "/api/v1/source-template-assignments",
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await refreshTagInventoryAfterMutation();
      return response;
    },
    [apiToken, refreshTagInventoryAfterMutation],
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
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await refreshTagInventoryAfterMutation();
      return response;
    },
    [apiToken, refreshTagInventoryAfterMutation],
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
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await loadRuntimeConfigApplyStates();
      return response;
    },
    [apiToken, loadRuntimeConfigApplyStates],
  );

  const deleteRuntimeConfigPatchGenerator = useCallback(
    async (generatorId: string, request: DeleteRuntimeConfigPatchGeneratorRequest) => {
      await apiDelete(`/api/v1/runtime-config/patch-generators/${encodeURIComponent(generatorId)}`, apiToken, request);
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await refreshTagInventoryAfterMutation();
    },
    [apiToken, refreshTagInventoryAfterMutation],
  );

  const resolveBulkPreview = useCallback(
    async (selectorExpression: string) =>
      apiPostPreview<BulkResolveResponse>("/api/v1/bulk/resolve", apiToken, {
        selector_expression: selectorExpression,
      }),
    [apiToken],
  );

  const resolveJobTargets = useCallback(
    async (selection: JobTargetSelection) =>
      apiPostPreview<BulkResolveResponse>("/api/v1/bulk/resolve", apiToken, selection),
    [apiToken],
  );

  const clearInventory = useCallback(() => {
    tagInventoryLoadGeneration.current += 1;
    sourceTemplatesLoadGeneration.current += 1;
    runtimeConfigApplyLoadGeneration.current += 1;
    tagOrderMutationGeneration.current += 1;
    loadTagInventoryInFlight.current = null;
    currentApiToken.current = "";
    tagInventoryError.current = null;
    sourceTemplatesError.current = null;
    setTags([]);
    setSourceTemplates([]);
    setSourceTemplateAssignments([]);
    setSourceStatus([]);
    setRuntimeConfigApplyStates([]);
    setRuntimeConfigPatchGenerators([]);
    setTagsError(null);
    setRuntimeConfigApplyError(null);
    setTagInventoryEvidenceAvailable(false);
    setRuntimeConfigApplyEvidenceAvailable(false);
    setRuntimeConfigApplyLoading(false);
    setTagsLoading(false);
  }, []);

  return {
    assignSourceTemplate,
    assignTag,
    bulkMutateTags,
    clearInventory,
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
    loadSourceTemplates,
    loadRuntimeConfigApplyStates,
    runtimeConfigApplyEvidenceAvailable,
    runtimeConfigApplyError,
    runtimeConfigApplyLoading,
    runtimeConfigApplyStates,
    runtimeConfigPatchGenerators,
    renderTemplateRuntimeConfig,
    renderRuntimeConfigPatchGenerator,
    resolveBulkPreview,
    resolveJobTargets,
    testSourceTemplate,
    tagInventoryEvidenceAvailable,
    tags,
    tagsError,
    tagsLoading,
    updateTagOrder,
    updateSourceTemplate,
    upsertRuntimeConfigPatchGenerator,
  };
}

function unavailableSourceSummary(
  prefix: string,
  results: readonly PromiseSettledResult<unknown>[],
  labels: readonly string[],
): string | null {
  const failedLabels = results.flatMap((result, index) =>
    result.status === "rejected" ? [labels[index]] : [],
  );
  return failedLabels.length > 0
    ? `${prefix}: ${failedLabels.join(", ")}`
    : null;
}

function settledSourceError(
  label: string,
  result: PromiseSettledResult<unknown>,
): string | null {
  if (result.status === "fulfilled") {
    return null;
  }
  return result.reason instanceof Error
    ? `${label}: ${result.reason.message}`
    : `${label} unavailable`;
}
