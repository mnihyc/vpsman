import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { NumberedTextarea } from "../../components/NumberedTextarea";
import { ActionFeedback } from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { PrivilegeVaultBox } from "../../components/PrivilegeVaultBox";
import { buildPrivilegeAssertion, canonicalDbPrivilegeIntent, type PrivilegeMaterial } from "../../privilege";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { ConsoleActionDrawer } from "../../components/ConsoleLayout";
import { scrollIntoViewWithMotion } from "../../motion";
import type {
  JsonValue,
  AgentView,
  NetworkAdapterDefinitionRecord,
  NetworkAdapterMutationResponse,
  NetworkAdapterPreviewResponse,
  NetworkAdapterKind,
  TunnelPlanRecord,
  UpsertNetworkAdapterDefinitionRequest,
  UpdateNetworkAdapterDefinitionRequest,
  UpdateNetworkAdapterDetailsRequest,
} from "../../types";
import { formatTime, runPanelAction } from "../../utils";

type EditorState =
  | { mode: "create"; kind: NetworkAdapterKind }
  | { mode: "details" | "commands"; definition: NetworkAdapterDefinitionRecord }
  | null;

export type NetworkAdapterReviewControls = {
  agents: AgentView[];
  onPreview: (id: string, request: UpsertNetworkAdapterDefinitionRequest) => Promise<NetworkAdapterPreviewResponse>;
  onUpdateDetails: (id: string, request: UpdateNetworkAdapterDetailsRequest) => Promise<NetworkAdapterDefinitionRecord>;
  onOpenPrivilegeUnlock: () => void;
  onOpenJobHistory?: () => void;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => Promise<void>;
};

type CommandReview = {
  id: string;
  request: UpsertNetworkAdapterDefinitionRequest;
  preview: NetworkAdapterPreviewResponse;
  endpoints: Record<string, { label: string; online: boolean }>;
};

export function NetworkAdapterDefinitionsPanel({
  definitions,
  editorRequest,
  editorOnly = false,
  initialKind,
  onCreate,
  onDelete,
  onInitialKindConsumed,
  onEditorClosed,
  onUpdate,
  reviewControls,
  tunnelPlans,
}: {
  definitions: NetworkAdapterDefinitionRecord[];
  editorRequest?: Exclude<EditorState, null>;
  editorOnly?: boolean;
  initialKind: NetworkAdapterKind | null;
  onCreate: (
    request: UpsertNetworkAdapterDefinitionRequest,
  ) => Promise<NetworkAdapterDefinitionRecord>;
  onDelete: (definitionId: string) => Promise<void>;
  onInitialKindConsumed: () => void;
  onEditorClosed?: (message?: string, hasDispatch?: boolean) => void;
  onUpdate: (
    definitionId: string,
    request: UpdateNetworkAdapterDefinitionRequest,
  ) => Promise<NetworkAdapterMutationResponse>;
  reviewControls: NetworkAdapterReviewControls;
  tunnelPlans: TunnelPlanRecord[];
}) {
  const [editor, setEditor] = useState<EditorState>(null);
  const [deleteTarget, setDeleteTarget] =
    useState<NetworkAdapterDefinitionRecord | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [kind, setKind] = useState<NetworkAdapterKind>("runtime_tunnel");
  const [definition, setDefinition] = useState<Record<string, JsonValue>>(() =>
    defaultAdapterDefinition("runtime_tunnel"),
  );
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [review, setReview] = useState<CommandReview | null>(null);
  const [hasDispatch, setHasDispatch] = useState(false);
  const draftRevision = useRef(0);
  const registryFeedbackRef = useRef<HTMLDivElement | null>(null);
  const editorFeedbackRef = useRef<HTMLDivElement | null>(null);
  const registryFeedbackMessage = editor === null ? (error ?? feedback) : null;

  useEffect(() => {
    if (!registryFeedbackMessage) return;
    const frame = window.requestAnimationFrame(() => {
      if (registryFeedbackRef.current) {
        scrollIntoViewWithMotion(registryFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [registryFeedbackMessage]);

  useEffect(() => {
    if (!editor || !error) return;
    const frame = window.requestAnimationFrame(() => {
      if (editorFeedbackRef.current) {
        scrollIntoViewWithMotion(editorFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [editor, error]);

  function changeEditorDraft(change: () => void) {
    draftRevision.current += 1;
    setReview(null);
    setError(null);
    change();
  }

  useEffect(() => {
    if (!initialKind) return;
    openCreate(initialKind);
    onInitialKindConsumed();
  }, [initialKind, onInitialKindConsumed]);

  useEffect(() => {
    if (!editorRequest) return;
    if (editorRequest.mode === "create") openCreate(editorRequest.kind);
    else openEdit(editorRequest.definition, editorRequest.mode);
  }, [editorRequest]);

  function openCreate(adapterKind: NetworkAdapterKind) {
    draftRevision.current += 1;
    setReview(null);
    setKind(adapterKind);
    setName("");
    setDescription("");
    setDefinition(defaultAdapterDefinition(adapterKind));
    setError(null);
    setFeedback(null);
    setEditor({ mode: "create", kind: adapterKind });
  }

  function openEdit(record: NetworkAdapterDefinitionRecord, mode: "details" | "commands") {
    draftRevision.current += 1;
    setReview(null);
    setKind(record.adapter_kind);
    setName(record.name);
    setDescription(record.description ?? "");
    setDefinition(asObject(record.definition));
    setError(null);
    setFeedback(null);
    setEditor({ mode, definition: record });
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    setFeedback(null);
    await runPanelAction(setPending, setError, async () => {
      if (!name.trim()) throw new Error("Adapter name is required");
      if (editor?.mode === "details") {
        await reviewControls.onUpdateDetails(editor.definition.id, {
          expected_updated_at: editor.definition.updated_at,
          name: name.trim(),
          description: description.trim() || null,
        });
        const message = `Saved details for ${name.trim()}. Commands and running resources are unchanged.`;
        setFeedback(message);
        setHasDispatch(false);
        setEditor(null);
        onEditorClosed?.(message);
        return;
      }
      const definitionError = validateAdapterDefinition(kind, definition);
      if (definitionError) throw new Error(definitionError);
      const request: UpsertNetworkAdapterDefinitionRequest = {
        adapter_kind: kind,
        name: editor?.mode === "commands"
          ? (definitions.find((record) => record.id === editor.definition.id) ?? editor.definition).name
          : name.trim(),
        description: editor?.mode === "commands"
          ? (definitions.find((record) => record.id === editor.definition.id) ?? editor.definition).description
          : description.trim() || null,
        definition,
      };
      if (editor?.mode === "commands") {
        const revision = draftRevision.current;
        const preview = await reviewControls.onPreview(editor.definition.id, request);
        if (draftRevision.current !== revision) return;
        if (preview.change_kind !== "commands") {
          setFeedback("No command changes. No restart or runtime job is needed.");
          return;
        }
        setReview({
          id: editor.definition.id,
          request,
          preview,
          endpoints: Object.fromEntries(preview.affected_resources.flatMap((resource) =>
            resource.client_ids.map((id) => {
              const agent = reviewControls.agents.find((candidate) => candidate.id === id);
              return [id, {
                label: `${agent?.display_name || id} (${id}) — ${agent?.status ?? "unknown"}`,
                online: agent?.status === "online",
              }];
            }),
          )),
        });
        return;
      } else {
        await onCreate(request);
        setFeedback(`Created ${name.trim()}`);
      }
      setEditor(null);
      onEditorClosed?.();
    });
  }

  async function applyCommands() {
    if (!review || !reviewControls.privilegeMaterial) return;
    const snapshot = review;
    await runPanelAction(setPending, setError, async () => {
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: "network_adapter_definition.update",
          target: `network_adapter_definition:${snapshot.id}`,
          confirmed: true,
          payloadHash: snapshot.preview.review_hash,
          resolvedTargets: snapshot.preview.target_client_ids,
        }),
        privilegeMaterial: reviewControls.privilegeMaterial!,
      });
      const response = await onUpdate(snapshot.id, {
        ...snapshot.request,
        review_hash: snapshot.preview.review_hash,
        privilege_assertion: privilegeAssertion,
      });
      const message = snapshot.request.adapter_kind === "routing_cost"
        ? "Saved routing adapter commands. Refresh and review routing status before applying a cost; no tunnel cleanup or immediate cost update was queued."
        : response.sync.length
        ? `Saved adapter commands. ${response.sync.map((item) => `${item.client_id}: ${item.status}${item.error ? ` — ${item.error}` : ""}`).join("; ")}. Queued work is not confirmation of application; inspect Jobs for outcomes.`
        : "Saved adapter commands. No active runtime targets were queued; disabled bindings remain disabled.";
      setReview(null);
      setEditor(null);
      setFeedback(message);
      setHasDispatch(response.sync.length > 0);
      onEditorClosed?.(message, response.sync.length > 0);
    });
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    setFeedback(null);
    await runPanelAction(setPending, setError, async () => {
      await onDelete(deleteTarget.id);
      setFeedback(`Deleted ${deleteTarget.name}`);
      setDeleteTarget(null);
    });
  }

  const columns = useMemo<
    ConsoleDataGridColumn<NetworkAdapterDefinitionRecord>[]
  >(
    () => [
      {
        id: "name",
        header: "Adapter definition",
        cell: (record) => (
          <span className="historyPrimary">
            <strong>{record.name}</strong>
            <small>{record.description ?? "No description"}</small>
          </span>
        ),
        searchValue: (record) => `${record.name} ${record.description ?? ""}`,
        sortValue: (record) => record.name,
      },
      {
        id: "kind",
        header: "Purpose",
        cell: (record) => adapterKindLabel(record.adapter_kind),
        searchValue: (record) => record.adapter_kind,
        sortValue: (record) => record.adapter_kind,
      },
      {
        id: "use",
        header: "Bindings",
        cell: (record) => adapterUseCount(record, tunnelPlans),
        searchValue: (record) => adapterUseCount(record, tunnelPlans),
        sortValue: (record) => adapterUseCount(record, tunnelPlans),
      },
      {
        id: "updated",
        header: "Updated",
        cell: (record) => formatTime(record.updated_at),
        searchValue: (record) => formatTime(record.updated_at),
        sortValue: (record) => record.updated_at,
      },
    ],
    [tunnelPlans],
  );
  const actions = useMemo<
    ConsoleDataGridAction<NetworkAdapterDefinitionRecord>[]
  >(
    () => [
      {
        label: "Edit details",
        icon: <Pencil size={14} />,
        onSelect: (rows) => openEdit(rows[0], "details"),
        disabled: (rows) => rows.length !== 1,
        description: () => "Edit name and description without changing commands or restarting resources.",
      },
      {
        label: "Edit commands",
        icon: <Pencil size={14} />,
        onSelect: (rows) => openEdit(rows[0], "commands"),
        disabled: (rows) => rows.length !== 1,
        description: () => "Review command changes and affected resources before privileged application.",
      },
      {
        label: "Delete",
        icon: <Trash2 size={14} />,
        tone: "danger",
        onSelect: (rows) => {
          setError(null);
          setFeedback(null);
          setDeleteTarget(rows[0]);
        },
        disabled: (rows) =>
          rows.length !== 1 || adapterUseCount(rows[0], tunnelPlans) > 0,
        description: (rows) =>
          rows.length === 1 && adapterUseCount(rows[0], tunnelPlans) > 0
            ? `Used by ${adapterUseCount(rows[0], tunnelPlans)} bindings; unbind it and finish pending cleanup first.`
            : "Delete this unreferenced adapter definition.",
      },
    ],
    [tunnelPlans],
  );

  return (
    <section
      aria-label="Network adapter definitions"
      className="tunnelPlansRegistryPanel"
    >
      {!editorOnly && (
        <>
          <div className="sectionHeader compact">
            <div>
              <h3>Adapter definitions</h3>
              <span>
                Operator-owned commands bound to tunnel plans and port-forward
                rules.
              </span>
            </div>
          </div>
          <ActionFeedback
            className="localActionFeedback"
            message={registryFeedbackMessage}
            ref={registryFeedbackRef}
            tone={error ? "danger" : hasDispatch ? "info" : "success"}
          />
          {hasDispatch && reviewControls.onOpenJobHistory && (
            <button className="secondaryAction compactAction" type="button" onClick={reviewControls.onOpenJobHistory}>View jobs</button>
          )}
          <ConsoleDataGrid
            actions={actions}
            columns={columns}
            empty={
              <div className="emptyState">
                <strong>No adapter definitions</strong>
                <span>
                  Agent builtin tunnels need no runtime adapter. Create one only
                  for custom tunnel, routing, or port-forward commands.
                </span>
              </div>
            }
            getRowId={(record) => record.id}
            itemLabel="adapter definitions"
            renderExpandedRow={(record) => (
              <div className="consoleInlineDetailGrid">
                <span>Purpose</span>
                <strong>{adapterKindLabel(record.adapter_kind)}</strong>
                <span>Used by plans</span>
                <strong>
                  {adapterPlanNames(record.id, tunnelPlans) || "None"}
                </strong>
                <span>Port-forward bindings</span>
                <strong>{record.port_forward_rule_count ?? 0}</strong>
                <span>Contract</span>
                <strong>
                  <pre>{JSON.stringify(record.definition, null, 2)}</pre>
                </strong>
              </div>
            )}
            rows={definitions}
            searchPlaceholder="Search adapter definitions"
            storageKey="vpsman.network.adapterDefinitions"
            title="Adapter definitions"
            toolbarActions={
              <div className="previewMeta">
                <button
                  className="secondaryAction compactAction"
                  onClick={() => openCreate("port_forward")}
                  title="Create a reusable adapter for custom port-forward services"
                  type="button"
                >
                  <Plus size={14} /> Port forwarding adapter
                </button>
                <button
                  className="secondaryAction compactAction"
                  onClick={() => openCreate("routing_cost")}
                  title="Create an adapter contract for reading and updating routing cost"
                  type="button"
                >
                  <Plus size={14} />
                  Routing cost adapter
                </button>
                <button
                  className="primaryAction compactAction"
                  onClick={() => openCreate("runtime_tunnel")}
                  title="Create an adapter contract for custom tunnel runtime commands"
                  type="button"
                >
                  <Plus size={14} />
                  Tunnel runtime adapter
                </button>
              </div>
            }
          />

          <ConfirmationPrompt
            confirmLabel="Delete adapter definition"
            detail="Delete this unused definition. Unbind tunnel plans and port-forward rules and finish pending cleanup first."
            error={error}
            items={
              deleteTarget
                ? [
                    { label: "Definition", value: deleteTarget.name },
                    {
                      label: "Purpose",
                      value: adapterKindLabel(deleteTarget.adapter_kind),
                    },
                  ]
                : []
            }
            onCancel={() => {
              setError(null);
              setDeleteTarget(null);
            }}
            onConfirm={() => void confirmDelete()}
            open={deleteTarget !== null}
            pending={pending}
            title="Delete adapter definition"
            tone="danger"
          />
        </>
      )}

      <ConsoleActionDrawer
        description={editor?.mode === "details"
          ? "Name and description only. Saving details does not run commands or restart resources."
          : "The agent invokes these exact absolute commands; vpsman does not install or modify them."}
        onClose={() => {
          draftRevision.current += 1;
          setReview(null);
          setError(null);
          setEditor(null);
          onEditorClosed?.();
        }}
        open={editor !== null}
        title={
          editor && editor.mode !== "create"
            ? `Edit ${editor.mode}: ${editor.definition.name}`
            : `New ${adapterKindLabel(kind).toLowerCase()}`
        }
      >
        <form className="compactForm structuredDefinitionForm" onSubmit={save}>
          <ActionFeedback
            message={editor ? error : null}
            ref={editorFeedbackRef}
            tone="danger"
          />
          <div className="formRow">
            <label
              title={
                editor?.mode !== "create"
                  ? "Adapter purpose is immutable after creation; create another definition for a different purpose"
                  : "Choose whether this adapter manages a tunnel runtime, routing cost, or port forwarding"
              }
            >
              <span>Purpose</span>
              <select
                aria-label="Adapter purpose"
                data-tooltip-disabled-reason="Adapter purpose is immutable after creation; create another definition for a different purpose."
                disabled={editor?.mode !== "create" || editorOnly}
                onChange={(event) => {
                  const nextKind = event.target.value as NetworkAdapterKind;
                  changeEditorDraft(() => {
                    setKind(nextKind);
                    setDefinition(defaultAdapterDefinition(nextKind));
                  });
                }}
                value={kind}
              >
                <option value="runtime_tunnel">Tunnel runtime</option>
                <option value="routing_cost">Routing cost</option>
                <option value="port_forward">Port forwarding</option>
              </select>
            </label>
            <label>
              <span>Name</span>
              <input
                aria-label="Adapter definition name"
                readOnly={editor?.mode === "commands"}
                onChange={(event) =>
                  changeEditorDraft(() => setName(event.target.value))
                }
                value={editor?.mode === "commands"
                  ? (definitions.find((record) => record.id === editor.definition.id) ?? editor.definition).name
                  : name}
              />
            </label>
          </div>
          <label>
            <span>Description</span>
            <input
              aria-label="Adapter definition description"
              readOnly={editor?.mode === "commands"}
              onChange={(event) =>
                changeEditorDraft(() => setDescription(event.target.value))
              }
              value={editor?.mode === "commands"
                ? (definitions.find((record) => record.id === editor.definition.id) ?? editor.definition).description ?? ""
                : description}
            />
          </label>
          {editor?.mode !== "details" && <AdapterCommandFields
            definition={definition}
            kind={kind}
            onChange={(nextDefinition) =>
              changeEditorDraft(() => setDefinition(nextDefinition))
            }
          />}
          {editor?.mode !== "details" && <details>
            <summary>Advanced contract preview</summary>
            <pre>{JSON.stringify(definition, null, 2)}</pre>
          </details>}
          {editor && feedback && <ActionFeedback message={feedback} tone="info" />}
          <button
            className="primaryAction"
            disabled={pending || !name.trim()}
            title={
              pending
                ? "Wait for the current adapter definition operation to finish"
                : !name.trim()
                  ? "Enter an adapter definition name before saving"
                  : editor?.mode === "commands"
                    ? "Preview the exact changed commands and affected resources"
                    : editor?.mode === "details"
                      ? "Save name and description without changing commands or runtime resources"
                    : "Create this adapter definition"
            }
            type="submit"
          >
            {editor?.mode === "details"
              ? "Save details"
              : editor?.mode === "commands"
                ? "Review changes"
              : "Create adapter definition"}
          </button>
        </form>
      </ConsoleActionDrawer>
      <ConfirmationPrompt
        confirmDisabled={!reviewControls.privilegeMaterial}
        confirmLabel="Apply adapter changes"
        detail={review?.request.adapter_kind === "routing_cost"
          ? "Save the reviewed routing-cost contract. Existing OSPF status/review and automatic cost workflows use the new commands; this does not clean up tunnels or immediately apply a cost."
          : "Each affected agent attempts cleanup with its old commands, installs the latest definition, then starts enabled resources with the new commands. Expect downtime. Cleanup or startup failure does not roll back the definition; inspect the job and repair any residue. Offline agents adopt changes on reconnect. Disabled bindings stay disabled."}
        error={error}
        items={review ? [
          { label: "Adapter", value: review.request.name },
          { label: "Purpose", value: adapterKindLabel(review.request.adapter_kind) },
        ] : []}
        onCancel={() => setReview(null)}
        onConfirm={() => void applyCommands()}
        open={review !== null}
        pending={pending}
        title="Review adapter command changes"
        tone="warning"
      >
        {review && (
          <div
            aria-label="Affected adapter resources"
            className="configurationReviewList"
            role="group"
            tabIndex={0}
          >
            <strong>Affected resources</strong>
            {review.preview.affected_resources.length
              ? review.preview.affected_resources.map((resource) => (
                <span key={`${resource.kind}:${resource.resource_id}`}>
                  <strong>{resource.resource_name}</strong>
                  <span>
                    {adapterKindLabel(resource.kind)}; {resource.enabled ? "enabled" : "disabled"}
                    {resource.cleanup_pending ? "; cleanup pending" : ""}
                  </span>
                  <span>{resource.client_ids.map((id) => {
                    const endpoint = review.endpoints[id];
                    const waiting = resource.kind !== "routing_cost" &&
                      (resource.enabled || resource.cleanup_pending) && !endpoint?.online;
                    return `${endpoint?.label ?? id}${waiting ? "; waiting for agent" : ""}`;
                  }).join("; ")}</span>
                </span>
              ))
              : <span>No bindings; no runtime resources will be restarted.</span>}
          </div>
        )}
        {review && !reviewControls.privilegeMaterial && (
          <PrivilegeVaultBox
            labelPrefix="Adapter commands"
            lastPayloadHash={review.preview.review_hash}
            onOpenUnlock={reviewControls.onOpenPrivilegeUnlock}
            onPrivilegeMaterialChange={reviewControls.setPrivilegeMaterial}
            privilegeMaterial={reviewControls.privilegeMaterial}
            showVaultClear={false}
            usePrivilegeLabel="Unlock adapter apply"
          />
        )}
      </ConfirmationPrompt>
    </section>
  );
}

function AdapterCommandFields({
  definition,
  kind,
  onChange,
}: {
  definition: Record<string, JsonValue>;
  kind: NetworkAdapterKind;
  onChange: (definition: Record<string, JsonValue>) => void;
}) {
  const fields =
    kind === "runtime_tunnel"
      ? [
          {
            field: "status_command",
            label: "Status",
            hint: "required",
            required: true,
          },
          {
            field: "startup_command",
            label: "Start",
            hint: "provide Start or Restart",
            required: false,
          },
          {
            field: "restart_command",
            label: "Restart",
            hint: "provide Start or Restart",
            required: false,
          },
          {
            field: "stop_command",
            label: "Stop",
            hint: "provide Stop or Cleanup",
            required: false,
          },
          {
            field: "cleanup_command",
            label: "Cleanup",
            hint: "provide Stop or Cleanup",
            required: false,
          },
          {
            field: "traffic_limit_command",
            label: "Apply traffic limit",
            hint: "optional",
            required: false,
          },
        ]
      : kind === "port_forward"
        ? [
            {
              field: "apply_command",
              label: "Apply",
              hint: "required",
              required: true,
            },
            {
              field: "remove_command",
              label: "Remove",
              hint: "required",
              required: true,
            },
            {
              field: "status_command",
              label: "Status",
              hint: "required",
              required: true,
            },
          ]
        : [
            {
              field: "status_command",
              label: "Read cost",
              hint: "required",
              required: true,
            },
            {
              field: "update_command",
              label: "Update cost",
              hint: "required",
              required: true,
            },
          ];
  return (
    <div className="compactForm">
      <strong>Commands</strong>
      <span className="formHint">
        Enter one argument per line; each line is passed as one exact argv
        value, and quote characters are literal. The first line must be an
        absolute executable path.
        {kind === "runtime_tunnel"
          ? " Tunnel runtimes require Status, one of Start or Restart, and one of Stop or Cleanup."
          : kind === "port_forward"
            ? " Port forwarding requires idempotent Apply, Remove, and Status commands. Commands manage services and return; they do not run as the listener."
            : " Routing cost adapters require both Read cost and Update cost."}
      </span>
      <span className="formHint">
        {kind === "runtime_tunnel"
          ? "New definitions contain editable examples. Replace the executable and argument layout for your adapter; values such as {interface}, {remote_underlay}, and {local_address} are replaced from each endpoint's tunnel plan."
          : kind === "port_forward"
            ? 'Use {forwarding_type} for port_mapping or upstream_pool. Every command requires {rule_config_json}, passed as one JSON argument containing contract_version, client_id, config_hash, and rule (ID, revision, protocol, and mapping pairs or pool ports, upstreams, strategy, failure and retry settings). Status returns JSON with state applied, absent, or drifted; applied also returns the request config_hash. Settings follow the forwarding contract; the adapter owns listener collisions and reports execution failures. See docs/port-forwarding.md for the full JSON structure and command contract.'
            : "New definitions contain editable examples. vpsman replaces {plan_id}, {interface}, {endpoint_side}, and {desired_cost} in direct argv and sends no stdin. Read cost must print one number from 1 to 65535. Update reports failure by exit code; its output is retained as the message, then vpsman reads the cost again to verify it."}
      </span>
      {fields.map(({ field, hint, label, required }) => {
        const command = asObject(definition[field]);
        return (
          <div className="compactForm" key={field}>
            <label
              title={`Configure the ${label.toLowerCase()} adapter command arguments.`}
            >
              <span>
                {label}
                {` (${hint})`}
              </span>
              <NumberedTextarea
                aria-label={`${label} adapter command`}
                onChange={(event) => {
                  const argv = lines(event.target.value);
                  const next = { ...definition };
                  if (argv.length === 0 && !required) {
                    delete next[field];
                  } else {
                    next[field] = {
                      argv,
                      max_timeout_secs: number(command.max_timeout_secs, 30),
                      max_output_bytes: number(command.max_output_bytes, 16384),
                    };
                  }
                  onChange(next);
                }}
                placeholder={"/absolute/path/to/executable\n--argument"}
                value={strings(command.argv).join("\n")}
              />
            </label>
            {strings(command.argv).length > 0 || required ? (
              <div className="formRow">
                <label title="Hard wall-clock limit for each adapter command invocation. A timed-out invocation is reported as a failure.">
                  <span>Timeout seconds</span>
                  <input
                    aria-label={`${label} timeout seconds`}
                    max={120}
                    min={1}
                    onChange={(event) =>
                      onChange({
                        ...definition,
                        [field]: {
                          ...command,
                          argv: strings(command.argv),
                          max_timeout_secs: Number(event.target.value),
                          max_output_bytes: number(
                            command.max_output_bytes,
                            16384,
                          ),
                        },
                      })
                    }
                    type="number"
                    value={number(command.max_timeout_secs, 30)}
                  />
                </label>
                <label title="Maximum command output retained as adapter evidence. It does not limit what the process can write elsewhere.">
                  <span>Maximum output bytes</span>
                  <input
                    aria-label={`${label} maximum output bytes`}
                    max={65536}
                    min={1024}
                    onChange={(event) =>
                      onChange({
                        ...definition,
                        [field]: {
                          ...command,
                          argv: strings(command.argv),
                          max_timeout_secs: number(
                            command.max_timeout_secs,
                            30,
                          ),
                          max_output_bytes: Number(event.target.value),
                        },
                      })
                    }
                    step={1024}
                    type="number"
                    value={number(command.max_output_bytes, 16384)}
                  />
                </label>
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

function defaultAdapterDefinition(
  kind: NetworkAdapterKind,
): Record<string, JsonValue> {
  const command = (...argv: string[]) => ({
    argv,
    max_timeout_secs: 30,
    max_output_bytes: 16384,
  });
  if (kind === "port_forward") {
    const operation = (action: string) =>
      command(
        "/opt/operator/port-forward-adapter",
        action,
        "{forwarding_type}",
        "{rule_config_json}",
      );
    return {
      contract_version: 2,
      apply_command: operation("apply"),
      remove_command: operation("remove"),
      status_command: operation("status"),
    };
  }
  if (kind === "routing_cost") {
    return {
      contract_version: 2,
      status_command: command(
        "/opt/operator/routing-cost",
        "status",
        "--plan-id",
        "{plan_id}",
        "--interface",
        "{interface}",
        "--side",
        "{endpoint_side}",
      ),
      update_command: command(
        "/opt/operator/routing-cost",
        "apply",
        "--plan-id",
        "{plan_id}",
        "--interface",
        "{interface}",
        "--side",
        "{endpoint_side}",
        "--cost",
        "{desired_cost}",
      ),
    };
  }
  return {
    manager: "custom_adapter",
    contract_version: 1,
    startup_command: command(
      "/opt/operator/tunnel-adapter",
      "start",
      "--interface",
      "{interface}",
      "--kind",
      "{kind}",
      "--remote-underlay",
      "{remote_underlay}",
      "--local-address",
      "{local_address}/{prefix_len}",
      "--remote-address",
      "{remote_address}",
    ),
    cleanup_command: command(
      "/opt/operator/tunnel-adapter",
      "cleanup",
      "--interface",
      "{interface}",
    ),
    status_command: command(
      "/opt/operator/tunnel-adapter",
      "status",
      "--interface",
      "{interface}",
    ),
  };
}

function adapterKindLabel(kind: NetworkAdapterKind): string {
  return kind === "runtime_tunnel"
    ? "Tunnel runtime adapter"
    : kind === "port_forward"
      ? "Port forwarding adapter"
      : "Routing cost adapter";
}

function adapterUseCount(
  record: NetworkAdapterDefinitionRecord,
  plans: TunnelPlanRecord[],
): number {
  return (
    plans.filter((plan) => planUsesAdapter(plan, record.id)).length +
    (record.port_forward_rule_count ?? 0)
  );
}

function adapterPlanNames(id: string, plans: TunnelPlanRecord[]): string {
  return plans
    .filter((plan) => planUsesAdapter(plan, id))
    .map((plan) => plan.name)
    .sort()
    .join(", ");
}

function planUsesAdapter(plan: TunnelPlanRecord, id: string): boolean {
  return [
    plan.plan.runtime_control?.left_adapter_template_id,
    plan.plan.runtime_control?.right_adapter_template_id,
    plan.plan.ospf?.left_adapter_template_id,
    plan.plan.ospf?.right_adapter_template_id,
  ].includes(id);
}

function asObject(value: JsonValue | undefined): Record<string, JsonValue> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? { ...value }
    : {};
}

function strings(value: JsonValue | undefined): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
}

function lines(value: string): string[] {
  return value === "" ? [] : value.split("\n");
}

function number(value: JsonValue | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function validateAdapterDefinition(
  kind: NetworkAdapterKind,
  definition: Record<string, JsonValue>,
): string | null {
  const commands =
    kind === "runtime_tunnel"
      ? [["status_command", "Status"]]
      : kind === "port_forward"
        ? [
            ["apply_command", "Apply"],
            ["remove_command", "Remove"],
            ["status_command", "Status"],
          ]
        : [
            ["status_command", "Read cost"],
            ["update_command", "Update cost"],
          ];
  for (const [field, label] of commands) {
    const error = validateAdapterCommand(definition[field], label);
    if (error) return error;
  }
  if (kind === "runtime_tunnel") {
    if (
      !hasAdapterCommand(definition.startup_command) &&
      !hasAdapterCommand(definition.restart_command)
    ) {
      return "Provide either a Start command or a Restart command";
    }
    if (
      !hasAdapterCommand(definition.stop_command) &&
      !hasAdapterCommand(definition.cleanup_command)
    ) {
      return "Provide either a Stop command or a Cleanup command";
    }
  }
  for (const [field, label] of [
    ["startup_command", "Start"],
    ["stop_command", "Stop"],
    ["restart_command", "Restart"],
    ["cleanup_command", "Cleanup"],
    ["traffic_limit_command", "Apply traffic limit"],
  ]) {
    if (hasAdapterCommand(definition[field])) {
      const error = validateAdapterCommand(definition[field], label);
      if (error) return error;
    }
  }
  return null;
}

function hasAdapterCommand(value: JsonValue | undefined): boolean {
  return strings(asObject(value).argv).length > 0;
}

function validateAdapterCommand(
  value: JsonValue | undefined,
  label: string,
): string | null {
  const command = asObject(value);
  const argv = strings(command.argv);
  if (argv.length === 0) return `${label} command is required`;
  if (argv.some((argument) => argument.length === 0)) {
    return `${label} command cannot contain an empty argument line`;
  }
  if (!argv[0].startsWith("/")) {
    return `${label} command must start with an absolute executable path`;
  }
  const timeout = command.max_timeout_secs;
  if (
    typeof timeout !== "number" ||
    !Number.isInteger(timeout) ||
    timeout < 1 ||
    timeout > 120
  ) {
    return `${label} timeout must be a whole number from 1 to 120 seconds`;
  }
  const output = command.max_output_bytes;
  if (
    typeof output !== "number" ||
    !Number.isInteger(output) ||
    output < 1024 ||
    output > 65536
  ) {
    return `${label} maximum output must be 1024 to 65536 bytes`;
  }
  return null;
}
