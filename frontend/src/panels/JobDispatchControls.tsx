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
  canaryCount,
  confirmed,
  destructive,
  proofTtlSecs,
  setCanaryCount,
  setConfirmed,
  setDestructive,
  setProofTtlSecs,
  setTimeoutSecs,
  timeoutSecs,
}: {
  canaryCount: number;
  confirmed: boolean;
  destructive: boolean;
  proofTtlSecs: number;
  setCanaryCount: (value: number) => void;
  setConfirmed: (value: boolean) => void;
  setDestructive: (value: boolean) => void;
  setProofTtlSecs: (value: number) => void;
  setTimeoutSecs: (value: number) => void;
  timeoutSecs: number;
}) {
  return (
    <>
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
        <label>
          <span>Proof TTL</span>
          <input
            aria-label="Proof TTL seconds"
            max={3600}
            min={15}
            onChange={(event) => setProofTtlSecs(Number(event.target.value))}
            type="number"
            value={proofTtlSecs}
          />
        </label>
        <label>
          <span>Canary</span>
          <input
            aria-label="Canary count"
            max={10000}
            min={0}
            onChange={(event) => setCanaryCount(Number(event.target.value))}
            type="number"
            value={canaryCount}
          />
        </label>
      </div>

      <div className="dispatchChecks">
        <label className="checkLine">
          <input checked={destructive} onChange={(event) => setDestructive(event.target.checked)} type="checkbox" />
          <span>Destructive</span>
        </label>
        <label className="checkLine">
          <input checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} type="checkbox" />
          <span>Confirmed</span>
        </label>
      </div>
    </>
  );
}
