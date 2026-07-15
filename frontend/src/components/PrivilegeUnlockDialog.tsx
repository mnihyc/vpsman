import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { ShieldCheck, X } from "lucide-react";
import { PrivilegeVaultBox } from "./PrivilegeVaultBox";
import type { PrivilegeMaterial } from "../privilege";

type ModalSiblingState = {
  ariaHidden: string | null;
  element: HTMLElement;
  inert: boolean;
};

export function PrivilegeUnlockDialog({
  onClose,
  onPrivilegeMaterialChange,
  open,
}: {
  onClose: () => void;
  onPrivilegeMaterialChange: (material: PrivilegeMaterial | null) => void;
  open: boolean;
}) {
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const dialogRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open || !overlayRef.current || !dialogRef.current) {
      return undefined;
    }
    const overlay = overlayRef.current;
    const dialog = dialogRef.current;
    const previousFocus =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const siblings: ModalSiblingState[] = Array.from(document.body.children)
      .filter(
        (element): element is HTMLElement =>
          element instanceof HTMLElement && element !== overlay,
      )
      .map((element) => ({
        ariaHidden: element.getAttribute("aria-hidden"),
        element,
        inert: element.inert,
      }));
    for (const sibling of siblings) {
      sibling.element.inert = true;
      sibling.element.setAttribute("aria-hidden", "true");
    }

    window.requestAnimationFrame(() => {
      const firstInput = dialog.querySelector<HTMLElement>("input:not([disabled])");
      (firstInput ?? dialog).focus({ preventScroll: true });
    });

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      const focusable = getFocusableElements(dialog);
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus({ preventScroll: true });
        return;
      }
      const first = focusable[0]!;
      const last = focusable[focusable.length - 1]!;
      const active = document.activeElement;
      if (event.shiftKey && (active === first || !dialog.contains(active))) {
        event.preventDefault();
        last.focus({ preventScroll: true });
      } else if (!event.shiftKey && (active === last || !dialog.contains(active))) {
        event.preventDefault();
        first.focus({ preventScroll: true });
      }
    }

    function handleFocusIn(event: FocusEvent) {
      if (event.target instanceof Node && dialog.contains(event.target)) {
        return;
      }
      dialog.focus({ preventScroll: true });
    }

    document.addEventListener("keydown", handleKeyDown, true);
    document.addEventListener("focusin", handleFocusIn);
    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
      document.removeEventListener("focusin", handleFocusIn);
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
  }, [onClose, open]);

  if (!open) {
    return null;
  }

  return createPortal(
    <div className="privilegeUnlockOverlay" ref={overlayRef}>
      <section
        aria-labelledby="privilege-unlock-title"
        aria-modal="true"
        className="privilegeUnlockDialog"
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <header>
          <div>
            <ShieldCheck size={18} />
            <span>
              <strong id="privilege-unlock-title">Unlock privilege</strong>
              <small>Current page and draft stay open</small>
            </span>
          </div>
          <button
            aria-label="Close privilege unlock"
            className="iconButton"
            onClick={onClose}
            title="Close privilege unlock"
            type="button"
          >
            <X size={16} />
          </button>
        </header>
        <PrivilegeVaultBox
          labelPrefix="Unlock"
          lastPayloadHash={null}
          onPrivilegeMaterialChange={(material) => {
            onPrivilegeMaterialChange(material);
            if (material) {
              onClose();
            }
          }}
          privilegeMaterial={null}
          showVaultClear={false}
          usePrivilegeLabel="Unlock"
        />
      </section>
    </div>,
    document.body,
  );
}

function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
    ),
  ).filter(
    (element) =>
      !element.hidden &&
      element.getAttribute("aria-hidden") !== "true" &&
      element.getClientRects().length > 0,
  );
}
