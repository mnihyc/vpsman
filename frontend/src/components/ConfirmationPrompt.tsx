import { useEffect, useRef, type ReactNode } from "react";
import { AlertTriangle, X } from "lucide-react";

export function ConfirmationPrompt({
  cancelLabel = "Cancel",
  confirmLabel,
  detail,
  error,
  expiresAtUnix = null,
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
  expiresAtUnix?: number | null;
  items?: Array<{ label: string; value: ReactNode }>;
  onCancel: () => void;
  onConfirm: () => void;
  open: boolean;
  pending?: boolean;
  title: string;
  tone?: "danger" | "normal";
}) {
  const promptRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open || !promptRef.current) {
      return;
    }
    const element = promptRef.current;
    let focusTimeout: number | null = null;
    window.requestAnimationFrame(() => {
      const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      element.scrollIntoView({ behavior: reduceMotion ? "auto" : "smooth", block: "center" });
      element.focus({ preventScroll: true });
      focusTimeout = window.setTimeout(() => {
        if (element.isConnected) {
          element.focus({ preventScroll: true });
        }
      }, 100);
    });
    return () => {
      if (focusTimeout !== null) {
        window.clearTimeout(focusTimeout);
      }
    };
  }, [open]);

  useEffect(() => {
    if (!open || pending || expiresAtUnix === null || expiresAtUnix === undefined) {
      return undefined;
    }
    const delayMs = expiresAtUnix * 1000 - Date.now();
    if (delayMs <= 0) {
      onCancel();
      return undefined;
    }
    const timeoutId = window.setTimeout(onCancel, delayMs);
    return () => window.clearTimeout(timeoutId);
  }, [expiresAtUnix, onCancel, open, pending]);

  if (!open) {
    return null;
  }
  return (
    <section
      ref={promptRef}
      className={`confirmationPrompt ${tone}`}
      aria-label={title}
      tabIndex={-1}
    >
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
      <button
        aria-label="Close confirmation"
        className="iconButton confirmationPromptClose"
        disabled={pending}
        onClick={onCancel}
        title="Close confirmation"
        type="button"
      >
        <X size={16} />
      </button>
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
