import { useEffect, useMemo, useState } from "react";
import type { AgentView } from "../types";

const FLEET_VIEW_STORAGE_KEY = "vpsman.fleetViews";

export type SavedFleetView = {
  id: string;
  name: string;
  query: string;
  createdAt: string;
  updatedAt: string;
};

type StoredFleetViewState = {
  activeSavedViewId?: string | null;
  query?: string;
  savedViews?: SavedFleetView[];
};

export function useFleetViews(agents: AgentView[]) {
  const [storedState] = useState(readFleetViewState);
  const [fleetQuery, setFleetQueryState] = useState(storedState.query ?? "");
  const [savedViews, setSavedViews] = useState<SavedFleetView[]>(storedState.savedViews ?? []);
  const [activeSavedViewId, setActiveSavedViewId] = useState<string | null>(storedState.activeSavedViewId ?? null);
  const [draftSavedViewName, setDraftSavedViewName] = useState("");
  const activeSavedView = savedViews.find((view) => view.id === activeSavedViewId) ?? null;
  const filteredAgents = useMemo(() => filterAgents(agents, fleetQuery), [agents, fleetQuery]);

  useEffect(() => {
    writeFleetViewState({ activeSavedViewId, query: fleetQuery, savedViews });
  }, [activeSavedViewId, fleetQuery, savedViews]);

  function setFleetQuery(query: string) {
    setFleetQueryState(query);
    if (activeSavedView && query !== activeSavedView.query) {
      setActiveSavedViewId(null);
    }
  }

  function saveFleetView() {
    const now = new Date().toISOString();
    const query = fleetQuery.trim();
    const name = draftSavedViewName.trim() || defaultFleetViewName(query, savedViews.length);
    if (activeSavedView) {
      setSavedViews((views) =>
        views.map((view) => (view.id === activeSavedView.id ? { ...view, name, query, updatedAt: now } : view)),
      );
      setDraftSavedViewName(name);
      return;
    }
    const view: SavedFleetView = {
      id: `view-${Date.now().toString(36)}`,
      name,
      query,
      createdAt: now,
      updatedAt: now,
    };
    setSavedViews((views) => [...views, view].sort((left, right) => left.name.localeCompare(right.name)));
    setActiveSavedViewId(view.id);
    setDraftSavedViewName(name);
  }

  function applySavedFleetView(viewId: string) {
    const view = savedViews.find((candidate) => candidate.id === viewId);
    if (!view) {
      setActiveSavedViewId(null);
      return;
    }
    setFleetQueryState(view.query);
    setActiveSavedViewId(view.id);
    setDraftSavedViewName(view.name);
  }

  function deleteSavedFleetView() {
    if (!activeSavedView) {
      return;
    }
    setSavedViews((views) => views.filter((view) => view.id !== activeSavedView.id));
    setActiveSavedViewId(null);
    setDraftSavedViewName("");
  }

  function clearFleetView() {
    setFleetQueryState("");
    setActiveSavedViewId(null);
    setDraftSavedViewName("");
  }

  return {
    activeSavedView,
    activeSavedViewId,
    clearFleetView,
    deleteSavedFleetView,
    draftSavedViewName,
    filteredAgents,
    fleetQuery,
    savedViews,
    saveFleetView,
    setDraftSavedViewName,
    setFleetQuery,
    applySavedFleetView,
  };
}

function filterAgents(agents: AgentView[], query: string): AgentView[] {
  const terms = query
    .trim()
    .toLowerCase()
    .split(/[\s,]+/)
    .filter(Boolean);
  if (terms.length === 0) {
    return agents;
  }
  return agents.filter((agent) => {
    return terms.every((term) => agentMatchesTerm(agent, term));
  });
}

function agentMatchesTerm(agent: AgentView, term: string): boolean {
  const [kind, value] = splitFilterTerm(term);
  if (kind === "tag") {
    return matchesAny(agent.tags, value);
  }
  if (kind === "provider") {
    return matchesAny(agent.tags, `provider:${value}`);
  }
  if (kind === "country" || kind === "region") {
    return matchesAny(agent.tags, `country:${value}`);
  }
  if (kind === "status") {
    return agent.status.toLowerCase().includes(value);
  }
  const haystack = [
    agent.id,
    agent.display_name,
    agent.status,
    agent.capabilities.privilege_mode,
    ...agent.tags,
  ]
    .join(" ")
    .toLowerCase();
  return haystack.includes(term);
}

function splitFilterTerm(term: string): [string | null, string] {
  const separator = term.indexOf(":");
  if (separator <= 0 || separator === term.length - 1) {
    return [null, term];
  }
  return [term.slice(0, separator), term.slice(separator + 1)];
}

function matchesAny(values: string[], term: string): boolean {
  return values.some((value) => value.toLowerCase().includes(term));
}

function defaultFleetViewName(query: string, existingCount: number): string {
  if (query) {
    return query.length > 36 ? `${query.slice(0, 33)}...` : query;
  }
  return `Fleet view ${existingCount + 1}`;
}

function readFleetViewState(): StoredFleetViewState {
  if (typeof window === "undefined") {
    return {};
  }
  try {
    const raw = window.localStorage.getItem(FLEET_VIEW_STORAGE_KEY);
    if (!raw) {
      return {};
    }
    const parsed = JSON.parse(raw) as StoredFleetViewState;
    if (typeof parsed !== "object" || parsed === null) {
      return {};
    }
    const savedViews = Array.isArray(parsed.savedViews)
      ? parsed.savedViews.filter(isSavedFleetView).sort((left, right) => left.name.localeCompare(right.name))
      : [];
    const activeSavedViewId =
      typeof parsed.activeSavedViewId === "string" && savedViews.some((view) => view.id === parsed.activeSavedViewId)
        ? parsed.activeSavedViewId
        : null;
    return {
      activeSavedViewId,
      query: typeof parsed.query === "string" ? parsed.query : "",
      savedViews,
    };
  } catch {
    return {};
  }
}

function writeFleetViewState(state: StoredFleetViewState) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.localStorage.setItem(FLEET_VIEW_STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Best-effort frequent-use preference only; local storage failures must not block the console.
  }
}

function isSavedFleetView(value: unknown): value is SavedFleetView {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Partial<SavedFleetView>;
  return (
    typeof candidate.id === "string" &&
    typeof candidate.name === "string" &&
    typeof candidate.query === "string" &&
    typeof candidate.createdAt === "string" &&
    typeof candidate.updatedAt === "string"
  );
}
