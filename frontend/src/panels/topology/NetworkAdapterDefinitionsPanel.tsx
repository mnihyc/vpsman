import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { ActionFeedback } from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { ConsoleActionDrawer } from "../../components/ConsoleLayout";
import type {
  JsonValue,
  NetworkAdapterDefinitionRecord,
  NetworkAdapterKind,
  TunnelPlanRecord,
  UpsertNetworkAdapterDefinitionRequest,
} from "../../types";
import { formatTime, runPanelAction } from "../../utils";

type EditorState =
  | { mode: "create"; kind: NetworkAdapterKind }
  | { mode: "edit"; definition: NetworkAdapterDefinitionRecord }
  | null;

export function NetworkAdapterDefinitionsPanel({
  definitions,
  initialKind,
  onCreate,
  onDelete,
  onInitialKindConsumed,
  onUpdate,
  tunnelPlans,
}: {
  definitions: NetworkAdapterDefinitionRecord[];
  initialKind: NetworkAdapterKind | null;
  onCreate: (
    request: UpsertNetworkAdapterDefinitionRequest,
  ) => Promise<NetworkAdapterDefinitionRecord>;
  onDelete: (definitionId: string) => Promise<void>;
  onInitialKindConsumed: () => void;
  onUpdate: (
    definitionId: string,
    request: UpsertNetworkAdapterDefinitionRequest,
  ) => Promise<NetworkAdapterDefinitionRecord>;
  tunnelPlans: TunnelPlanRecord[];
}) {
  const [editor, setEditor] = useState<EditorState>(null);
  const [deleteTarget, setDeleteTarget] =
    useState<NetworkAdapterDefinitionRecord | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [kind, setKind] = useState<NetworkAdapterKind>("runtime_tunnel");
  const [definition, setDefinition] = useState<Record<string, JsonValue>>(
    () => defaultAdapterDefinition("runtime_tunnel"),
  );
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);

  useEffect(() => {
    if (!initialKind) return;
    openCreate(initialKind);
    onInitialKindConsumed();
  }, [initialKind, onInitialKindConsumed]);

  function openCreate(adapterKind: NetworkAdapterKind) {
    setKind(adapterKind);
    setName("");
    setDescription("");
    setDefinition(defaultAdapterDefinition(adapterKind));
    setError(null);
    setEditor({ mode: "create", kind: adapterKind });
  }

  function openEdit(record: NetworkAdapterDefinitionRecord) {
    setKind(record.adapter_kind);
    setName(record.name);
    setDescription(record.description ?? "");
    setDefinition(asObject(record.definition));
    setError(null);
    setEditor({ mode: "edit", definition: record });
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    await runPanelAction(setPending, setError, async () => {
      if (!name.trim()) throw new Error("Adapter name is required");
      const definitionError = validateAdapterDefinition(kind, definition);
      if (definitionError) throw new Error(definitionError);
      const request: UpsertNetworkAdapterDefinitionRequest = {
        adapter_kind: kind,
        name: name.trim(),
        description: description.trim() || null,
        definition,
      };
      if (editor?.mode === "edit") {
        await onUpdate(editor.definition.id, request);
        setFeedback(`Updated ${name.trim()}`);
      } else {
        await onCreate(request);
        setFeedback(`Created ${name.trim()}`);
      }
      setEditor(null);
    });
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
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
        searchValue: (record) =>
          `${record.name} ${record.description ?? ""}`,
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
        header: "Plan use",
        cell: (record) => adapterUseCount(record.id, tunnelPlans),
        searchValue: (record) => adapterUseCount(record.id, tunnelPlans),
        sortValue: (record) => adapterUseCount(record.id, tunnelPlans),
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
        label: "Edit",
        icon: <Pencil size={14} />,
        onSelect: (rows) => openEdit(rows[0]),
        disabled: (rows) =>
          rows.length !== 1 || adapterUseCount(rows[0].id, tunnelPlans) > 0,
        description: (rows) =>
          rows.length === 1 &&
          adapterUseCount(rows[0].id, tunnelPlans) > 0
            ? `Used by ${adapterUseCount(rows[0].id, tunnelPlans)} tunnel plans; create a replacement and change each plan explicitly.`
            : "Edit this unreferenced adapter definition.",
      },
      {
        label: "Delete",
        icon: <Trash2 size={14} />,
        tone: "danger",
        onSelect: (rows) => {
          setError(null);
          setDeleteTarget(rows[0]);
        },
        disabled: (rows) =>
          rows.length !== 1 || adapterUseCount(rows[0].id, tunnelPlans) > 0,
        description: (rows) =>
          rows.length === 1 &&
          adapterUseCount(rows[0].id, tunnelPlans) > 0
            ? `Used by ${adapterUseCount(rows[0].id, tunnelPlans)} tunnel plans; unbind it from every plan first.`
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
      <div className="sectionHeader compact">
        <div>
          <h3>Adapter definitions</h3>
          <span>
            Operator-owned commands bound only from explicit tunnel-plan
            endpoints.
          </span>
        </div>
      </div>
      <ActionFeedback
        className="localActionFeedback"
        message={editor === null && deleteTarget === null ? error : null}
        tone="danger"
      />
      <ActionFeedback
        className="localActionFeedback"
        message={feedback}
        tone="success"
      />
      <ConsoleDataGrid
        actions={actions}
        columns={columns}
        empty={
          <div className="emptyState">
            <strong>No adapter definitions</strong>
            <span>
              Agent-managed tunnels need none. Create one only for an external
              runtime or routing daemon.
            </span>
          </div>
        }
        getRowId={(record) => record.id}
        itemLabel="adapter definitions"
        onOpenRow={(record) => {
          if (adapterUseCount(record.id, tunnelPlans) === 0) {
            openEdit(record);
          }
        }}
        showMobileOpenRowAction={false}
        renderExpandedRow={(record) => (
          <div className="consoleInlineDetailGrid">
            <span>Purpose</span>
            <strong>{adapterKindLabel(record.adapter_kind)}</strong>
            <span>Used by plans</span>
            <strong>
              {adapterPlanNames(record.id, tunnelPlans) || "None"}
            </strong>
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
              onClick={() => openCreate("routing_cost")}
              type="button"
            >
              <Plus size={14} />
              Routing cost adapter
            </button>
            <button
              className="primaryAction compactAction"
              onClick={() => openCreate("runtime_tunnel")}
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
        detail="Delete this unused definition. Definitions bound to any tunnel plan must be unbound first."
        error={deleteTarget ? error : null}
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

      <ConsoleActionDrawer
        description="The agent invokes these exact absolute commands; vpsman does not install or modify them."
        onClose={() => {
          setError(null);
          setEditor(null);
        }}
        open={editor !== null}
        title={
          editor?.mode === "edit"
            ? `Edit ${editor.definition.name}`
            : `New ${adapterKindLabel(kind).toLowerCase()}`
        }
      >
        <form className="compactForm structuredDefinitionForm" onSubmit={save}>
          <ActionFeedback message={editor ? error : null} tone="danger" />
          <div className="formRow">
            <label>
              <span>Purpose</span>
              <select
                aria-label="Adapter purpose"
                disabled={editor?.mode === "edit"}
                onChange={(event) => {
                  const nextKind = event.target.value as NetworkAdapterKind;
                  setKind(nextKind);
                  setDefinition(defaultAdapterDefinition(nextKind));
                }}
                value={kind}
              >
                <option value="runtime_tunnel">Tunnel runtime</option>
                <option value="routing_cost">Routing cost</option>
              </select>
            </label>
            <label>
              <span>Name</span>
              <input
                aria-label="Adapter definition name"
                onChange={(event) => setName(event.target.value)}
                value={name}
              />
            </label>
          </div>
          <label>
            <span>Description</span>
            <input
              aria-label="Adapter definition description"
              onChange={(event) => setDescription(event.target.value)}
              value={description}
            />
          </label>
          <AdapterCommandFields
            definition={definition}
            kind={kind}
            onChange={setDefinition}
          />
          <details>
            <summary>Advanced contract preview</summary>
            <pre>{JSON.stringify(definition, null, 2)}</pre>
          </details>
          <button
            className="primaryAction"
            disabled={pending || !name.trim()}
            type="submit"
          >
            {editor?.mode === "edit"
              ? "Save adapter definition"
              : "Create adapter definition"}
          </button>
        </form>
      </ConsoleActionDrawer>
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
        Enter one argument per line. The first line must be an absolute
        executable path. Tunnel runtimes require Status, one of Start or
        Restart, and one of Stop or Cleanup.
      </span>
      {fields.map(({ field, hint, label, required }) => {
        const command = asObject(definition[field]);
        return (
          <div className="compactForm" key={field}>
            <label>
              <span>
                {label}
                {` (${hint})`}
              </span>
              <textarea
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
                      max_output_bytes: number(
                        command.max_output_bytes,
                        16384,
                      ),
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
  const command = () => ({
    argv: [],
    max_timeout_secs: 30,
    max_output_bytes: 16384,
  });
  if (kind === "routing_cost") {
    return {
      contract_version: 1,
      status_command: command(),
      update_command: command(),
    };
  }
  return {
    manager: "external_managed_adapter",
    contract_version: 1,
    status_command: command(),
  };
}

function adapterKindLabel(kind: NetworkAdapterKind): string {
  return kind === "runtime_tunnel"
    ? "Tunnel runtime adapter"
    : "Routing cost adapter";
}

function adapterUseCount(id: string, plans: TunnelPlanRecord[]): number {
  return plans.filter((plan) => planUsesAdapter(plan, id)).length;
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
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function number(value: JsonValue | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : fallback;
}

function validateAdapterDefinition(
  kind: NetworkAdapterKind,
  definition: Record<string, JsonValue>,
): string | null {
  const commands =
    kind === "runtime_tunnel"
      ? [["status_command", "Status"]]
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
