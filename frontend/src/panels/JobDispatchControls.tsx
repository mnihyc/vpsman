import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { MAX_CONFIGURABLE_JOB_TIMEOUT_SECS } from "../jobMaxTimeout";
import type { AgentView } from "../types";

export function JobTargetSelector({
  agents,
  selectorExpression,
  setSelectorExpression,
  verification,
  verificationMessage,
}: {
  agents: AgentView[];
  selectorExpression: string;
  setSelectorExpression: (value: string) => void;
  verification: "checking" | "invalid" | "neutral" | "valid";
  verificationMessage: string | null;
}) {
  const scopeSummary = dispatchTargetScopeSummary(
    selectorExpression,
    agents.length,
    verificationMessage,
  );
  return (
    <div className="targetSelector">
      <div className="targetSelectorHeader">
        <strong>Targets</strong>
        <span>{scopeSummary}</span>
      </div>
      <SearchExpressionInput
        agents={agents}
        ariaLabel="Bulk target selector expression"
        className="targetExpressionBar"
        onChange={setSelectorExpression}
        placeholder="All scoped VPSs, or filter by id/tag/provider/country/status"
        showMatchCount
        value={selectorExpression}
        verification={verification}
        verificationMessage={verificationMessage}
      />
    </div>
  );
}

function dispatchTargetScopeSummary(
  selectorExpression: string,
  agentCount: number,
  verificationMessage: string | null,
): string {
  const trimmed = selectorExpression.trim();
  if (!trimmed) {
    return `All ${agentCount} scoped VPSs`;
  }
  if (trimmed === "id:*" || trimmed === "*") {
    return `All ${agentCount} scoped VPSs`;
  }
  return verificationMessage
    ? `${verificationMessage} resolved from selector`
    : "Select VPSs by id, tag, provider, country, or status";
}

export function DispatchOptions({
  setMaxTimeoutSecs,
  maxTimeoutSecs,
}: {
  setMaxTimeoutSecs: (value: string) => void;
  maxTimeoutSecs: string;
}) {
  return (
    <div className="dispatchControls">
      <label>
        <span>Max timeout</span>
        <input
          aria-label="Max timeout seconds"
          inputMode="numeric"
          maxLength={String(MAX_CONFIGURABLE_JOB_TIMEOUT_SECS).length}
          onChange={(event) => {
            if (/^\d*$/.test(event.target.value)) {
              setMaxTimeoutSecs(event.target.value);
            }
          }}
          onKeyDown={(event) => {
            if (event.ctrlKey || event.metaKey || event.altKey) {
              return;
            }
            if (event.key.length === 1 && !/^\d$/.test(event.key)) {
              event.preventDefault();
            }
          }}
          onPaste={(event) => {
            if (!/^\d*$/.test(event.clipboardData.getData("text"))) {
              event.preventDefault();
            }
          }}
          pattern="[0-9]*"
          placeholder="Default max job timeout (1h)"
          type="text"
          value={maxTimeoutSecs}
        />
      </label>
    </div>
  );
}
