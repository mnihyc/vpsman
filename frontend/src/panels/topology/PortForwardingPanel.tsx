import {
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  CirclePlus,
  Copy,
  Info,
  Pencil,
  Power,
  PowerOff,
  RefreshCcw,
  RotateCcw,
  Search,
  ShieldAlert,
  Trash2,
  X,
} from "lucide-react";
import {
  type FormEvent,
  type ReactNode,
  forwardRef,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ActionFeedback, type ActionFeedbackTone } from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
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
  PortForwardRuleRecord,
  ResolveHostnameResponse,
  UpdatePortForwardRuleRequest,
} from "../../types";
import {
  dispatchFailureReason,
  formatCompactTime,
  shortId,
} from "../../utils";

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
  onLoad: () => Promise<void>;
  onMutate: (
    ruleId: string,
    operation: "enable" | "disable" | "delete" | "forget" | "reapply",
    request: { expected_revision: number; confirmed: boolean; reason?: string | null },
  ) => Promise<PortForwardMutationResponse>;
  onResolveHostname: (hostname: string) => Promise<ResolveHostnameResponse>;
  onUpdate: (
    ruleId: string,
    request: UpdatePortForwardRuleRequest,
  ) => Promise<PortForwardMutationResponse>;
  rules: PortForwardRuleRecord[];
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
  | { kind: "single"; operation: "enable" | "disable" | "delete" | "forget" | "reapply"; origin: "detail" | "registry"; reason?: string; rule: PortForwardRuleRecord }
  | { kind: "bulk"; action: PortForwardBulkAction; rules: PortForwardRuleRecord[] };

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
  rules,
}: PortForwardingPanelProps) {
  const [query, setQuery] = useState("");
  const [clientFilter, setClientFilter] = useState("all");
  const [familyFilter, setFamilyFilter] = useState("all");
  const [protocolFilter, setProtocolFilter] = useState("all");
  const [desiredFilter, setDesiredFilter] = useState("all");
  const [runtimeFilter, setRuntimeFilter] = useState("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [editor, setEditor] = useState<{
    draft: EditorDraft;
    editing: PortForwardRuleRecord | null;
  } | null>(null);
  const [confirmation, setConfirmation] = useState<ConfirmationState | null>(null);
  const [pending, setPending] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [forgetReason, setForgetReason] = useState("");
  const editorRef = useRef<HTMLElement | null>(null);
  const detailRef = useRef<HTMLElement | null>(null);
  const writeBoundary = "Operator role and network:write scope required";
  const forgetBoundary = "Admin role and network:write scope required";

  const agentById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent])),
    [agents],
  );
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return rules.filter((rule) => {
      const family = rule.target_ip.includes(":") ? "ipv6" : "ipv4";
      const haystack = [
        rule.name,
        rule.client_id,
        agentById.get(rule.client_id)?.display_name ?? "",
        rule.target_ip,
        rule.protocol,
        formatPortMappings(rule.mappings),
      ]
        .join(" ")
        .toLowerCase();
      return (
        (!needle || haystack.includes(needle)) &&
        (clientFilter === "all" || rule.client_id === clientFilter) &&
        (familyFilter === "all" || family === familyFilter) &&
        (protocolFilter === "all" || rule.protocol === protocolFilter) &&
        (desiredFilter === "all" || rule.desired_status === desiredFilter) &&
        (runtimeFilter === "all" || rule.runtime_status === runtimeFilter)
      );
    });
  }, [
    agentById,
    clientFilter,
    desiredFilter,
    familyFilter,
    protocolFilter,
    query,
    rules,
    runtimeFilter,
  ]);
  const selectedRules = rules.filter((rule) => selected.has(rule.id));
  const selectedEnableRules = selectedRules.filter(
    (rule) =>
      !rule.enabled &&
      !rule.deleted_at &&
      agentById.get(rule.client_id)?.capabilities.port_forwarding?.status === "supported",
  );
  const selectedDisableRules = selectedRules.filter((rule) => rule.enabled && !rule.deleted_at);
  const selectedActiveRules = selectedRules.filter((rule) => !rule.deleted_at);
  const selectedReapplyRules = selectedActiveRules.filter(
    (rule) =>
      agentById.get(rule.client_id)?.capabilities.port_forwarding?.status === "supported",
  );
  const expandedRule = rules.find((rule) => rule.id === expandedId) ?? null;
  const enabledCount = rules.filter((rule) => rule.enabled && !rule.deleted_at).length;
  const appliedCount = rules.filter((rule) =>
    ["applied", "applied_warning"].includes(rule.runtime_status),
  ).length;
  const pendingCount = rules.filter((rule) => rule.runtime_status === "pending").length;
  const attentionCount = rules.filter((rule) =>
    ["applied_warning", "drifted", "failed", "unsupported", "removal_pending", "unknown"].includes(
      rule.runtime_status,
    ),
  ).length;
  const supportedAgents = agents.filter(
    (agent) => agent.capabilities.port_forwarding?.status === "supported",
  ).length;

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
    if (!expandedRule) return;
    window.setTimeout(() => {
      if (detailRef.current) {
        scrollIntoViewWithMotion(detailRef.current, { block: "nearest" });
      }
      detailRef.current?.focus({ preventScroll: true });
    }, 0);
  }, [expandedRule?.id]);

  useEffect(() => {
    setForgetReason("");
  }, [expandedRule?.id]);

  function openCreate() {
    if (!canWrite) return;
    setExpandedId(null);
    setFeedback(null);
    setEditor({
      editing: null,
      draft: { ...EMPTY_DRAFT },
    });
  }

  function openEdit(rule: PortForwardRuleRecord) {
    if (!canWrite) return;
    const expressions = mappingsToExpressions(rule.mappings);
    setExpandedId(null);
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
    setExpandedId(null);
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
    const actionAnchor = snapshot.kind === "save"
      ? "editor"
      : snapshot.kind === "single"
        ? snapshot.origin
        : "registry";
    if (!canWrite || (snapshot.kind === "single" && snapshot.operation === "forget" && !canForget)) {
      setConfirmation(null);
      setFeedback({
        anchor: actionAnchor,
        message: snapshot.kind === "single" && snapshot.operation === "forget" ? forgetBoundary : writeBoundary,
        tone: "danger",
      });
      return;
    }
    setConfirmation(null);
    setPending(true);
    setFeedback({ anchor: actionAnchor, message: actionProgressLabel(snapshot), tone: "progress" });
    try {
      if (snapshot.kind === "save") {
        const response = await saveDraft(snapshot.draft, snapshot.editing, true);
        setEditor(null);
        setFeedback({
          ...syncFeedback(response, snapshot.editing ? "Rule updated" : "Rule created"),
          anchor: "registry",
        });
      } else if (snapshot.kind === "single") {
        const response = await onMutate(snapshot.rule.id, snapshot.operation, {
          confirmed: true,
          expected_revision: snapshot.rule.revision,
          reason: snapshot.reason,
        });
        if (snapshot.operation === "delete" || snapshot.operation === "forget") {
          setExpandedId((current) => (current === snapshot.rule.id ? null : current));
          setSelected((current) => {
            const next = new Set(current);
            next.delete(snapshot.rule.id);
            return next;
          });
        }
        setFeedback({
          ...syncFeedback(response, singleActionPast(snapshot.operation)),
          anchor: snapshot.operation === "delete" || snapshot.operation === "forget"
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
        setSelected(new Set());
        setFeedback({
          ...bulkSyncFeedback(response, snapshot.action, snapshot.rules.length),
          anchor: "registry",
        });
      }
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
    if (!draft.targetIp) throw new Error("Resolve and select a literal target IP");
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
        setConfirmation({ kind: "save", ...editor });
        return;
      }
      setPending(true);
      setFeedback({ anchor: "editor", message: "Saving disabled rule", tone: "progress" });
      const response = await saveDraft(editor.draft, editor.editing, false);
      setEditor(null);
      setFeedback({
        ...syncFeedback(response, editor.editing ? "Rule updated" : "Rule created"),
        anchor: "registry",
      });
    } catch (actionError) {
      if (editor) setEditor(editor);
      setFeedback({
        anchor: "editor",
        message: actionError instanceof Error ? actionError.message : "Rule is invalid",
        tone: "danger",
      });
    } finally {
      setPending(false);
    }
  }

  async function refreshRules() {
    setFeedback({ anchor: "summary", message: "Reloading stored forwarding state", tone: "progress" });
    try {
      await onLoad();
      setFeedback({ anchor: "summary", message: "Latest stored forwarding state loaded", tone: "success" });
    } catch (refreshError) {
      setFeedback({
        anchor: "summary",
        message:
          refreshError instanceof Error
            ? refreshError.message
            : "Forwarding state refresh failed",
        tone: "danger",
      });
    }
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
          <div className="headerActionStack">
            <button aria-busy={loading} className="secondaryAction" disabled={loading || pending} onClick={() => void refreshRules()} title="Reload latest stored desired state and agent evidence; this does not request a live agent inspection" type="button">
              <RefreshCcw size={16} /> {loading ? "Refreshing" : "Refresh"}
            </button>
            <button className="primaryAction" disabled={!canWrite || Boolean(editor) || pending} onClick={openCreate} title={!canWrite ? writeBoundary : editor ? "Close the current editor first" : "Create a port-forward rule"} type="button">
              <CirclePlus size={17} /> Create rule
            </button>
          </div>
        </div>
        <ActionFeedback className="localActionFeedback portForwardActionFeedback" message={error ?? (loading ? "Reloading stored forwarding state" : feedback?.anchor === "summary" ? feedback.message : null)} tone={error ? "danger" : loading ? "progress" : feedback?.anchor === "summary" ? feedback.tone : undefined} />
        <div aria-label="Port-forwarding summary" className="networkMetricStrip">
          <Metric label="Rules" value={rules.length} />
          <Metric label="Enabled" value={enabledCount} />
          <Metric label="Applied" value={appliedCount} />
          <Metric label="Pending" value={pendingCount} />
          <Metric label="Attention" tone={attentionCount > 0 ? "warning" : "normal"} value={attentionCount} />
          <Metric label="NFT-capable" tone={supportedAgents < agents.length ? "warning" : "normal"} value={`${supportedAgents}/${agents.length}`} />
        </div>
      </section>

      <section className="fleetPanel portForwardRegistry">
        <div className="sectionHeader compactSectionHeader">
          <div>
            <h2>Rules</h2>
            <span>Desired state and latest owned-table evidence</span>
          </div>
        </div>
        <div className="portForwardToolbar">
          <label className="searchControl compactSearch">
            <Search size={15} />
            <input aria-label="Search port-forward rules" onChange={(event) => setQuery(event.target.value)} placeholder="Search rules" value={query} />
          </label>
          <FilterSelect label="VPS" onChange={setClientFilter} value={clientFilter}>
            <option value="all">All VPSs</option>
            {agents.map((agent) => <option key={agent.id} value={agent.id}>{agent.display_name || agent.id}</option>)}
          </FilterSelect>
          <FilterSelect label="Family" onChange={setFamilyFilter} value={familyFilter}>
            <option value="all">All families</option><option value="ipv4">IPv4</option><option value="ipv6">IPv6</option>
          </FilterSelect>
          <FilterSelect label="Protocol" onChange={setProtocolFilter} value={protocolFilter}>
            <option value="all">All protocols</option><option value="tcp">TCP</option><option value="udp">UDP</option><option value="both">Both</option>
          </FilterSelect>
          <FilterSelect label="Desired" onChange={setDesiredFilter} value={desiredFilter}>
            <option value="all">All desired</option><option value="enabled">Enabled</option><option value="disabled">Disabled</option><option value="removal_pending">Removal pending</option>
          </FilterSelect>
          <FilterSelect label="Runtime" onChange={setRuntimeFilter} value={runtimeFilter}>
            <option value="all">All runtime</option><option value="applied">Applied</option><option value="applied_warning">Applied with warning</option><option value="pending">Pending</option><option value="drifted">Drifted</option><option value="failed">Failed</option><option value="unsupported">Unsupported</option><option value="unknown">Unknown</option>
          </FilterSelect>
        </div>
        {selectedRules.length > 0 && (
          <div className="selectionActionBar portForwardBulkBar" aria-label="Selected port-forward actions">
            <span>{selectedRules.length} selected</span>
            <button className="secondaryAction compactAction" disabled={!canWrite || pending || Boolean(editor) || selectedEnableRules.length === 0} onClick={() => setConfirmation({ kind: "bulk", action: "enable", rules: selectedEnableRules })} title={!canWrite ? writeBoundary : `Enable ${selectedEnableRules.length} eligible selected rule${selectedEnableRules.length === 1 ? "" : "s"}`} type="button"><Power size={14} /> Enable {selectedEnableRules.length}</button>
            <button className="secondaryAction compactAction" disabled={!canWrite || pending || Boolean(editor) || selectedDisableRules.length === 0} onClick={() => setConfirmation({ kind: "bulk", action: "disable", rules: selectedDisableRules })} title={!canWrite ? writeBoundary : `Disable ${selectedDisableRules.length} eligible selected rule${selectedDisableRules.length === 1 ? "" : "s"}`} type="button"><PowerOff size={14} /> Disable {selectedDisableRules.length}</button>
            <button className="secondaryAction compactAction" disabled={!canWrite || pending || Boolean(editor) || selectedReapplyRules.length === 0} onClick={() => setConfirmation({ kind: "bulk", action: "reapply", rules: selectedReapplyRules })} title={!canWrite ? writeBoundary : `Reapply complete forwarding tables for ${selectedReapplyRules.length} eligible selected rule${selectedReapplyRules.length === 1 ? "" : "s"}`} type="button"><RotateCcw size={14} /> Reapply {selectedReapplyRules.length}</button>
            <button className="dangerAction compactAction" disabled={!canWrite || pending || Boolean(editor) || selectedActiveRules.length === 0} onClick={() => setConfirmation({ kind: "bulk", action: "delete", rules: selectedActiveRules })} title={!canWrite ? writeBoundary : `Delete ${selectedActiveRules.length} eligible selected rule${selectedActiveRules.length === 1 ? "" : "s"}`} type="button"><Trash2 size={14} /> Delete {selectedActiveRules.length}</button>
          </div>
        )}
        <ActionFeedback className="localActionFeedback portForwardRegistryFeedback" message={feedback?.anchor === "registry" ? feedback.message : null} tone={feedback?.anchor === "registry" ? feedback.tone : undefined} />
        {filtered.length === 0 ? (
          <div className="emptyState compactEmptyState">
            <strong>{rules.length === 0 ? "No port-forward rules" : "No matching rules"}</strong>
            <span>{rules.length === 0 ? "Create a disabled draft, or enable a rule on a VPS that reports nftables support." : "Adjust the filters."}</span>
          </div>
        ) : (
          <div className="portForwardTableWrap">
            <table aria-label="Port-forward rules" className="portForwardTable">
              <thead><tr>
                <th className="selectionCell"><input aria-label="Select visible port-forward rules" checked={filtered.every((rule) => selected.has(rule.id))} disabled={!canWrite} title={!canWrite ? writeBoundary : "Select all visible rules"} onChange={(event) => {
                  const next = new Set(selected); for (const rule of filtered) event.target.checked ? next.add(rule.id) : next.delete(rule.id); setSelected(next);
                }} type="checkbox" /></th>
                <th>Rule / VPS</th><th>Mapping</th><th>Return</th><th>Desired</th><th>Runtime</th><th>NAT matches</th><th aria-label="Actions" />
              </tr></thead>
              <tbody>{filtered.map((rule) => {
                const agent = agentById.get(rule.client_id);
                const targetAddress = rule.target_ip.includes(":") ? `[${rule.target_ip}]` : rule.target_ip;
                const mapping = `${rule.protocol.toUpperCase()} · ${rule.mappings.map((item) => formatPortRange(item.incoming)).join(",")} -> ${targetAddress}:${rule.mappings.map((item) => formatPortRange(item.target)).join(",")}`;
                return <tr className={expandedId === rule.id ? "expanded" : ""} key={rule.id} onClick={() => setExpandedId((current) => current === rule.id ? null : rule.id)} tabIndex={0} onKeyDown={(event) => { if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); setExpandedId((current) => current === rule.id ? null : rule.id); } }}>
                  <td className="selectionCell" data-label="Select" onClick={(event) => event.stopPropagation()}><input aria-label={`Select ${rule.name}`} checked={selected.has(rule.id)} disabled={!canWrite} title={!canWrite ? writeBoundary : `Select ${rule.name}`} onChange={(event) => { const next = new Set(selected); event.target.checked ? next.add(rule.id) : next.delete(rule.id); setSelected(next); }} type="checkbox" /></td>
                  <td data-label="Rule / VPS"><strong className="truncateValue" title={rule.name}>{rule.name}</strong><span className="truncateValue" title={`${agent?.display_name || rule.client_id} (${rule.client_id})`}>{agent?.display_name || rule.client_id}</span><div className="portForwardMobileStatus"><StatusBadge status={rule.desired_status} /><StatusBadge status={rule.runtime_status} title={runtimeStatusTitle(rule)} /></div></td>
                  <td data-label="Mapping"><span className="truncateValue mappingValue" title={mapping}>{mapping}</span></td>
                  <td data-label="Return"><span title={rule.masquerade ? "Masquerade only connections DNATed by this rule" : "Preserve the original source address"}>{rule.masquerade ? "Masquerade" : "Preserve source"}</span></td>
                  <td data-label="Desired"><StatusBadge status={rule.desired_status} /></td>
                  <td data-label="Runtime"><StatusBadge status={rule.runtime_status} title={runtimeStatusTitle(rule)} /></td>
                  <td data-label="NAT matches"><span title="First-packet NAT matches since the latest table apply; this is not throughput">{rule.nat_matches.toLocaleString()}</span></td>
                  <td className="rowActions" data-label="Actions" onClick={(event) => event.stopPropagation()}>
                    <div className="portForwardDesktopActions">
                      {!rule.deleted_at && <button className="iconButton" disabled={!canWrite || pending || Boolean(editor)} onClick={() => openEdit(rule)} title={!canWrite ? writeBoundary : "Edit rule"} type="button"><Pencil size={15} /></button>}
                      {!rule.deleted_at && <button className="iconButton" disabled={!canWrite || pending || Boolean(editor)} onClick={() => openClone(rule)} title={!canWrite ? writeBoundary : "Clone as a disabled rule"} type="button"><Copy size={15} /></button>}
                      {!rule.deleted_at && <button className="iconButton" disabled={!canWrite || pending || Boolean(editor) || (!rule.enabled && agent?.capabilities.port_forwarding?.status !== "supported")} onClick={() => setConfirmation({ kind: "single", operation: rule.enabled ? "disable" : "enable", origin: "registry", rule })} title={!canWrite ? writeBoundary : rule.enabled ? "Disable rule" : agent?.capabilities.port_forwarding?.status === "supported" ? "Enable rule" : capabilityLabel(agent?.capabilities.port_forwarding?.status, agent?.capabilities.port_forwarding?.reason)} type="button">{rule.enabled ? <PowerOff size={15} /> : <Power size={15} />}</button>}
                      {!rule.deleted_at && <button className="iconButton" disabled={!canWrite || pending || Boolean(editor) || agent?.capabilities.port_forwarding?.status !== "supported"} onClick={() => setConfirmation({ kind: "single", operation: "reapply", origin: "registry", rule })} title={!canWrite ? writeBoundary : agent?.capabilities.port_forwarding?.status === "supported" ? "Reapply this VPS's complete forwarding table" : capabilityLabel(agent?.capabilities.port_forwarding?.status, agent?.capabilities.port_forwarding?.reason)} type="button"><RotateCcw size={15} /></button>}
                      {!rule.deleted_at && <button className="iconButton dangerIconButton" disabled={!canWrite || pending || Boolean(editor)} onClick={() => setConfirmation({ kind: "single", operation: "delete", origin: "registry", rule })} title={!canWrite ? writeBoundary : "Delete rule"} type="button"><Trash2 size={15} /></button>}
                    </div>
                    <button aria-label={`${expandedId === rule.id ? "Collapse" : "Expand"} ${rule.name} rule details`} className="iconButton portForwardMobileDetailsButton" onClick={() => setExpandedId((current) => current === rule.id ? null : rule.id)} title={`${expandedId === rule.id ? "Close" : "Open"} rule details`} type="button">{expandedId === rule.id ? <ChevronUp size={16} /> : <ChevronDown size={16} />}</button>
                  </td>
                </tr>;
              })}</tbody>
            </table>
          </div>
        )}
      </section>

      {expandedRule && (
        <section aria-label={`Details for ${expandedRule.name}`} className="fleetPanel portForwardDetails" ref={detailRef} tabIndex={-1}>
          <div className="sectionHeader">
            <div><h2>{expandedRule.name}</h2><span title={expandedRule.id}>{shortId(expandedRule.id)} · revision {expandedRule.revision}</span></div>
            <button aria-label="Close port-forward details" className="iconButton" onClick={() => setExpandedId(null)} title="Close details" type="button"><X size={17} /></button>
          </div>
          <dl className="portForwardDetailGrid">
            <Detail label="VPS" value={`${agentById.get(expandedRule.client_id)?.display_name || expandedRule.client_id} (${expandedRule.client_id})`} />
            <Detail label="Protocol" value={expandedRule.protocol.toUpperCase()} />
            <Detail label="Desired" value={expandedRule.desired_status.replace(/_/g, " ")} />
            <Detail label="Listener scope" value={`All current local ${expandedRule.target_ip.includes(":") ? "IPv6" : "IPv4"} addresses`} />
            <Detail label="Target" value={expandedRule.target_ip} />
            <Detail label="Mappings" value={formatPortMappings(expandedRule.mappings)} />
            <Detail label="Return path" value={expandedRule.masquerade ? "Targeted masquerade" : "Preserve source"} />
            <Detail label="Runtime" value={expandedRule.runtime_error ? `${expandedRule.runtime_status}: ${expandedRule.runtime_error}` : expandedRule.runtime_status} />
            <Detail label="Capability" title={agentById.get(expandedRule.client_id)?.capabilities.port_forwarding?.reason ?? undefined} value={capabilitySummary(agentById.get(expandedRule.client_id)?.capabilities.port_forwarding)} />
            <Detail label={`${expandedRule.target_ip.includes(":") ? "IPv6" : "IPv4"} forwarding`} tone={expandedRule.forwarding_enabled === false ? "warning" : "normal"} value={forwardingSummary(expandedRule.forwarding_enabled)} />
            <Detail label="Observed" value={expandedRule.runtime_observed_unix ? formatCompactTime(new Date(expandedRule.runtime_observed_unix * 1000).toISOString()) : "No agent evidence"} />
            <Detail label="NAT matches" value={expandedRule.nat_matches.toLocaleString()} />
            <Detail label="Updated" value={formatCompactTime(expandedRule.updated_at)} />
            <Detail display={shortId(expandedRule.desired_hash)} label="Control desired" title={`Control-plane desired config hash: ${expandedRule.desired_hash ?? "not applicable"}`} value={expandedRule.desired_hash ?? "Not applicable"} />
            <Detail display={shortId(expandedRule.agent_desired_hash)} label="Agent desired" title={`Latest desired config hash reported by the agent: ${expandedRule.agent_desired_hash ?? "not reported"}`} value={expandedRule.agent_desired_hash ?? "Not reported"} />
            <Detail display={shortId(expandedRule.observed_hash)} label="Observed table" title={`Latest normalized owned-table hash reported by the agent: ${expandedRule.observed_hash ?? "no owned table hash"}`} value={expandedRule.observed_hash ?? "No owned table hash"} />
          </dl>
          <ActionFeedback className="localActionFeedback portForwardDetailFeedback" message={feedback?.anchor === "detail" ? feedback.message : null} tone={feedback?.anchor === "detail" ? feedback.tone : undefined} />
          {!expandedRule.deleted_at && (
            <div aria-label={`Actions for ${expandedRule.name}`} className="portForwardDetailActions">
              <button className="secondaryAction compactAction" disabled={!canWrite || pending || Boolean(editor)} onClick={() => openEdit(expandedRule)} title={!canWrite ? writeBoundary : "Edit rule"} type="button"><Pencil size={14} /> Edit</button>
              <button className="secondaryAction compactAction" disabled={!canWrite || pending || Boolean(editor)} onClick={() => openClone(expandedRule)} title={!canWrite ? writeBoundary : "Clone as a disabled rule"} type="button"><Copy size={14} /> Clone</button>
              <button className="secondaryAction compactAction" disabled={!canWrite || pending || Boolean(editor) || (!expandedRule.enabled && agentById.get(expandedRule.client_id)?.capabilities.port_forwarding?.status !== "supported")} onClick={() => setConfirmation({ kind: "single", operation: expandedRule.enabled ? "disable" : "enable", origin: "detail", rule: expandedRule })} title={!canWrite ? writeBoundary : expandedRule.enabled ? "Disable rule" : capabilityActionTitle(agentById.get(expandedRule.client_id), "Enable rule")} type="button">{expandedRule.enabled ? <PowerOff size={14} /> : <Power size={14} />} {expandedRule.enabled ? "Disable" : "Enable"}</button>
              <button className="secondaryAction compactAction" disabled={!canWrite || pending || Boolean(editor) || agentById.get(expandedRule.client_id)?.capabilities.port_forwarding?.status !== "supported"} onClick={() => setConfirmation({ kind: "single", operation: "reapply", origin: "detail", rule: expandedRule })} title={!canWrite ? writeBoundary : capabilityActionTitle(agentById.get(expandedRule.client_id), "Reapply this VPS's complete forwarding table")} type="button"><RotateCcw size={14} /> Reapply</button>
              <button className="dangerAction compactAction" disabled={!canWrite || pending || Boolean(editor)} onClick={() => setConfirmation({ kind: "single", operation: "delete", origin: "detail", rule: expandedRule })} title={!canWrite ? writeBoundary : "Delete rule"} type="button"><Trash2 size={14} /> Delete</button>
            </div>
          )}
          {expandedRule.deleted_at && !expandedRule.removal_confirmed_at && (
            <div className="portForwardRemovalNotice">
              <ShieldAlert size={17} />
              <span>Removal pending until the agent confirms the owned table no longer contains this rule.</span>
              <label className="forgetReasonField"><span className="srOnly">Forget reason</span><input disabled={!canForget || pending} maxLength={512} onChange={(event) => setForgetReason(event.target.value)} placeholder="Decommission reason" title={!canForget ? forgetBoundary : forgetReason} value={forgetReason} /></label>
              <button className="dangerAction compactAction" disabled={!canForget || pending || !forgetReason.trim()} onClick={() => setConfirmation({ kind: "single", operation: "forget", origin: "detail", reason: forgetReason.trim(), rule: expandedRule })} title={!canForget ? forgetBoundary : "Forget only when this VPS is permanently unreachable or decommissioned"} type="button">Forget</button>
            </div>
          )}
        </section>
      )}

      {editor && (
        <PortForwardEditor
          agents={agents}
          draft={editor.draft}
          editing={editor.editing}
          feedback={feedback?.anchor === "editor" ? feedback : null}
          onChange={(draft) => setEditor((current) => current ? { ...current, draft } : current)}
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
        items={confirmationItems(confirmation, agentById)}
        onCancel={() => setConfirmation(null)}
        onConfirm={() => confirmation && void executeConfirmation(confirmation)}
        open={Boolean(confirmation)}
        pending={false}
        title={confirmationTitle(confirmation)}
        tone={confirmation?.kind === "single" && ["delete", "forget"].includes(confirmation.operation) || confirmation?.kind === "bulk" && confirmation.action === "delete" ? "danger" : "normal"}
      />
    </div>
  );
}

const PortForwardEditor = forwardRef<HTMLElement, {
  agents: AgentView[];
  draft: EditorDraft;
  editing: PortForwardRuleRecord | null;
  feedback: Feedback | null;
  onChange: (draft: EditorDraft) => void;
  onClose: () => void;
  onResolveHostname: (hostname: string) => Promise<ResolveHostnameResponse>;
  onSubmit: (event: FormEvent) => void;
  pending: boolean;
}>(function PortForwardEditor({
  agents,
  draft,
  editing,
  feedback,
  onChange,
  onClose,
  onResolveHostname,
  onSubmit,
  pending,
}, ref) {
  const [resolution, setResolution] = useState<ResolveHostnameResponse | null>(null);
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const targetInputRef = useRef(draft.targetInput);
  const draftRef = useRef(draft);
  targetInputRef.current = draft.targetInput;
  draftRef.current = draft;
  const mappingStarted = Boolean(draft.incoming.trim() || draft.target.trim());
  const mappingPreview = useMemo(() => {
    try {
      return { error: null, mappings: pairPortExpressions(draft.incoming, draft.target) };
    } catch (error) {
      return { error: error instanceof Error ? error.message : "Invalid mapping", mappings: [] };
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

  async function resolveHostname() {
    const requestedHostname = targetInputRef.current.trim();
    setResolving(true); setResolveError(null); setResolution(null);
    try {
      const result = await onResolveHostname(requestedHostname);
      if (targetInputRef.current.trim() !== requestedHostname) return;
      setResolution(result);
      onChange({ ...draftRef.current, targetIp: "" });
    } catch (error) {
      if (targetInputRef.current.trim() !== requestedHostname) return;
      setResolveError(error instanceof Error ? error.message : "Hostname resolution failed");
    } finally { setResolving(false); }
  }

  return (
    <section className="fleetPanel portForwardEditor" ref={ref}>
      <div className="sectionHeader">
        <div><h2>{editing ? "Edit port-forward rule" : "Create port-forward rule"}</h2><span>{editing ? `Revision ${editing.revision}` : "New per-VPS desired state"}</span></div>
        <button aria-label="Close port-forward editor" className="iconButton" disabled={pending} onClick={onClose} title="Close editor" type="button"><X size={17} /></button>
      </div>
      <form className="portForwardForm" onSubmit={onSubmit}>
        <ActionFeedback className="localActionFeedback" message={feedback?.message} tone={feedback?.tone} />
        <label><span>VPS</span><select disabled={Boolean(editing) || pending} onChange={(event) => onChange({ ...draft, clientId: event.target.value })} required value={draft.clientId}>
          <option value="">Select a VPS</option>{agents.map((agent) => <option key={agent.id} value={agent.id}>{agent.display_name || agent.id}</option>)}
        </select></label>
        <label><span>Name</span><input disabled={pending} maxLength={128} onChange={(event) => onChange({ ...draft, name: event.target.value })} placeholder="Public web" required title={draft.name} value={draft.name} /></label>
        <fieldset className="compactFieldset"><legend>Protocol</legend><div className="segmentedControl" role="group" aria-label="Protocol">
          {(["tcp", "udp", "both"] as const).map((protocol) => <button aria-pressed={draft.protocol === protocol} className={draft.protocol === protocol ? "active" : ""} disabled={pending} key={protocol} onClick={() => onChange({ ...draft, protocol })} type="button">{protocol === "both" ? "Both" : protocol.toUpperCase()}</button>)}
        </div></fieldset>
        <label><span>Incoming ports</span><input disabled={pending} onChange={(event) => onChange({ ...draft, incoming: event.target.value })} placeholder="80,443,10000-10010" required title={draft.incoming} value={draft.incoming} /><small>PORT or START-END, comma separated</small></label>
        <label><span>Target ports</span><input disabled={pending} onChange={(event) => onChange({ ...draft, target: event.target.value })} placeholder="8080 or 8080,20000-20010" required title={draft.target} value={draft.target} /><small>One port for all, or corresponding items</small></label>
        <div className="targetAddressField">
          <label><span>Target IP or hostname</span><input disabled={pending} onChange={(event) => {
            const value = event.target.value; const literal = literalIpFamily(value); setResolution(null); setResolveError(null); onChange({ ...draft, targetInput: value, targetIp: literal ? value.trim() : "" });
          }} placeholder="192.0.2.40 or app.internal" required title={draft.targetInput} value={draft.targetInput} /></label>
          {!targetIsLiteral && draft.targetInput.trim() && <button className="secondaryAction compactAction" disabled={pending || resolving} onClick={() => void resolveHostname()} title="Resolve on the control plane and select one literal address" type="button"><RefreshCcw size={14} /> {resolving ? "Resolving" : "Resolve"}</button>}
        </div>
        <ActionFeedback message={resolveError} tone="danger" />
        {resolution && <fieldset className="dnsCandidateList" disabled={pending}><legend>Resolved addresses</legend>{resolution.candidates.map((candidate) => <label key={candidate.address} title={`${candidate.family.toUpperCase()} ${candidate.address}`}><input checked={draft.targetIp === candidate.address} name="resolved-target" onChange={() => onChange({ ...draft, targetIp: candidate.address })} type="radio" /><span>{candidate.address}</span><small>{candidate.family.toUpperCase()}</small></label>)}</fieldset>}
        <fieldset className="compactFieldset returnPathField"><legend>Return path</legend><div className="segmentedControl" role="group" aria-label="Return path">
          <button aria-pressed={draft.masquerade} className={draft.masquerade ? "active" : ""} disabled={pending} onClick={() => onChange({ ...draft, masquerade: true })} title="Masquerade only connections DNATed by this rule" type="button">Masquerade</button>
          <button aria-pressed={!draft.masquerade} className={!draft.masquerade ? "active" : ""} disabled={pending} onClick={() => onChange({ ...draft, masquerade: false })} title="Keep source addresses; the target must have a valid return route" type="button">Preserve source</button>
        </div></fieldset>
        <label className="compactCheckbox portForwardEnabled"><input checked={draft.enabled} disabled={pending} onChange={(event) => onChange({ ...draft, enabled: event.target.checked })} type="checkbox" /><span>Enabled</span></label>
        <div className={`portMappingPreview ${!mappingStarted ? "idle" : mappingPreview.error ? "invalid" : "valid"}`}>
          {!mappingStarted
            ? <><Info size={16} /><span title="Enter incoming and target ports to preview the exact mappings">Enter incoming and target ports to preview the exact mappings</span></>
            : mappingPreview.error
              ? <><ShieldAlert size={16} /><span title={mappingPreview.error}>{mappingPreview.error}</span></>
              : <><CheckCircle2 size={16} /><span title={formatPortMappings(mappingPreview.mappings)}>{formatPortMappings(mappingPreview.mappings)} · {draft.targetIp || "select target IP"}</span></>}
        </div>
        {selectedAgent && capability?.status !== "supported" && <div className="portForwardCapabilityNotice"><ShieldAlert size={16} /><span title={capability?.reason ?? capability?.status ?? "unknown"}>{capabilityLabel(capability?.status, capability?.reason)}</span></div>}
        <div className="formActions"><button className="secondaryAction" disabled={pending} onClick={onClose} type="button">Cancel</button><button className="primaryAction" disabled={saveDisabled} title={saveDisabledReason ?? (editing ? "Review rule update" : "Review rule creation")} type="submit">{editing ? "Save changes" : "Create rule"}</button></div>
      </form>
    </section>
  );
});

function Metric({ label, tone = "normal", value }: { label: string; tone?: "normal" | "warning"; value: string | number }) {
  return <span className={tone === "warning" ? "hasAttention" : ""}><small>{label}</small><strong>{value}</strong></span>;
}

function FilterSelect({ children, label, onChange, value }: { children: ReactNode; label: string; onChange: (value: string) => void; value: string }) {
  return <label className="compactFilter" title={`Filter by ${label.toLowerCase()}`}><span className="srOnly">{label}</span><select aria-label={`${label} filter`} onChange={(event) => onChange(event.target.value)} value={value}>{children}</select></label>;
}

function Detail({ display, label, title, tone = "normal", value }: { display?: string; label: string; title?: string; tone?: "normal" | "warning"; value: string }) {
  return <div className={tone === "warning" ? "hasAttention" : ""}><dt>{label}</dt><dd title={title ?? value}>{display ?? value}</dd></div>;
}

function StatusBadge({ status, title }: { status: string; title?: string }) {
  const label = status === "applied_warning" ? "applied · warning" : status.replace(/_/g, " ");
  return <span className={`portForwardStatus status-${status}`} title={title ?? label}>{label}</span>;
}

function runtimeStatusTitle(rule: PortForwardRuleRecord) {
  if (rule.runtime_error) return rule.runtime_error;
  if (rule.runtime_status === "applied_warning") {
    return `${rule.target_ip.includes(":") ? "IPv6" : "IPv4"} forwarding is disabled outside vpsman`;
  }
  switch (rule.runtime_status) {
    case "applied": return "Owned nftables table matches desired state; target reachability is not tested";
    case "disabled": return "Rule is stored but omitted from the agent's desired nftables table";
    case "absent": return "No owned nftables table is expected or observed";
    case "pending": return "Desired state is queued; latest agent evidence has not confirmed it yet";
    case "drifted": return "Latest owned-table evidence does not match the agent's desired state";
    case "failed": return "The agent could not inspect or apply the owned nftables table";
    case "unsupported": return "This agent cannot manage port forwarding with the required nftables features";
    case "removal_pending": return "Deletion is saved but agent cleanup evidence is still pending";
    default: return "No current agent evidence is available";
  }
}

function forwardingSummary(value?: boolean | null) {
  if (value === true) return "Enabled";
  if (value === false) return "Disabled outside vpsman";
  return "Not reported";
}

function capabilitySummary(capability?: AgentView["capabilities"]["port_forwarding"]) {
  if (!capability) return "Not reported";
  const status = capability.status.replace(/_/g, " ");
  return capability.nft_version ? `${status} · ${capability.nft_version}` : status;
}

function literalIpFamily(value: string): "ipv4" | "ipv6" | null {
  const input = value.trim();
  const ipv4 = input.split(".");
  if (ipv4.length === 4 && ipv4.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)) return "ipv4";
  if (input.includes(":") && /^[0-9a-f:.]+$/i.test(input)) {
    try { new URL(`http://[${input}]/`); return "ipv6"; } catch { return null; }
  }
  return null;
}

function validateEditor(draft: EditorDraft, agent: AgentView | undefined) {
  if (!draft.name.trim()) throw new Error("Rule name is required");
  if (utf8ByteLength(draft.name.trim()) > MAX_RULE_NAME_BYTES) throw new Error(`Rule name must not exceed ${MAX_RULE_NAME_BYTES} UTF-8 bytes`);
  if (!agent) throw new Error("Select a VPS");
  if (!draft.targetIp) throw new Error("Resolve and select one literal target IP");
  if (draft.enabled && agent.capabilities.port_forwarding?.status !== "supported") throw new Error(capabilityLabel(agent.capabilities.port_forwarding?.status, agent.capabilities.port_forwarding?.reason));
}

function capabilityLabel(status?: string, reason?: string | null) {
  if (reason) return reason;
  switch (status) {
    case "nft_missing": return "nft is not installed on this VPS";
    case "insufficient_privilege": return "Agent lacks CAP_NET_ADMIN in the host network namespace";
    case "inet_nat_unsupported": return "Kernel or nft userspace does not support the required inet NAT features";
    case "probe_failed": return "Agent nftables capability probe failed";
    default: return "Agent has not reported port-forwarding capability";
  }
}

function capabilityActionTitle(agent: AgentView | undefined, supportedTitle: string) {
  return agent?.capabilities.port_forwarding?.status === "supported"
    ? supportedTitle
    : capabilityLabel(
        agent?.capabilities.port_forwarding?.status,
        agent?.capabilities.port_forwarding?.reason,
      );
}

function nextCloneName(name: string, rules: PortForwardRuleRecord[], clientId: string) {
  const base = fitNameWithSuffix(name, " (cloned)");
  if (!rules.some((rule) => rule.client_id === clientId && rule.name === base)) return base;
  for (let suffix = 2; suffix < 1000; suffix += 1) {
    const candidate = fitNameWithSuffix(name, ` (cloned ${suffix})`);
    if (!rules.some((rule) => rule.client_id === clientId && rule.name === candidate)) return candidate;
  }
  return fitNameWithSuffix(name, ` (cloned ${Date.now()})`);
}

function fitNameWithSuffix(name: string, suffix: string) {
  const characters = Array.from(name.trim());
  while (characters.length > 0 && utf8ByteLength(`${characters.join("")}${suffix}`) > MAX_RULE_NAME_BYTES) {
    characters.pop();
  }
  return `${characters.join("")}${suffix}`;
}

function utf8ByteLength(value: string) {
  return new TextEncoder().encode(value).length;
}

function syncFeedback(response: PortForwardMutationResponse, success: string): FeedbackContent {
  if (response.sync.status === "queue_failed" || response.sync.status === "not_queued") return { message: `${success}; desired state saved, but apply was not queued: ${dispatchFailureReason(response.sync.error, response.sync.status, "Port-forward apply job")}`, tone: "warning" };
  if (response.sync.status === "queued") return { message: `${success}; apply job ${shortId(response.sync.job_id ?? "")} queued`, tone: "progress" };
  if (["already_in_requested_state", "forgotten_without_host_cleanup", "retired_disabled_draft", "saved_disabled"].includes(response.sync.status)) {
    return { message: `${success}; no host apply required`, tone: "success" };
  }
  return { message: success, tone: "success" };
}

function bulkSyncFeedback(response: PortForwardBulkResponse, action: PortForwardBulkAction, count: number): FeedbackContent {
  const failed = response.sync.filter((item) => ["queue_failed", "not_queued"].includes(item.sync.status));
  const queued = response.sync.filter((item) => item.sync.status === "queued").length;
  const pastAction = action === "reapply" ? "reapplied" : `${action}d`;
  if (failed.length > 0) {
    return { message: `${count} rules ${pastAction}; desired state saved, ${failed.length} VPS apply${failed.length === 1 ? "" : "s"} not queued: ${failed.map((item) => `${item.client_id}: ${dispatchFailureReason(item.sync.error, item.sync.status, "Port-forward apply job")}`).join("; ")}`, tone: "warning" };
  }
  if (queued > 0) {
    return { message: `${count} rules ${pastAction}; ${queued} VPS apply job${queued === 1 ? "" : "s"} queued`, tone: "progress" };
  }
  return { message: `${count} rules ${pastAction}; no host apply required`, tone: "success" };
}

function actionProgressLabel(snapshot: ConfirmationState) {
  if (snapshot.kind === "save") return snapshot.editing ? "Updating rule" : "Creating rule";
  if (snapshot.kind === "single") return `${snapshot.operation[0]!.toUpperCase()}${snapshot.operation.slice(1)} in progress`;
  return `Bulk ${snapshot.action} in progress`;
}

function singleActionPast(operation: "enable" | "disable" | "delete" | "forget" | "reapply") {
  if (operation === "reapply") return "Reapply queued";
  if (operation === "forget") return "Removal evidence forgotten";
  return `Rule ${operation}d`;
}

function confirmationTitle(state: ConfirmationState | null) {
  if (!state) return "Confirm port-forwarding action";
  if (state.kind === "save") return state.editing ? "Confirm rule update" : "Confirm rule creation";
  if (state.kind === "bulk") return `Confirm bulk ${state.action}`;
  return `Confirm ${state.operation}`;
}

function confirmationLabel(state: ConfirmationState | null) {
  if (!state) return "Confirm";
  if (state.kind === "save") return state.editing ? "Save and apply" : "Create and apply";
  return state.kind === "bulk" ? `${state.action[0]!.toUpperCase()}${state.action.slice(1)} rules` : `${state.operation[0]!.toUpperCase()}${state.operation.slice(1)} rule`;
}

function confirmationDetail(state: ConfirmationState | null) {
  if (!state) return "Review the current action.";
  if (state.kind === "save") return "This saves desired state and replaces the VPS's complete vpsman-owned nftables table atomically. Claimed ports take precedence over conventional Docker or system DNAT for new connections.";
  if (state.kind === "single" && state.operation === "delete") {
    return isNeverAppliedDisabledDraft(state.rule)
      ? "This disabled draft has never been applied. It is removed immediately; no agent cleanup or apply job is required."
      : "The rule is omitted from desired state immediately and remains visible as Removal pending until the agent confirms cleanup. Existing conntrack entries may continue.";
  }
  if (state.kind === "single" && state.operation === "forget") return "This removes the cleanup tombstone without confirming host state. Use it only for a permanently unreachable or decommissioned VPS; nftables state may remain on that host.";
  if (state.kind === "single" && state.operation === "reapply") return "Reapply replaces this VPS's complete vpsman-owned forwarding table. It does not change system, Docker, or unrelated nftables tables.";
  if (state.kind === "bulk" && state.action === "delete") {
    const immediateDrafts = state.rules.filter(isNeverAppliedDisabledDraft).length;
    const cleanupRules = state.rules.length - immediateDrafts;
    if (cleanupRules === 0) {
      return `This removes ${immediateDrafts} never-applied disabled draft${immediateDrafts === 1 ? "" : "s"} immediately. No agent cleanup or apply job is required.`;
    }
    if (immediateDrafts > 0) {
      return `${immediateDrafts} never-applied disabled draft${immediateDrafts === 1 ? "" : "s"} will be removed immediately. ${cleanupRules} rule${cleanupRules === 1 ? "" : "s"} will remain Removal pending until the affected agents confirm cleanup.`;
    }
  }
  if (state.kind === "bulk") return `This applies one ${state.action} decision to ${state.rules.length} exact rule revisions and reconciles each affected VPS once.`;
  return "This updates desired state and queues an atomic apply for the affected VPS.";
}

function isNeverAppliedDisabledDraft(rule: PortForwardRuleRecord) {
  return !rule.enabled && rule.revision === 1 && !rule.deleted_at;
}

function confirmationItems(state: ConfirmationState | null, agents: Map<string, AgentView>) {
  if (!state) return [];
  if (state.kind === "save") {
    const mappings = (() => { try { return formatPortMappings(pairPortExpressions(state.draft.incoming, state.draft.target)); } catch { return state.draft.incoming; } })();
    return [
      { label: "VPS", value: agents.get(state.draft.clientId)?.display_name || state.draft.clientId },
      { label: "Listener scope", value: `All current local ${state.draft.targetIp.includes(":") ? "IPv6" : "IPv4"} addresses` },
      { label: "Claimed ports", title: state.draft.incoming, value: `${state.draft.protocol.toUpperCase()} ${state.draft.incoming}` },
      { label: "Target", value: state.draft.targetIp },
      { label: "Mapping", title: mappings, value: mappings },
      { label: "Return", value: state.draft.masquerade ? "Targeted masquerade" : "Preserve source" },
    ];
  }
  const rules = state.kind === "bulk" ? state.rules : [state.rule];
  const items = rules.slice(0, 12).map((rule) => ({ label: agents.get(rule.client_id)?.display_name || rule.client_id, title: `${rule.name}: ${formatPortMappings(rule.mappings)}`, value: rule.name }));
  if (rules.length > 12) items.push({ label: "Additional rules", title: `${rules.length - 12} additional exact revisions are included`, value: `+${rules.length - 12} more` });
  return items;
}
