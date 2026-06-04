import type { ReactNode } from "react";
import {
  BookmarkPlus,
  Cloud,
  Command,
  FolderKanban,
  KeyRound,
  RadioTower,
  Search,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import { Metric } from "./Metric";
import { navSections } from "../constants";
import type { ActiveView, FleetSummary } from "../types";
import type { SavedFleetView } from "../hooks/useFleetViews";

type ConsoleShellProps = {
  activeSavedFleetViewId: string | null;
  activeView: ActiveView;
  apiToken: string;
  children: ReactNode;
  connectedRatio: string;
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
  onSaveFleetView: () => void;
  onSelectView: (view: ActiveView) => void;
  onSavedFleetViewNameChange: (name: string) => void;
  savedFleetViews: SavedFleetView[];
  summary: FleetSummary;
};

export function ConsoleShell({
  activeSavedFleetViewId,
  activeView,
  apiToken,
  children,
  connectedRatio,
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
  onSaveFleetView,
  onSelectView,
  onSavedFleetViewNameChange,
  savedFleetViews,
  summary,
}: ConsoleShellProps) {
  const hasFleetScope = fleetQuery.trim().length > 0 || activeSavedFleetViewId !== null;
  const activeSavedFleetView = savedFleetViews.find((view) => view.id === activeSavedFleetViewId) ?? null;
  const scopeName = activeSavedFleetView?.name ?? (fleetQuery.trim() ? "Filtered resources" : "All VPS resources");

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
                return (
                  <button
                    aria-current={activeView === item.view ? "page" : undefined}
                    className={activeView === item.view ? "navItem active" : "navItem"}
                    key={item.view}
                    onClick={() => onSelectView(item.view)}
                    type="button"
                  >
                    <Icon size={18} />
                    <span>{item.view}</span>
                  </button>
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
              <span>Resource scope</span>
              <strong>{scopeName}</strong>
              <small>
                {filteredAgentCount} / {summary.total} resources
              </small>
            </span>
          </button>
          <div className="search">
            <Search size={18} />
            <input
              aria-label="Search fleet"
              id="fleet-search"
              name="fleet-search"
              onChange={(event) => onFleetQueryChange(event.target.value)}
              placeholder="Search VPS, tag, pool, job"
              value={fleetQuery}
            />
          </div>
          <div className="topbarActions">
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
            <button className="iconButton" aria-label="Open command palette" title="Command palette" type="button">
              <Command size={19} />
            </button>
            {apiToken && (
              <button className="sessionButton" onClick={onClearSession} type="button">
                <KeyRound size={18} />
                <span>Session</span>
              </button>
            )}
            <button className="primaryAction" type="button">
              <ShieldCheck size={18} />
              <span>Unlock</span>
            </button>
          </div>
        </header>

        <section className="consoleHeader">
          <div className="titleBlock">
            <span className="breadcrumb">vpsman / {activeView}</span>
            <h1>{heroTitle}</h1>
            <p>{heroCopy}</p>
          </div>
          <div className="quickStats">
            <Metric label="Connected" value={String(summary.connected)} tone="green" />
            <Metric label="Warnings" value={String(summary.warnings)} tone="yellow" />
            <Metric label="Jobs" value={String(summary.running_jobs)} tone="blue" />
            <Metric label="Online" value={connectedRatio} tone="green" />
          </div>
        </section>

        {children}
      </main>
    </div>
  );
}
