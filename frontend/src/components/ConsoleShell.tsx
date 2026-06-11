import { useEffect, useRef, useState, type ReactNode } from "react";
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
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import { Metric } from "./Metric";
import { SearchExpressionInput } from "./SearchExpressionInput";
import { navSections, subpageDescription, subpageLabel, viewSubpages } from "../constants";
import type { ActiveView, AgentView, FleetSummary } from "../types";
import type { SavedFleetView } from "../hooks/useFleetViews";
import { usePanelDisplaySettings } from "../panelDisplay";

const SIDEBAR_SUBPANEL_STORAGE_KEY = "vpsman.sidebarSubpanels";

type ConsoleShellProps = {
  activeSavedFleetViewId: string | null;
  activeSubpage: string;
  activeView: ActiveView;
  agents: AgentView[];
  apiToken: string;
  children: ReactNode;
  onlineRatio: string;
  draftSavedFleetViewName: string;
  filteredAgentCount: number;
  fleetQuery: string;
  heroCopy: string;
  heroTitle: string;
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
  privilegeUnlocked: boolean;
  savedFleetViews: SavedFleetView[];
  summary: FleetSummary;
};

export function ConsoleShell({
  activeSavedFleetViewId,
  activeSubpage,
  activeView,
  agents,
  apiToken,
  children,
  onlineRatio,
  draftSavedFleetViewName,
  filteredAgentCount,
  fleetQuery,
  heroCopy,
  heroTitle,
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
  privilegeUnlocked,
  savedFleetViews,
  summary,
}: ConsoleShellProps) {
  const { preferences } = usePanelDisplaySettings();
  const initialSubpanelPreferences = useRef(readSidebarSubpanelPreferences());
  const storedDefaultRef = useRef<string | null>(initialSubpanelPreferences.current.defaultMode);
  const [manualSubpanelState, setManualSubpanelState] = useState<Record<string, boolean>>(
    initialSubpanelPreferences.current.state,
  );
  const hasFleetScope = fleetQuery.trim().length > 0 || activeSavedFleetViewId !== null;
  const activeSavedFleetView = savedFleetViews.find((view) => view.id === activeSavedFleetViewId) ?? null;
  const scopeName = activeSavedFleetView?.name ?? (fleetQuery.trim() ? "Filtered resources" : "All VPS resources");
  const activeSubpageLabel = subpageLabel(activeView, activeSubpage);
  const activeSubpageDescription = subpageDescription(activeView, activeSubpage);
  const mobilePageValue = `${activeView}::${activeSubpage}`;
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

  return (
    <div className="shell">
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
                const subpages = viewSubpages[item.view] ?? [];
                const hasSubpages = subpages.length > 1;
                const expanded = isSubpanelExpanded(item.view, hasSubpages);
                return (
                  <div className="navGroup" key={item.view}>
                    <div className={activeView === item.view ? "navItemRow active" : "navItemRow"}>
                      <button
                        aria-current={activeView === item.view ? "page" : undefined}
                        className={activeView === item.view ? "navItem active" : "navItem"}
                        onClick={() => onSelectView(item.view)}
                        type="button"
                      >
                        <Icon size={18} />
                        <span>{item.view}</span>
                      </button>
                      {hasSubpages && (
                        <button
                          aria-expanded={expanded}
                          aria-label={expanded ? "Collapse subpages" : "Expand subpages"}
                          className="subnavToggle"
                          onClick={() => toggleSubpanel(item.view, expanded)}
                          title={`${expanded ? "Collapse" : "Expand"} ${item.view} sections`}
                          type="button"
                        >
                          {expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
                        </button>
                      )}
                    </div>
                    {expanded && (
                      <div className="subnav" aria-label={`${item.view} sections`}>
                        {subpages.map((subpage) => {
                          const active =
                            activeView === item.view &&
                            activeSubpage === subpage.id;
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

      <main className="content">
        <header className="topbar">
          <button className="scopeSelector" onClick={onClearFleetView} title="Clear fleet scope" type="button">
            <FolderKanban size={18} />
            <span className="scopeMeta">
              <strong>{scopeName}</strong>
              <small>
                {filteredAgentCount} / {summary.total} resources
              </small>
            </span>
          </button>
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
            <select
              aria-label="Console page"
              className="mobilePageSelector"
              onChange={(event) => selectMobilePage(event.target.value)}
              value={mobilePageValue}
            >
              {navSections.map((section) => (
                <optgroup key={section.label} label={section.label}>
                  {section.items.flatMap((item) =>
                    (viewSubpages[item.view] ?? []).map((subpage) => (
                      <option key={`${item.view}:${subpage.id}`} value={`${item.view}::${subpage.id}`}>
                        {item.view} / {subpage.label}
                      </option>
                    )),
                  )}
                </optgroup>
              ))}
            </select>
            <div className="savedViewControls" aria-label="Saved fleet views">
              <select
                aria-label="Saved fleet view"
                onChange={(event) => onApplySavedFleetView(event.target.value)}
                value={activeSavedFleetViewId ?? ""}
              >
                <option value="">Saved views</option>
                {savedFleetViews.map((view) => (
                  <option key={view.id} value={view.id}>
                    {view.name}
                  </option>
                ))}
              </select>
              <input
                aria-label="Saved fleet view name"
                onChange={(event) => onSavedFleetViewNameChange(event.target.value)}
                placeholder="View name"
                value={draftSavedFleetViewName}
              />
              <button
                aria-label="Save current fleet view"
                className="iconButton"
                disabled={!fleetQuery.trim() && !draftSavedFleetViewName.trim()}
                onClick={onSaveFleetView}
                title="Save current fleet view"
                type="button"
              >
                <BookmarkPlus size={18} />
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
            <span className="controlPlanePill">
              <RadioTower size={17} />
              <span>Live control plane</span>
            </span>
            <button
              className="iconButton"
              aria-label="Focus fleet search"
              onClick={() => document.getElementById("fleet-search")?.focus()}
              title="Focus fleet search"
              type="button"
            >
              <Command size={19} />
            </button>
            {apiToken && (
              <button className="sessionButton" onClick={onClearSession} type="button">
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

        <section className="consoleHeader">
          <div className="titleBlock">
            <span className="breadcrumb">
              vpsman / {activeView} / {activeSubpageLabel}
            </span>
            <h1>{heroTitle}</h1>
            <p>{heroCopy || activeSubpageDescription}</p>
          </div>
          <div className="quickStats">
            <Metric label="Online" value={String(summary.online)} tone="green" />
            <Metric label="Offline" value={String(summary.offline)} tone="yellow" />
            <Metric label="Stale" value={String(summary.stale)} tone="yellow" />
            <Metric label="Warnings" value={String(summary.warnings)} tone="yellow" />
            <Metric label="Jobs" value={String(summary.running_jobs)} tone="blue" />
            <Metric label="Online %" value={onlineRatio} tone="green" />
          </div>
        </section>

        {children}
      </main>
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
