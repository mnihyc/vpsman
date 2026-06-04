import { TerminalSquare } from "lucide-react";
import type { TerminalAction } from "../jobDispatchModel";

export function TerminalOperationControls({
  terminalAction,
  terminalArgv,
  terminalCloseReason,
  terminalCols,
  terminalCwd,
  terminalFlowWindowBytes,
  terminalIdleTimeoutSecs,
  terminalInputSeq,
  terminalInputText,
  terminalReplayFromSeq,
  terminalRows,
  terminalSessionId,
  setTerminalAction,
  setTerminalArgv,
  setTerminalCloseReason,
  setTerminalCols,
  setTerminalCwd,
  setTerminalFlowWindowBytes,
  setTerminalIdleTimeoutSecs,
  setTerminalInputSeq,
  setTerminalInputText,
  setTerminalReplayFromSeq,
  setTerminalRows,
  setTerminalSessionId,
}: {
  terminalAction: TerminalAction;
  terminalArgv: string;
  terminalCloseReason: string;
  terminalCols: number;
  terminalCwd: string;
  terminalFlowWindowBytes: number;
  terminalIdleTimeoutSecs: number;
  terminalInputSeq: number;
  terminalInputText: string;
  terminalReplayFromSeq: string;
  terminalRows: number;
  terminalSessionId: string;
  setTerminalAction: (value: TerminalAction) => void;
  setTerminalArgv: (value: string) => void;
  setTerminalCloseReason: (value: string) => void;
  setTerminalCols: (value: number) => void;
  setTerminalCwd: (value: string) => void;
  setTerminalFlowWindowBytes: (value: number) => void;
  setTerminalIdleTimeoutSecs: (value: number) => void;
  setTerminalInputSeq: (value: number) => void;
  setTerminalInputText: (value: string) => void;
  setTerminalReplayFromSeq: (value: string) => void;
  setTerminalRows: (value: number) => void;
  setTerminalSessionId: (value: string) => void;
}) {
  return (
    <div className="operationNote compactOperation terminalOperation">
      <TerminalSquare size={18} />
      <div>
        <strong>Terminal session</strong>
        <span>Proof-gated open/input/poll/resize/close controls</span>
      </div>
      <label>
        <span>Action</span>
        <select
          aria-label="Terminal action"
          onChange={(event) => setTerminalAction(event.target.value as TerminalAction)}
          value={terminalAction}
        >
          <option value="open">open</option>
          <option value="input">input</option>
          <option value="poll">poll</option>
          <option value="resize">resize</option>
          <option value="close">close</option>
        </select>
      </label>
      <label className="wideField">
        <span>Session</span>
        <input
          aria-label="Terminal session id"
          onChange={(event) => setTerminalSessionId(event.target.value)}
          value={terminalSessionId}
        />
      </label>
      {terminalAction === "open" && (
        <>
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
          <label>
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
        </>
      )}
      {(terminalAction === "open" || terminalAction === "poll") && (
        <label>
          <span>Replay seq</span>
          <input
            aria-label="Terminal replay from sequence"
            onChange={(event) => setTerminalReplayFromSeq(event.target.value)}
            placeholder="latest"
            value={terminalReplayFromSeq}
          />
        </label>
      )}
      {terminalAction === "input" && (
        <>
          <label>
            <span>Input seq</span>
            <input
              aria-label="Terminal input sequence"
              min={0}
              onChange={(event) => setTerminalInputSeq(Number(event.target.value))}
              type="number"
              value={terminalInputSeq}
            />
          </label>
          <label className="wideField">
            <span>Input</span>
            <textarea
              aria-label="Terminal input"
              onChange={(event) => setTerminalInputText(event.target.value)}
              rows={3}
              value={terminalInputText}
            />
          </label>
        </>
      )}
      {(terminalAction === "open" || terminalAction === "resize") && (
        <>
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
        </>
      )}
      {terminalAction === "close" && (
        <label className="wideField">
          <span>Reason</span>
          <input
            aria-label="Terminal close reason"
            onChange={(event) => setTerminalCloseReason(event.target.value)}
            value={terminalCloseReason}
          />
        </label>
      )}
    </div>
  );
}
