import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
import { scrollIntoViewWithMotion } from "../motion";

export function ConsoleDetailPanel({
  actions,
  children,
  description,
  onClose,
  reviewPrompt,
  title,
}: {
  actions?: ReactNode;
  children: ReactNode;
  description?: ReactNode;
  onClose?: () => void;
  reviewPrompt?: ReactNode;
  title: ReactNode;
}) {
  const panelRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    let secondFrame = 0;
    const firstFrame = window.requestAnimationFrame(() => {
      secondFrame = window.requestAnimationFrame(() => {
        const panel = panelRef.current;
        if (!panel) return;
        scrollIntoViewWithMotion(panel, { block: "start" });
        panel.focus({ preventScroll: true });
      });
    });
    return () => {
      window.cancelAnimationFrame(firstFrame);
      window.cancelAnimationFrame(secondFrame);
    };
  }, []);

  return (
    <section
      className="consoleDetailPanel"
      ref={panelRef}
      tabIndex={-1}
      title="Expanded record details and contextual actions."
    >
      <div className="consoleDetailPanelHeader" title="Detail title and context.">
        <span>
          <strong>{title}</strong>
          {description && <small>{description}</small>}
        </span>
        {onClose && (
          <button
            aria-label="Close detail panel"
            className="iconButton"
            onClick={onClose}
            title="Close detail panel"
            type="button"
          >
            <X size={16} />
          </button>
        )}
      </div>
      {children}
      {actions && <div className="consoleFormActions">{actions}</div>}
      {reviewPrompt}
    </section>
  );
}
