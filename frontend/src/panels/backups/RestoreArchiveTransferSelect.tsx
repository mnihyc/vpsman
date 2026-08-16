import { Download, Upload } from "lucide-react";
import { useByteCountFormatter } from "../../panelDisplay";
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
  onDownloadPackage?: () => void;
  onOpenTransfers?: () => void;
  options: RestoreArchiveTransferOption[];
  pending?: boolean;
  value: string;
};

export function RestoreArchiveTransferSelect({
  disabled = false,
  emptyMessage,
  label = "Staged archive",
  onChange,
  onDownloadPackage,
  onOpenTransfers,
  options,
  pending = false,
  value,
}: RestoreArchiveTransferSelectProps) {
  const formatBytes = useByteCountFormatter();
  const selected = options.find((option) => option.key === value) ?? null;
  return (
    <div className="restoreArchiveSelect">
      <label title="Completed upload transfer whose bytes match the selected backup artifact.">
        <span>{label}</span>
        <select
          aria-label={label}
          data-tooltip-disabled-reason={
            disabled
              ? "Archive selection is unavailable for this restore state"
              : options.length === 0
                ? emptyMessage
                : undefined
          }
          disabled={disabled || options.length === 0}
          onChange={(event) => onChange(event.target.value)}
          value={selected ? selected.key : ""}
        >
          <option value="">
            {options.length === 0
              ? onDownloadPackage || onOpenTransfers
                ? "No matching upload"
                : emptyMessage
              : "Select staged archive"}
          </option>
          {options.map((option) => (
            <option key={option.key} value={option.key}>
              {shortId(option.sessionId)} / {formatBytes(option.sizeBytes)} /{" "}
              {shortHash(option.sha256Hex)}
            </option>
          ))}
        </select>
      </label>
      <div
        className="restoreArchiveSummary"
        aria-live="polite"
        title={
          selected
            ? `${selected.path}; ${formatBytes(selected.sizeBytes)}; SHA-256 ${selected.sha256Hex}; observed ${selected.observedAt}`
            : undefined
        }
      >
        {selected ? (
          <>
            <div>
              <span>Path</span>
              <strong>{selected.path}</strong>
            </div>
            <div>
              <span>Size</span>
              <strong>{formatBytes(selected.sizeBytes)}</strong>
            </div>
            <div>
              <span>SHA-256</span>
              <strong title={selected.sha256Hex}>
                {shortHash(selected.sha256Hex)}
              </strong>
            </div>
            <div>
              <span>Observed</span>
              <strong>{formatTime(selected.observedAt)}</strong>
            </div>
          </>
        ) : (
          <div className="restoreArchiveEmptyState">
            <span className="restoreArchiveEmpty">{emptyMessage}</span>
            {onDownloadPackage || onOpenTransfers ? (
              <div className="restoreArchiveActions">
                {onDownloadPackage ? (
                  <button
                    className="secondaryAction"
                    disabled={pending}
                    onClick={onDownloadPackage}
                    title={
                      pending
                        ? "Wait for the current restore operation to finish"
                        : "Download the selected backup package to this browser"
                    }
                    type="button"
                  >
                    <Download size={15} />
                    Download package
                  </button>
                ) : null}
                {onOpenTransfers ? (
                  <button
                    className="secondaryAction"
                    disabled={pending}
                    onClick={onOpenTransfers}
                    title={
                      pending
                        ? "Wait for the current restore operation to finish"
                        : "Open Remote / Transfers with the selected restore target"
                    }
                    type="button"
                  >
                    <Upload size={15} />
                    Open Transfers
                  </button>
                ) : null}
              </div>
            ) : null}
          </div>
        )}
      </div>
    </div>
  );
}
