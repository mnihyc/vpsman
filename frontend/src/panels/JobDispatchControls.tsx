import { SearchExpressionInput } from "../components/SearchExpressionInput";
import { MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS } from "../commandTimeout";
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
  setTimeoutSecs: (value: string) => void;
  timeoutSecs: string;
}) {
  return (
    <div className="dispatchControls">
      <label>
        <span>Timeout</span>
        <input
          aria-label="Timeout seconds"
          inputMode="numeric"
          maxLength={String(MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS).length}
          onChange={(event) => {
            if (/^\d*$/.test(event.target.value)) {
              setTimeoutSecs(event.target.value);
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
          placeholder="Default agent timeout (1h)"
          type="text"
          value={timeoutSecs}
        />
      </label>
    </div>
  );
}
