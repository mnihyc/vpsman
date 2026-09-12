import { Copy } from "lucide-react";
import { useRef, useState } from "react";
import type { JobSubmittedRequestRecord } from "../types";
import { ActionFeedback } from "./ActionFeedback";

export function SubmittedJobRequest({
  loadRequest,
  operationType,
}: {
  loadRequest: () => Promise<JobSubmittedRequestRecord>;
  operationType: string;
}) {
  const [request, setRequest] = useState<JobSubmittedRequestRecord>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  const inFlight = useRef(false);

  async function load() {
    if (inFlight.current || request !== undefined) return;
    inFlight.current = true;
    setLoading(true);
    setError(null);
    try {
      setRequest(await loadRequest());
    } catch (cause) {
      setError(
        cause instanceof Error ? cause.message : "Could not load submitted request",
      );
    } finally {
      inFlight.current = false;
      setLoading(false);
    }
  }

  const operation = request?.operation;
  const object =
    operation && typeof operation === "object" && !Array.isArray(operation)
      ? operation
      : null;
  const argv = object?.type === "shell" && Array.isArray(object.argv)
    ? object.argv
    : null;
  const script = object?.type === "shell_script" && typeof object.script === "string"
    ? object.script
    : null;
  const value = argv !== null
    ? JSON.stringify(argv)
    : script !== null
      ? script
      : operation != null
        ? JSON.stringify(operation, null, 2)
        : null;
  const copyLabel = argv !== null
    ? "Copy argv JSON"
    : script !== null
      ? "Copy script"
      : "Copy request JSON";
  const kind = argv !== null
    ? object?.pty ? "Argv · PTY" : "Argv"
    : script !== null
      ? "Shell script"
      : operationType.replace(/_/g, " ");

  async function copy() {
    if (value === null) return;
    try {
      await navigator.clipboard.writeText(value);
      setCopyStatus("Copied");
    } catch {
      setCopyStatus("Copy failed; select the request text manually");
    }
  }

  return (
    <details
      className="auditEventAdvanced submittedJobRequest"
      onToggle={(event) => {
        if (event.currentTarget.open && !error) void load();
      }}
    >
      <summary>Submitted request{kind ? ` · ${kind}` : ""}</summary>
      <div className="submittedJobRequestBody">
        {loading ? (
          <span className="mutedText" role="status">Loading submitted request…</span>
        ) : null}
        {error ? (
          <>
            <ActionFeedback message={error} tone="danger" />
            <button
              className="secondaryAction compactAction"
              onClick={() => void load()}
              type="button"
            >
              Retry
            </button>
          </>
        ) : null}
        {request && operation == null ? (
          <span className="mutedText">No request payload recorded</span>
        ) : null}
        {value !== null ? (
          <>
            <div className="submittedJobRequestActions">
              <span className="mutedText" role="status">{copyStatus}</span>
              <button
                className="secondaryAction compactAction"
                onClick={() => void copy()}
                type="button"
              >
                <Copy size={14} />
                {copyLabel}
              </button>
            </div>
            <pre
              aria-label="Submitted request payload"
              className="auditEventMetadata"
              tabIndex={0}
            >
              <code>{value}</code>
            </pre>
          </>
        ) : null}
      </div>
    </details>
  );
}
