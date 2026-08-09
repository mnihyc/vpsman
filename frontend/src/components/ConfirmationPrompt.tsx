import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { AlertTriangle, X } from "lucide-react";
import { usePanelDisplaySettings } from "../panelDisplay";

type ModalSiblingState = {
  ariaHidden: string | null;
  element: HTMLElement;
  inert: boolean;
};

type ConfirmationFocusState = {
  focusHistory: HTMLElement[];
  installed: boolean;
  lastExternalFocus: HTMLElement | null;
};

type ConfirmationFocusTarget = {
  ariaLabel: string | null;
  element: HTMLElement;
  name: string | null;
  scope: HTMLElement | null;
  tagName: string;
  text: string;
  title: string | null;
  type: string | null;
};

export type ConfirmationPromptItem =
  | {
      label: string;
      sensitive: true;
      title?: never;
      value: ReactNode;
    }
  | {
      label: string;
      sensitive?: false;
      title?: string;
      value: ReactNode;
    };

const RAPID_CONFIRM_POINTER_WINDOW_MS = 600;
let rapidConfirmPointerUntil = 0;
let rapidConfirmPointerTimer: number | null = null;

function blockRapidConfirmClickThrough(event: MouseEvent) {
  if (event.detail <= 1 || performance.now() >= rapidConfirmPointerUntil) {
    return;
  }
  event.preventDefault();
  event.stopImmediatePropagation();
}

function guardRapidConfirmPointerRepeat(detail: number) {
  if (detail <= 0 || typeof document === "undefined") return;
  rapidConfirmPointerUntil =
    performance.now() + RAPID_CONFIRM_POINTER_WINDOW_MS;
  document.addEventListener("click", blockRapidConfirmClickThrough, true);
  if (rapidConfirmPointerTimer !== null) {
    window.clearTimeout(rapidConfirmPointerTimer);
  }
  rapidConfirmPointerTimer = window.setTimeout(() => {
    document.removeEventListener("click", blockRapidConfirmClickThrough, true);
    rapidConfirmPointerTimer = null;
    rapidConfirmPointerUntil = 0;
  }, RAPID_CONFIRM_POINTER_WINDOW_MS);
}

declare global {
  interface Window {
    __vpsmanConfirmationFocusState?: ConfirmationFocusState;
  }
}

function confirmationFocusState(): ConfirmationFocusState | null {
  if (typeof window === "undefined") {
    return null;
  }
  window.__vpsmanConfirmationFocusState ??= {
    focusHistory: [],
    installed: false,
    lastExternalFocus: null,
  };
  window.__vpsmanConfirmationFocusState.focusHistory ??= [];
  return window.__vpsmanConfirmationFocusState;
}

function trackExternalFocus(event: Event) {
  const target = event.target;
  if (
    target instanceof HTMLElement &&
    target !== document.body &&
    target !== document.documentElement &&
    !target.closest(".confirmationPrompt")
  ) {
    const state = confirmationFocusState();
    if (state) {
      state.lastExternalFocus = target;
      state.focusHistory = [
        target,
        ...state.focusHistory.filter((element) => element !== target),
      ].slice(0, 8);
    }
  }
}

function installExternalFocusTracker() {
  const state = confirmationFocusState();
  if (!state || state.installed || typeof document === "undefined") {
    return;
  }
  state.installed = true;
  document.addEventListener("focusin", trackExternalFocus, true);
  document.addEventListener("pointerdown", trackExternalFocus, true);
}

installExternalFocusTracker();

export function ConfirmationPrompt({
  cancelLabel = "Cancel",
  children,
  className,
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
  className?: string;
  confirmDisabled?: boolean;
  confirmLabel: string;
  detail: ReactNode;
  error?: ReactNode;
  expiresAtUnix?: number | null;
  items?: ConfirmationPromptItem[];
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
  const overlaySubmissionRef = useRef(false);
  const confirmLatchedRef = useRef(false);
  const observedPendingRef = useRef(false);
  const onCancelRef = useRef(onCancel);
  const pendingRef = useRef(pending);
  const previousFocusRef = useRef<ConfirmationFocusTarget[]>([]);
  const previouslyOpenRef = useRef(false);
  const [typedConfirmation, setTypedConfirmation] = useState("");
  const [confirmLatched, setConfirmLatched] = useState(false);
  const errorId = useId();
  const typedConfirmationRequired = Boolean(typedConfirmationText);
  const typedConfirmationMatches =
    !typedConfirmationText ||
    typedConfirmation.trim() === typedConfirmationText;
  const displayMode =
    preferences.review_prompt_mode === "overlay" ? "overlay" : "inline";
  const confirmBlocked =
    pending || confirmLatched || confirmDisabled || !typedConfirmationMatches;
  const cancelDisabledReason = pending
    ? "The confirmation is being submitted; wait for it to finish before cancelling."
    : confirmLatched
      ? "The confirmation was already accepted and is waiting for the request to start."
      : undefined;
  const confirmDisabledReason = pending
    ? "The confirmation is being submitted."
    : confirmLatched
      ? "This confirmation was already accepted."
      : !typedConfirmationMatches
        ? `Type ${typedConfirmationText} exactly to enable ${confirmLabel}.`
        : confirmDisabled
          ? `${confirmLabel} is unavailable until the reviewed requirements are satisfied.`
          : undefined;

  if (open && !previouslyOpenRef.current) {
    overlaySubmissionRef.current = false;
    const activeElement =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const state = confirmationFocusState();
    const primary =
      activeElement &&
      activeElement !== document.body &&
      activeElement !== document.documentElement &&
      !activeElement.closest(".confirmationPrompt")
        ? activeElement
        : (state?.lastExternalFocus ?? null);
    previousFocusRef.current = captureConfirmationFocusTargets([
      primary,
      ...(state?.focusHistory ?? []),
    ]);
  }
  previouslyOpenRef.current = open;

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
        // prompt visibility and focus after that handoff completes, unless
        // the operator has already moved to another control.
        const activeElement = document.activeElement;
        const openingFocusTarget = previousFocusRef.current[0];
        const openingFocus = openingFocusTarget
          ? resolveConfirmationFocusTarget(openingFocusTarget)
          : null;
        const restoredMenuTrigger =
          activeElement instanceof HTMLElement &&
          activeElement.getAttribute("aria-haspopup") === "menu";
        if (
          activeElement === document.body ||
          activeElement === document.documentElement ||
          activeElement === element ||
          activeElement === openingFocus ||
          restoredMenuTrigger ||
          (activeElement instanceof Node && element.contains(activeElement))
        ) {
          focusPrompt();
        }
      }, 150);
    });
    return () => {
      if (focusTimeout !== null) {
        window.clearTimeout(focusTimeout);
      }
    };
  }, [displayMode, open]);

  useLayoutEffect(() => {
    if (!open || displayMode !== "overlay" || !overlayRef.current) {
      return undefined;
    }
    const overlay = overlayRef.current;
    const previousFocusTargets = previousFocusRef.current;
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
      window.requestAnimationFrame(() => {
        restoreConfirmationFocus(previousFocusTargets);
      });
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
    if (displayMode === "overlay" && overlaySubmissionRef.current && error) {
      return createPortal(
        <div
          aria-atomic="true"
          className="confirmationPromptDetachedError"
          role="alert"
        >
          <AlertTriangle aria-hidden="true" size={18} />
          <div>
            <strong>{title} failed</strong>
            <span>{error}</span>
          </div>
        </div>,
        document.body,
      );
    }
    return null;
  }
  function handleConfirm(event: ReactMouseEvent<HTMLButtonElement>) {
    if (confirmBlocked || confirmLatchedRef.current) {
      return;
    }
    guardRapidConfirmPointerRepeat(event.detail);
    confirmLatchedRef.current = true;
    pendingRef.current = true;
    setConfirmLatched(true);
    try {
      if (displayMode === "overlay") {
        overlaySubmissionRef.current = true;
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
      className={`confirmationPrompt ${tone} ${displayMode}Prompt${className ? ` ${className}` : ""}`}
      aria-label={title}
      aria-describedby={error ? errorId : undefined}
      aria-modal={displayMode === "overlay" ? true : undefined}
      role={displayMode === "overlay" ? "dialog" : "region"}
      tabIndex={-1}
      title={`${title}. Review the exact scope and effect before confirming.`}
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
              const valueTitle = item.sensitive
                ? confirmationItemTitle(item.label, item.value, true)
                : (item.title ??
                  confirmationItemTitle(item.label, item.value, false));
              return (
                <div key={item.label}>
                  <dt title={`${item.label} review field.`}>{item.label}</dt>
                  <dd
                    data-tooltip-sensitive={item.sensitive ? "true" : undefined}
                    data-value-tooltip-skip={
                      item.sensitive ? "true" : undefined
                    }
                    title={valueTitle}
                  >
                    {item.value}
                  </dd>
                </div>
              );
            })}
          </dl>
        )}
        {typedConfirmationRequired && (
          <label className="confirmationTypedInput">
            <span>
              {typedConfirmationLabel ??
                `Type ${typedConfirmationText} to confirm`}
            </span>
            <input
              aria-label={
                typedConfirmationLabel ??
                `Type ${typedConfirmationText} to confirm`
              }
              autoComplete="off"
              onChange={(event) => setTypedConfirmation(event.target.value)}
              value={typedConfirmation}
            />
          </label>
        )}
        {children}
        {error && (
          <small
            aria-atomic="true"
            className="confirmationPromptError"
            id={errorId}
            role="alert"
          >
            {error}
          </small>
        )}
      </div>
      <button
        aria-label="Close confirmation"
        className="iconButton confirmationPromptClose"
        disabled={pending || confirmLatched}
        data-tooltip-disabled-reason={cancelDisabledReason}
        onClick={onCancel}
        title={
          cancelDisabledReason ??
          "Close confirmation without applying the reviewed action."
        }
        type="button"
      >
        <X size={16} />
      </button>
      <div className="confirmationPromptActions">
        <button
          className="secondaryAction compactAction"
          disabled={pending || confirmLatched}
          data-tooltip-disabled-reason={cancelDisabledReason}
          onClick={onCancel}
          title={
            cancelDisabledReason ??
            `${cancelLabel} and leave the reviewed state unchanged.`
          }
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
          data-tooltip-disabled-reason={confirmDisabledReason}
          onClick={handleConfirm}
          title={
            confirmDisabledReason ?? `${confirmLabel} using the reviewed scope.`
          }
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
  const delta = scrollDelta(
    target.getBoundingClientRect(),
    visibleTop,
    visibleBottom,
  );
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
  const delta = scrollDelta(
    target.getBoundingClientRect(),
    visibleTop,
    visibleBottom,
  );
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

function captureConfirmationFocusTargets(
  elements: Array<HTMLElement | null>,
): ConfirmationFocusTarget[] {
  const seen = new Set<HTMLElement>();
  return elements.flatMap((element) => {
    if (!element || seen.has(element)) {
      return [];
    }
    seen.add(element);
    return [
      {
        ariaLabel: element.getAttribute("aria-label"),
        element,
        name: element.getAttribute("name"),
        scope: element.closest<HTMLElement>(
          "#console-main-content, .actionDrawer, .sidebar, main",
        ),
        tagName: element.tagName.toLocaleLowerCase(),
        text: normalizeFocusText(element.textContent ?? ""),
        // Generated value tooltips are presentation, not stable control
        // identity. A synchronously mounted prompt can replace its trigger
        // before the tooltip observer decorates the replacement.
        title:
          element.dataset.valueTooltip === "true"
            ? null
            : element.getAttribute("title"),
        type: element.getAttribute("type"),
      },
    ];
  });
}

function restoreConfirmationFocus(
  targets: ConfirmationFocusTarget[],
  attempt = 0,
) {
  for (const target of targets) {
    const element = resolveConfirmationFocusTarget(target);
    if (!element) {
      continue;
    }
    const unavailable =
      element.matches(":disabled") ||
      element.getAttribute("aria-disabled") === "true";
    if (unavailable) {
      continue;
    }
    element.focus({ preventScroll: true });
    if (document.activeElement === element) {
      return;
    }
  }
  if (attempt >= 10) {
    return;
  }
  window.setTimeout(() => restoreConfirmationFocus(targets, attempt + 1), 50);
}

function resolveConfirmationFocusTarget(
  target: ConfirmationFocusTarget,
): HTMLElement | null {
  if (target.element.isConnected) {
    return target.element;
  }
  const scope = target.scope?.isConnected ? target.scope : document.body;
  const candidates = Array.from(
    scope.querySelectorAll<HTMLElement>(target.tagName),
  ).filter(
    (element) =>
      !element.closest(".confirmationPrompt") &&
      element.getClientRects().length > 0,
  );
  return (
    candidates.find(
      (element) =>
        (!target.ariaLabel ||
          element.getAttribute("aria-label") === target.ariaLabel) &&
        (!target.name || element.getAttribute("name") === target.name) &&
        (!target.title || element.getAttribute("title") === target.title) &&
        (!target.type || element.getAttribute("type") === target.type) &&
        (!target.text ||
          normalizeFocusText(element.textContent ?? "") === target.text),
    ) ?? null
  );
}

function normalizeFocusText(value: string) {
  return value.replace(/\s+/g, " ").trim();
}

function confirmationItemTitle(
  label: string,
  value: ReactNode,
  sensitive: boolean,
): string {
  if (sensitive) {
    return `${label} is shown in this confirmation; its exact value is excluded from tooltips.`;
  }
  if (typeof value === "string" || typeof value === "number") {
    const text = String(value);
    return text === "-"
      ? `${label} has no available value.`
      : `${label}: ${text}.`;
  }
  return `${label} review value.`;
}
