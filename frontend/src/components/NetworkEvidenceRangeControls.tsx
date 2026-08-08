import {
  MonitoringRangeTabs,
  type MonitoringWindow,
} from "./MonitoringRangeTabs";

export function NetworkEvidenceRangeControls({
  ariaLabel,
  endAt,
  onEndAtChange,
  onStartAtChange,
  onWindowChange,
  startAt,
  window,
}: {
  ariaLabel: string;
  endAt: string;
  onEndAtChange: (value: string) => void;
  onStartAtChange: (value: string) => void;
  onWindowChange: (value: MonitoringWindow) => void;
  startAt: string;
  window: MonitoringWindow;
}) {
  return (
    <div className="networkEvidenceRangeControls" aria-label={ariaLabel}>
      <MonitoringRangeTabs
        ariaLabel={`${ariaLabel} preset`}
        onChange={onWindowChange}
        value={window}
      />
      {window === "custom" ? (
        <div className="networkEvidenceCustomRange">
          <label>
            <span>Start</span>
            <input
              aria-label={`${ariaLabel} start`}
              onChange={(event) => onStartAtChange(event.target.value)}
              type="datetime-local"
              value={startAt}
            />
          </label>
          <label>
            <span>End</span>
            <input
              aria-label={`${ariaLabel} end`}
              onChange={(event) => onEndAtChange(event.target.value)}
              type="datetime-local"
              value={endAt}
            />
          </label>
        </div>
      ) : null}
    </div>
  );
}
