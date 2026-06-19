import { formatTime, shortHash, shortId } from "../../utils";

export type RestoreArchiveTransferOption = {
  key: string;
  observedAt: string;
  path: string;
  sessionId: string;
  sha256Hex: string;
  sizeBytes: number;
};

type RestoreArchiveTransferSelectProps = {
  disabled?: boolean;
  emptyMessage: string;
  label?: string;
  onChange: (value: string) => void;
  options: RestoreArchiveTransferOption[];
  value: string;
};

export function RestoreArchiveTransferSelect({
  disabled = false,
  emptyMessage,
  label = "Staged archive",
  onChange,
  options,
  value,
}: RestoreArchiveTransferSelectProps) {
  const selected = options.find((option) => option.key === value) ?? null;
  return (
    <div className="restoreArchiveSelect">
      <label>
        <span>{label}</span>
        <select
          aria-label={label}
          disabled={disabled || options.length === 0}
          onChange={(event) => onChange(event.target.value)}
          title={
            selected
              ? selected.path
              : "Completed upload transfer whose bytes match the selected backup artifact"
          }
          value={selected ? selected.key : ""}
        >
          <option value="">{options.length === 0 ? emptyMessage : "Select staged archive"}</option>
          {options.map((option) => (
            <option key={option.key} title={option.path} value={option.key}>
              {shortId(option.sessionId)} / {formatBytes(option.sizeBytes)} / {shortHash(option.sha256Hex)}
            </option>
          ))}
        </select>
      </label>
      <div className="restoreArchiveSummary" aria-live="polite">
        {selected ? (
          <>
            <div>
              <span>Path</span>
              <strong title={selected.path}>{selected.path}</strong>
            </div>
            <div>
              <span>Size</span>
              <strong title={String(selected.sizeBytes)}>{formatBytes(selected.sizeBytes)}</strong>
            </div>
            <div>
              <span>SHA-256</span>
              <strong title={selected.sha256Hex}>{shortHash(selected.sha256Hex)}</strong>
            </div>
            <div>
              <span>Observed</span>
              <strong title={selected.observedAt}>{formatTime(selected.observedAt)}</strong>
            </div>
          </>
        ) : (
          <span className="restoreArchiveEmpty">{emptyMessage}</span>
        )}
      </div>
    </div>
  );
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
  }
  if (value >= 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${value} B`;
}
