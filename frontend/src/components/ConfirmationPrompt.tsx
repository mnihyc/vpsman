import { useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { AlertTriangle, X } from "lucide-react";
import { usePanelDisplaySettings } from "../panelDisplay";

type ModalSiblingState = {
  ariaHidden: string | null;
  element: HTMLElement;
  inert: boolean;
};

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
  tone?: "danger" | "normal" | "warning";
}) {
  const { preferences } = usePanelDisplaySettings();
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const promptRef = useRef<HTMLElement | null>(null);
  const confirmLatchedRef = useRef(false);
  const observedPendingRef = useRef(false);
  const onCancelRef = useRef(onCancel);
  const pendingRef = useRef(pending);
  const [typedConfirmation, setTypedConfirmation] = useState("");
  const [confirmLatched, setConfirmLatched] = useState(false);
  const typedConfirmationRequired = Boolean(typedConfirmationText);
  const typedConfirmationMatches =
    !typedConfirmationText || typedConfirmation.trim() === typedConfirmationText;
  const displayMode =
    preferences.review_prompt_mode === "overlay" ? "overlay" : "inline";
  const confirmBlocked =
    pending || confirmLatched || confirmDisabled || !typedConfirmationMatches;

  useEffect(() => {
    onCancelRef.current = onCancel;
  }, [onCancel]);

  useEffect(() => {
    pendingRef.current = pending;
    if (pending) {
      observedPendingRef.current = true;
      return;
    }
    if (observedPendingRef.current) {
      observedPendingRef.current = false;
      confirmLatchedRef.current = false;
      setConfirmLatched(false);
    }
  }, [pending]);

  useEffect(() => {
    if (!open || error) {
      confirmLatchedRef.current = false;
      observedPendingRef.current = false;
      setConfirmLatched(false);
    }
  }, [error, open]);

  useEffect(() => {
    if (!open || !promptRef.current) {
      return;
    }
    const element = promptRef.current;
    let focusTimeout: number | null = null;
    const focusPrompt = () => {
      if (!element.isConnected) {
        return;
      }
      if (displayMode === "inline") {
        scrollInlinePromptIntoView(element);
      }
      element.focus({ preventScroll: true });
    };
    window.requestAnimationFrame(() => {
      focusPrompt();
      focusTimeout = window.setTimeout(() => {
        // Menus restore focus to their trigger after closing. Reassert both
        // prompt visibility and focus after that handoff completes.
        focusPrompt();
      }, 150);
    });
    return () => {
      if (focusTimeout !== null) {
        window.clearTimeout(focusTimeout);
      }
    };
  }, [displayMode, open]);

  useEffect(() => {
    if (!open || displayMode !== "overlay" || !overlayRef.current) {
      return undefined;
    }
    const overlay = overlayRef.current;
    const previousFocus =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const siblings: ModalSiblingState[] = Array.from(
      document.body.children,
    ).flatMap((element) => {
      if (!(element instanceof HTMLElement) || element === overlay) {
        return [];
      }
      return [
        {
          ariaHidden: element.getAttribute("aria-hidden"),
          element,
          inert: element.inert,
        },
      ];
    });
    for (const sibling of siblings) {
      sibling.element.inert = true;
      sibling.element.setAttribute("aria-hidden", "true");
    }
    return () => {
      for (const sibling of siblings) {
        sibling.element.inert = sibling.inert;
        if (sibling.ariaHidden === null) {
          sibling.element.removeAttribute("aria-hidden");
        } else {
          sibling.element.setAttribute("aria-hidden", sibling.ariaHidden);
        }
      }
      if (previousFocus?.isConnected) {
        previousFocus.focus({ preventScroll: true });
      }
    };
  }, [displayMode, open]);

  useEffect(() => {
    if (!open || displayMode !== "overlay") {
      return undefined;
    }
    function handleKeyDown(event: KeyboardEvent) {
      const element = promptRef.current;
      if (!element) {
        return;
      }
      if (event.key === "Escape") {
        if (!pendingRef.current) {
          event.preventDefault();
          onCancelRef.current();
        }
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      const focusable = getFocusableElements(element);
      if (focusable.length === 0) {
        event.preventDefault();
        element.focus({ preventScroll: true });
        return;
      }
      const activeElement = document.activeElement;
      const first = focusable[0]!;
      const last = focusable[focusable.length - 1]!;
      const focusInsideControls =
        activeElement instanceof HTMLElement &&
        activeElement !== element &&
        element.contains(activeElement);
      if (event.shiftKey) {
        if (activeElement === first || !focusInsideControls) {
          event.preventDefault();
          last.focus({ preventScroll: true });
        }
        return;
      }
      if (activeElement === last || !focusInsideControls) {
        event.preventDefault();
        first.focus({ preventScroll: true });
      }
    }
    function handleFocusIn(event: FocusEvent) {
      const element = promptRef.current;
      if (!element) {
        return;
      }
      if (event.target instanceof Node && element.contains(event.target)) {
        return;
      }
      element.focus({ preventScroll: true });
    }
    document.addEventListener("keydown", handleKeyDown, true);
    document.addEventListener("focusin", handleFocusIn);
    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
      document.removeEventListener("focusin", handleFocusIn);
    };
  }, [displayMode, open]);

  useEffect(() => {
    if (open) {
      setTypedConfirmation("");
    }
  }, [open, typedConfirmationText]);

  useEffect(() => {
    if (
      !open ||
      pending ||
      confirmLatched ||
      expiresAtUnix === null ||
      expiresAtUnix === undefined
    ) {
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
  function handleConfirm() {
    if (confirmBlocked || confirmLatchedRef.current) {
      return;
    }
    confirmLatchedRef.current = true;
    pendingRef.current = true;
    setConfirmLatched(true);
    try {
      if (displayMode === "overlay") {
        onCancel();
      }
      onConfirm();
    } catch (error) {
      confirmLatchedRef.current = false;
      pendingRef.current = false;
      setConfirmLatched(false);
      throw error;
    }
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
        disabled={pending || confirmLatched}
        onClick={onCancel}
        title="Close confirmation"
        type="button"
      >
        <X size={16} />
      </button>
      <div className="confirmationPromptActions">
        <button
          className="secondaryAction compactAction"
          disabled={pending || confirmLatched}
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
          disabled={confirmBlocked}
          onClick={handleConfirm}
          type="button"
        >
          {confirmLabel}
        </button>
      </div>
    </section>
  );
  if (displayMode === "overlay") {
    const overlay = (
      <div className="confirmationPromptOverlay" ref={overlayRef}>
        {prompt}
      </div>
    );
    return createPortal(overlay, document.body);
  }
  return prompt;
}

function scrollInlinePromptIntoView(element: HTMLElement) {
  const content = element.closest<HTMLElement>(".content");
  const scrollContainers = verticalScrollContainers(element);
  const contentScrollContainer =
    scrollContainers.find((container) => container === content) ?? null;
  const nestedScrollContainers = scrollContainers.filter(
    (container) => container !== content,
  );
  const behavior: ScrollBehavior = "auto";

  let outerTarget = element;
  for (const container of nestedScrollContainers) {
    scrollTargetWithinContainer(outerTarget, container, behavior);
    outerTarget = container;
  }
  if (nestedScrollContainers.length > 0) {
    outerTarget = element.closest<HTMLElement>(".actionDrawer") ?? outerTarget;
  }

  if (contentScrollContainer) {
    scrollTargetWithinContainer(
      outerTarget,
      contentScrollContainer,
      behavior,
      stickyTopbarBottom(content),
    );
    return;
  }

  scrollTargetWithinViewport(
    outerTarget,
    behavior,
    stickyTopbarBottom(content),
  );
}

function scrollTargetWithinContainer(
  target: HTMLElement,
  container: HTMLElement,
  behavior: ScrollBehavior,
  occludedTop = 0,
) {
  const containerBox = container.getBoundingClientRect();
  const visibleTop = Math.max(containerBox.top, occludedTop) + 12;
  const visibleBottom = Math.min(containerBox.bottom, window.innerHeight) - 12;
  const delta = scrollDelta(target.getBoundingClientRect(), visibleTop, visibleBottom);
  if (Math.abs(delta) >= 1) {
    container.scrollBy({ behavior, top: delta });
  }
}

function scrollTargetWithinViewport(
  target: HTMLElement,
  behavior: ScrollBehavior,
  occludedTop: number,
) {
  const visibleTop = Math.max(0, occludedTop) + 12;
  const visibleBottom = window.innerHeight - 12;
  const delta = scrollDelta(target.getBoundingClientRect(), visibleTop, visibleBottom);
  if (Math.abs(delta) >= 1) {
    window.scrollBy({ behavior, top: delta });
  }
}

function scrollDelta(box: DOMRect, visibleTop: number, visibleBottom: number) {
  const availableHeight = Math.max(0, visibleBottom - visibleTop);
  if (box.height > availableHeight || box.top < visibleTop) {
    return box.top - visibleTop;
  }
  if (box.bottom > visibleBottom) {
    return box.bottom - visibleBottom;
  }
  return 0;
}

function stickyTopbarBottom(content: HTMLElement | null) {
  const topbar = content?.querySelector<HTMLElement>(":scope > .topbar");
  if (!topbar) {
    return 0;
  }
  const position = window.getComputedStyle(topbar).position;
  return position === "sticky" || position === "fixed"
    ? topbar.getBoundingClientRect().bottom
    : 0;
}

function verticalScrollContainers(element: HTMLElement) {
  const containers: HTMLElement[] = [];
  let current = element.parentElement;
  while (current && current !== document.body) {
    const overflowY = window.getComputedStyle(current).overflowY;
    if (
      (overflowY === "auto" || overflowY === "scroll") &&
      current.scrollHeight > current.clientHeight
    ) {
      containers.push(current);
    }
    current = current.parentElement;
  }
  return containers;
}

function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      [
        "a[href]",
        "button:not([disabled])",
        "textarea:not([disabled])",
        "input:not([disabled])",
        "select:not([disabled])",
        "[tabindex]:not([tabindex='-1'])",
        "[contenteditable='true']",
      ].join(", "),
    ),
  ).filter(
    (element) =>
      !element.hasAttribute("hidden") &&
      element.getAttribute("aria-hidden") !== "true",
  );
}

function confirmationItemTitle(value: ReactNode): string | undefined {
  if (typeof value === "string" || typeof value === "number") {
    return String(value);
  }
  return undefined;
}
