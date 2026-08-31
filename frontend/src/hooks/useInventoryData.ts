import { useCallback, useRef, useState } from "react";
import {
  apiDelete,
  apiGet,
  apiPost,
  apiPostPreview,
  apiPut,
  isApiUnauthorized,
  LatestReadConsumer,
} from "../api";
import type {
  ApplyConfigurationSourceOverrideRequest,
  ApplyConfigurationSourceOverrideResponse,
  ApplyRuntimeConfigBulkOverrideRequest,
  ApplyRuntimeConfigBulkOverrideResponse,
  ApplyRuntimeConfigOverrideRequest,
  ApplyRuntimeConfigOverrideResponse,
  BulkResolveManyRequest,
  BulkResolveManyResponse,
  BulkTagMutationRequest,
  BulkResolveResponse,
  CloneConfigurationPresetRequest,
  ConfigurationPresetPreview,
  ConfigurationPresetRecord,
  ConfigurationSourceOverridePreview,
  ConfigurationSourceOverrideRequest,
  ConfigurationSourceView,
  CreateConfigurationPresetRequest,
  EffectiveAgentConfigResponse,
  PreviewRuntimeConfigBulkOverrideRequest,
  PreviewRuntimeConfigOverrideRequest,
  PreviewConfigurationPresetRequest,
  DeleteRuntimeConfigPatchGeneratorRequest,
  RuntimeConfigApplyStateRecord,
  RuntimeConfigBulkOverridePreview,
  RuntimeConfigClientWorkspace,
  RuntimeConfigOverridePreview,
  RuntimeConfigPatchGeneratorRecord,
  RuntimeConfigPatchGeneratorRenderRequest,
  RuntimeConfigPatchGeneratorRenderResponse,
  JobTargetSelection,
  PrivilegeAssertion,
  TagMutationResponse,
  TagOrderState,
  TagView,
  UpdateTagOrderRequest,
  UpdateConfigurationPresetRequest,
  UpdateConfigurationPresetResponse,
  UpsertRuntimeConfigPatchGeneratorRequest,
} from "../types";
import { retainMutationSuccessAfterRefresh } from "../utils";

export function useInventoryData(
  apiToken: string,
  onUnauthorized: () => void,
  onAgentTagsChanged: (response: TagMutationResponse) => void,
) {
  const [tags, setTags] = useState<TagView[]>([]);
  const [namespaceNaturalSortEnabled, setNamespaceNaturalSortEnabled] =
    useState(false);
  const [configurationPresets, setConfigurationPresets] = useState<
    ConfigurationPresetRecord[]
  >([]);
  const [configurationSources, setConfigurationSources] = useState<
    ConfigurationSourceView[]
  >([]);
  const [runtimeConfigApplyStates, setRuntimeConfigApplyStates] = useState<
    RuntimeConfigApplyStateRecord[]
  >([]);
  const [runtimeConfigPatchGenerators, setRuntimeConfigPatchGenerators] =
    useState<RuntimeConfigPatchGeneratorRecord[]>([]);
  const [tagsError, setTagsError] = useState<string | null>(null);
  const [runtimeConfigApplyError, setRuntimeConfigApplyError] = useState<
    string | null
  >(null);
  const [configurationPresetsError, setConfigurationPresetsError] = useState<
    string | null
  >(null);
  const [configurationSourcesError, setConfigurationSourcesError] = useState<
    string | null
  >(null);
  const [tagsLoading, setTagsLoading] = useState(false);
  const [configurationPresetsLoading, setConfigurationPresetsLoading] =
    useState(false);
  const [configurationSourcesLoading, setConfigurationSourcesLoading] =
    useState(false);
  const [runtimeConfigApplyLoading, setRuntimeConfigApplyLoading] =
    useState(false);
  const [tagInventoryEvidenceAvailable, setTagInventoryEvidenceAvailable] =
    useState(false);
  const [
    runtimeConfigApplyEvidenceAvailable,
    setRuntimeConfigApplyEvidenceAvailable,
  ] = useState(false);
  const [
    configurationPresetsEvidenceAvailable,
    setConfigurationPresetsEvidenceAvailable,
  ] = useState(false);
  const [
    configurationSourcesEvidenceAvailable,
    setConfigurationSourcesEvidenceAvailable,
  ] = useState(false);
  const tagOrderLoadConsumer = useRef(new LatestReadConsumer());
  const patchGeneratorLoadConsumer = useRef(new LatestReadConsumer());
  const runtimeConfigApplyLoadConsumer = useRef(new LatestReadConsumer());
  const loadConfigurationPresetsInFlight = useRef<{
    request: Promise<void>;
    token: string;
  } | null>(null);
  const loadConfigurationSourcesInFlight = useRef<{
    request: Promise<void>;
    token: string;
  } | null>(null);
  const tagOrderLoadGeneration = useRef(0);
  const patchGeneratorLoadGeneration = useRef(0);
  const tagOrderSourceAvailable = useRef(false);
  const patchGeneratorSourceAvailable = useRef(false);
  const tagOrderSourceError = useRef<string | null>(null);
  const patchGeneratorSourceError = useRef<string | null>(null);
  const tagLoadSequence = useRef(0);
  const tagLoadsPending = useRef(new Set<number>());
  const configurationPresetsLoadGeneration = useRef(0);
  const configurationSourcesLoadGeneration = useRef(0);
  const runtimeConfigApplyLoadGeneration = useRef(0);
  const tagOrderMutationGeneration = useRef(0);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const publishTagInventoryState = useCallback(() => {
    setTagInventoryEvidenceAvailable(
      tagOrderSourceAvailable.current && patchGeneratorSourceAvailable.current,
    );
    const errors = [
      tagOrderSourceError.current,
      patchGeneratorSourceError.current,
    ].filter((message): message is string => message !== null);
    setTagsError(errors.length > 0 ? errors.join("; ") : null);
  }, []);

  const beginTagLoad = useCallback(() => {
    const operation = ++tagLoadSequence.current;
    tagLoadsPending.current.add(operation);
    setTagsLoading(true);
    return operation;
  }, []);

  const finishTagLoad = useCallback((operation: number) => {
    tagLoadsPending.current.delete(operation);
    setTagsLoading(tagLoadsPending.current.size > 0);
  }, []);

  const loadTagOrder = useCallback((): Promise<void> => {
    if (currentApiToken.current !== apiToken) {
      return Promise.resolve();
    }
    const generation = tagOrderLoadGeneration.current + 1;
    tagOrderLoadGeneration.current = generation;
    const mutationGeneration = tagOrderMutationGeneration.current;
    const tagLoadOperation = beginTagLoad();
    tagOrderSourceError.current = null;
    publishTagInventoryState();
    return tagOrderLoadConsumer.current
      .enqueue(async () => {
        try {
          const state = await apiGet<TagOrderState>(
            "/api/v1/tags/order",
            apiToken,
          );
          if (
            currentApiToken.current !== apiToken ||
            tagOrderLoadGeneration.current !== generation ||
            tagOrderMutationGeneration.current !== mutationGeneration
          ) {
            return;
          }
          setTags(state.tags);
          setNamespaceNaturalSortEnabled(state.namespace_natural_sort_enabled);
          tagOrderSourceAvailable.current = true;
          tagOrderSourceError.current = null;
          publishTagInventoryState();
        } catch (error) {
          if (
            currentApiToken.current !== apiToken ||
            tagOrderLoadGeneration.current !== generation
          ) {
            return;
          }
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            setTags([]);
            setNamespaceNaturalSortEnabled(false);
            tagOrderSourceAvailable.current = false;
            tagOrderSourceError.current = "Operator login required";
            publishTagInventoryState();
            return;
          }
          tagOrderSourceAvailable.current = false;
          tagOrderSourceError.current = inventorySourceFailure("Tags", error);
          publishTagInventoryState();
        }
      })
      .finally(() => finishTagLoad(tagLoadOperation));
  }, [
    apiToken,
    beginTagLoad,
    finishTagLoad,
    onUnauthorized,
    publishTagInventoryState,
  ]);

  const loadRuntimeConfigPatchGenerators = useCallback((): Promise<void> => {
    if (currentApiToken.current !== apiToken) {
      return Promise.resolve();
    }
    const generation = patchGeneratorLoadGeneration.current + 1;
    patchGeneratorLoadGeneration.current = generation;
    const tagLoadOperation = beginTagLoad();
    patchGeneratorSourceError.current = null;
    publishTagInventoryState();
    return patchGeneratorLoadConsumer.current
      .enqueue(async () => {
        try {
          const records = await apiGet<RuntimeConfigPatchGeneratorRecord[]>(
            "/api/v1/runtime-config/patch-generators",
            apiToken,
          );
          if (
            currentApiToken.current !== apiToken ||
            patchGeneratorLoadGeneration.current !== generation
          ) {
            return;
          }
          setRuntimeConfigPatchGenerators(records);
          patchGeneratorSourceAvailable.current = true;
          patchGeneratorSourceError.current = null;
          publishTagInventoryState();
        } catch (error) {
          if (
            currentApiToken.current !== apiToken ||
            patchGeneratorLoadGeneration.current !== generation
          ) {
            return;
          }
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            setRuntimeConfigPatchGenerators([]);
            patchGeneratorSourceAvailable.current = false;
            patchGeneratorSourceError.current = "Operator login required";
            publishTagInventoryState();
            return;
          }
          patchGeneratorSourceAvailable.current = false;
          patchGeneratorSourceError.current = inventorySourceFailure(
            "Runtime configuration patch generators",
            error,
          );
          publishTagInventoryState();
        }
      })
      .finally(() => finishTagLoad(tagLoadOperation));
  }, [
    apiToken,
    beginTagLoad,
    finishTagLoad,
    onUnauthorized,
    publishTagInventoryState,
  ]);

  const refreshTagOrderAfterMutation = useCallback(
    () => loadTagOrder(),
    [loadTagOrder],
  );

  const loadConfigurationPresets = useCallback(
    async (forceFresh = false) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (
        !forceFresh &&
        loadConfigurationPresetsInFlight.current?.token === apiToken
      ) {
        return loadConfigurationPresetsInFlight.current.request;
      }
      const generation = configurationPresetsLoadGeneration.current + 1;
      configurationPresetsLoadGeneration.current = generation;
      const request = (async () => {
        setConfigurationPresetsError(null);
        setConfigurationPresetsLoading(true);
        setConfigurationPresetsEvidenceAvailable(false);
        try {
          const presets = await apiGet<ConfigurationPresetRecord[]>(
            "/api/v1/configuration-presets",
            apiToken,
          );
          if (
            configurationPresetsLoadGeneration.current !== generation ||
            currentApiToken.current !== apiToken
          ) {
            return;
          }
          setConfigurationPresets(presets);
          setConfigurationPresetsError(null);
          setConfigurationPresetsEvidenceAvailable(true);
        } catch (error) {
          if (
            configurationPresetsLoadGeneration.current !== generation ||
            currentApiToken.current !== apiToken
          ) {
            return;
          }
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            setConfigurationPresets([]);
            setConfigurationPresetsEvidenceAvailable(false);
            setConfigurationPresetsError("Operator login required");
            throw new Error("Operator login required");
          }
          setConfigurationPresetsError(
            error instanceof Error
              ? `Configuration presets: ${error.message}`
              : "Configuration presets unavailable",
          );
          setConfigurationPresetsEvidenceAvailable(false);
          throw error;
        } finally {
          if (
            configurationPresetsLoadGeneration.current === generation &&
            currentApiToken.current === apiToken
          ) {
            setConfigurationPresetsLoading(false);
          }
        }
      })();
      loadConfigurationPresetsInFlight.current = { request, token: apiToken };
      try {
        await request;
      } finally {
        if (loadConfigurationPresetsInFlight.current?.request === request) {
          loadConfigurationPresetsInFlight.current = null;
        }
      }
    },
    [apiToken, onUnauthorized],
  );

  const loadConfigurationSources = useCallback(
    async (forceFresh = false) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (
        !forceFresh &&
        loadConfigurationSourcesInFlight.current?.token === apiToken
      ) {
        return loadConfigurationSourcesInFlight.current.request;
      }
      const generation = configurationSourcesLoadGeneration.current + 1;
      configurationSourcesLoadGeneration.current = generation;
      const request = (async () => {
        setConfigurationSourcesError(null);
        setConfigurationSourcesLoading(true);
        setConfigurationSourcesEvidenceAvailable(false);
        try {
          const sources = await apiGet<ConfigurationSourceView[]>(
            "/api/v1/configuration-sources",
            apiToken,
          );
          if (
            configurationSourcesLoadGeneration.current !== generation ||
            currentApiToken.current !== apiToken
          ) {
            return;
          }
          setConfigurationSources(sources);
          setConfigurationSourcesError(null);
          setConfigurationSourcesEvidenceAvailable(true);
        } catch (error) {
          if (
            configurationSourcesLoadGeneration.current !== generation ||
            currentApiToken.current !== apiToken
          ) {
            return;
          }
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            setConfigurationSources([]);
            setConfigurationSourcesEvidenceAvailable(false);
            setConfigurationSourcesError("Operator login required");
            throw new Error("Operator login required");
          }
          setConfigurationSourcesError(
            error instanceof Error
              ? `Configuration sources: ${error.message}`
              : "Configuration sources unavailable",
          );
          setConfigurationSourcesEvidenceAvailable(false);
          throw error;
        } finally {
          if (
            configurationSourcesLoadGeneration.current === generation &&
            currentApiToken.current === apiToken
          ) {
            setConfigurationSourcesLoading(false);
          }
        }
      })();
      loadConfigurationSourcesInFlight.current = { request, token: apiToken };
      try {
        await request;
      } finally {
        if (loadConfigurationSourcesInFlight.current?.request === request) {
          loadConfigurationSourcesInFlight.current = null;
        }
      }
    },
    [apiToken, onUnauthorized],
  );

  // Presets are authoring catalog data; sources are effective per-VPS evidence.
  // Compose them only for workflows that render or mutate both resources.
  const loadConfigurationInventory = useCallback(
    async (forceFresh = false) => {
      const results = await Promise.allSettled([
        loadConfigurationPresets(forceFresh),
        loadConfigurationSources(forceFresh),
      ]);
      const failure = results.find(
        (result): result is PromiseRejectedResult =>
          result.status === "rejected",
      );
      if (failure) {
        throw failure.reason;
      }
    },
    [loadConfigurationPresets, loadConfigurationSources],
  );

  const loadRuntimeConfigApplyStates = useCallback((): Promise<void> => {
    if (currentApiToken.current !== apiToken) {
      return Promise.resolve();
    }
    const generation = runtimeConfigApplyLoadGeneration.current + 1;
    runtimeConfigApplyLoadGeneration.current = generation;
    setRuntimeConfigApplyError(null);
    setRuntimeConfigApplyLoading(true);
    return runtimeConfigApplyLoadConsumer.current.enqueue(async () => {
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
    });
  }, [apiToken, onUnauthorized]);

  // Aggregate consumers compose the exact source owners. This keeps each
  // transport generation-fenced and coalesced in one place regardless of
  // whether a page requests the aggregate or an individual source.
  const loadTagInventory = useCallback(
    async (_forceFresh = false): Promise<void> => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await Promise.all([
        loadTagOrder(),
        loadRuntimeConfigApplyStates(),
        loadRuntimeConfigPatchGenerators(),
      ]);
    },
    [
      apiToken,
      loadRuntimeConfigApplyStates,
      loadRuntimeConfigPatchGenerators,
      loadTagOrder,
    ],
  );

  const createTag = useCallback(
    async (name: string, privilegeAssertion: PrivilegeAssertion) => {
      await apiPost("/api/v1/tags", apiToken, {
        confirmed: true,
        name,
        privilege_assertion: privilegeAssertion,
      });
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await refreshTagOrderAfterMutation();
    },
    [apiToken, refreshTagOrderAfterMutation],
  );

  const updateTagOrder = useCallback(
    async (request: UpdateTagOrderRequest) => {
      const operationGeneration = tagOrderMutationGeneration.current + 1;
      tagOrderMutationGeneration.current = operationGeneration;
      const response = await apiPut<TagOrderState>(
        "/api/v1/tags/order",
        apiToken,
        request,
      );
      if (
        currentApiToken.current !== apiToken ||
        tagOrderMutationGeneration.current !== operationGeneration
      ) {
        return response;
      }
      tagOrderMutationGeneration.current = operationGeneration + 1;
      setTags(response.tags);
      setNamespaceNaturalSortEnabled(response.namespace_natural_sort_enabled);
      tagOrderSourceAvailable.current = true;
      tagOrderSourceError.current = null;
      publishTagInventoryState();
      return response;
    },
    [apiToken, publishTagInventoryState],
  );

  const assignTag = useCallback(
    async (
      clientId: string,
      tag: string,
      privilegeAssertion: PrivilegeAssertion,
    ) => {
      const response = await apiPost<TagMutationResponse>(
        `/api/v1/agents/${encodeURIComponent(clientId)}/tags`,
        apiToken,
        {
          confirmed: true,
          privilege_assertion: privilegeAssertion,
          tag,
        },
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      if (!response.confirmation_required) {
        onAgentTagsChanged(response);
        await refreshTagOrderAfterMutation();
      }
      return response;
    },
    [apiToken, onAgentTagsChanged, refreshTagOrderAfterMutation],
  );

  const bulkMutateTags = useCallback(
    async (request: BulkTagMutationRequest) => {
      const response = await (
        request.confirmed ? apiPost : apiPostPreview
      )<TagMutationResponse>("/api/v1/tags/bulk", apiToken, request);
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      if (!response.confirmation_required) {
        onAgentTagsChanged(response);
        await refreshTagOrderAfterMutation();
      }
      return response;
    },
    [apiToken, onAgentTagsChanged, refreshTagOrderAfterMutation],
  );

  const deleteTag = useCallback(
    async (
      tag: string,
      confirmed: boolean,
      privilegeAssertion?: PrivilegeAssertion | null,
      previewHash?: string | null,
    ) => {
      const response = await apiDelete<TagMutationResponse>(
        `/api/v1/tags/${encodeURIComponent(tag)}`,
        apiToken,
        {
          confirmed,
          preview_hash: previewHash ?? null,
          privilege_assertion: privilegeAssertion,
        },
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      if (!response.confirmation_required) {
        onAgentTagsChanged(response);
        await refreshTagOrderAfterMutation();
      }
      return response;
    },
    [apiToken, onAgentTagsChanged, refreshTagOrderAfterMutation],
  );

  const createConfigurationPreset = useCallback(
    async (request: CreateConfigurationPresetRequest) => {
      const response = await apiPost<ConfigurationPresetRecord>(
        "/api/v1/configuration-presets",
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await retainMutationSuccessAfterRefresh(() =>
        loadConfigurationInventory(true),
      );
      return response;
    },
    [apiToken, loadConfigurationInventory],
  );

  const cloneConfigurationPreset = useCallback(
    async (presetId: string, request: CloneConfigurationPresetRequest) => {
      const response = await apiPost<ConfigurationPresetRecord>(
        `/api/v1/configuration-presets/${encodeURIComponent(presetId)}/clone`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await retainMutationSuccessAfterRefresh(() =>
        loadConfigurationInventory(true),
      );
      return response;
    },
    [apiToken, loadConfigurationInventory],
  );

  const previewConfigurationPreset = useCallback(
    async (presetId: string, request: PreviewConfigurationPresetRequest) =>
      apiPostPreview<ConfigurationPresetPreview>(
        `/api/v1/configuration-presets/${encodeURIComponent(presetId)}/preview`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const updateConfigurationPreset = useCallback(
    async (presetId: string, request: UpdateConfigurationPresetRequest) => {
      const response = await apiPut<UpdateConfigurationPresetResponse>(
        `/api/v1/configuration-presets/${encodeURIComponent(presetId)}`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await retainMutationSuccessAfterRefresh(() =>
        loadConfigurationInventory(true),
      );
      return response;
    },
    [apiToken, loadConfigurationInventory],
  );

  const deleteConfigurationPreset = useCallback(
    async (presetId: string) => {
      await apiDelete(
        `/api/v1/configuration-presets/${encodeURIComponent(presetId)}`,
        apiToken,
      );
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await retainMutationSuccessAfterRefresh(() =>
        loadConfigurationInventory(true),
      );
    },
    [apiToken, loadConfigurationInventory],
  );

  const previewConfigurationSourceOverride = useCallback(
    async (request: ConfigurationSourceOverrideRequest) =>
      apiPostPreview<ConfigurationSourceOverridePreview>(
        "/api/v1/configuration-source-overrides/preview",
        apiToken,
        request,
      ),
    [apiToken],
  );

  const applyConfigurationSourceOverride = useCallback(
    async (request: ApplyConfigurationSourceOverrideRequest) => {
      const response = await apiPost<ApplyConfigurationSourceOverrideResponse>(
        "/api/v1/configuration-source-overrides/apply",
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await retainMutationSuccessAfterRefresh(() =>
        loadConfigurationInventory(true),
      );
      return response;
    },
    [apiToken, loadConfigurationInventory],
  );

  const loadEffectiveAgentConfig = useCallback(
    async (clientId: string) =>
      apiGet<EffectiveAgentConfigResponse>(
        `/api/v1/effective-agent-config?client_id=${encodeURIComponent(clientId)}`,
        apiToken,
      ),
    [apiToken],
  );

  const loadRuntimeConfigClientWorkspace = useCallback(
    async (clientId: string) =>
      apiGet<RuntimeConfigClientWorkspace>(
        `/api/v1/runtime-config/clients/${encodeURIComponent(clientId)}/workspace`,
        apiToken,
      ),
    [apiToken],
  );

  const previewRuntimeConfigOverride = useCallback(
    async (clientId: string, request: PreviewRuntimeConfigOverrideRequest) =>
      apiPostPreview<RuntimeConfigOverridePreview>(
        `/api/v1/runtime-config/clients/${encodeURIComponent(clientId)}/override/preview`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const applyRuntimeConfigOverride = useCallback(
    async (clientId: string, request: ApplyRuntimeConfigOverrideRequest) => {
      const response = await apiPost<ApplyRuntimeConfigOverrideResponse>(
        `/api/v1/runtime-config/clients/${encodeURIComponent(clientId)}/override/apply`,
        apiToken,
        request,
      );
      if (currentApiToken.current === apiToken) {
        await retainMutationSuccessAfterRefresh(loadRuntimeConfigApplyStates);
      }
      return response;
    },
    [apiToken, loadRuntimeConfigApplyStates],
  );

  const previewRuntimeConfigBulkOverride = useCallback(
    async (request: PreviewRuntimeConfigBulkOverrideRequest) =>
      apiPostPreview<RuntimeConfigBulkOverridePreview>(
        "/api/v1/runtime-config/overrides/bulk/preview",
        apiToken,
        request,
      ),
    [apiToken],
  );

  const applyRuntimeConfigBulkOverride = useCallback(
    async (request: ApplyRuntimeConfigBulkOverrideRequest) => {
      const response = await apiPost<ApplyRuntimeConfigBulkOverrideResponse>(
        "/api/v1/runtime-config/overrides/bulk/apply",
        apiToken,
        request,
      );
      if (currentApiToken.current === apiToken) {
        await retainMutationSuccessAfterRefresh(loadRuntimeConfigApplyStates);
      }
      return response;
    },
    [apiToken, loadRuntimeConfigApplyStates],
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
      await loadRuntimeConfigPatchGenerators();
      return response;
    },
    [apiToken, loadRuntimeConfigPatchGenerators],
  );

  const renderRuntimeConfigPatchGenerator = useCallback(
    async (
      generatorId: string,
      request: RuntimeConfigPatchGeneratorRenderRequest,
    ) =>
      apiPost<RuntimeConfigPatchGeneratorRenderResponse>(
        `/api/v1/runtime-config/patch-generators/${encodeURIComponent(generatorId)}/render`,
        apiToken,
        request,
      ),
    [apiToken],
  );

  const deleteRuntimeConfigPatchGenerator = useCallback(
    async (
      generatorId: string,
      request: DeleteRuntimeConfigPatchGeneratorRequest,
    ) => {
      await apiDelete(
        `/api/v1/runtime-config/patch-generators/${encodeURIComponent(generatorId)}`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await loadRuntimeConfigPatchGenerators();
    },
    [apiToken, loadRuntimeConfigPatchGenerators],
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
      apiPostPreview<BulkResolveResponse>(
        "/api/v1/bulk/resolve",
        apiToken,
        selection,
      ),
    [apiToken],
  );

  const resolveManyJobTargets = useCallback(
    async (request: BulkResolveManyRequest) =>
      apiPostPreview<BulkResolveManyResponse>(
        "/api/v1/bulk/resolve-many",
        apiToken,
        request,
      ),
    [apiToken],
  );

  const clearInventory = useCallback(() => {
    tagOrderLoadGeneration.current += 1;
    patchGeneratorLoadGeneration.current += 1;
    tagOrderLoadConsumer.current.discardPending();
    patchGeneratorLoadConsumer.current.discardPending();
    runtimeConfigApplyLoadConsumer.current.discardPending();
    configurationPresetsLoadGeneration.current += 1;
    configurationSourcesLoadGeneration.current += 1;
    runtimeConfigApplyLoadGeneration.current += 1;
    tagOrderMutationGeneration.current += 1;
    loadConfigurationPresetsInFlight.current = null;
    loadConfigurationSourcesInFlight.current = null;
    currentApiToken.current = "";
    tagOrderSourceAvailable.current = false;
    patchGeneratorSourceAvailable.current = false;
    tagOrderSourceError.current = null;
    patchGeneratorSourceError.current = null;
    tagLoadsPending.current.clear();
    setTags([]);
    setNamespaceNaturalSortEnabled(false);
    setConfigurationPresets([]);
    setConfigurationSources([]);
    setRuntimeConfigApplyStates([]);
    setRuntimeConfigPatchGenerators([]);
    setTagsError(null);
    setConfigurationPresetsError(null);
    setConfigurationPresetsEvidenceAvailable(false);
    setConfigurationSourcesError(null);
    setConfigurationSourcesEvidenceAvailable(false);
    setRuntimeConfigApplyError(null);
    setTagInventoryEvidenceAvailable(false);
    setRuntimeConfigApplyEvidenceAvailable(false);
    setRuntimeConfigApplyLoading(false);
    setConfigurationPresetsLoading(false);
    setConfigurationSourcesLoading(false);
    setTagsLoading(false);
  }, []);

  return {
    applyConfigurationSourceOverride,
    applyRuntimeConfigBulkOverride,
    applyRuntimeConfigOverride,
    assignTag,
    bulkMutateTags,
    clearInventory,
    cloneConfigurationPreset,
    configurationPresets,
    configurationPresetsEvidenceAvailable,
    configurationPresetsError,
    configurationPresetsLoading,
    configurationSourcesEvidenceAvailable,
    configurationSourcesError,
    configurationSourcesLoading,
    configurationSources,
    createConfigurationPreset,
    createTag,
    deleteConfigurationPreset,
    deleteRuntimeConfigPatchGenerator,
    deleteTag,
    loadEffectiveAgentConfig,
    loadRuntimeConfigClientWorkspace,
    loadTagInventory,
    loadTagOrder,
    loadConfigurationInventory,
    loadConfigurationSources,
    loadRuntimeConfigApplyStates,
    loadRuntimeConfigPatchGenerators,
    namespaceNaturalSortEnabled,
    runtimeConfigApplyEvidenceAvailable,
    runtimeConfigApplyError,
    runtimeConfigApplyLoading,
    runtimeConfigApplyStates,
    runtimeConfigPatchGenerators,
    previewConfigurationPreset,
    previewConfigurationSourceOverride,
    previewRuntimeConfigBulkOverride,
    previewRuntimeConfigOverride,
    renderRuntimeConfigPatchGenerator,
    resolveBulkPreview,
    resolveManyJobTargets,
    resolveJobTargets,
    tagInventoryEvidenceAvailable,
    tags,
    tagsError,
    tagsLoading,
    updateTagOrder,
    updateConfigurationPreset,
    upsertRuntimeConfigPatchGenerator,
  };
}

function inventorySourceFailure(label: string, error: unknown): string {
  return `${label}: ${
    error instanceof Error ? error.message : "source unavailable"
  }`;
}
