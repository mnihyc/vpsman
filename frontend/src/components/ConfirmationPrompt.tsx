import { useEffect, useRef, useState, type ReactNode } from "react";
import { AlertTriangle, X } from "lucide-react";
import { scrollIntoViewWithMotion } from "../motion";
import { usePanelDisplaySettings } from "../panelDisplay";

export function ConfirmationPrompt({
  cancelLabel = "Cancel",
  children,
  confirmDisabled = false,
  confirmLabel,
  detail,
  error,
  expiresAtUnix = null,
  items = [],
  onCancel,
  onConfirm,
  open,
  pending = false,
  typedConfirmationLabel,
  typedConfirmationText,
  title,
  tone = "normal",
}: {
  cancelLabel?: string;
  children?: ReactNode;
  confirmDisabled?: boolean;
  confirmLabel: string;
  detail: ReactNode;
  error?: ReactNode;
  expiresAtUnix?: number | null;
  items?: Array<{ label: string; title?: string; value: ReactNode }>;
  onCancel: () => void;
  onConfirm: () => void;
  open: boolean;
  pending?: boolean;
  typedConfirmationLabel?: string;
  typedConfirmationText?: string;
  title: string;
  tone?: "danger" | "normal";
}) {
  const { preferences } = usePanelDisplaySettings();
  const promptRef = useRef<HTMLElement | null>(null);
  const [typedConfirmation, setTypedConfirmation] = useState("");
  const typedConfirmationRequired = Boolean(typedConfirmationText);
  const typedConfirmationMatches =
    !typedConfirmationText || typedConfirmation.trim() === typedConfirmationText;
  const displayMode =
    preferences.review_prompt_mode === "overlay" ? "overlay" : "inline";

  useEffect(() => {
    if (!open || !promptRef.current) {
      return;
    }
    const element = promptRef.current;
    let focusTimeout: number | null = null;
    window.requestAnimationFrame(() => {
      scrollIntoViewWithMotion(element, { block: "center" });
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
    if (open) {
      setTypedConfirmation("");
    }
  }, [open, typedConfirmationText]);

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
  const prompt = (
    <section
      ref={promptRef}
      className={`confirmationPrompt ${tone} ${displayMode}Prompt`}
      aria-label={title}
      aria-modal={displayMode === "overlay" ? true : undefined}
      role={displayMode === "overlay" ? "dialog" : "region"}
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
            {items.map((item) => {
              const valueTitle = item.title ?? confirmationItemTitle(item.value);
              return (
                <div key={item.label}>
                  <dt>{item.label}</dt>
                  <dd title={valueTitle}>{item.value}</dd>
                </div>
              );
            })}
          </dl>
        )}
        {typedConfirmationRequired && (
          <label className="confirmationTypedInput">
            <span>{typedConfirmationLabel ?? `Type ${typedConfirmationText} to confirm`}</span>
            <input
              aria-label={typedConfirmationLabel ?? `Type ${typedConfirmationText} to confirm`}
              autoComplete="off"
              onChange={(event) => setTypedConfirmation(event.target.value)}
              value={typedConfirmation}
            />
          </label>
        )}
        {children}
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
          disabled={pending || confirmDisabled || !typedConfirmationMatches}
          onClick={onConfirm}
          type="button"
        >
          {confirmLabel}
        </button>
      </div>
    </section>
  );
  if (displayMode === "overlay") {
    return <div className="confirmationPromptOverlay">{prompt}</div>;
  }
  return prompt;
}

function confirmationItemTitle(value: ReactNode): string | undefined {
  if (typeof value === "string" || typeof value === "number") {
    return String(value);
  }
  return undefined;
}
