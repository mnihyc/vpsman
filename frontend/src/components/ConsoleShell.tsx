import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react";
import {
  BookmarkPlus,
  ChevronDown,
  ChevronRight,
  Cloud,
  Command,
  FolderKanban,
  KeyRound,
  LockKeyhole,
  RadioTower,
  Save,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import { Metric } from "./Metric";
import { SearchExpressionInput } from "./SearchExpressionInput";
import {
  formatLowerBoundCount,
  navSections,
  subpageDescription,
  subpageLabel,
  viewLabel,
  viewSubpages,
} from "../constants";
import type { ActiveView, AgentView, FleetSummary } from "../types";
import type { SavedFleetView } from "../hooks/useFleetViews";
import { usePanelDisplaySettings } from "../panelDisplay";

const SIDEBAR_SUBPANEL_STORAGE_KEY = "vpsman.sidebarSubpanels";

export type CommandPaletteItem = {
  id: string;
  group:
    | "Page"
    | "VPS"
    | "Job"
    | "Terminal"
    | "Transfer"
    | "Backup"
    | "Audit"
    | "Schedule"
    | "Saved view";
  label: string;
  detail: string;
  keywords?: string;
  onSelect: () => void;
};

type ConsoleShellProps = {
  activeSavedFleetViewId: string | null;
  activeSubpage: string;
  activeView: ActiveView;
  agents: AgentView[];
  alertCounts: {
    critical: number;
    info: number;
    total: number;
    truncated: boolean;
    warning: number;
  };
  apiToken: string;
  children: ReactNode;
  commandItems: CommandPaletteItem[];
  onlineRatio: string;
  draftSavedFleetViewName: string;
  filteredAgentCount: number;
  fleetQuery: string;
  hideFleetStatusSummary?: boolean;
  onApplySavedFleetView: (viewId: string) => void;
  onClearSession: () => void;
  onClearFleetView: () => void;
  onDeleteSavedFleetView: () => void;
  onFleetQueryChange: (query: string) => void;
  onOpenAccessControls: () => void;
  onLockPrivilege: () => void;
  onSaveFleetView: () => void;
  onSelectView: (view: ActiveView, subpage?: string) => void;
  onSavedFleetViewNameChange: (name: string) => void;
  operatorPreferencesReady: boolean;
  pageDescription: string;
  pageTitle: string;
  privilegeUnlocked: boolean;
  savedFleetViews: SavedFleetView[];
  summary: FleetSummary;
  summaryScopeLabel: string;
  wsState: string;
};

export function ConsoleShell({
  activeSavedFleetViewId,
  activeSubpage,
  activeView,
  agents,
  alertCounts,
  apiToken,
  children,
  commandItems,
  onlineRatio,
  draftSavedFleetViewName,
  filteredAgentCount,
  fleetQuery,
  hideFleetStatusSummary = false,
  onApplySavedFleetView,
  onClearFleetView,
  onClearSession,
  onDeleteSavedFleetView,
  onFleetQueryChange,
  onLockPrivilege,
  onOpenAccessControls,
  onSaveFleetView,
  onSelectView,
  onSavedFleetViewNameChange,
  operatorPreferencesReady,
  pageDescription,
  pageTitle,
  privilegeUnlocked,
  savedFleetViews,
  summary,
  summaryScopeLabel,
  wsState,
}: ConsoleShellProps) {
  const { preferences } = usePanelDisplaySettings();
  const initialSubpanelPreferences = useRef(readSidebarSubpanelPreferences());
  const storedDefaultRef = useRef<string | null>(initialSubpanelPreferences.current.defaultMode);
  const [manualSubpanelState, setManualSubpanelState] = useState<Record<string, boolean>>(
    initialSubpanelPreferences.current.state,
  );
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [commandQuery, setCommandQuery] = useState("");
  const [activeCommandIndex, setActiveCommandIndex] = useState(0);
  const [browserOnline, setBrowserOnline] = useState(() =>
    typeof navigator === "undefined" ? true : navigator.onLine,
  );
  const commandButtonRef = useRef<HTMLButtonElement | null>(null);
  const commandBackdropRef = useRef<HTMLDivElement | null>(null);
  const commandInputRef = useRef<HTMLInputElement | null>(null);
  const commandReturnFocusRef = useRef<HTMLElement | null>(null);
  const contentRef = useRef<HTMLElement | null>(null);
  const topbarRef = useRef<HTMLElement | null>(null);
  const hasFleetScope = fleetQuery.trim().length > 0 || activeSavedFleetViewId !== null;
  const activeSavedFleetView = savedFleetViews.find((view) => view.id === activeSavedFleetViewId) ?? null;
  const draftSavedFleetViewMatchesExisting = Boolean(
    draftSavedFleetViewName.trim() &&
      savedFleetViews.some(
        (view) =>
          view.name.trim().toLocaleLowerCase() ===
          draftSavedFleetViewName.trim().toLocaleLowerCase(),
      ),
  );
  const savedFleetViewActionLabel = draftSavedFleetViewMatchesExisting
    ? "Override saved fleet view"
    : "Save current fleet view";
  const scopeName = activeSavedFleetView?.name ?? (fleetQuery.trim() ? "Filtered resources" : "All VPS resources");
  const showFullFleetMetrics =
    activeView === "Home" || (activeView === "Fleet" && activeSubpage === "monitor");
  const noContactCount = summary.never + summary.unknown;
  const unavailableCount = summary.offline + summary.stale + noContactCount;
  const fleetStatusTitle = `${summaryScopeLabel}: ${summary.total} VPS; ${summary.online} online; ${summary.offline} offline; ${summary.stale} stale; ${noContactCount} no contact; ${summary.running_jobs} running jobs`;
  const fleetStatusText = [
    `${summaryScopeLabel}: ${summary.total} VPS`,
    `${summary.online} online`,
    summary.offline > 0 ? `${summary.offline} offline` : null,
    summary.stale > 0 ? `${summary.stale} stale` : null,
    noContactCount > 0 ? `${noContactCount} no contact` : null,
    `${summary.running_jobs} running job${summary.running_jobs === 1 ? "" : "s"}`,
  ]
    .filter(Boolean)
    .join(" · ");
  const activeViewLabel = viewLabel(activeView);
  const activeSubpageBase = activeSubpage.split(":")[0];
  const activeSubpageLabel = subpageLabel(activeView, activeSubpage);
  const activeSubpageDescription = subpageDescription(activeView, activeSubpage);
  const controlPlaneLabel = !browserOnline
    ? "Offline"
    : wsState === "connected"
      ? "Live"
      : wsState === "reconnecting"
        ? "Reconnecting"
        : wsState === "connecting"
          ? "Connecting"
          : "Unavailable";
  const controlPlaneDetail = !browserOnline
    ? "Network offline; showing the last loaded data. Actions may fail until connectivity returns."
    : wsState === "reconnecting"
      ? "The live event stream is reconnecting. Loaded data remains visible and may lag until it resumes."
      : wsState === "connecting"
        ? "Connecting to the live event stream."
        : wsState === "connected"
          ? "Live event stream connected."
          : "The live event stream is unavailable.";
  const controlPlaneTone =
    !browserOnline || wsState === "auth required"
      ? "danger"
      : wsState === "connected"
        ? "ok"
        : "warning";

  useEffect(() => {
    const markOnline = () => setBrowserOnline(true);
    const markOffline = () => setBrowserOnline(false);
    window.addEventListener("online", markOnline);
    window.addEventListener("offline", markOffline);
    return () => {
      window.removeEventListener("online", markOnline);
      window.removeEventListener("offline", markOffline);
    };
  }, []);
  const mobilePageValue = `${activeView}::${activeSubpageBase}`;
  const selectMobilePage = (value: string) => {
    const option = navSections
      .flatMap((section) =>
        section.items.flatMap((item) =>
          (viewSubpages[item.view] ?? []).map((subpage) => ({
            subpage: subpage.id,
            view: item.view,
          })),
        ),
      )
      .find((item) => `${item.view}::${item.subpage}` === value);
    if (!option) {
      return;
    }
    onSelectView(option.view, option.subpage);
  };
  const isSubpanelExpanded = (view: ActiveView, hasSubpages: boolean) => {
    if (!hasSubpages) {
      return false;
    }
    const manual = manualSubpanelState[view];
    if (manual !== undefined) {
      return manual;
    }
    return preferences.sidebar_subpanel_default === "all" || activeView === view;
  };
  const toggleSubpanel = (view: ActiveView, expanded: boolean) => {
    setManualSubpanelState((current) => {
      const next = { ...current, [view]: !expanded };
      writeSidebarSubpanelPreferences(preferences.sidebar_subpanel_default, next);
      return next;
    });
  };
  const selectPrimaryNavItem = (view: ActiveView, hasSubpages: boolean, expanded: boolean) => {
    if (hasSubpages && !expanded) {
      setManualSubpanelState((current) => {
        const next = { ...current, [view]: true };
        writeSidebarSubpanelPreferences(preferences.sidebar_subpanel_default, next);
        return next;
      });
    }
    onSelectView(view);
  };
  const filteredCommandItems = useMemo(() => {
    const terms = commandQuery
      .trim()
      .toLocaleLowerCase()
      .split(/\s+/)
      .filter(Boolean);
    const matched =
      terms.length === 0
        ? commandItems
        : commandItems.filter((item) => {
            const haystack = [
              item.group,
              item.label,
              item.detail,
              item.keywords ?? "",
            ]
              .join(" ")
              .toLocaleLowerCase();
            return terms.every((term) => haystack.includes(term));
          });
    return matched.slice(0, 60);
  }, [commandItems, commandQuery]);
  const groupedCommandItems = useMemo(() => {
    const groups: Array<{
      group: CommandPaletteItem["group"];
      items: CommandPaletteItem[];
    }> = [];
    for (const item of filteredCommandItems) {
      const existing = groups.find((entry) => entry.group === item.group);
      if (existing) {
        existing.items.push(item);
      } else {
        groups.push({ group: item.group, items: [item] });
      }
    }
    return groups;
  }, [filteredCommandItems]);
  const activeCommandItem = filteredCommandItems[activeCommandIndex] ?? null;
  const activeCommandOptionId = activeCommandItem
    ? commandPaletteOptionId(activeCommandIndex)
    : undefined;
  const openCommandPalette = () => {
    if (commandPaletteOpen) {
      commandInputRef.current?.focus({ preventScroll: true });
      return;
    }
    commandReturnFocusRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : commandButtonRef.current;
    setActiveCommandIndex(0);
    setCommandPaletteOpen(true);
  };
  const closeCommandPalette = (restoreFocus = true) => {
    setCommandPaletteOpen(false);
    setCommandQuery("");
    setActiveCommandIndex(0);
    if (restoreFocus) {
      window.requestAnimationFrame(() => {
        (commandReturnFocusRef.current ?? commandButtonRef.current)?.focus({
          preventScroll: true,
        });
      });
    }
  };
  const selectCommandItem = (item: CommandPaletteItem) => {
    closeCommandPalette(false);
    item.onSelect();
    window.requestAnimationFrame(() =>
      contentRef.current?.focus({ preventScroll: true }),
    );
  };
  const skipToPageContent = (event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault();
    contentRef.current?.focus({ preventScroll: true });
    contentRef.current?.scrollTo({ top: 0, behavior: "auto" });
  };
  const openFleetScopeEditor = () => {
    const search = document.getElementById("fleet-search");
    if (search instanceof HTMLInputElement) {
      search.focus();
      search.select();
      return;
    }
    if (search instanceof HTMLElement) {
      search.focus();
      if (search.isContentEditable) {
        const selection = window.getSelection();
        const range = document.createRange();
        range.selectNodeContents(search);
        selection?.removeAllRanges();
        selection?.addRange(range);
      }
    }
  };
  const renderSavedFleetViewControls = (className = "savedViewControls") => (
    <div className={className} aria-label="Saved fleet views">
      <select
        aria-label="Saved fleet view"
        name="saved_fleet_view"
        onChange={(event) => onApplySavedFleetView(event.target.value)}
        value={activeSavedFleetViewId ?? ""}
      >
        <option value="" title="Saved views">Saved views</option>
        {savedFleetViews.map((view) => (
          <option key={view.id} value={view.id} title={view.name}>
            {view.name}
          </option>
        ))}
      </select>
      <input
        aria-label="Saved fleet view name"
        name="saved_fleet_view_name"
        onChange={(event) => onSavedFleetViewNameChange(event.target.value)}
        placeholder="View name"
        title={draftSavedFleetViewName || "View name"}
        value={draftSavedFleetViewName}
      />
      <button
        aria-label={savedFleetViewActionLabel}
        className="iconButton"
        disabled={!fleetQuery.trim() && !draftSavedFleetViewName.trim()}
        onClick={onSaveFleetView}
        title={savedFleetViewActionLabel}
        type="button"
      >
        {draftSavedFleetViewMatchesExisting ? (
          <Save size={18} />
        ) : (
          <BookmarkPlus size={18} />
        )}
      </button>
      <button
        aria-label="Delete saved fleet view"
        className="iconButton"
        disabled={activeSavedFleetViewId === null}
        onClick={onDeleteSavedFleetView}
        title="Delete saved fleet view"
        type="button"
      >
        <Trash2 size={18} />
      </button>
      <button
        aria-label="Clear fleet view"
        className="iconButton"
        disabled={!hasFleetScope}
        onClick={onClearFleetView}
        title="Clear fleet view"
        type="button"
      >
        <X size={18} />
      </button>
    </div>
  );

  useEffect(() => {
    if (!operatorPreferencesReady) {
      return;
    }
    const defaultMode = preferences.sidebar_subpanel_default;
    if (storedDefaultRef.current && storedDefaultRef.current !== defaultMode) {
      storedDefaultRef.current = defaultMode;
      setManualSubpanelState({});
      writeSidebarSubpanelPreferences(defaultMode, {});
      return;
    }
    storedDefaultRef.current = defaultMode;
    writeSidebarSubpanelPreferences(defaultMode, manualSubpanelState);
  }, [manualSubpanelState, operatorPreferencesReady, preferences.sidebar_subpanel_default]);

  useEffect(() => {
    if ((viewSubpages[activeView] ?? []).length <= 1) {
      return;
    }
    setManualSubpanelState((current) => {
      if (current[activeView] === true) {
        return current;
      }
      const next = { ...current, [activeView]: true };
      writeSidebarSubpanelPreferences(preferences.sidebar_subpanel_default, next);
      return next;
    });
  }, [activeView, preferences.sidebar_subpanel_default]);

  useEffect(() => {
    const content = contentRef.current;
    const topbar = topbarRef.current;
    if (!content || !topbar) {
      return;
    }
    const root = document.documentElement;
    const previousRootScrollOffset = root.style.getPropertyValue(
      "--console-sticky-offset",
    );
    const updateScrollOffset = () => {
      const height = Math.ceil(topbar.getBoundingClientRect().height);
      const offset = getComputedStyle(topbar).position === "sticky"
        ? `${height + 16}px`
        : "16px";
      content.style.setProperty("--console-sticky-offset", offset);
      root.style.setProperty("--console-sticky-offset", offset);
    };
    updateScrollOffset();
    const observer = new ResizeObserver(updateScrollOffset);
    observer.observe(topbar);
    window.addEventListener("resize", updateScrollOffset);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", updateScrollOffset);
      if (previousRootScrollOffset) {
        root.style.setProperty(
          "--console-sticky-offset",
          previousRootScrollOffset,
        );
      } else {
        root.style.removeProperty("--console-sticky-offset");
      }
    };
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLocaleLowerCase() === "k") {
        event.preventDefault();
        openCommandPalette();
        return;
      }
      if (event.key === "Escape" && commandPaletteOpen) {
        event.preventDefault();
        closeCommandPalette();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [commandPaletteOpen]);

  useEffect(() => {
    if (!commandPaletteOpen) {
      return;
    }
    window.setTimeout(() => commandInputRef.current?.focus(), 0);
  }, [commandPaletteOpen]);

  useEffect(() => {
    setActiveCommandIndex((current) =>
      filteredCommandItems.length === 0
        ? 0
        : Math.min(current, filteredCommandItems.length - 1),
    );
  }, [filteredCommandItems.length]);

  useEffect(() => {
    if (!commandPaletteOpen || !activeCommandOptionId) {
      return;
    }
    document
      .getElementById(activeCommandOptionId)
      ?.scrollIntoView({ block: "nearest" });
  }, [activeCommandOptionId, commandPaletteOpen]);

  useLayoutEffect(() => {
    if (!commandPaletteOpen || !commandBackdropRef.current) {
      return undefined;
    }
    const backdrop = commandBackdropRef.current;
    const siblings = Array.from(backdrop.parentElement?.children ?? [])
      .filter(
        (element): element is HTMLElement =>
          element instanceof HTMLElement && element !== backdrop,
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
    const containFocus = (event: FocusEvent) => {
      if (
        event.target instanceof Node &&
        !backdrop.contains(event.target)
      ) {
        commandInputRef.current?.focus({ preventScroll: true });
      }
    };
    const trapTab = (event: KeyboardEvent) => {
      if (event.key !== "Tab") {
        return;
      }
      const focusable = commandPaletteFocusableElements(backdrop);
      if (focusable.length === 0) {
        event.preventDefault();
        commandInputRef.current?.focus({ preventScroll: true });
        return;
      }
      const first = focusable[0]!;
      const last = focusable[focusable.length - 1]!;
      const active = document.activeElement;
      if (
        (event.shiftKey && (active === first || !backdrop.contains(active))) ||
        (!event.shiftKey && (active === last || !backdrop.contains(active)))
      ) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus({ preventScroll: true });
      }
    };
    document.addEventListener("focusin", containFocus);
    document.addEventListener("keydown", trapTab, true);
    return () => {
      document.removeEventListener("focusin", containFocus);
      document.removeEventListener("keydown", trapTab, true);
      for (const sibling of siblings) {
        sibling.element.inert = sibling.inert;
        if (sibling.ariaHidden === null) {
          sibling.element.removeAttribute("aria-hidden");
        } else {
          sibling.element.setAttribute("aria-hidden", sibling.ariaHidden);
        }
      }
    };
  }, [commandPaletteOpen]);

  useEffect(() => {
    if (contentRef.current) {
      contentRef.current.scrollTop = 0;
    }
    window.scrollTo({ left: 0, top: 0, behavior: "auto" });
  }, [activeSubpage, activeView]);

  return (
    <div className="shell">
      <a
        className="skipLink"
        href="#console-main-content"
        onClick={skipToPageContent}
      >
        Skip to page content
      </a>
      <aside className="sidebar">
        <div className="brand">
          <Cloud size={24} />
          <span>vpsman</span>
        </div>
        <nav aria-label="Primary console navigation">
          {navSections.map((section) => (
            <div className="navSection" key={section.label}>
              <span className="navSectionTitle">{section.label}</span>
              {section.items.map((item) => {
                const Icon = item.icon;
                const label = viewLabel(item.view);
                const subpages = viewSubpages[item.view] ?? [];
                const hasSubpages = subpages.length > 1;
                const expanded = isSubpanelExpanded(item.view, hasSubpages);
                return (
                  <div className="navGroup" key={item.view}>
                    <div className={activeView === item.view ? "navItemRow active" : "navItemRow"}>
                      <button
                        aria-current={activeView === item.view ? "page" : undefined}
                        className={activeView === item.view ? "navItem active" : "navItem"}
                        onClick={() => selectPrimaryNavItem(item.view, hasSubpages, expanded)}
                        type="button"
                      >
                        <Icon size={18} />
                        <span>{label}</span>
                      </button>
                      {hasSubpages && (
                        <button
                          aria-expanded={expanded}
                          aria-label={expanded ? "Collapse subpages" : "Expand subpages"}
                          className="subnavToggle"
                          onClick={() => toggleSubpanel(item.view, expanded)}
                          title={`${expanded ? "Collapse" : "Expand"} ${label} sections`}
                          type="button"
                        >
                          {expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
                        </button>
                      )}
                    </div>
                    {expanded && (
                      <div className="subnav" aria-label={`${label} sections`}>
                        {subpages.map((subpage) => {
                          const active =
                            activeView === item.view &&
                            activeSubpageBase === subpage.id;
                          return (
                            <button
                              aria-current={active ? "page" : undefined}
                              className={
                                active ? "subnavItem active" : "subnavItem"
                              }
                              key={subpage.id}
                              onClick={() =>
                                onSelectView(item.view, subpage.id)
                              }
                              title={subpage.description}
                              type="button"
                            >
                              {subpage.label}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </nav>
      </aside>

      <main className="content" id="console-main-content" ref={contentRef} tabIndex={-1}>
        <header className="topbar" ref={topbarRef}>
          <div className="scopeSelectorGroup">
            <button
              aria-label={`Edit fleet scope: ${scopeName}. ${filteredAgentCount} of ${summary.total} resources shown`}
              className="scopeSelector"
              onClick={openFleetScopeEditor}
              title="Edit fleet scope in search"
              type="button"
            >
              <FolderKanban size={18} />
              <span className="scopeMeta">
                <strong>{scopeName}</strong>
                <small>
                  {filteredAgentCount} / {summary.total} resources
                </small>
              </span>
            </button>
            <button
              aria-label="Clear fleet scope"
              className="iconButton scopeClearButton"
              disabled={!hasFleetScope}
              onClick={onClearFleetView}
              title="Clear fleet scope"
              type="button"
            >
              <X size={16} />
            </button>
          </div>
          <SearchExpressionInput
            agents={agents}
            ariaLabel="Search fleet"
            className="search"
            inputId="fleet-search"
            onChange={onFleetQueryChange}
            placeholder="Search VPS, tag, provider, job"
            showMatchCount
            value={fleetQuery}
          />
          <div className="topbarActions">
            <label className="mobilePageMenu" title="Choose console page">
              <FolderKanban aria-hidden="true" size={17} />
              <span className="visuallyHidden">Console page</span>
              <select
                aria-label="Console page"
                className="mobilePageSelector"
                name="console_page"
                onChange={(event) => selectMobilePage(event.target.value)}
                title={`${activeViewLabel} / ${activeSubpageLabel}`}
                value={mobilePageValue}
              >
                {navSections.map((section) => (
                  <optgroup key={section.label} label={section.label}>
                    {section.items.flatMap((item) =>
                      (viewSubpages[item.view] ?? []).map((subpage) => (
                        <option key={`${item.view}:${subpage.id}`} value={`${item.view}::${subpage.id}`}>
                          {viewLabel(item.view)} / {subpage.label}
                        </option>
                      )),
                    )}
                  </optgroup>
                ))}
              </select>
            </label>
            {renderSavedFleetViewControls()}
            <details className="mobileSavedViewMenu">
              <summary aria-label="Open saved fleet views menu">
                <BookmarkPlus size={17} />
                <span>Views</span>
              </summary>
              {renderSavedFleetViewControls(
                "savedViewControls mobileSavedViewControls",
              )}
            </details>
            <span
              aria-label={`Control plane status: ${controlPlaneLabel}`}
              className={`controlPlanePill ${controlPlaneTone}`}
              title={controlPlaneDetail}
            >
              <RadioTower size={17} />
              <span>{controlPlaneLabel}</span>
            </span>
            <button
              className="iconButton"
              aria-label="Open command palette"
              onClick={openCommandPalette}
              ref={commandButtonRef}
              title="Open command palette"
              type="button"
            >
              <Command size={19} />
            </button>
            {apiToken && (
              <button
                aria-label="Clear operator session"
                className="sessionButton"
                onClick={onClearSession}
                title="Sign out of the current operator session"
                type="button"
              >
                <KeyRound size={18} />
                <span>Session</span>
              </button>
            )}
            {privilegeUnlocked ? (
              <button
                aria-label="Lock privilege"
                className="secondaryAction"
                onClick={onLockPrivilege}
                title="Lock privilege unlock in browser memory"
                type="button"
              >
                <LockKeyhole size={18} />
                <span>Lock</span>
              </button>
            ) : (
              <button
                aria-label="Open privilege unlock"
                className="primaryAction"
                onClick={onOpenAccessControls}
                type="button"
              >
                <ShieldCheck size={18} />
                <span>Unlock</span>
              </button>
            )}
          </div>
        </header>

        {controlPlaneTone !== "ok" && (
          <div
            aria-live="polite"
            className={`controlPlaneNotice ${controlPlaneTone}`}
            role="status"
          >
            <RadioTower aria-hidden="true" size={17} />
            <strong>{controlPlaneLabel}</strong>
            <span>{controlPlaneDetail}</span>
          </div>
        )}

        <section
          className={`consoleHeader${hideFleetStatusSummary ? " withoutFleetStatus" : ""}`}
        >
          <div className="titleBlock">
            <span className="breadcrumb" title={`vpsman / ${activeViewLabel} / ${activeSubpageLabel}`}>
              vpsman / {activeViewLabel} / {activeSubpageLabel}
            </span>
            <h1>{pageTitle}</h1>
            <p className="pageDescription" title={pageDescription || activeSubpageDescription}>{pageDescription || activeSubpageDescription}</p>
            <div className="pageHeaderContext" aria-label="Page operational context">
              <span>
                <strong>Scope</strong>
                {scopeName}
              </span>
              <span>
                <strong>Resources</strong>
                {filteredAgentCount} / {agents.length}
              </span>
              <span>
                <strong>Section</strong>
                {activeSubpageLabel}
              </span>
            </div>
          </div>
          {hideFleetStatusSummary ? null : showFullFleetMetrics ? (
            <div className="quickStats" aria-label="Fleet status summary">
              <span className="summaryScopeLabel">{summaryScopeLabel}</span>
              <Metric label="Online" value={String(summary.online)} tone="green" />
              <Metric
                label="Offline"
                value={String(summary.offline)}
                tone={summary.offline > 0 ? "yellow" : "neutral"}
              />
              <Metric
                label="Stale"
                value={String(summary.stale)}
                tone={summary.stale > 0 ? "yellow" : "neutral"}
              />
              <Metric
                label="No contact"
                title="Never-connected VPSs plus records whose reported online state lacks trustworthy gateway contact evidence"
                value={String(summary.never + summary.unknown)}
                tone={summary.never + summary.unknown > 0 ? "yellow" : "neutral"}
              />
              <Metric
                label="Unavailable"
                title="Offline, stale, never-connected, and contact-unknown VPSs"
                value={String(unavailableCount)}
                tone={unavailableCount > 0 ? "yellow" : "neutral"}
              />
              <Metric
                label="Alerts"
                title={`${alertCounts.critical} critical, ${alertCounts.warning} warning, ${alertCounts.info} info${alertCounts.truncated ? " in the loaded alert page; additional matching alerts may exist" : ""}`}
                value={
                  alertCounts.truncated && alertCounts.total === 0
                    ? "0 loaded"
                    : formatLowerBoundCount(
                        alertCounts.total,
                        alertCounts.truncated,
                      )
                }
                tone={
                  alertCounts.total > 0 || alertCounts.truncated
                    ? "yellow"
                    : "green"
                }
              />
              <Metric
                label="Running jobs"
                title="Jobs that have not reached a terminal state"
                value={String(summary.running_jobs)}
                tone="blue"
              />
            </div>
          ) : (
            <div className="fleetStatusStrip" aria-label="Fleet status summary">
              <strong className="fleetStatusFull" title={fleetStatusTitle}>
                {fleetStatusText}
              </strong>
              <strong className="fleetStatusCompact" title={fleetStatusTitle}>
                {summary.total} VPS · {summary.online} online · {unavailableCount} unavailable · {summary.running_jobs} running
              </strong>
              <span className={alertCounts.critical > 0 || alertCounts.warning > 0 || alertCounts.truncated ? "warn" : "ok"}>
                {alertCounts.total > 0
                  ? `${formatLowerBoundCount(alertCounts.total, alertCounts.truncated)} open · ${alertCounts.critical} critical · ${alertCounts.warning} warning · ${alertCounts.info} info${alertCounts.truncated ? " in loaded alerts" : ""}`
                  : alertCounts.truncated
                    ? "No matching alerts in the loaded page · more may exist"
                    : "No active alerts"}
              </span>
              <small>{onlineRatio} online</small>
            </div>
          )}
        </section>

        {children}
      </main>
      {commandPaletteOpen && (
        <div
          aria-labelledby="command-palette-title"
          aria-modal="true"
          className="commandPaletteBackdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) {
              closeCommandPalette();
            }
          }}
          ref={commandBackdropRef}
          role="dialog"
        >
          <div className="commandPalette">
            <div className="commandPaletteHeader">
              <Command size={18} />
              <input
                aria-activedescendant={activeCommandOptionId}
                aria-autocomplete="list"
                aria-controls="command-palette-results"
                aria-expanded="true"
                aria-label="Command palette search"
                autoComplete="off"
                name="command_palette_search"
                onChange={(event) => {
                  setCommandQuery(event.target.value);
                  setActiveCommandIndex(0);
                }}
                onKeyDown={(event) => {
                  if (filteredCommandItems.length === 0) {
                    return;
                  }
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    setActiveCommandIndex(
                      (current) =>
                        (current + 1) % filteredCommandItems.length,
                    );
                  } else if (event.key === "ArrowUp") {
                    event.preventDefault();
                    setActiveCommandIndex(
                      (current) =>
                        (current - 1 + filteredCommandItems.length) %
                        filteredCommandItems.length,
                    );
                  } else if (event.key === "Home") {
                    event.preventDefault();
                    setActiveCommandIndex(0);
                  } else if (event.key === "End") {
                    event.preventDefault();
                    setActiveCommandIndex(filteredCommandItems.length - 1);
                  } else if (event.key === "Enter" && activeCommandItem) {
                    event.preventDefault();
                    selectCommandItem(activeCommandItem);
                  }
                }}
                placeholder="Search pages, VPS, jobs, sessions, transfers, backups, audit"
                ref={commandInputRef}
                role="combobox"
                type="search"
                value={commandQuery}
              />
              <kbd>Esc</kbd>
            </div>
            <div className="commandPaletteMeta">
              <h2 id="command-palette-title">Command palette</h2>
              <span aria-live="polite">
                {filteredCommandItems.length} results
              </span>
            </div>
            <div
              className="commandPaletteResults"
              id="command-palette-results"
              role="listbox"
              aria-label="Command palette results"
            >
              {groupedCommandItems.length > 0 ? (
                groupedCommandItems.map((group, groupIndex) => (
                  <section
                    aria-labelledby={`command-palette-group-${groupIndex}`}
                    className="commandPaletteGroup"
                    key={group.group}
                    role="group"
                  >
                    <span
                      className="commandPaletteGroupTitle"
                      id={`command-palette-group-${groupIndex}`}
                    >
                      {group.group}
                    </span>
                    {group.items.map((item) => {
                      const itemIndex = filteredCommandItems.indexOf(item);
                      return (
                        <button
                          aria-label={`${item.group}: ${item.label}. ${item.detail}`}
                          aria-selected={activeCommandIndex === itemIndex}
                          className="commandPaletteResult"
                          data-command-group={item.group}
                          id={commandPaletteOptionId(itemIndex)}
                          key={item.id}
                          onClick={() => selectCommandItem(item)}
                          onMouseMove={() => setActiveCommandIndex(itemIndex)}
                          role="option"
                          tabIndex={-1}
                          title={`${item.label}\n${item.detail}`}
                          type="button"
                        >
                          <span className="commandPaletteResultText">
                            <strong title={item.label}>{item.label}</strong>
                            <small title={item.detail}>{item.detail}</small>
                          </span>
                          <span className="commandPaletteGroupBadge">{item.group}</span>
                        </button>
                      );
                    })}
                  </section>
                ))
              ) : (
                <div className="commandPaletteEmpty" role="status">
                  No matching commands or entities
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

type SidebarSubpanelPreferences = {
  defaultMode: string | null;
  state: Record<string, boolean>;
};

function readSidebarSubpanelPreferences(): SidebarSubpanelPreferences {
  try {
    const raw = window.localStorage.getItem(SIDEBAR_SUBPANEL_STORAGE_KEY);
    if (!raw) {
      return { defaultMode: null, state: {} };
    }
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return { defaultMode: null, state: {} };
    }
    if ("state" in parsed) {
      const record = parsed as { defaultMode?: unknown; state?: unknown };
      return {
        defaultMode: typeof record.defaultMode === "string" ? record.defaultMode : null,
        state: sanitizeSidebarSubpanelState(record.state),
      };
    }
    return {
      defaultMode: null,
      state: sanitizeSidebarSubpanelState(parsed),
    };
  } catch {
    return { defaultMode: null, state: {} };
  }
}

function sanitizeSidebarSubpanelState(value: unknown): Record<string, boolean> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return {};
  }
  return Object.fromEntries(
    Object.entries(value).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"),
  );
}

function writeSidebarSubpanelPreferences(defaultMode: string, state: Record<string, boolean>) {
  try {
    window.localStorage.setItem(
      SIDEBAR_SUBPANEL_STORAGE_KEY,
      JSON.stringify({
        defaultMode,
        state,
      }),
    );
  } catch {
    // Local navigation chrome state is non-critical.
  }
}

function commandPaletteOptionId(index: number) {
  return `command-palette-option-${index}`;
}

function commandPaletteFocusableElements(container: HTMLElement) {
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      [
        "a[href]",
        "button:not([disabled]):not([tabindex='-1'])",
        "input:not([disabled]):not([tabindex='-1'])",
        "select:not([disabled]):not([tabindex='-1'])",
        "textarea:not([disabled]):not([tabindex='-1'])",
        "[tabindex]:not([tabindex='-1'])",
      ].join(","),
    ),
  ).filter(
    (element) =>
      element.getAttribute("aria-hidden") !== "true" &&
      getComputedStyle(element).display !== "none" &&
      getComputedStyle(element).visibility !== "hidden",
  );
}
