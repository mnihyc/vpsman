import {
  CheckCircle2,
  CirclePlus,
  Copy,
  Info,
  Pencil,
  Power,
  PowerOff,
  RefreshCcw,
  RotateCw,
  ShieldAlert,
  Trash2,
  X,
} from "lucide-react";
import {
  type FormEvent,
  forwardRef,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { VpsCombobox } from "../../components/VpsCombobox";
import { scrollIntoViewWithMotion } from "../../motion";
import {
  formatPortMappings,
  formatPortRange,
  mappingsToExpressions,
  pairPortExpressions,
} from "../../portForwarding";
import type {
  AgentView,
  CreatePortForwardRuleRequest,
  PortForwardBulkAction,
  PortForwardBulkResponse,
  PortForwardMutationResponse,
  PortForwardProtocol,
  PortForwardRuleCorruptRecord,
  PortForwardRuleListItem,
  PortForwardRuleRecord,
  ResolveHostnameResponse,
  UpdatePortForwardRuleRequest,
} from "../../types";
import { dispatchFailureReason, formatCompactTime, shortId } from "../../utils";

type PortForwardingPanelProps = {
  agents: AgentView[];
  canForget: boolean;
  canWrite: boolean;
  error: string | null;
  loading: boolean;
  onBulkMutate: (
    action: PortForwardBulkAction,
    items: Array<{ id: string; expected_revision: number }>,
    reason?: string,
  ) => Promise<PortForwardBulkResponse>;
  onCreate: (
    request: CreatePortForwardRuleRequest,
  ) => Promise<PortForwardMutationResponse>;
  onLoad: () => Promise<string | null>;
  onMutate: (
    ruleId: string,
    operation: "enable" | "disable" | "delete" | "forget" | "reapply",
    request: {
      expected_revision: number;
      confirmed: boolean;
      reason?: string | null;
    },
  ) => Promise<PortForwardMutationResponse>;
  onResolveHostname: (hostname: string) => Promise<ResolveHostnameResponse>;
  onUpdate: (
    ruleId: string,
    request: UpdatePortForwardRuleRequest,
  ) => Promise<PortForwardMutationResponse>;
  rules: PortForwardRuleListItem[];
};

type EditorDraft = {
  enabled: boolean;
  incoming: string;
  masquerade: boolean;
  name: string;
  protocol: PortForwardProtocol;
  target: string;
  targetInput: string;
  targetIp: string;
  clientId: string;
};

type ConfirmationState =
  | { kind: "save"; draft: EditorDraft; editing: PortForwardRuleRecord | null }
  | {
      kind: "single";
      operation: "enable" | "disable" | "delete" | "forget" | "reapply";
      origin: "detail" | "registry";
      reason?: string;
      rule: PortForwardRuleRecord;
    }
  | {
      kind: "bulk";
      action: PortForwardBulkAction;
      rules: PortForwardRuleRecord[];
    };

type FeedbackContent = {
  message: string;
  tone: ActionFeedbackTone;
};

type Feedback = FeedbackContent & {
  anchor: "detail" | "editor" | "registry" | "summary";
};

const EMPTY_DRAFT: EditorDraft = {
  clientId: "",
  enabled: false,
  incoming: "",
  masquerade: true,
  name: "",
  protocol: "tcp",
  target: "",
  targetInput: "",
  targetIp: "",
};
const MAX_RULE_NAME_BYTES = 128;

export function PortForwardingPanel({
  agents,
  canForget,
  canWrite,
  error,
  loading,
  onBulkMutate,
  onCreate,
  onLoad,
  onMutate,
  onResolveHostname,
  onUpdate,
  rules: ruleItems,
}: PortForwardingPanelProps) {
  const [editor, setEditor] = useState<{
    draft: EditorDraft;
    editing: PortForwardRuleRecord | null;
  } | null>(null);
  const [confirmation, setConfirmation] = useState<ConfirmationState | null>(
    null,
  );
  const [pending, setPending] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [forgetReason, setForgetReason] = useState("");
  const [corruptDelete, setCorruptDelete] =
    useState<PortForwardRuleCorruptRecord | null>(null);
  const editorRef = useRef<HTMLElement | null>(null);
  const registryFeedbackRef = useRef<HTMLDivElement | null>(null);
  const writeBoundary = "Operator role and network:write scope required";
  const forgetBoundary = "Admin role and network:write scope required";
  const corruptRules = ruleItems.filter(isCorruptPortForwardRule);
  const rules = ruleItems.filter(isHealthyPortForwardRule);

  const agentById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent])),
    [agents],
  );
  const enabledCount = rules.filter(
    (rule) => rule.enabled && !rule.deleted_at,
  ).length;
  const appliedCount = rules.filter((rule) =>
    ["applied", "applied_warning"].includes(rule.runtime_status),
  ).length;
  const pendingCount = rules.filter(
    (rule) => rule.runtime_status === "pending",
  ).length;
  const attentionCount = rules.filter((rule) =>
    [
      "applied_warning",
      "drifted",
      "failed",
      "unsupported",
      "removal_pending",
      "unknown",
    ].includes(rule.runtime_status),
  ).length;
  const supportedAgents = agents.filter(
    (agent) => agent.capabilities.port_forwarding?.status === "supported",
  ).length;

  async function executeCorruptDelete(rule: PortForwardRuleCorruptRecord) {
    setPending(true);
    setFeedback(null);
    try {
      const response = await onMutate(rule.id, "delete", {
        confirmed: true,
        expected_revision: rule.revision,
        reason: "retire_corrupt_persisted_configuration",
      });
      setCorruptDelete(null);
      setFeedback({
        ...syncFeedback(response, `Deleted corrupt rule ${rule.name}`),
        anchor: "registry",
      });
    } catch (actionError) {
      setFeedback({
        anchor: "registry",
        message:
          actionError instanceof Error
            ? actionError.message
            : "Corrupt rule deletion failed",
        tone: "danger",
      });
    } finally {
      setPending(false);
    }
  }

  useEffect(() => {
    if (!editor) return;
    window.setTimeout(() => {
      if (editorRef.current) {
        scrollIntoViewWithMotion(editorRef.current, { block: "start" });
      }
      editorRef.current
        ?.querySelector<HTMLElement>("form input, form select, form button")
        ?.focus({ preventScroll: true });
    }, 0);
  }, [Boolean(editor), editor?.editing?.id]);

  useEffect(() => {
    if (feedback?.anchor !== "registry") return;
    const frame = window.requestAnimationFrame(() => {
      if (registryFeedbackRef.current) {
        scrollIntoViewWithMotion(registryFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [feedback]);

  function openCreate() {
    if (!canWrite) return;
    setFeedback(null);
    setEditor({
      editing: null,
      draft: { ...EMPTY_DRAFT },
    });
  }

  function openEdit(rule: PortForwardRuleRecord) {
    if (!canWrite) return;
    const expressions = mappingsToExpressions(rule.mappings);
    setFeedback(null);
    setEditor({
      editing: rule,
      draft: {
        clientId: rule.client_id,
        enabled: rule.enabled,
        incoming: expressions.incoming,
        masquerade: rule.masquerade,
        name: rule.name,
        protocol: rule.protocol,
        target: expressions.target,
        targetInput: rule.target_ip,
        targetIp: rule.target_ip,
      },
    });
  }

  function openClone(rule: PortForwardRuleRecord) {
    if (!canWrite) return;
    const expressions = mappingsToExpressions(rule.mappings);
    setFeedback(null);
    setEditor({
      editing: null,
      draft: {
        clientId: rule.client_id,
        enabled: false,
        incoming: expressions.incoming,
        masquerade: rule.masquerade,
        name: nextCloneName(rule.name, rules, rule.client_id),
        protocol: rule.protocol,
        target: expressions.target,
        targetInput: rule.target_ip,
        targetIp: rule.target_ip,
      },
    });
  }

  async function executeConfirmation(snapshot: ConfirmationState) {
    const actionAnchor =
      snapshot.kind === "save"
        ? "editor"
        : snapshot.kind === "single"
          ? snapshot.origin
          : "registry";
    if (
      !canWrite ||
      (snapshot.kind === "single" &&
        snapshot.operation === "forget" &&
        !canForget)
    ) {
      setFeedback({
        anchor: actionAnchor,
        message:
          snapshot.kind === "single" && snapshot.operation === "forget"
            ? forgetBoundary
            : writeBoundary,
        tone: "danger",
      });
      return;
    }
    setPending(true);
    setFeedback({
      anchor: actionAnchor,
      message: actionProgressLabel(snapshot),
      tone: "progress",
    });
    try {
      if (snapshot.kind === "save") {
        const response = await saveDraft(
          snapshot.draft,
          snapshot.editing,
          true,
        );
        setEditor(null);
        setFeedback({
          ...syncFeedback(
            response,
            snapshot.editing ? "Rule updated" : "Rule created",
          ),
          anchor: "registry",
        });
      } else if (snapshot.kind === "single") {
        const response = await onMutate(snapshot.rule.id, snapshot.operation, {
          confirmed: true,
          expected_revision: snapshot.rule.revision,
          reason: snapshot.reason,
        });
        setFeedback({
          ...syncFeedback(response, singleActionPast(snapshot.operation)),
          anchor:
            snapshot.operation === "delete" || snapshot.operation === "forget"
              ? "registry"
              : snapshot.origin,
        });
      } else {
        const response = await onBulkMutate(
          snapshot.action,
          snapshot.rules.map((rule) => ({
            id: rule.id,
            expected_revision: rule.revision,
          })),
        );
        setFeedback({
          ...bulkSyncFeedback(response, snapshot.action, snapshot.rules.length),
          anchor: "registry",
        });
      }
      setConfirmation(null);
    } catch (actionError) {
      if (snapshot.kind === "save") {
        setEditor({ draft: snapshot.draft, editing: snapshot.editing });
      }
      setFeedback({
        anchor: actionAnchor,
        message:
          actionError instanceof Error
            ? actionError.message
            : "Port-forwarding action failed",
        tone: "danger",
      });
    } finally {
      setPending(false);
    }
  }

  async function saveDraft(
    draft: EditorDraft,
    editing: PortForwardRuleRecord | null,
    confirmed: boolean,
  ) {
    const mappings = pairPortExpressions(draft.incoming, draft.target);
    if (!draft.targetIp)
      throw new Error("Resolve and select a literal target IP");
    const base = {
      confirmed,
      enabled: draft.enabled,
      mappings,
      masquerade: draft.masquerade,
      name: draft.name.trim(),
      protocol: draft.protocol,
      target_ip: draft.targetIp,
    };
    return editing
      ? onUpdate(editing.id, {
          ...base,
          expected_revision: editing.revision,
        })
      : onCreate({ ...base, client_id: draft.clientId });
  }

  async function submitEditor(event: FormEvent) {
    event.preventDefault();
    if (!editor || pending || !canWrite) return;
    try {
      validateEditor(editor.draft, agentById.get(editor.draft.clientId));
      pairPortExpressions(editor.draft.incoming, editor.draft.target);
      if (editor.draft.enabled || editor.editing?.enabled) {
        setFeedback(null);
        setConfirmation({ kind: "save", ...editor });
        return;
      }
      setPending(true);
      setFeedback({
        anchor: "editor",
        message: "Saving disabled rule",
        tone: "progress",
      });
      const response = await saveDraft(editor.draft, editor.editing, false);
      setEditor(null);
      setFeedback({
        ...syncFeedback(
          response,
          editor.editing ? "Rule updated" : "Rule created",
        ),
        anchor: "registry",
      });
    } catch (actionError) {
      if (editor) setEditor(editor);
      setFeedback({
        anchor: "editor",
        message:
          actionError instanceof Error
            ? actionError.message
            : "Rule is invalid",
        tone: "danger",
      });
    } finally {
      setPending(false);
    }
  }

  async function refreshRules() {
    setFeedback({
      anchor: "registry",
      message: "Reloading stored forwarding state",
      tone: "progress",
    });
    const refreshError = await onLoad();
    if (refreshError) {
      setFeedback({
        anchor: "registry",
        message: refreshError,
        tone: "danger",
      });
    } else {
      setFeedback({
        anchor: "registry",
        message: "Latest stored forwarding state loaded",
        tone: "success",
      });
    }
  }

  const columns = useMemo<ConsoleDataGridColumn<PortForwardRuleRecord>[]>(
    () => [
      {
        id: "rule",
        header: "Rule / VPS",
        cell: (rule) => (
          <span className="historyPrimary">
            <strong title={rule.name}>{rule.name}</strong>
            <small
              title={`${agentById.get(rule.client_id)?.display_name || rule.client_id} (${rule.client_id})`}
            >
              {agentById.get(rule.client_id)?.display_name || rule.client_id}
            </small>
          </span>
        ),
        mobilePrimary: true,
        searchValue: (rule) =>
          `${rule.name} ${agentById.get(rule.client_id)?.display_name ?? ""} ${rule.client_id}`,
        sortValue: (rule) => rule.name,
        minSize: 180,
        size: 220,
      },
      {
        id: "mapping",
        header: "Mapping",
        cell: (rule) => (
          <span
            className="truncateValue mappingValue"
            title={portForwardMappingLabel(rule)}
          >
            {portForwardMappingLabel(rule)}
          </span>
        ),
        searchValue: (rule) =>
          `${portForwardMappingLabel(rule)} ${rule.target_ip.includes(":") ? "IPv6" : "IPv4"}`,
        sortValue: (rule) => portForwardMappingLabel(rule),
        minSize: 220,
        size: 300,
      },
      {
        id: "return",
        header: "Return",
        cell: (rule) => (
          <span
            title={
              rule.masquerade
                ? "Masquerade only connections DNATed by this rule"
                : "Preserve the original source address"
            }
          >
            {rule.masquerade ? "Masquerade" : "Preserve source"}
          </span>
        ),
        searchValue: (rule) =>
          rule.masquerade ? "masquerade" : "preserve source",
        sortValue: (rule) => (rule.masquerade ? "masquerade" : "preserve"),
        size: 150,
      },
      {
        id: "desired",
        header: "Desired",
        cell: (rule) => <StatusBadge status={rule.desired_status} />,
        mobileState: true,
        searchValue: (rule) => rule.desired_status.replace(/_/g, " "),
        sortValue: (rule) => rule.desired_status,
        size: 140,
      },
      {
        id: "runtime",
        header: "Runtime",
        cell: (rule) => (
          <StatusBadge
            status={rule.runtime_status}
            title={runtimeStatusTitle(rule)}
          />
        ),
        searchValue: (rule) =>
          `${rule.runtime_status.replace(/_/g, " ")} ${runtimeStatusTitle(rule)}`,
        sortValue: (rule) => rule.runtime_status,
        minSize: 150,
        size: 170,
      },
      {
        id: "matches",
        header: "NAT matches",
        cell: (rule) => (
          <span title="First-packet NAT matches since the latest table apply; this is not throughput">
            {rule.nat_matches.toLocaleString()}
          </span>
        ),
        searchValue: (rule) => rule.nat_matches,
        sortValue: (rule) => rule.nat_matches,
        align: "end",
        size: 120,
      },
    ],
    [agentById],
  );

  function activeRows(rows: PortForwardRuleRecord[]) {
    return rows.filter((rule) => !rule.deleted_at);
  }

  function enableRows(rows: PortForwardRuleRecord[]) {
    return activeRows(rows).filter(
      (rule) =>
        !rule.enabled &&
        agentById.get(rule.client_id)?.capabilities.port_forwarding?.status ===
          "supported",
    );
  }

  function disableRows(rows: PortForwardRuleRecord[]) {
    return activeRows(rows).filter((rule) => rule.enabled);
  }

  function reapplyRows(rows: PortForwardRuleRecord[]) {
    return activeRows(rows).filter(
      (rule) =>
        agentById.get(rule.client_id)?.capabilities.port_forwarding?.status ===
        "supported",
    );
  }

  function reviewMutation(
    operation: PortForwardBulkAction,
    candidates: PortForwardRuleRecord[],
  ) {
    const eligible =
      operation === "enable"
        ? enableRows(candidates)
        : operation === "disable"
          ? disableRows(candidates)
          : operation === "reapply"
            ? reapplyRows(candidates)
            : activeRows(candidates);
    if (eligible.length === 0) return;
    setFeedback(null);
    if (eligible.length === 1) {
      setConfirmation({
        kind: "single",
        operation,
        origin: "registry",
        rule: eligible[0],
      });
      return;
    }
    setConfirmation({ kind: "bulk", action: operation, rules: eligible });
  }

  const actions: ConsoleDataGridAction<PortForwardRuleRecord>[] = [
    {
      description: (rows) =>
        !canWrite
          ? writeBoundary
          : rows.length !== 1
            ? "Select one active rule to edit."
            : rows[0].deleted_at
              ? "A rule awaiting removal cannot be edited."
              : editor
                ? "Close the current editor first."
                : `Edit ${rows[0].name}.`,
      disabled: (rows) =>
        !canWrite ||
        pending ||
        Boolean(editor) ||
        rows.length !== 1 ||
        Boolean(rows[0]?.deleted_at),
      hidden: (rows) => rows.length === 1 && Boolean(rows[0].deleted_at),
      icon: <Pencil size={14} />,
      label: "Edit",
      onSelect: (rows) => rows[0] && openEdit(rows[0]),
    },
    {
      description: (rows) =>
        !canWrite
          ? writeBoundary
          : rows.length !== 1
            ? "Select one active rule to clone."
            : rows[0].deleted_at
              ? "A rule awaiting removal cannot be cloned."
              : editor
                ? "Close the current editor first."
                : `Clone ${rows[0].name} as a disabled rule.`,
      disabled: (rows) =>
        !canWrite ||
        pending ||
        Boolean(editor) ||
        rows.length !== 1 ||
        Boolean(rows[0]?.deleted_at),
      hidden: (rows) => rows.length === 1 && Boolean(rows[0].deleted_at),
      icon: <Copy size={14} />,
      label: "Clone",
      onSelect: (rows) => rows[0] && openClone(rows[0]),
    },
    {
      description: (rows) => {
        const eligible = enableRows(rows);
        return eligible.length > 0
          ? `Review enabling ${eligible.length} eligible selected rule${eligible.length === 1 ? "" : "s"}.`
          : "No selected disabled rule is eligible to enable on its VPS.";
      },
      disabled: (rows) =>
        !canWrite ||
        pending ||
        Boolean(editor) ||
        enableRows(rows).length === 0,
      hidden: (rows) => rows.length === 1 && Boolean(rows[0].deleted_at),
      icon: <Power size={14} />,
      label: "Enable",
      onSelect: (rows) => reviewMutation("enable", rows),
    },
    {
      description: (rows) => {
        const eligible = disableRows(rows);
        return eligible.length > 0
          ? `Review disabling ${eligible.length} selected enabled rule${eligible.length === 1 ? "" : "s"}.`
          : "No selected rule is enabled.";
      },
      disabled: (rows) =>
        !canWrite ||
        pending ||
        Boolean(editor) ||
        disableRows(rows).length === 0,
      hidden: (rows) => rows.length === 1 && Boolean(rows[0].deleted_at),
      icon: <PowerOff size={14} />,
      label: "Disable",
      onSelect: (rows) => reviewMutation("disable", rows),
    },
    {
      description: (rows) => {
        const eligible = reapplyRows(rows);
        return eligible.length > 0
          ? `Review reapplying the complete forwarding table on ${eligible.length} eligible VPS${eligible.length === 1 ? "" : "s"}.`
          : "No selected active rule is on a VPS with supported forwarding control.";
      },
      disabled: (rows) =>
        !canWrite ||
        pending ||
        Boolean(editor) ||
        reapplyRows(rows).length === 0,
      hidden: (rows) => rows.length === 1 && Boolean(rows[0].deleted_at),
      icon: <RotateCw size={14} />,
      label: "Reapply",
      onSelect: (rows) => reviewMutation("reapply", rows),
    },
    {
      description: (rows) => {
        const eligible = activeRows(rows);
        return eligible.length > 0
          ? `Review deleting ${eligible.length} selected active rule${eligible.length === 1 ? "" : "s"}.`
          : "No selected rule is active.";
      },
      disabled: (rows) =>
        !canWrite ||
        pending ||
        Boolean(editor) ||
        activeRows(rows).length === 0,
      hidden: (rows) => rows.length === 1 && Boolean(rows[0].deleted_at),
      icon: <Trash2 size={14} />,
      label: "Delete",
      onSelect: (rows) => reviewMutation("delete", rows),
      separatorBefore: true,
      tone: "danger",
    },
  ];

  function renderRuleDetails(rule: PortForwardRuleRecord) {
    return (
      <div
        aria-label={`Details for ${rule.name}`}
        className="portForwardInlineDetail"
      >
        <dl className="portForwardDetailGrid">
          <Detail
            display={shortId(rule.id)}
            label="Rule ID"
            title={rule.id}
            value={rule.id}
          />
          <Detail label="Revision" value={String(rule.revision)} />
          <Detail
            label="VPS"
            value={`${agentById.get(rule.client_id)?.display_name || rule.client_id} (${rule.client_id})`}
          />
          <Detail label="Protocol" value={rule.protocol.toUpperCase()} />
          <Detail
            label="Desired"
            value={rule.desired_status.replace(/_/g, " ")}
          />
          <Detail
            label="Listener scope"
            value={`All current local ${rule.target_ip.includes(":") ? "IPv6" : "IPv4"} addresses`}
          />
          <Detail label="Target" value={rule.target_ip} />
          <Detail label="Mappings" value={formatPortMappings(rule.mappings)} />
          <Detail
            label="Return path"
            value={rule.masquerade ? "Targeted masquerade" : "Preserve source"}
          />
          <Detail
            label="Runtime"
            value={
              rule.runtime_error
                ? `${rule.runtime_status}: ${rule.runtime_error}`
                : rule.runtime_status
            }
          />
          <Detail
            label="Capability"
            title={
              agentById.get(rule.client_id)?.capabilities.port_forwarding
                ?.reason ?? undefined
            }
            value={capabilitySummary(
              agentById.get(rule.client_id)?.capabilities.port_forwarding,
            )}
          />
          <Detail
            label={`${rule.target_ip.includes(":") ? "IPv6" : "IPv4"} forwarding`}
            tone={rule.forwarding_enabled === false ? "warning" : "normal"}
            value={forwardingSummary(rule.forwarding_enabled)}
          />
          <Detail
            label="Observed"
            value={
              rule.runtime_observed_unix
                ? formatCompactTime(
                    new Date(rule.runtime_observed_unix * 1000).toISOString(),
                  )
                : "No agent evidence"
            }
          />
          <Detail
            label="NAT matches"
            value={rule.nat_matches.toLocaleString()}
          />
          <Detail label="Updated" value={formatCompactTime(rule.updated_at)} />
          <Detail
            display={shortId(rule.desired_hash)}
            label="Control desired"
            title={`Control-plane desired config hash: ${rule.desired_hash ?? "not applicable"}`}
            value={rule.desired_hash ?? "Not applicable"}
          />
          <Detail
            display={shortId(rule.agent_desired_hash)}
            label="Agent desired"
            title={`Latest desired config hash reported by the agent: ${rule.agent_desired_hash ?? "not reported"}`}
            value={rule.agent_desired_hash ?? "Not reported"}
          />
          <Detail
            display={shortId(rule.observed_hash)}
            label="Observed table"
            title={`Latest normalized owned-table hash reported by the agent: ${rule.observed_hash ?? "no owned table hash"}`}
            value={rule.observed_hash ?? "No owned table hash"}
          />
        </dl>
        <ActionFeedback
          className="localActionFeedback portForwardDetailFeedback"
          message={feedback?.anchor === "detail" ? feedback.message : null}
          tone={feedback?.anchor === "detail" ? feedback.tone : undefined}
        />
        {rule.deleted_at && !rule.removal_confirmed_at && (
          <div className="portForwardRemovalNotice">
            <ShieldAlert size={17} />
            <span>
              Removal pending until the agent confirms the owned table no longer
              contains this rule.
            </span>
            <label
              className="forgetReasonField"
              title="Audit reason for removing a rule whose VPS can no longer confirm runtime cleanup."
            >
              <span className="srOnly">Forget reason</span>
              <input
                data-tooltip-disabled-reason={
                  pending
                    ? "Wait for the current port-forward operation to finish."
                    : forgetBoundary
                }
                disabled={!canForget || pending}
                maxLength={512}
                onChange={(event) => setForgetReason(event.target.value)}
                placeholder="Decommission reason"
                value={forgetReason}
              />
            </label>
            <button
              className="dangerAction compactAction"
              disabled={!canForget || pending || !forgetReason.trim()}
              onClick={() => {
                setFeedback(null);
                setConfirmation({
                  kind: "single",
                  operation: "forget",
                  origin: "detail",
                  reason: forgetReason.trim(),
                  rule,
                });
              }}
              title={
                pending
                  ? "Wait for the current port-forward operation to finish"
                  : !canForget
                    ? forgetBoundary
                    : !forgetReason.trim()
                      ? "Enter a decommission reason before forgetting this removal-pending rule"
                      : "Forget only when this VPS is permanently unreachable or decommissioned"
              }
              type="button"
            >
              Forget
            </button>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="portForwardPage topologyPageStack">
      <section className="fleetPanel portForwardSummary">
        <div className="sectionHeader">
          <div>
            <h2>Port forwarding</h2>
            <span>
              {rules.length} {rules.length === 1 ? "rule" : "rules"} across{" "}
              {new Set(rules.map((rule) => rule.client_id)).size}{" "}
              {new Set(rules.map((rule) => rule.client_id)).size === 1
                ? "VPS"
                : "VPSs"}
            </span>
          </div>
        </div>
        <ActionFeedback
          className="localActionFeedback portForwardActionFeedback"
          message={
            error ??
            (loading
              ? "Reloading stored forwarding state"
              : feedback?.anchor === "summary"
                ? feedback.message
                : null)
          }
          tone={
            error
              ? "danger"
              : loading
                ? "progress"
                : feedback?.anchor === "summary"
                  ? feedback.tone
                  : undefined
          }
        />
        <div
          aria-label="Port-forwarding summary"
          className="networkMetricStrip"
        >
          <Metric label="Rules" value={rules.length} />
          <Metric label="Enabled" value={enabledCount} />
          <Metric label="Applied" value={appliedCount} />
          <Metric label="Pending" value={pendingCount} />
          <Metric
            label="Attention"
            tone={attentionCount > 0 ? "warning" : "normal"}
            value={attentionCount}
          />
          <Metric
            label="NFT-capable"
            tone={supportedAgents < agents.length ? "warning" : "normal"}
            value={`${supportedAgents}/${agents.length}`}
          />
        </div>
      </section>

      <section className="fleetPanel portForwardRegistry">
        <div className="sectionHeader compactSectionHeader">
          <div>
            <h2>Rules</h2>
            <span>Desired state and latest owned-table evidence</span>
          </div>
        </div>
        {corruptRules.length > 0 && (
          <div
            className="portForwardRemovalNotice"
            role="alert"
            title="A stored rule that cannot be parsed must be removed and recreated before it can return to normal lifecycle management."
          >
            <ShieldAlert size={17} />
            <div>
              <strong>
                {corruptRules.length} persisted rule
                {corruptRules.length === 1 ? "" : "s"} need repair
              </strong>
              {corruptRules.map((rule) => (
                <div key={rule.id}>
                  <span>
                    {rule.name} ·{" "}
                    {agentById.get(rule.client_id)?.display_name ||
                      rule.client_id}{" "}
                    · revision {rule.revision}: {rule.configuration_error}
                  </span>
                  {!rule.deleted_at && (
                    <button
                      className="dangerAction compactAction"
                      disabled={!canWrite || pending || Boolean(editor)}
                      onClick={() => {
                        setFeedback(null);
                        setCorruptDelete(rule);
                      }}
                      title={
                        pending
                          ? "Wait for the current port-forward operation to finish"
                          : editor
                            ? "Close the current port-forward editor before deleting a corrupt rule"
                            : !canWrite
                              ? writeBoundary
                              : "Review deletion of this exact corrupt rule"
                      }
                      type="button"
                    >
                      <Trash2 size={14} /> Delete
                    </button>
                  )}
                </div>
              ))}
            </div>
          </div>
        )}
        <ActionFeedback
          className="localActionFeedback portForwardRegistryFeedback"
          message={feedback?.anchor === "registry" ? feedback.message : null}
          ref={registryFeedbackRef}
          tone={feedback?.anchor === "registry" ? feedback.tone : undefined}
        />
        <ConsoleDataGrid
          actions={actions}
          columns={columns}
          defaultPageSize={100}
          empty={
            <div className="emptyState compactEmptyState">
              <strong>
                {loading
                  ? "Loading port-forward rules"
                  : "No port-forward rules"}
              </strong>
              <span>
                {loading
                  ? "Reading desired state and the latest agent evidence."
                  : "Create a disabled draft, or enable a rule on a VPS that reports nftables support."}
              </span>
            </div>
          }
          getRowId={(rule) => rule.id}
          itemLabel="port-forward rules"
          onExpandedRowChange={() => setForgetReason("")}
          openRowOnClick={false}
          renderExpandedRow={renderRuleDetails}
          rows={rules}
          searchPlaceholder="Search name, VPS, mapping, family, desired, or runtime"
          selectable={canWrite}
          showMobileRowActions={false}
          singleExpandedRow
          storageKey="vpsman.network.portForwardRules"
          title="Port-forward rules"
          toolbarActions={
            <div className="previewMeta">
              <button
                aria-busy={loading}
                className="secondaryAction compactAction"
                disabled={loading || pending}
                onClick={() => void refreshRules()}
                title={
                  loading || pending
                    ? "Wait for the current port-forward request to finish"
                    : "Reload latest stored desired state and agent evidence; this does not request a live agent inspection"
                }
                type="button"
              >
                <RefreshCcw size={14} /> {loading ? "Refreshing" : "Refresh"}
              </button>
              <button
                className="primaryAction compactAction"
                disabled={!canWrite || Boolean(editor) || pending}
                onClick={openCreate}
                title={
                  !canWrite
                    ? writeBoundary
                    : editor
                      ? "Close the current editor first"
                      : "Create a port-forward rule"
                }
                type="button"
              >
                <CirclePlus size={15} /> Create rule
              </button>
            </div>
          }
        />
      </section>

      {editor && (
        <PortForwardEditor
          agents={agents}
          draft={editor.draft}
          editing={editor.editing}
          feedback={feedback?.anchor === "editor" ? feedback : null}
          onChange={(draft) =>
            setEditor((current) => (current ? { ...current, draft } : current))
          }
          onClose={() => setEditor(null)}
          onResolveHostname={onResolveHostname}
          onSubmit={submitEditor}
          pending={pending || confirmation?.kind === "save"}
          ref={editorRef}
        />
      )}

      <ConfirmationPrompt
        confirmLabel={confirmationLabel(confirmation)}
        detail={confirmationDetail(confirmation)}
        error={
          confirmation &&
          feedback?.tone === "danger" &&
          feedback.anchor ===
            (confirmation.kind === "save"
              ? "editor"
              : confirmation.kind === "single"
                ? confirmation.origin
                : "registry")
            ? feedback.message
            : null
        }
        items={confirmationItems(confirmation, agentById)}
        onCancel={() => setConfirmation(null)}
        onConfirm={() => confirmation && void executeConfirmation(confirmation)}
        open={Boolean(confirmation)}
        pending={pending}
        title={confirmationTitle(confirmation)}
        tone={
          (confirmation?.kind === "single" &&
            ["delete", "forget"].includes(confirmation.operation)) ||
          (confirmation?.kind === "bulk" && confirmation.action === "delete")
            ? "danger"
            : "normal"
        }
      />
      <ConfirmationPrompt
        confirmLabel="Delete corrupt rule"
        detail="This retires the exact persisted revision. No missing mapping or protocol value is guessed."
        error={
          corruptDelete &&
          feedback?.anchor === "registry" &&
          feedback.tone === "danger"
            ? feedback.message
            : null
        }
        items={
          corruptDelete
            ? [
                { label: "Rule", value: corruptDelete.name },
                {
                  label: "VPS",
                  value:
                    agentById.get(corruptDelete.client_id)?.display_name ||
                    corruptDelete.client_id,
                },
                { label: "Revision", value: String(corruptDelete.revision) },
                {
                  label: "Configuration error",
                  value: corruptDelete.configuration_error,
                },
              ]
            : []
        }
        onCancel={() => setCorruptDelete(null)}
        onConfirm={() =>
          corruptDelete && void executeCorruptDelete(corruptDelete)
        }
        open={Boolean(corruptDelete)}
        pending={pending}
        title="Confirm corrupt rule deletion"
        tone="danger"
      />
    </div>
  );
}

const PortForwardEditor = forwardRef<
  HTMLElement,
  {
    agents: AgentView[];
    draft: EditorDraft;
    editing: PortForwardRuleRecord | null;
    feedback: Feedback | null;
    onChange: (draft: EditorDraft) => void;
    onClose: () => void;
    onResolveHostname: (hostname: string) => Promise<ResolveHostnameResponse>;
    onSubmit: (event: FormEvent) => void;
    pending: boolean;
  }
>(function PortForwardEditor(
  {
    agents,
    draft,
    editing,
    feedback,
    onChange,
    onClose,
    onResolveHostname,
    onSubmit,
    pending,
  },
  ref,
) {
  const [resolution, setResolution] = useState<ResolveHostnameResponse | null>(
    null,
  );
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const feedbackRef = useRef<HTMLDivElement | null>(null);
  const targetInputRef = useRef(draft.targetInput);
  const draftRef = useRef(draft);
  targetInputRef.current = draft.targetInput;
  draftRef.current = draft;
  const mappingStarted = Boolean(draft.incoming.trim() || draft.target.trim());
  const mappingPreview = useMemo(() => {
    try {
      return {
        error: null,
        mappings: pairPortExpressions(draft.incoming, draft.target),
      };
    } catch (error) {
      return {
        error: error instanceof Error ? error.message : "Invalid mapping",
        mappings: [],
      };
    }
  }, [draft.incoming, draft.target]);
  const selectedAgent = agents.find((agent) => agent.id === draft.clientId);
  const capability = selectedAgent?.capabilities.port_forwarding;
  const targetIsLiteral = literalIpFamily(draft.targetInput) !== null;
  const saveDisabledReason = pending
    ? "Another port-forwarding action is in progress"
    : !draft.name.trim()
      ? "Enter a rule name"
      : utf8ByteLength(draft.name.trim()) > MAX_RULE_NAME_BYTES
        ? `Rule name must not exceed ${MAX_RULE_NAME_BYTES} UTF-8 bytes`
        : !draft.clientId
          ? "Select a VPS"
          : !draft.targetIp
            ? "Enter a literal target IP, or resolve and select a hostname result"
            : mappingPreview.error
              ? mappingPreview.error
              : draft.enabled && capability?.status !== "supported"
                ? capabilityLabel(capability?.status, capability?.reason)
                : null;
  const saveDisabled = saveDisabledReason !== null;

  useEffect(() => {
    if (!feedback?.message) return;
    const frame = window.requestAnimationFrame(() => {
      if (feedbackRef.current) {
        scrollIntoViewWithMotion(feedbackRef.current, { block: "nearest" });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [feedback?.message]);

  async function resolveHostname() {
    const requestedHostname = targetInputRef.current.trim();
    setResolving(true);
    setResolveError(null);
    setResolution(null);
    try {
      const result = await onResolveHostname(requestedHostname);
      if (targetInputRef.current.trim() !== requestedHostname) return;
      setResolution(result);
      onChange({ ...draftRef.current, targetIp: "" });
    } catch (error) {
      if (targetInputRef.current.trim() !== requestedHostname) return;
      setResolveError(
        error instanceof Error ? error.message : "Hostname resolution failed",
      );
    } finally {
      setResolving(false);
    }
  }

  return (
    <section className="fleetPanel portForwardEditor" ref={ref}>
      <div className="sectionHeader">
        <div>
          <h2>
            {editing ? "Edit port-forward rule" : "Create port-forward rule"}
          </h2>
          <span>
            {editing
              ? `Revision ${editing.revision}`
              : "New per-VPS desired state"}
          </span>
        </div>
        <button
          aria-label="Close port-forward editor"
          className="iconButton"
          disabled={pending}
          onClick={onClose}
          title={
            pending
              ? "Wait for the current port-forward operation to finish before closing the editor"
              : "Close port-forward editor"
          }
          type="button"
        >
          <X size={17} />
        </button>
      </div>
      <form className="portForwardForm" onSubmit={onSubmit}>
        <ActionFeedback
          className="localActionFeedback"
          message={feedback?.message}
          ref={feedbackRef}
          tone={feedback?.tone}
        />
        <label
          title={
            pending
              ? "VPS selection is disabled while a port-forward operation is pending"
              : editing
                ? "The VPS is immutable after rule creation; clone or create a rule for another VPS"
                : "VPS whose vpsman-owned nftables table receives this rule"
          }
        >
          <span>VPS</span>
          <VpsCombobox
            agents={agents}
            ariaLabel="Port-forward rule VPS"
            disabled={Boolean(editing) || pending}
            onChange={(clientId) => onChange({ ...draft, clientId })}
            placeholder="Search VPS name or ID"
            value={draft.clientId}
          />
        </label>
        <label title="Operator-facing name used to identify this desired port-forward rule.">
          <span>Name</span>
          <input
            data-tooltip-disabled-reason="Wait for the current port-forward operation to finish before editing the rule name."
            disabled={pending}
            maxLength={128}
            onChange={(event) =>
              onChange({ ...draft, name: event.target.value })
            }
            placeholder="Public web"
            required
            value={draft.name}
          />
        </label>
        <fieldset className="compactFieldset">
          <legend>Protocol</legend>
          <div className="segmentedControl" role="group" aria-label="Protocol">
            {(["tcp", "udp", "both"] as const).map((protocol) => (
              <button
                aria-pressed={draft.protocol === protocol}
                className={draft.protocol === protocol ? "active" : ""}
                disabled={pending}
                key={protocol}
                onClick={() => onChange({ ...draft, protocol })}
                title={
                  pending
                    ? "Wait for the current port-forward operation to finish before changing protocol"
                    : `Match ${protocol === "both" ? "both TCP and UDP" : protocol.toUpperCase()} traffic`
                }
                type="button"
              >
                {protocol === "both" ? "Both" : protocol.toUpperCase()}
              </button>
            ))}
          </div>
        </fieldset>
        <label title="Local listener ports matched by this rule; enter a port, range, or comma-separated mappings.">
          <span>Incoming ports</span>
          <input
            data-tooltip-disabled-reason="Wait for the current port-forward operation to finish before editing incoming ports."
            disabled={pending}
            onChange={(event) =>
              onChange({ ...draft, incoming: event.target.value })
            }
            placeholder="80,443,10000-10010"
            required
            value={draft.incoming}
          />
          <small>PORT or START-END, comma separated</small>
        </label>
        <label title="Destination ports paired with the incoming mappings; one port may serve every incoming port.">
          <span>Target ports</span>
          <input
            data-tooltip-disabled-reason="Wait for the current port-forward operation to finish before editing target ports."
            disabled={pending}
            onChange={(event) =>
              onChange({ ...draft, target: event.target.value })
            }
            placeholder="8080 or 8080,20000-20010"
            required
            value={draft.target}
          />
          <small>One port for all, or corresponding items</small>
        </label>
        <div className="targetAddressField">
          <label title="Literal destination address, or a hostname resolved to one address during review.">
            <span>Target IP or hostname</span>
            <input
              data-tooltip-disabled-reason="Wait for the current port-forward operation to finish before editing the target address."
              disabled={pending}
              onChange={(event) => {
                const value = event.target.value;
                const literal = literalIpFamily(value);
                setResolution(null);
                setResolveError(null);
                onChange({
                  ...draft,
                  targetInput: value,
                  targetIp: literal ? value.trim() : "",
                });
              }}
              placeholder="192.0.2.40 or app.internal"
              required
              value={draft.targetInput}
            />
          </label>
          {!targetIsLiteral && draft.targetInput.trim() && (
            <button
              className="secondaryAction compactAction"
              disabled={pending || resolving}
              onClick={() => void resolveHostname()}
              title={
                pending
                  ? "Wait for the current port-forward operation to finish"
                  : resolving
                    ? "The target hostname is already resolving"
                    : "Resolve on the control plane and select one literal address"
              }
              type="button"
            >
              <RefreshCcw size={14} /> {resolving ? "Resolving" : "Resolve"}
            </button>
          )}
        </div>
        <ActionFeedback message={resolveError} tone="danger" />
        {resolution && (
          <fieldset
            className="dnsCandidateList"
            disabled={pending}
            title={
              pending
                ? "Resolved-address selection is disabled while a port-forward operation is pending"
                : "Select the exact literal address stored in the reviewed rule"
            }
          >
            <legend>Resolved addresses</legend>
            {resolution.candidates.map((candidate) => (
              <label
                key={candidate.address}
                title={`${candidate.family.toUpperCase()} ${candidate.address}`}
              >
                <input
                  checked={draft.targetIp === candidate.address}
                  name="resolved-target"
                  onChange={() =>
                    onChange({ ...draft, targetIp: candidate.address })
                  }
                  type="radio"
                />
                <span>{candidate.address}</span>
                <small>{candidate.family.toUpperCase()}</small>
              </label>
            ))}
          </fieldset>
        )}
        <fieldset className="compactFieldset returnPathField">
          <legend>Return path</legend>
          <div
            className="segmentedControl"
            role="group"
            aria-label="Return path"
          >
            <button
              aria-pressed={draft.masquerade}
              className={draft.masquerade ? "active" : ""}
              disabled={pending}
              onClick={() => onChange({ ...draft, masquerade: true })}
              title={
                pending
                  ? "Wait for the current port-forward operation to finish before changing return-path behavior"
                  : "Masquerade only connections DNATed by this rule"
              }
              type="button"
            >
              Masquerade
            </button>
            <button
              aria-pressed={!draft.masquerade}
              className={!draft.masquerade ? "active" : ""}
              disabled={pending}
              onClick={() => onChange({ ...draft, masquerade: false })}
              title={
                pending
                  ? "Wait for the current port-forward operation to finish before changing return-path behavior"
                  : "Keep source addresses; the target must have a valid return route"
              }
              type="button"
            >
              Preserve source
            </button>
          </div>
        </fieldset>
        <label
          className="compactCheckbox portForwardEnabled"
          title={
            pending
              ? "Wait for the current port-forward operation to finish before changing enabled state"
              : "Enable this desired port-forward rule after the reviewed apply"
          }
        >
          <input
            checked={draft.enabled}
            data-tooltip-disabled-reason={
              pending
                ? "Wait for the current port-forward operation to finish before changing enabled state"
                : undefined
            }
            disabled={pending}
            onChange={(event) =>
              onChange({ ...draft, enabled: event.target.checked })
            }
            type="checkbox"
          />
          <span>Enabled</span>
        </label>
        <div
          className={`portMappingPreview ${!mappingStarted ? "idle" : mappingPreview.error ? "invalid" : "valid"}`}
        >
          {!mappingStarted ? (
            <>
              <Info size={16} />
              <span title="Enter incoming and target ports to preview the exact mappings">
                Enter incoming and target ports to preview the exact mappings
              </span>
            </>
          ) : mappingPreview.error ? (
            <>
              <ShieldAlert size={16} />
              <span title={mappingPreview.error}>{mappingPreview.error}</span>
            </>
          ) : (
            <>
              <CheckCircle2 size={16} />
              <span title={formatPortMappings(mappingPreview.mappings)}>
                {formatPortMappings(mappingPreview.mappings)} ·{" "}
                {draft.targetIp || "select target IP"}
              </span>
            </>
          )}
        </div>
        {selectedAgent && capability?.status !== "supported" && (
          <div className="portForwardCapabilityNotice">
            <ShieldAlert size={16} />
            <span title={capability?.reason ?? capability?.status ?? "unknown"}>
              {capabilityLabel(capability?.status, capability?.reason)}
            </span>
          </div>
        )}
        <div className="consoleFormActions fieldFull">
          <button
            className="secondaryAction"
            disabled={pending}
            onClick={onClose}
            title={
              pending
                ? "Wait for the current port-forward operation to finish before closing the editor"
                : "Cancel and close the port-forward editor"
            }
            type="button"
          >
            Cancel
          </button>
          <button
            className="primaryAction"
            disabled={saveDisabled}
            title={
              saveDisabledReason ??
              (editing ? "Review rule update" : "Review rule creation")
            }
            type="submit"
          >
            {editing ? "Save changes" : "Create rule"}
          </button>
        </div>
      </form>
    </section>
  );
});

function Metric({
  label,
  tone = "normal",
  value,
}: {
  label: string;
  tone?: "normal" | "warning";
  value: string | number;
}) {
  return (
    <span className={tone === "warning" ? "hasAttention" : ""}>
      <small>{label}</small>
      <strong>{value}</strong>
    </span>
  );
}

function Detail({
  display,
  label,
  title,
  tone = "normal",
  value,
}: {
  display?: string;
  label: string;
  title?: string;
  tone?: "normal" | "warning";
  value: string;
}) {
  return (
    <div className={tone === "warning" ? "hasAttention" : ""}>
      <dt>{label}</dt>
      <dd title={title ?? (display !== undefined ? value : undefined)}>
        {display ?? value}
      </dd>
    </div>
  );
}

function portForwardMappingLabel(rule: PortForwardRuleRecord) {
  const targetAddress = rule.target_ip.includes(":")
    ? `[${rule.target_ip}]`
    : rule.target_ip;
  const incoming = rule.mappings
    .map((item) => formatPortRange(item.incoming))
    .join(",");
  const target = rule.mappings
    .map((item) => formatPortRange(item.target))
    .join(",");
  return `${rule.protocol.toUpperCase()} · ${incoming} -> ${targetAddress}:${target}`;
}

function StatusBadge({ status, title }: { status: string; title?: string }) {
  const label =
    status === "applied_warning"
      ? "applied · warning"
      : status.replace(/_/g, " ");
  return (
    <span
      className={`portForwardStatus status-${status}`}
      title={title ?? label}
    >
      {label}
    </span>
  );
}

function runtimeStatusTitle(rule: PortForwardRuleRecord) {
  if (rule.runtime_error) return rule.runtime_error;
  if (rule.runtime_status === "applied_warning") {
    return `${rule.target_ip.includes(":") ? "IPv6" : "IPv4"} forwarding is disabled outside vpsman`;
  }
  switch (rule.runtime_status) {
    case "applied":
      return "Owned nftables table matches desired state; target reachability is not tested";
    case "disabled":
      return "Rule is stored but omitted from the agent's desired nftables table";
    case "absent":
      return "No owned nftables table is expected or observed";
    case "pending":
      return "Desired state is queued; latest agent evidence has not confirmed it yet";
    case "drifted":
      return "Latest owned-table evidence does not match the agent's desired state";
    case "failed":
      return "The agent could not inspect or apply the owned nftables table";
    case "unsupported":
      return "This agent cannot manage port forwarding with the required nftables features";
    case "removal_pending":
      return "Deletion is saved but agent cleanup evidence is still pending";
    default:
      return "No current agent evidence is available";
  }
}

function forwardingSummary(value?: boolean | null) {
  if (value === true) return "Enabled";
  if (value === false) return "Disabled outside vpsman";
  return "Not reported";
}

function capabilitySummary(
  capability?: AgentView["capabilities"]["port_forwarding"],
) {
  if (!capability) return "Not reported";
  const status = capability.status.replace(/_/g, " ");
  return capability.nft_version
    ? `${status} · ${capability.nft_version}`
    : status;
}

function literalIpFamily(value: string): "ipv4" | "ipv6" | null {
  const input = value.trim();
  const ipv4 = input.split(".");
  if (
    ipv4.length === 4 &&
    ipv4.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)
  )
    return "ipv4";
  if (input.includes(":") && /^[0-9a-f:.]+$/i.test(input)) {
    try {
      new URL(`http://[${input}]/`);
      return "ipv6";
    } catch {
      return null;
    }
  }
  return null;
}

function validateEditor(draft: EditorDraft, agent: AgentView | undefined) {
  if (!draft.name.trim()) throw new Error("Rule name is required");
  if (utf8ByteLength(draft.name.trim()) > MAX_RULE_NAME_BYTES)
    throw new Error(
      `Rule name must not exceed ${MAX_RULE_NAME_BYTES} UTF-8 bytes`,
    );
  if (!agent) throw new Error("Select a VPS");
  if (!draft.targetIp)
    throw new Error("Resolve and select one literal target IP");
  if (
    draft.enabled &&
    agent.capabilities.port_forwarding?.status !== "supported"
  )
    throw new Error(
      capabilityLabel(
        agent.capabilities.port_forwarding?.status,
        agent.capabilities.port_forwarding?.reason,
      ),
    );
}

function capabilityLabel(status?: string, reason?: string | null) {
  if (reason) return reason;
  switch (status) {
    case "nft_missing":
      return "nft is not installed on this VPS";
    case "insufficient_privilege":
      return "Agent lacks CAP_NET_ADMIN in the host network namespace";
    case "inet_nat_unsupported":
      return "Kernel or nft userspace does not support the required inet NAT features";
    case "probe_failed":
      return "Agent nftables capability probe failed";
    default:
      return "Agent has not reported port-forwarding capability";
  }
}

function capabilityActionTitle(
  agent: AgentView | undefined,
  supportedTitle: string,
) {
  return agent?.capabilities.port_forwarding?.status === "supported"
    ? supportedTitle
    : capabilityLabel(
        agent?.capabilities.port_forwarding?.status,
        agent?.capabilities.port_forwarding?.reason,
      );
}

function nextCloneName(
  name: string,
  rules: PortForwardRuleRecord[],
  clientId: string,
) {
  const base = fitNameWithSuffix(name, " (cloned)");
  if (!rules.some((rule) => rule.client_id === clientId && rule.name === base))
    return base;
  for (let suffix = 2; suffix < 1000; suffix += 1) {
    const candidate = fitNameWithSuffix(name, ` (cloned ${suffix})`);
    if (
      !rules.some(
        (rule) => rule.client_id === clientId && rule.name === candidate,
      )
    )
      return candidate;
  }
  return fitNameWithSuffix(name, ` (cloned ${Date.now()})`);
}

function fitNameWithSuffix(name: string, suffix: string) {
  const characters = Array.from(name.trim());
  while (
    characters.length > 0 &&
    utf8ByteLength(`${characters.join("")}${suffix}`) > MAX_RULE_NAME_BYTES
  ) {
    characters.pop();
  }
  return `${characters.join("")}${suffix}`;
}

function utf8ByteLength(value: string) {
  return new TextEncoder().encode(value).length;
}

function syncFeedback(
  response: PortForwardMutationResponse,
  success: string,
): FeedbackContent {
  if (
    response.sync.status === "queue_failed" ||
    response.sync.status === "not_queued"
  )
    return {
      message: `${success}; desired state saved, but apply was not queued: ${dispatchFailureReason(response.sync.error, response.sync.status, "Port-forward apply job")}`,
      tone: "warning",
    };
  if (response.sync.status === "queued")
    return {
      message: `${success}; apply job ${shortId(response.sync.job_id ?? "")} queued`,
      tone: "progress",
    };
  if (
    [
      "already_in_requested_state",
      "forgotten_without_host_cleanup",
      "retired_disabled_draft",
      "saved_disabled",
    ].includes(response.sync.status)
  ) {
    return { message: `${success}; no host apply required`, tone: "success" };
  }
  return { message: success, tone: "success" };
}

function bulkSyncFeedback(
  response: PortForwardBulkResponse,
  action: PortForwardBulkAction,
  count: number,
): FeedbackContent {
  const failed = response.sync.filter((item) =>
    ["queue_failed", "not_queued"].includes(item.sync.status),
  );
  const queued = response.sync.filter(
    (item) => item.sync.status === "queued",
  ).length;
  const pastAction = action === "reapply" ? "reapplied" : `${action}d`;
  if (failed.length > 0) {
    return {
      message: `${count} rules ${pastAction}; desired state saved, ${failed.length} VPS apply${failed.length === 1 ? "" : "s"} not queued: ${failed.map((item) => `${item.client_id}: ${dispatchFailureReason(item.sync.error, item.sync.status, "Port-forward apply job")}`).join("; ")}`,
      tone: "warning",
    };
  }
  if (queued > 0) {
    return {
      message: `${count} rules ${pastAction}; ${queued} VPS apply job${queued === 1 ? "" : "s"} queued`,
      tone: "progress",
    };
  }
  return {
    message: `${count} rules ${pastAction}; no host apply required`,
    tone: "success",
  };
}

function actionProgressLabel(snapshot: ConfirmationState) {
  if (snapshot.kind === "save")
    return snapshot.editing ? "Updating rule" : "Creating rule";
  if (snapshot.kind === "single")
    return `${snapshot.operation[0]!.toUpperCase()}${snapshot.operation.slice(1)} in progress`;
  return `Bulk ${snapshot.action} in progress`;
}

function singleActionPast(
  operation: "enable" | "disable" | "delete" | "forget" | "reapply",
) {
  if (operation === "reapply") return "Reapply queued";
  if (operation === "forget") return "Removal evidence forgotten";
  return `Rule ${operation}d`;
}

function confirmationTitle(state: ConfirmationState | null) {
  if (!state) return "Confirm port-forwarding action";
  if (state.kind === "save")
    return state.editing ? "Confirm rule update" : "Confirm rule creation";
  if (state.kind === "bulk") return `Confirm bulk ${state.action}`;
  return `Confirm ${state.operation}`;
}

function confirmationLabel(state: ConfirmationState | null) {
  if (!state) return "Confirm";
  if (state.kind === "save")
    return state.editing ? "Save and apply" : "Create and apply";
  return state.kind === "bulk"
    ? `${state.action[0]!.toUpperCase()}${state.action.slice(1)} rules`
    : `${state.operation[0]!.toUpperCase()}${state.operation.slice(1)} rule`;
}

function confirmationDetail(state: ConfirmationState | null) {
  if (!state) return "Review the current action.";
  if (state.kind === "save")
    return "This saves desired state and replaces the VPS's complete vpsman-owned nftables table atomically. Claimed ports take precedence over conventional Docker or system DNAT for new connections.";
  if (state.kind === "single" && state.operation === "delete") {
    return isNeverAppliedDisabledDraft(state.rule)
      ? "This disabled draft has never been applied. It is removed immediately; no agent cleanup or apply job is required."
      : "The rule is omitted from desired state immediately and remains visible as Removal pending until the agent confirms cleanup. Existing conntrack entries may continue.";
  }
  if (state.kind === "single" && state.operation === "forget")
    return "This removes the cleanup tombstone without confirming host state. Use it only for a permanently unreachable or decommissioned VPS; nftables state may remain on that host.";
  if (state.kind === "single" && state.operation === "reapply")
    return "Reapply replaces this VPS's complete vpsman-owned forwarding table. It does not change system, Docker, or unrelated nftables tables.";
  if (state.kind === "bulk" && state.action === "delete") {
    const immediateDrafts = state.rules.filter(
      isNeverAppliedDisabledDraft,
    ).length;
    const cleanupRules = state.rules.length - immediateDrafts;
    if (cleanupRules === 0) {
      return `This removes ${immediateDrafts} never-applied disabled draft${immediateDrafts === 1 ? "" : "s"} immediately. No agent cleanup or apply job is required.`;
    }
    if (immediateDrafts > 0) {
      return `${immediateDrafts} never-applied disabled draft${immediateDrafts === 1 ? "" : "s"} will be removed immediately. ${cleanupRules} rule${cleanupRules === 1 ? "" : "s"} will remain Removal pending until the affected agents confirm cleanup.`;
    }
  }
  if (state.kind === "bulk")
    return `This applies one ${state.action} decision to ${state.rules.length} exact rule revisions and reconciles each affected VPS once.`;
  return "This updates desired state and queues an atomic apply for the affected VPS.";
}

function isNeverAppliedDisabledDraft(rule: PortForwardRuleRecord) {
  return !rule.enabled && rule.revision === 1 && !rule.deleted_at;
}

function isCorruptPortForwardRule(
  rule: PortForwardRuleListItem,
): rule is PortForwardRuleCorruptRecord {
  return "configuration_error" in rule;
}

function isHealthyPortForwardRule(
  rule: PortForwardRuleListItem,
): rule is PortForwardRuleRecord {
  return !isCorruptPortForwardRule(rule);
}

function confirmationItems(
  state: ConfirmationState | null,
  agents: Map<string, AgentView>,
) {
  if (!state) return [];
  if (state.kind === "save") {
    const mappings = (() => {
      try {
        return formatPortMappings(
          pairPortExpressions(state.draft.incoming, state.draft.target),
        );
      } catch {
        return state.draft.incoming;
      }
    })();
    return [
      {
        label: "VPS",
        value:
          agents.get(state.draft.clientId)?.display_name ||
          state.draft.clientId,
      },
      {
        label: "Listener scope",
        value: `All current local ${state.draft.targetIp.includes(":") ? "IPv6" : "IPv4"} addresses`,
      },
      {
        label: "Claimed ports",
        title: state.draft.incoming,
        value: `${state.draft.protocol.toUpperCase()} ${state.draft.incoming}`,
      },
      { label: "Target", value: state.draft.targetIp },
      { label: "Mapping", title: mappings, value: mappings },
      {
        label: "Return",
        value: state.draft.masquerade
          ? "Targeted masquerade"
          : "Preserve source",
      },
    ];
  }
  const rules = state.kind === "bulk" ? state.rules : [state.rule];
  const items = rules.slice(0, 12).map((rule) => ({
    label: agents.get(rule.client_id)?.display_name || rule.client_id,
    title: `${rule.name}: ${formatPortMappings(rule.mappings)}`,
    value: rule.name,
  }));
  if (rules.length > 12)
    items.push({
      label: "Additional rules",
      title: `${rules.length - 12} additional exact revisions are included`,
      value: `+${rules.length - 12} more`,
    });
  return items;
}
