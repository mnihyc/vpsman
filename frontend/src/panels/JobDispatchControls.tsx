import type { Dispatch, SetStateAction } from "react";
import { Layers3, Server, Tag } from "lucide-react";
import type { AgentView, ResourcePoolView, TagView } from "../types";
import { shortId, toggleValue } from "../utils";

type SetValue<T> = Dispatch<SetStateAction<T>>;

export function JobTargetSelector({
  agents,
  pools,
  selectedClients,
  selectedPools,
  selectedTags,
  setSelectedClients,
  setSelectedPools,
  setSelectedTags,
  setTagMode,
  tagMode,
  tags,
}: {
  agents: AgentView[];
  pools: ResourcePoolView[];
  selectedClients: string[];
  selectedPools: string[];
  selectedTags: string[];
  setSelectedClients: SetValue<string[]>;
  setSelectedPools: SetValue<string[]>;
  setSelectedTags: SetValue<string[]>;
  setTagMode: (value: "any" | "all") => void;
  tagMode: "any" | "all";
  tags: TagView[];
}) {
  return (
    <div className="targetSelector">
      <strong>Targets</strong>
      <div className="chipList">
        {agents.map((agent) => (
          <label className="checkChip" key={agent.id}>
            <input
              checked={selectedClients.includes(agent.id)}
              onChange={() => setSelectedClients((values) => toggleValue(values, agent.id))}
              type="checkbox"
            />
            <Server size={14} />
            <span>{agent.display_name || shortId(agent.id)}</span>
          </label>
        ))}
        {pools.map((pool) => (
          <label className="checkChip" key={pool.id}>
            <input
              checked={selectedPools.includes(pool.id)}
              onChange={() => setSelectedPools((values) => toggleValue(values, pool.id))}
              type="checkbox"
            />
            <Layers3 size={14} />
            <span>{pool.name}</span>
          </label>
        ))}
        {tags.map((tag) => (
          <label className="checkChip" key={tag.name}>
            <input
              checked={selectedTags.includes(tag.name)}
              onChange={() => setSelectedTags((values) => toggleValue(values, tag.name))}
              type="checkbox"
            />
            <Tag size={14} />
            <span>{tag.name}</span>
          </label>
        ))}
      </div>
      <div className="targetModeControls" role="group" aria-label="Tag match mode">
        <span>Tags</span>
        <button className={tagMode === "any" ? "selected" : ""} onClick={() => setTagMode("any")} type="button">
          Any
        </button>
        <button className={tagMode === "all" ? "selected" : ""} onClick={() => setTagMode("all")} type="button">
          All
        </button>
      </div>
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
