import {
  useEffect,
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";
import * as Collapsible from "@radix-ui/react-collapsible";
import * as ContextMenu from "@radix-ui/react-context-menu";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { Group, Panel, Separator } from "react-resizable-panels";
import { ChevronDown, GripVertical, X } from "lucide-react";
import { scrollIntoViewWithMotion } from "../motion";

const ACTION_DRAWER_FOCUS_SETTLE_MS = 50;

export function ConsoleActionDrawer({
  children,
  description,
  footer,
  onClose,
  open,
  title,
}: {
  children: ReactNode;
  description?: string;
  footer?: ReactNode;
  onClose: () => void;
  open: boolean;
  title: string;
}) {
  const drawerRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) {
      return undefined;
    }
    const activeElement =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    returnFocusRef.current =
      activeElement &&
      activeElement !== document.body &&
      activeElement !== document.documentElement &&
      !drawerRef.current?.contains(activeElement)
        ? activeElement
        : null;

    return () => {
      const returnTarget = returnFocusRef.current;
      returnFocusRef.current = null;
      window.requestAnimationFrame(() => {
        if (returnTarget?.isConnected) {
          returnTarget.focus({ preventScroll: true });
        }
      });
    };
  }, [open]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const focusTimer = window.setTimeout(() => {
      if (drawerRef.current) {
        scrollIntoViewWithMotion(drawerRef.current, { block: "start" });
      }
      const preferredControl = drawerRef.current?.querySelector<HTMLElement>(
        ".actionDrawerBody .actionDrawerInitialFocus input:not([disabled]), .actionDrawerBody .actionDrawerInitialFocus select:not([disabled]), .actionDrawerBody .actionDrawerInitialFocus textarea:not([disabled]), .actionDrawerBody .actionDrawerInitialFocus button:not([disabled]), .actionDrawerBody [data-action-drawer-initial-focus='true']:not([disabled])",
      );
      const firstControl = drawerRef.current?.querySelector<HTMLElement>(
        ".actionDrawerBody input:not([disabled]), .actionDrawerBody select:not([disabled]), .actionDrawerBody textarea:not([disabled]), .actionDrawerBody button:not([disabled]), .actionDrawerBody [tabindex]:not([tabindex='-1'])",
      );
      (preferredControl ?? firstControl ?? closeButtonRef.current)?.focus({
        preventScroll: true,
      });
    }, ACTION_DRAWER_FOCUS_SETTLE_MS);
    return () => window.clearTimeout(focusTimer);
  }, [open, title]);

  if (!open) {
    return null;
  }

  return (
    <aside className="actionDrawer" aria-label={title} ref={drawerRef}>
      <div className="actionDrawerHeader">
        <div>
          <h2>{title}</h2>
          {description && <span>{description}</span>}
        </div>
        <button
          aria-label={`Close ${title}`}
          className="iconButton"
          onClick={onClose}
          ref={closeButtonRef}
          title={`Close ${title}`}
          type="button"
        >
          <X size={18} />
        </button>
      </div>
      <div className="actionDrawerBody">{children}</div>
      {footer && <div className="actionDrawerFooter">{footer}</div>}
    </aside>
  );
}

export function ConsoleStatusBadge({
  children,
  tone = "neutral",
}: {
  children: ReactNode;
  tone?: "critical" | "warning" | "ok" | "info" | "neutral";
}) {
  return <span className={`consoleStatusBadge ${tone}`}>{children}</span>;
}

export type ConsoleMenuAction = {
  disabled?: boolean;
  label: string;
  onSelect: () => void;
  title?: string;
  tone?: "danger" | "normal";
};

export function ConsoleActionMenu({
  actions,
  label = "Actions",
}: {
  actions: ConsoleMenuAction[];
  label?: string;
}) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          aria-label={label}
          className="secondaryAction compactAction"
          type="button"
        >
          <span>Actions</span>
          <ChevronDown size={16} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          className="consoleMenu"
          collisionPadding={12}
          loop
          sideOffset={6}
        >
          {actions.map((action) => (
            <DropdownMenu.Item
              className={
                action.tone === "danger"
                  ? "consoleMenuItem danger"
                  : "consoleMenuItem"
              }
              disabled={action.disabled}
              key={action.label}
              onSelect={action.onSelect}
              title={action.title}
            >
              {action.label}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export function ConsoleInlineActions({
  actions,
  label,
}: {
  actions: ConsoleMenuAction[];
  label: string;
}) {
  return (
    <div aria-label={label} className="gridMobileActions">
      {actions.map((action) => (
        <button
          className={
            action.tone === "danger"
              ? "secondaryAction compactAction danger"
              : "secondaryAction compactAction"
          }
          disabled={action.disabled}
          key={action.label}
          onClick={action.onSelect}
          title={action.title}
          type="button"
        >
          <span>{action.label}</span>
        </button>
      ))}
    </div>
  );
}

export function ConsoleContextActionMenu({
  actions,
  children,
  label = "Row actions",
}: {
  actions: ConsoleMenuAction[];
  children: ReactElement;
  label?: string;
}) {
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>{children}</ContextMenu.Trigger>
      {actions.length > 0 && (
        <ContextMenu.Portal>
          <ContextMenu.Content
            className="consoleMenu"
            collisionPadding={12}
            loop
          >
            <ContextMenu.Label className="consoleMenuLabel">
              {label}
            </ContextMenu.Label>
            {actions.map((action) => (
              <ContextMenu.Item
                className={
                  action.tone === "danger"
                    ? "consoleMenuItem danger"
                    : "consoleMenuItem"
                }
                disabled={action.disabled}
                key={action.label}
                onSelect={action.onSelect}
                title={action.title}
              >
                {action.label}
              </ContextMenu.Item>
            ))}
          </ContextMenu.Content>
        </ContextMenu.Portal>
      )}
    </ContextMenu.Root>
  );
}

export function ConsoleCollapsibleSection({
  children,
  defaultOpen = false,
  forceOpenKey,
  storageKey,
  summary,
  title,
}: {
  children: ReactNode;
  defaultOpen?: boolean;
  forceOpenKey?: string | null;
  storageKey: string;
  summary?: ReactNode;
  title: string;
}) {
  const [open, setOpen] = useStoredBoolean(storageKey, defaultOpen);
  useEffect(() => {
    if (forceOpenKey) {
      setOpen(true);
    }
  }, [forceOpenKey, setOpen]);
  return (
    <Collapsible.Root
      className="consoleCollapsible"
      onOpenChange={setOpen}
      open={open}
    >
      <div className="consoleCollapsibleHeader">
        <div>
          <h2>{title}</h2>
          {summary && <span>{summary}</span>}
        </div>
        <Collapsible.Trigger asChild>
          <button
            aria-label={`${open ? "Collapse" : "Expand"} ${title}`}
            className="secondaryAction compactAction"
            type="button"
          >
            <ChevronDown
              className={open ? "collapseChevron open" : "collapseChevron"}
              size={17}
            />
            <span>{open ? "Hide" : "Show"}</span>
          </button>
        </Collapsible.Trigger>
      </div>
      <Collapsible.Content className="consoleCollapsibleBody">
        {children}
      </Collapsible.Content>
    </Collapsible.Root>
  );
}

export function ConsoleSplitWorkspace({
  detail,
  detailMin = 24,
  detailSize = 32,
  id,
  main,
}: {
  detail: ReactNode;
  detailMin?: number;
  detailSize?: number;
  id: string;
  main: ReactNode;
}) {
  const storageKey = `vpsman.split.${id}`;
  const [layout, setLayout] = useStoredLayout(storageKey, [
    100 - detailSize,
    detailSize,
  ]);
  return (
    <Group
      className="consoleSplitWorkspace"
      defaultLayout={{ [idMain(id)]: layout[0], [idDetail(id)]: layout[1] }}
      id={id}
      onLayoutChanged={(nextLayout) => {
        const mainSize = nextLayout[idMain(id)] ?? layout[0];
        const detailSize = nextLayout[idDetail(id)] ?? layout[1];
        setLayout([mainSize, detailSize]);
      }}
      orientation="horizontal"
    >
      <Panel defaultSize={layout[0]} id={idMain(id)} minSize={42}>
        {main}
      </Panel>
      <Separator className="consoleResizeHandle">
        <GripVertical size={16} />
      </Separator>
      <Panel defaultSize={layout[1]} id={idDetail(id)} minSize={detailMin}>
        {detail}
      </Panel>
    </Group>
  );
}

function useStoredBoolean(
  storageKey: string,
  fallback: boolean,
): [boolean, (value: boolean) => void] {
  const [value, setValue] = useState(() => {
    if (typeof window === "undefined") {
      return fallback;
    }
    try {
      const stored = window.localStorage.getItem(storageKey);
      return stored === null ? fallback : stored === "true";
    } catch {
      return fallback;
    }
  });
  function update(next: boolean) {
    setValue(next);
    try {
      window.localStorage.setItem(storageKey, String(next));
    } catch {
      // Best-effort local UI preference only.
    }
  }
  return [value, update];
}

function idMain(id: string): string {
  return `${id}-main`;
}

function idDetail(id: string): string {
  return `${id}-detail`;
}

function useStoredLayout(
  storageKey: string,
  fallback: [number, number],
): [[number, number], (value: [number, number]) => void] {
  const [value, setValue] = useState<[number, number]>(() => {
    if (typeof window === "undefined") {
      return fallback;
    }
    try {
      const parsed = JSON.parse(
        window.localStorage.getItem(storageKey) ?? "null",
      ) as [number, number] | null;
      return Array.isArray(parsed) &&
        parsed.length === 2 &&
        parsed.every((item) => typeof item === "number")
        ? parsed
        : fallback;
    } catch {
      return fallback;
    }
  });
  function update(next: [number, number]) {
    setValue(next);
    try {
      window.localStorage.setItem(storageKey, JSON.stringify(next));
    } catch {
      // Best-effort local UI preference only.
    }
  }
  return [value, update];
}
