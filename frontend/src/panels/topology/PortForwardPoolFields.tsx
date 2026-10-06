import { ChevronDown, ChevronRight, CirclePlus, RefreshCcw, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  allowsBackup, failureLabel, literalIpFamily, newUpstream,
  normalizedIp, poolFailureCapability, POOL_STRATEGIES, retrySummary, upstreamError, upstreamSummary,
  type PoolDraft, type UpstreamDraft,
} from "../../portForwardPools";
import { formatPortRange, parsePortExpression } from "../../portForwarding";
import type {
  PortForwardMode, PortForwardPool, PortForwardPoolCapabilities,
  PortForwardProtocol, PortForwardUpstreamObservation, ResolveHostnameResponse,
} from "../../types";
import { ActionFeedback } from "../../components/ActionFeedback";

type Candidate = { address: string; family: "ipv4" | "ipv6"; retained: boolean; returned: boolean };
type Resolution = {
  rowId: string; hostname: string; busy: boolean; error: string | null;
  candidates: Candidate[]; selected: string[]; family: "ipv4" | "ipv6";
  truncated: boolean; templateId: string;
};

function normalizedHostname(hostname: string) {
  return hostname.trim().replace(/\.$/, "").toLowerCase();
}

// Hostnames are virtual owners of explicit IP rows, not an extra runtime target.
function upstreamGroups<T extends { id: string }>(rows: T[], hostnameOf: (row: T) => string | null | undefined) {
  const groups = new Map<string, { key: string; hostname: string | null; rows: T[] }>();
  for (const row of rows) {
    const hostname = normalizedHostname(hostnameOf(row) ?? "") || null;
    const key = hostname ? `domain:${hostname}` : `ip:${row.id}`;
    if (!groups.has(key)) groups.set(key, { key, hostname, rows: [] });
    groups.get(key)!.rows.push(row);
  }
  return [...groups.values()];
}

export function PortForwardPoolFields({
  draft, capabilities, mode, protocol, pending, onChange, onResolve, onResolutionPendingChange,
}: {
  draft: PoolDraft;
  capabilities: PortForwardPoolCapabilities;
  mode: PortForwardMode;
  protocol: PortForwardProtocol;
  pending: boolean;
  onChange: (draft: PoolDraft) => void;
  onResolve: (hostname: string, mode?: PortForwardMode) => Promise<ResolveHostnameResponse>;
  onResolutionPendingChange: (pending: boolean) => void;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [resolution, setResolution] = useState<Resolution | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const currentDraft = useRef(draft);
  currentDraft.current = draft;
  const generation = useRef(0);
  const contextKey = JSON.stringify([mode, protocol, capabilities]);
  useEffect(() => {
    generation.current++;
    setResolution(null);
    return () => { generation.current++; };
  }, [contextKey]);
  useEffect(() => {
    onResolutionPendingChange(resolution !== null);
    return () => onResolutionPendingChange(false);
  }, [resolution !== null, onResolutionPendingChange]);

  const backup = allowsBackup(draft, capabilities);
  const failure = poolFailureCapability(draft, capabilities);
  const strategy = draft.strategy;
  const strategies = capabilities.strategies.filter((item) =>
    !draft.upstreams.some((row) => row.role === "backup") || capabilities.backup_strategies?.includes(item));
  const layout = `poolUpstreamGrid${backup ? " withBackup" : ""}`;
  const groups = upstreamGroups(draft.upstreams, (row) => row.hostname);
  const indices = new Map(draft.upstreams.map((row, index) => [row.id, index]));

  function resolutionRows(value: Resolution, rows = draft.upstreams) {
    const origin = rows.find((row) => row.id === value.rowId);
    return rows.filter((row) => row.id === value.rowId || (origin?.hostname && normalizedHostname(row.hostname ?? "") === value.hostname));
  }

  function cancelResolution() {
    generation.current++;
    setResolution(null);
  }

  function updateRow(id: string, values: Partial<UpstreamDraft>) {
    setNotice(null);
    if (("address" in values || "ports" in values) && resolution && resolutionRows(resolution).some((row) => row.id === id)) {
      cancelResolution();
    }
    onChange({ ...draft, upstreams: draft.upstreams.map((row) => row.id === id ? { ...row, ...values } : row) });
  }

  function removeRows(ids: string[]) {
    if (resolution && resolutionRows(resolution).some((row) => ids.includes(row.id))) cancelResolution();
    setNotice(null);
    onChange({ ...draft, upstreams: draft.upstreams.filter((row) => !ids.includes(row.id)) });
  }

  async function resolve(row: UpstreamDraft) {
    const hostname = normalizedHostname(row.hostname || row.address);
    const ticket = ++generation.current;
    setNotice(null);
    setResolution({ rowId: row.id, hostname, templateId: row.id, busy: true, error: null, candidates: [], selected: [], family: "ipv4", truncated: false });
    try {
      const result = await onResolve(hostname, mode);
      if (ticket !== generation.current) return;
      const current = currentDraft.current;
      const origin = current.upstreams.find((item) => item.id === row.id);
      if (!origin || origin.address !== row.address || origin.ports !== row.ports) return;
      const related = row.hostname ? current.upstreams.filter((item) => normalizedHostname(item.hostname ?? "") === hostname && literalIpFamily(item.address)) : [];
      const currentIps = new Set(related.map((item) => normalizedIp(item.address)));
      const candidates = new Map<string, Candidate>();
      for (const item of result.candidates) {
        const family = literalIpFamily(item.address);
        if (!family || !capabilities.address_families.includes(family)) continue;
        const address = normalizedIp(item.address);
        candidates.set(address, { address, family, retained: currentIps.has(address), returned: true });
      }
      for (const item of related) {
        const address = normalizedIp(item.address);
        if (!candidates.has(address)) candidates.set(address, { address, family: literalIpFamily(address)!, retained: true, returned: false });
      }
      const fixedFamily = !capabilities.mixed_families
        ? current.upstreams.map((item) => literalIpFamily(item.address)).find(Boolean)
        : null;
      const usable = [...candidates.values()].filter((item) => item.retained || !fixedFamily || item.family === fixedFamily);
      setResolution({ rowId: row.id, hostname, templateId: row.id, busy: false, error: null,
        candidates: usable, selected: [...currentIps], family: fixedFamily ?? usable[0]?.family ?? "ipv4",
        truncated: Boolean((result as ResolveHostnameResponse & { truncated?: boolean }).truncated),
      });
    } catch (error) {
      if (ticket !== generation.current) return;
      setResolution((value) => value ? { ...value, busy: false, error: error instanceof Error ? error.message : "Hostname resolution failed" } : null);
    }
  }

  function applyResolution() {
    if (!resolution || resolution.busy || resolution.error) return;
    const origin = draft.upstreams.find((item) => item.id === resolution.rowId);
    const template = draft.upstreams.find((item) => item.id === resolution.templateId);
    if (!origin || !template) return;
    const selected = new Set(resolution.selected);
    const group = resolutionRows(resolution);
    const existing = new Set(group.filter((item) => literalIpFamily(item.address)).map((item) => normalizedIp(item.address)));
    const additions = [...selected].filter((ip) => !existing.has(ip)).map((address) => newUpstream({
      ...template, id: crypto.randomUUID(), address, hostname: resolution.hostname,
    }));
    const groupIds = new Set(group.map((item) => item.id));
    const rows = draft.upstreams.flatMap((item) => {
      const retained = !groupIds.has(item.id) || selected.has(normalizedIp(item.address));
      return [...(retained ? [item] : []), ...(item.id === origin.id ? additions : [])];
    });
    const removed = group.filter((item) => literalIpFamily(item.address) && !selected.has(normalizedIp(item.address))).length;
    onChange({ ...draft, upstreams: rows });
    setNotice(`${additions.length} added${removed ? ` · ${removed} removed` : ""} · existing row settings retained`);
    setResolution(null);
  }

  function renderResolution() {
    if (!resolution) return null;
    const value = resolution;
    const group = resolutionRows(value);
    const template = group.find((row) => row.id === value.templateId);
    const visibleCandidates = value.candidates.filter((candidate) => capabilities.mixed_families || candidate.family === value.family || candidate.retained);
    const dnsFamilies = [...new Set(value.candidates.map((candidate) => candidate.family))];
    function conflicts(candidate: Candidate) {
      if (candidate.retained || !template) return false;
      return draft.upstreams.some((other) => {
        if (other.id === value.rowId || normalizedIp(other.address) !== candidate.address) return false;
        try {
          return parsePortExpression(other.ports).some((a) => parsePortExpression(template.ports).some((b) => a.start <= b.end && b.start <= a.end));
        } catch { return false; }
      });
    }
    const newSelected = visibleCandidates.some((candidate) => !candidate.retained && value.selected.includes(candidate.address));
    const differentSettings = new Set(group.map(({ id: _id, address: _address, hostname: _hostname, ...settings }) => JSON.stringify(settings))).size > 1;
    const selectedConflict = visibleCandidates.some((candidate) => value.selected.includes(candidate.address) && conflicts(candidate));
    return <div className="poolDnsPicker" aria-label={`DNS addresses for ${value.hostname}`}>
      <div className="poolDnsHeading"><strong>DNS addresses</strong>
        {!capabilities.mixed_families && dnsFamilies.length > 1 && <select aria-label="DNS address family" disabled={pending || value.busy || value.candidates.some((candidate) => candidate.retained)} value={value.family} onChange={(event) => setResolution({ ...value, family: event.target.value as "ipv4" | "ipv6", selected: [] })}>{dnsFamilies.map((family) => <option key={family} value={family}>{family === "ipv4" ? "IPv4" : "IPv6"}</option>)}</select>}
        <button className="secondaryAction compactAction" disabled={pending} type="button" onClick={cancelResolution}>Cancel</button>
      </div>
      {value.busy ? <span className="formHint" role="status">Resolving…</span> : value.error ? <ActionFeedback tone="danger" message={value.error} /> : <>
        {visibleCandidates.length ? <div className="poolDnsCandidates">{visibleCandidates.map((candidate) => {
          const conflict = conflicts(candidate);
          const checked = value.selected.includes(candidate.address);
          return <label key={candidate.address} title={candidate.retained ? candidate.returned ? "Already configured; keep all port ranges and their independent settings" : "Stored address absent from this DNS answer; retained until you explicitly uncheck it" : conflict ? "This address and port range already belong to another configured row" : "Add an IP row using the selected source row's settings"}>
            <input checked={checked} disabled={pending || (conflict && !checked)} type="checkbox" onChange={(event) => setResolution({ ...value, selected: event.target.checked ? [...value.selected, candidate.address] : value.selected.filter((ip) => ip !== candidate.address) })} />
            <span>{candidate.address}</span><small>{candidate.retained ? candidate.returned ? "Stored" : "Stored · not returned" : conflict ? "Already configured" : candidate.family === "ipv4" ? "IPv4 · new" : "IPv6 · new"}</small>
          </label>;
        })}</div> : <span className="formHint">No eligible addresses. Existing rows are retained.</span>}
        {newSelected && differentSettings && <label className="poolDnsTemplate" title="New IPs copy this row's port range, weight, role and failure settings. Existing IPs keep every independently configured row; edit each new row after adding it.">
          <span>New IPs copy settings from</span>
          <select aria-label="New IP settings source" disabled={pending} value={value.templateId} onChange={(event) => setResolution({ ...value, templateId: event.target.value })}>{group.map((row) => <option key={row.id} value={row.id}>{row.address} · {row.ports} · {row.role === "backup" ? "Backup" : "Primary"} ×{row.weight}{row.enabled ? "" : " · disabled"}</option>)}</select>
        </label>}
        {value.truncated && <span className="formHint" role="status">The resolver returned a limited result set; this is not the complete DNS membership.</span>}
        {selectedConflict && <span className="poolRowError" role="status">A selected new address overlaps an existing port range.</span>}
        {visibleCandidates.length > 0 && <div className="poolDnsActions">
          <button className="secondaryAction compactAction" disabled={pending} type="button" onClick={() => setResolution({ ...value, selected: visibleCandidates.filter((candidate) => !conflicts(candidate)).map((item) => item.address) })}>Select all</button>
          <button className="primaryAction compactAction" disabled={pending || !value.selected.length || selectedConflict} type="button" onClick={applyResolution}>Use {value.selected.length} address{value.selected.length === 1 ? "" : "es"}</button>
        </div>}
      </>}
    </div>;
  }

  return (
    <div className="portForwardPoolFields fieldFull">
      <div className="poolStrategyRow">
        <label title={`${POOL_STRATEGIES[strategy].hint} Strategies that cannot use backups are hidden while backup rows exist; change their role to Primary to use those strategies.`}>
          <span>Strategy</span>
          <select aria-label="Pool strategy" disabled={pending} value={strategies.includes(strategy) ? strategy : ""} onChange={(event) => onChange({ ...draft, strategy: event.target.value as PoolDraft["strategy"] })}>
            {!strategies.includes(strategy) && <option value="" disabled>Choose a compatible strategy…</option>}
            {strategies.map((item) => <option key={item} value={item}>{POOL_STRATEGIES[item].label}</option>)}
          </select>
        </label>
        <span className="formHint" title="Every incoming port uses this entire pool. Use separate forwarding rules for different services. Each port in a range receives the row's weight and failure settings independently.">One shared pool for all incoming ports</span>
      </div>
      <div className="poolUpstreamList" aria-label="Upstream pool">
        <div className={`${layout} poolUpstreamHeader`} aria-hidden="true">
          <span>Domain / IP</span><span>Port / range</span><span>Weight</span>
          {backup && <span>Role</span>}<span>On</span><span />
        </div>
        {groups.map((group) => <div className={group.hostname ? "poolDomain" : "poolDirect"} key={group.key} data-upstream-domain={group.hostname ?? undefined}>
          {group.hostname && <>
            <div className="poolDomainHeading">
              <strong title={group.hostname}>{group.hostname}</strong>
              <span className="formHint">{new Set(group.rows.map((row) => normalizedIp(row.address))).size} IP{new Set(group.rows.map((row) => normalizedIp(row.address))).size === 1 ? "" : "s"}</span>
              <div className="poolRowActions">
                <button aria-label={`Resolve ${group.hostname}`} className="iconButton" disabled={pending || (resolution?.hostname === group.hostname && resolution.busy)} title="Resolve this domain again and review its IP membership. Existing rows retain their individual settings." type="button" onClick={() => void resolve(group.rows[0]!)}><RefreshCcw size={14} className={resolution?.hostname === group.hostname && resolution.busy ? "spin" : undefined} /></button>
                <button aria-label={`Remove ${group.hostname}`} className="iconButton" disabled={pending} title="Remove this domain and all its IP rows from the reviewed configuration" type="button" onClick={() => removeRows(group.rows.map((row) => row.id))}><Trash2 size={14} /></button>
              </div>
            </div>
            {resolution && group.rows.some((row) => row.id === resolution.rowId) && renderResolution()}
          </>}
          <div className={group.hostname ? "poolDomainChildren" : undefined}>
        {group.rows.map((row) => {
          const index = indices.get(row.id)!;
          const error = upstreamError(row, { ...capabilities, failure_exclusion: failure });
          const isHostname = Boolean(row.address.trim()) && !literalIpFamily(row.address);
          const showDns = isHostname && !group.hostname;
          const rowResolution = resolution?.rowId === row.id ? resolution : null;
          const rowOpen = expanded.has(row.id);
          const showError = error && row.address.trim() && !isHostname;
          const failureSummary = row.failureMode === "default" ? "Failure handling: default"
            : row.failureMode === "off" ? "Automatic failure exclusion off"
              : `${row.threshold} failures / ${row.window}s → exclude ${failure?.linked_timeout ? row.window : row.retryAfter}s, then retry`;
          return (
            <div className={`poolUpstream${!row.enabled ? " isDisabled" : ""}`} key={row.id} data-upstream-id={row.id}>
              <div className={layout}>
                <label className="poolAddressField" title={row.hostname ? `${row.address} · resolved from ${row.hostname}. Resolve again to review DNS membership.` : "Literal IP, or a hostname to resolve into separately configurable IP rows"}>
                  <span className="poolMobileLabel">{group.hostname ? "IP" : "IP / hostname"}</span>
                  <div className="poolAddressInput">
                    {group.hostname ? <span className="poolResolvedAddress" title={row.address}>{row.address}</span> : <input aria-label={`Upstream ${index + 1} address`} autoComplete="off" disabled={pending} placeholder="IP or hostname" value={row.address} onChange={(event) => updateRow(row.id, { address: event.target.value, hostname: null })} />}
                    {showDns && <button aria-label={`Resolve upstream ${index + 1}`} className="iconButton" disabled={pending || rowResolution?.busy} title={row.hostname ? `Resolve ${row.hostname} again` : "Resolve and select one or more addresses"} type="button" onClick={() => void resolve(row)}><RefreshCcw size={14} className={rowResolution?.busy ? "spin" : undefined} /></button>}
                  </div>
                </label>
                <label title="One port or contiguous range per row. Each IP:port is a separate selectable endpoint.">
                  <span className="poolMobileLabel">Port / range</span>
                  <input aria-label={`Upstream ${index + 1} ports`} disabled={pending} inputMode="text" placeholder="8443 or 8443-8445" value={row.ports} onChange={(event) => updateRow(row.id, { ports: event.target.value })} />
                </label>
                <label title="Positive relative weight per IP:port. A three-port range with weight 2 contributes three endpoints of weight 2.">
                  <span className="poolMobileLabel">Weight</span>
                  <input aria-label={`Upstream ${index + 1} weight`} disabled={pending} min={1} step={1} type="number" value={row.weight} onChange={(event) => updateRow(row.id, { weight: event.target.value })} />
                </label>
                {backup && <label title="Backups serve new flows when no eligible primary remains. Recovered primaries receive new flows again.">
                  <span className="poolMobileLabel">Role</span>
                  <select aria-label={`Upstream ${index + 1} role`} disabled={pending} value={row.role} onChange={(event) => updateRow(row.id, { role: event.target.value as UpstreamDraft["role"] })}><option value="primary">Primary</option><option value="backup">Backup</option></select>
                </label>}
                <label className="poolEnabledField" title="Exclude from new selections for planned maintenance. Keep the row and its settings; existing flows follow the forwarder's normal lifetime.">
                  <input aria-label={`Upstream ${index + 1} enabled`} checked={row.enabled} disabled={pending} type="checkbox" onChange={(event) => updateRow(row.id, { enabled: event.target.checked })} /><span className="poolMobileLabel">Enabled</span>
                </label>
                <div className="poolRowActions">
                  {failure && <button aria-label={`Upstream ${index + 1} settings`} aria-expanded={rowOpen} className={`iconButton${row.failureMode !== "default" ? " hasSettings" : ""}`} disabled={pending} title={failureSummary} type="button" onClick={() => setExpanded((current) => { const next = new Set(current); if (next.has(row.id)) next.delete(row.id); else next.add(row.id); return next; })}>{rowOpen ? <ChevronDown size={15} /> : <ChevronRight size={15} />}</button>}
                  <button aria-label={`Remove upstream ${index + 1}`} className="iconButton" disabled={pending} title="Remove this upstream from the reviewed configuration" type="button" onClick={() => removeRows([row.id])}><Trash2 size={14} /></button>
                </div>
              </div>
              {showError && <div className="poolRowError" role="status">{error}</div>}
              {failure && rowOpen && <div className="poolRowSettings topologyFormGrid fourColumn compactNumericGrid" aria-label={`Upstream ${index + 1} failure settings`}>
                <label title={`${failure.description} These settings apply separately to each IP:port in this row, within this rule's pool. They do not apply to the whole domain or other rules. Default retains the adapter's configured behavior.`}>
                  <span>Failure handling</span>
                  <select aria-label={`Upstream ${index + 1} failure handling`} disabled={pending} value={row.failureMode} onChange={(event) => updateRow(row.id, { failureMode: event.target.value as UpstreamDraft["failureMode"] })}><option value="default">Default</option><option value="off">Off</option><option value="temporary">Temporary exclusion</option></select>
                </label>
                {row.failureMode === "temporary" && <>
                  <label title="Number of qualifying failures within the failure window before this IP:port is excluded. One failing port does not exclude the entire row's range."><span>Failures</span><input aria-label={`Upstream ${index + 1} failure threshold`} disabled={pending} type="number" min={1} step={1} value={row.threshold} onChange={(event) => updateRow(row.id, { threshold: event.target.value })} /></label>
                  <label title={failure.linked_timeout ? "This adapter uses the same interval to count failures and temporarily exclude an endpoint. The endpoint can be tried again after this interval." : "Count qualifying failures within this interval."}><span>{failure.linked_timeout ? "Window / retry after (s)" : "Failure window (s)"}</span><input aria-label={`Upstream ${index + 1} failure window`} disabled={pending} type="number" min={1} step={1} value={row.window} onChange={(event) => updateRow(row.id, { window: event.target.value })} /></label>
                  {!failure.linked_timeout && <label title="After this exclusion interval, allow recovery attempts. A failed recovery attempt can renew exclusion."><span>Retry after (s)</span><input aria-label={`Upstream ${index + 1} retry after`} disabled={pending} type="number" min={1} step={1} value={row.retryAfter} onChange={(event) => updateRow(row.id, { retryAfter: event.target.value })} /></label>}
                </>}
              </div>}
              {!group.hostname && rowResolution && renderResolution()}
            </div>
          );
        })}
          </div>
        </div>)}
      </div>
      <div className="poolFooter"><button className="secondaryAction compactAction" disabled={pending} type="button" onClick={() => { setNotice(null); onChange({ ...draft, upstreams: [...draft.upstreams, newUpstream()] }); }}><CirclePlus size={14} /> Add upstream</button>{notice && <span className="formHint" role="status">{notice}</span>}</div>
      {(capabilities.connect_timeout || capabilities.retries) && <details className="topologyAdvancedFields poolConnectionOptions">
        <summary title="Settings for this forwarding rule, shared by its incoming ports and upstreams. They do not modify other rules or service-wide defaults.">Connection options</summary>
        <div className="poolConnectionSettings topologyFormGrid fourColumn compactNumericGrid">
          {capabilities.connect_timeout && <label title={`${capabilities.connect_timeout.description} Applies to each upstream connection attempt for this rule, even when retries are off. This is not the established connection's idle timeout. Blank retains the adapter default.`}><span>Connect timeout (s)</span><input aria-label="Pool connect timeout" disabled={pending} type="number" min={1} step={1} placeholder="Default" value={draft.connectTimeout} onChange={(event) => onChange({ ...draft, connectTimeout: event.target.value })} /></label>}
          {capabilities.retries && <label title={`${capabilities.retries.description} Default retains the adapter's configured behavior. This affects one incoming connection; it neither configures nor disables an upstream's temporary failure exclusion.`}><span>Retry</span><select aria-label="Connection retry" disabled={pending} value={draft.retryMode} onChange={(event) => onChange({ ...draft, retryMode: event.target.value as PoolDraft["retryMode"] })}><option value="default">Default</option><option value="off">Off</option><option value="connect_failure">On connect failure</option></select></label>}
          {capabilities.retries && draft.retryMode === "connect_failure" && <>
            <label title="Maximum upstream attempts for one incoming flow, including the first attempt. Blank retains the adapter default."><span>Maximum attempts</span><input aria-label="Pool maximum attempts" disabled={pending} type="number" min={1} step={1} placeholder="Default" value={draft.maxAttempts} onChange={(event) => onChange({ ...draft, maxAttempts: event.target.value })} /></label>
            <label title="Elapsed-time limit for starting another upstream attempt on one incoming connection. An attempt already running may finish after this limit. This does not limit an established connection or an endpoint's exclusion period. Blank retains the adapter default."><span>Retry budget (s)</span><input aria-label="Pool retry budget" disabled={pending} type="number" min={1} step={1} placeholder="Default" value={draft.retryBudget} onChange={(event) => onChange({ ...draft, retryBudget: event.target.value })} /></label>
          </>}
        </div>
      </details>}
    </div>
  );
}

export function PortForwardPoolDetails({ pool, mode, observations = [] }: { pool: PortForwardPool; mode: PortForwardMode; observations?: PortForwardUpstreamObservation[] }) {
  const hasFailureSettings = pool.upstreams.some((row) => row.failure_policy);
  return <div className="poolConfiguredDetails" aria-label="Configured upstreams">
    <div className="poolDetailHeading"><strong>Upstreams</strong>{mode === "custom_adapter" && <span className="formHint" title="Configured failure handling is separate from the adapter's observed eligibility. Applied confirms configuration, not upstream health.">Configured · latest observation</span>}</div>
    {upstreamGroups(pool.upstreams, (row) => row.target_hostname).map((group) => <div className={group.hostname ? "poolDetailDomain" : undefined} key={group.key}>
      {group.hostname && <div className="poolDomainHeading"><strong title={group.hostname}>{group.hostname}</strong></div>}
      <div className={group.hostname ? "poolDomainChildren" : undefined}>
    {group.rows.map((row) => {
      const evidence = [...new Map(observations.filter((item) => item.upstream_id === row.id && item.port >= row.ports.start && item.port <= row.ports.end).map((item) => [item.port, item])).values()];
      const count = row.ports.end - row.ports.start + 1;
      const excluded = evidence.filter((item) => item.state === "excluded");
      const eligible = evidence.filter((item) => item.state === "eligible");
      const label = !row.enabled ? "Disabled" : excluded.length ? `${excluded.length}/${count} excluded` : eligible.length === count ? "Eligible" : eligible.length ? `${eligible.length}/${count} eligible` : "Not reported";
      const status = !row.enabled ? "disabled" : excluded.length ? "applied_warning" : eligible.length === count ? "applied" : "disabled";
      const title = evidence.length ? evidence.map((item) => `${item.port}: ${item.state}${item.reason ? ` · ${item.reason}` : ""}${item.retry_after_unix ? ` · retry ${new Date(item.retry_after_unix * 1000).toLocaleTimeString()}` : ""}`).join("\n") : "No per-upstream runtime evidence has been reported";
      return <div className={`poolDetailRow${mode !== "custom_adapter" ? " isNative" : ""}${hasFailureSettings ? " withFailure" : ""}`} key={row.id}>
        <span className="poolDetailAddress" title={row.target_hostname ? `${row.target_ip} · ${row.target_hostname}` : row.target_ip}>{row.target_ip}</span>
        <span>{formatPortRange(row.ports)}</span>
        <span title={upstreamSummary(row)}>{mode === "custom_adapter" ? `${row.role === "backup" ? "Backup" : "Primary"} · ` : "Weight "}×{row.weight}{mode !== "custom_adapter" && !row.enabled ? " · disabled" : ""}</span>
        {hasFailureSettings && <span className="poolDetailFailure" title={failureLabel(row.failure_policy)}>{failureLabel(row.failure_policy)}</span>}
        {mode === "custom_adapter" && <span className={`portForwardStatus status-${status}`} title={title}>{label}</span>}
      </div>;
    })}
      </div>
    </div>)}
    {(pool.connect_timeout_secs !== undefined || pool.retry_policy) && <span className="formHint" title="Connection settings for this rule. Connect timeout applies per attempt; retry limits apply per incoming connection. Per-endpoint failure exclusion is configured separately above.">
      {pool.connect_timeout_secs !== undefined && <>Connect timeout: {pool.connect_timeout_secs}s{pool.retry_policy ? " · " : ""}</>}
      {pool.retry_policy && <>Retry: {retrySummary(pool)}</>}
    </span>}
  </div>;
}
