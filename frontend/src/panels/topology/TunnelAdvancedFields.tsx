import { useEffect, useRef, useState, type ReactNode } from "react";
import { NumberedTextarea } from "../../components/NumberedTextarea";
import { clientDisplayNameFromMap } from "../../utils";
import {
  TUNNEL_HOOK_PHASES,
  type TunnelAdvancedDraft,
  type TunnelEndpointAdvancedDraft,
  type TunnelHookDraft,
} from "../../tunnelAdvanced";
import type {
  TunnelEndpointSide,
  TunnelKind,
  TunnelPlanInput,
  TunnelPlanPreviewResponse,
} from "../../types";

export type PreviewTunnelPlan = (
  request: TunnelPlanInput,
  signal?: AbortSignal,
) => Promise<TunnelPlanPreviewResponse>;

export function TunnelAdvancedFields({
  children,
  clientNames,
  draft,
  draftKey,
  endpointClientIds,
  kind,
  onChange,
  onPreview,
  request,
  validationError,
}: {
  children: ReactNode;
  clientNames: Map<string, string>;
  draft: TunnelAdvancedDraft;
  draftKey: string;
  endpointClientIds: Record<TunnelEndpointSide, string>;
  kind: TunnelKind;
  onChange: (value: TunnelAdvancedDraft) => void;
  onPreview: PreviewTunnelPlan;
  request: TunnelPlanInput | null;
  validationError: string | null;
}) {
  const [side, setSide] = useState<TunnelEndpointSide>("left");
  const [pending, setPending] = useState(false);
  const [preview, setPreview] = useState<{
    key: string;
    result: TunnelPlanPreviewResponse;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copyFeedback, setCopyFeedback] = useState<string | null>(null);
  const currentKey = useRef(draftKey);
  currentKey.current = draftKey;
  const flight = useRef<AbortController | null>(null);
  useEffect(() => {
    setError(null);
    setCopyFeedback(null);
    // A draft change invalidates the in-flight preview without requesting another.
    flight.current?.abort();
  }, [draftKey]);
  useEffect(() => () => flight.current?.abort(), []);

  async function refresh() {
    if (flight.current || !request || validationError) return;
    const controller = new AbortController();
    flight.current = controller;
    const key = draftKey;
    setPending(true);
    setError(null);
    setCopyFeedback(null);
    try {
      const result = await onPreview(request, controller.signal);
      if (!controller.signal.aborted && currentKey.current === key) {
        setPreview({ key, result });
      }
    } catch (caught) {
      if (!controller.signal.aborted && currentKey.current === key) {
        setError(caught instanceof Error ? caught.message : "Preview failed");
      }
    } finally {
      if (flight.current === controller) {
        flight.current = null;
        setPending(false);
      }
    }
  }

  const endpoint = preview?.result.endpoints.find((item) => item.side === side);
  const stale = preview !== null && preview.key !== draftKey;
  const selectedClientId = endpointClientIds[side];
  const endpointScope = `${side === "left" ? "Left" : "Right"} endpoint · ${
    selectedClientId
      ? clientDisplayNameFromMap(selectedClientId, clientNames)
      : "No VPS selected"
  }`;
  const previewScope = endpoint && endpoint.client_id !== selectedClientId
    ? `${side === "left" ? "Left" : "Right"} endpoint · ${clientDisplayNameFromMap(endpoint.client_id, clientNames)} (${endpoint.client_id}) · previous draft`
    : endpointScope;
  function updateEndpoint(value: TunnelEndpointAdvancedDraft) {
    onChange({ ...draft, [side]: value });
  }
  function updateHook(
    key: keyof TunnelEndpointAdvancedDraft["hooks"],
    field: keyof TunnelHookDraft,
    value: string,
  ) {
    updateEndpoint({
      ...draft[side],
      hooks: { ...draft[side].hooks, [key]: { ...draft[side].hooks[key], [field]: value } },
    });
  }
  const configuredHooks = TUNNEL_HOOK_PHASES.filter(({ key }) => draft[side].hooks[key].argv.trim());
  async function copyPreview() {
    if (!endpoint) return;
    const content = [
      `Expected ${side} endpoint: ${endpoint.client_id}${stale ? " (outdated draft)" : ""}`,
      ...endpoint.artifacts.map((item) => `${item.label}\n${item.content}`),
      ...endpoint.commands.map((item) => `${item.phase}: ${item.label}\n${JSON.stringify(item.argv)}`),
    ].join("\n\n");
    try {
      await navigator.clipboard.writeText(content);
      setCopyFeedback("Copied");
    } catch {
      setCopyFeedback("Copy unavailable; select the preview text to copy it");
    }
  }

  return (
    <details
      className="topologyAdvancedFields tunnelAdvancedFields"
      onToggle={(event) => {
        if (event.target === event.currentTarget && event.currentTarget.open && !preview) void refresh();
      }}
    >
      <summary>Advanced</summary>
      <div className="tunnelAdvancedBody">
        <div aria-label="Advanced endpoint" className="tunnelAdvancedToolbar" role="group">
          <strong>Endpoint</strong>
          {(["left", "right"] as const).map((value) => (
            <button
              aria-pressed={side === value}
              className="secondaryAction compactAction"
              key={value}
              onClick={() => { setSide(value); setCopyFeedback(null); }}
              type="button"
            >
              {value === "left" ? "Left VPS" : "Right VPS"}
            </button>
          ))}
        </div>

        <section aria-label="Expected configuration and commands" className="tunnelAdvancedSection">
          <div className="tunnelAdvancedToolbar">
            <div className="tunnelAdvancedHeading">
              <strong>Expected configuration and commands</strong>
              <span className="formHint">{previewScope}</span>
            </div>
            <div className="tunnelAdvancedToolbar">
              <button
                className="secondaryAction compactAction"
                disabled={pending || !request || Boolean(validationError)}
                onClick={() => void refresh()}
                title={validationError ?? "Render this draft without saving or executing commands"}
                type="button"
              >
                {pending ? "Rendering…" : "Refresh"}
              </button>
              <button
                className="secondaryAction compactAction"
                disabled={!endpoint || stale}
                onClick={() => void copyPreview()}
                type="button"
              >Copy</button>
            </div>
          </div>
          <p className="formHint">
            Expected, not Applied. No commands are executed. Host-specific paths and credentials are placeholders; actual results remain in runtime evidence.
          </p>
          {stale && <p className="formHint" role="status">Preview outdated — refresh to include your changes.</p>}
          {error && <p className="formHint" role="alert">{error}</p>}
          {copyFeedback && <p className="formHint" role="status">{copyFeedback}</p>}
          {!endpoint && !pending && !error && (
            <p className="formHint">{validationError ? `Complete the plan to preview: ${validationError}` : "Refresh to render the current draft."}</p>
          )}
          {endpoint && (
            <div className="tunnelExpectedPreview" aria-label={`${side} expected configuration`}>
              {endpoint.artifacts.map((artifact, index) => (
                <div key={`${artifact.label}-${index}`}>
                  <strong>{artifact.label}</strong>
                  <pre>{artifact.content}</pre>
                </div>
              ))}
              {endpoint.commands.length > 0 && (
                <div>
                  <strong>Command argv</strong>
                  {endpoint.commands.map((command, index) => (
                    <div key={`${command.phase}-${index}`}>
                      <span className="formHint">{command.phase} · {command.label}</span>
                      <pre>{JSON.stringify(command.argv)}</pre>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </section>

        {kind === "openvpn" && (
          <section aria-label="OpenVPN configuration overrides" className="tunnelAdvancedSection">
            <div className="tunnelAdvancedHeading">
              <strong>OpenVPN configuration overrides</strong>
              <span className="formHint">{endpointScope}</span>
            </div>
            <label>
              <span>Native configuration</span>
              <NumberedTextarea
                aria-label={`${side} OpenVPN configuration overrides`}
                onChange={(event) => updateEndpoint({ ...draft[side], configOverride: event.target.value })}
                placeholder={"ping 15\nping-restart 90\nverb 4"}
                rows={4}
                spellCheck={false}
                value={draft[side].configOverride}
              />
              <span className="formHint">Empty uses generated defaults. Native options replace matching tuning defaults; the agent retains identity, credentials, endpoints, routes, and lifecycle ownership. OpenVPN reports invalid options through runtime evidence.</span>
            </label>
          </section>
        )}

        <section aria-label="Lifecycle hooks" className="tunnelAdvancedSection">
          <div className="tunnelAdvancedHeading">
            <strong>Lifecycle hooks</strong>
            <span className="formHint">{endpointScope}</span>
          </div>
          <p className="formHint">One argument per line; the first is an absolute executable path. Quotes and argument spacing are literal; blank lines are omitted on save. No implicit shell. Empty disables the hook.</p>
          <details className="tunnelHookHelp">
            <summary>Template placeholders and lifecycle</summary>
            <p className="formHint">{"{interface}, {plan}, {kind}, {local_client_id}, {peer_client_id}, {local_underlay}, {remote_underlay}, {local_address}, {remote_address}, {prefix_len}, {local_ipv4}, {remote_ipv4}, {prefix_len_ipv4}, {local_ipv6}, {remote_ipv6}, {prefix_len_ipv6}, {fou_port}, {fou_peer_port}, {fou_ipproto}, {egress_kbps}, {ingress_kbps}, {burst_kb}. Replacement happens once within each argument; unknown tokens remain literal."}</p>
            <p className="formHint">Hooks surround actual local startup/shutdown, not routine reconciliation or OpenVPN reconnects. Pre-hook failure stops that phase; post-hook failure is reported without undoing native work or automatic replay. Commands should be idempotent for later operator retries. Host crashes cannot guarantee hooks.</p>
          </details>
          <div className="topologyFormGrid twoColumn">
            {TUNNEL_HOOK_PHASES.map(({ key, label }) => (
              <label key={key}>
                <span>{label}</span>
                <NumberedTextarea
                  aria-label={`${side} ${label} hook argv`}
                  onChange={(event) => updateHook(key, "argv", event.target.value)}
                  placeholder={"/absolute/path/to/executable\n{interface}"}
                  rows={3}
                  spellCheck={false}
                  value={draft[side].hooks[key].argv}
                />
              </label>
            ))}
          </div>
          {configuredHooks.length > 0 && (
            <div className="topologyFormGrid twoColumn">
              {configuredHooks.map(({ key, label }) => {
                const command = draft[side].hooks[key];
                return (
                  <div className="tunnelHookEditor" key={key}>
                    <strong>{label} limits</strong>
                    <div className="topologyFormGrid twoColumn">
                      <label title="Hard wall-clock limit; a timeout is reported as a hook failure. Empty uses 10 seconds.">
                        <span>Timeout seconds</span>
                        <input aria-label={`${side} ${label} timeout seconds`} max={120} min={1} onChange={(event) => updateHook(key, "timeout", event.target.value)} placeholder="10" type="number" value={command.timeout} />
                      </label>
                      <label title="Maximum retained hook output. Empty uses 16384 bytes.">
                        <span>Maximum output bytes</span>
                        <input aria-label={`${side} ${label} maximum output bytes`} max={65536} min={1024} onChange={(event) => updateHook(key, "output", event.target.value)} placeholder="16384" type="number" value={command.output} />
                      </label>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </section>

        <section aria-label="Routes and cleanup" className="tunnelAdvancedSection">
          <div className="tunnelAdvancedHeading">
            <strong>Routes and cleanup</strong>
            <span className="formHint" title="Applies independently of the endpoint tab above.">Both endpoints · plan-scoped</span>
          </div>
          {children}
        </section>
      </div>
    </details>
  );
}
