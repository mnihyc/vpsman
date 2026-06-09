import { SearchExpressionInput } from "../components/SearchExpressionInput";
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
  return (
    <div className="targetSelector">
      <div className="targetSelectorHeader">
        <strong>Targets</strong>
        <span>{verificationMessage ?? "Select VPSs by id, tag, provider, country, or status"}</span>
      </div>
      <SearchExpressionInput
        agents={agents}
        ariaLabel="Bulk target selector expression"
        className="targetExpressionBar"
        onChange={setSelectorExpression}
        placeholder="id:edge-* || (provider:alpha && country:US)"
        showMatchCount
        value={selectorExpression}
        verification={verification}
        verificationMessage={verificationMessage}
      />
    </div>
  );
}

export function DispatchOptions({
  setTimeoutSecs,
  timeoutSecs,
}: {
  setTimeoutSecs: (value: number) => void;
  timeoutSecs: number;
}) {
  return (
    <div className="dispatchControls">
      <label>
        <span>Timeout</span>
        <input
          aria-label="Timeout seconds"
          max={3600}
          min={1}
          onChange={(event) => setTimeoutSecs(Number(event.target.value))}
          type="number"
          value={timeoutSecs}
        />
      </label>
    </div>
  );
}
