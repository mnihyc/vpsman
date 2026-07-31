import { TerminalSquare } from "lucide-react";

export function TerminalOperationControls({
  terminalArgv,
  terminalCols,
  terminalCwd,
  terminalUser,
  terminalUserPolicy,
  terminalFlowWindowBytes,
  terminalIdleTimeoutSecs,
  terminalReplayFromSeq,
  terminalRows,
  terminalSessionId,
  setTerminalArgv,
  setTerminalCols,
  setTerminalCwd,
  setTerminalUser,
  setTerminalUserPolicy,
  setTerminalFlowWindowBytes,
  setTerminalIdleTimeoutSecs,
  setTerminalReplayFromSeq,
  setTerminalRows,
  setTerminalSessionId,
}: {
  terminalArgv: string;
  terminalCols: number;
  terminalCwd: string;
  terminalUser: string;
  terminalUserPolicy: "fail" | "fallback";
  terminalFlowWindowBytes: number;
  terminalIdleTimeoutSecs: number;
  terminalReplayFromSeq: string;
  terminalRows: number;
  terminalSessionId: string;
  setTerminalArgv: (value: string) => void;
  setTerminalCols: (value: number) => void;
  setTerminalCwd: (value: string) => void;
  setTerminalUser: (value: string) => void;
  setTerminalUserPolicy: (value: "fail" | "fallback") => void;
  setTerminalFlowWindowBytes: (value: number) => void;
  setTerminalIdleTimeoutSecs: (value: number) => void;
  setTerminalReplayFromSeq: (value: string) => void;
  setTerminalRows: (value: number) => void;
  setTerminalSessionId: (value: string) => void;
}) {
  return (
    <div className="operationNote compactOperation terminalOperation">
      <TerminalSquare size={18} />
      <div>
        <strong>Open terminal session</strong>
        <span>One privileged job authorizes the interactive session</span>
      </div>
      <label className="wideField">
        <span>Session</span>
        <input
          aria-label="Terminal session id"
          onChange={(event) => setTerminalSessionId(event.target.value)}
          value={terminalSessionId}
        />
      </label>
      <label className="wideField">
        <span>Argv</span>
        <textarea
          aria-label="Terminal argv"
          onChange={(event) => setTerminalArgv(event.target.value)}
          rows={2}
          value={terminalArgv}
        />
      </label>
      <label>
        <span>CWD</span>
        <input
          aria-label="Terminal cwd"
          onChange={(event) => setTerminalCwd(event.target.value)}
          placeholder="/root"
          value={terminalCwd}
        />
      </label>
      <label>
        <span>User</span>
        <input
          aria-label="Terminal user"
          onChange={(event) => setTerminalUser(event.target.value)}
          placeholder="agent user"
          value={terminalUser}
        />
      </label>
      <label>
        <span>User policy</span>
        <select
          aria-label="Terminal user policy"
          onChange={(event) =>
            setTerminalUserPolicy(event.target.value as "fail" | "fallback")
          }
          value={terminalUserPolicy}
        >
          <option value="fail">fail</option>
          <option value="fallback">fallback</option>
        </select>
      </label>
      <label>
        <span>Idle secs</span>
        <input
          aria-label="Terminal idle timeout seconds"
          max={86400}
          min={10}
          onChange={(event) => setTerminalIdleTimeoutSecs(Number(event.target.value))}
          type="number"
          value={terminalIdleTimeoutSecs}
        />
      </label>
      <label title="Bounds live terminal replay and API-retained durable replay bytes for this session.">
        <span>Window bytes</span>
        <input
          aria-label="Terminal flow window bytes"
          max={1048576}
          min={4096}
          onChange={(event) => setTerminalFlowWindowBytes(Number(event.target.value))}
          type="number"
          value={terminalFlowWindowBytes}
        />
      </label>
      <label>
        <span>Replay seq</span>
        <input
          aria-label="Terminal replay from sequence"
          onChange={(event) => setTerminalReplayFromSeq(event.target.value)}
          placeholder="latest"
          value={terminalReplayFromSeq}
        />
      </label>
      <label>
        <span>Cols</span>
        <input
          aria-label="Terminal columns"
          max={240}
          min={20}
          onChange={(event) => setTerminalCols(Number(event.target.value))}
          type="number"
          value={terminalCols}
        />
      </label>
      <label>
        <span>Rows</span>
        <input
          aria-label="Terminal rows"
          max={120}
          min={5}
          onChange={(event) => setTerminalRows(Number(event.target.value))}
          type="number"
          value={terminalRows}
        />
      </label>
    </div>
  );
}
