import type { ReactNode } from "react";
import { AlertTriangle } from "lucide-react";

export function ConfirmationPrompt({
  cancelLabel = "Cancel",
  confirmLabel,
  detail,
  error,
  items = [],
  onCancel,
  onConfirm,
  open,
  pending = false,
  title,
  tone = "normal",
}: {
  cancelLabel?: string;
  confirmLabel: string;
  detail: ReactNode;
  error?: ReactNode;
  items?: Array<{ label: string; value: ReactNode }>;
  onCancel: () => void;
  onConfirm: () => void;
  open: boolean;
  pending?: boolean;
  title: string;
  tone?: "danger" | "normal";
}) {
  if (!open) {
    return null;
  }
  return (
    <section className={`confirmationPrompt ${tone}`} aria-label={title}>
      <div className="confirmationPromptIcon">
        <AlertTriangle size={18} />
      </div>
      <div className="confirmationPromptBody">
        <strong>{title}</strong>
        <span>{detail}</span>
        {items.length > 0 && (
          <dl>
            {items.map((item) => (
              <div key={item.label}>
                <dt>{item.label}</dt>
                <dd>{item.value}</dd>
              </div>
            ))}
          </dl>
        )}
        {error && <small className="confirmationPromptError">{error}</small>}
      </div>
      <div className="confirmationPromptActions">
        <button
          className="secondaryAction compactAction"
          disabled={pending}
          onClick={onCancel}
          type="button"
        >
          {cancelLabel}
        </button>
        <button
          className={
            tone === "danger"
              ? "primaryAction dangerPrimary compactAction"
              : "primaryAction compactAction"
          }
          disabled={pending}
          onClick={onConfirm}
          type="button"
        >
          {confirmLabel}
        </button>
      </div>
    </section>
  );
}
