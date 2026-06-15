import {
  filterBySearchExpression,
  parseSearchExpression,
  type SearchFields,
} from "../searchExpression";

const MAX_SEARCH_VALUE_SUGGESTIONS = 80;
const MAX_SUGGESTION_LENGTH = 80;

type SearchValue = string | number | boolean | null | undefined;

export function buildSearchValueSuggestions<T>(
  items: T[],
  valuesForItem: (item: T) => SearchValue[],
  limit = MAX_SEARCH_VALUE_SUGGESTIONS,
): string[] {
  const suggestions = new Set<string>();
  for (const item of items) {
    for (const value of valuesForItem(item)) {
      collectSearchValueSuggestion(suggestions, value);
      if (suggestions.size >= limit) {
        return Array.from(suggestions).sort((left, right) => left.localeCompare(right));
      }
    }
  }
  return Array.from(suggestions).sort((left, right) => left.localeCompare(right));
}

export function buildParseableSearchValueSuggestions<T>(
  items: T[],
  valuesForItem: (item: T) => SearchValue[],
  fieldsForItem: (item: T) => SearchFields,
  limit = MAX_SEARCH_VALUE_SUGGESTIONS,
): string[] {
  const candidates = buildSearchValueSuggestions(items, valuesForItem, limit * 3);
  const matching: string[] = [];
  const nonMatching: string[] = [];
  for (const candidate of candidates) {
    if (!isParseableSearchSuggestion(candidate)) {
      continue;
    }
    const result = filterBySearchExpression(items, candidate, fieldsForItem);
    if (result.error) {
      continue;
    }
    if (result.items.length > 0) {
      matching.push(candidate);
    } else {
      nonMatching.push(candidate);
    }
    if (matching.length + nonMatching.length >= limit) {
      break;
    }
  }
  return matching.concat(nonMatching).slice(0, limit);
}

export function isParseableSearchSuggestion(value: string): boolean {
  return parseSearchExpression(value).error === null;
}

export function searchFieldsForSearchValues(values: SearchValue[]): SearchFields {
  const all: string[] = [];
  const namespaces: Record<string, string[]> = {};
  const events: string[] = [];
  for (const value of values) {
    if (value === null || value === undefined) {
      continue;
    }
    const text = String(value).replace(/\s+/g, " ").trim();
    if (!text) {
      continue;
    }
    all.push(text);
    for (const term of text.split(/[,\s]+/)) {
      collectNamespacedTerm(namespaces, term);
      collectEventTerm(events, term);
    }
  }
  return {
    all,
    events: events.length > 0 ? Array.from(new Set(events)) : undefined,
    namespaces: Object.keys(namespaces).length > 0 ? namespaces : undefined,
  };
}

function collectSearchValueSuggestion(
  suggestions: Set<string>,
  value: SearchValue,
) {
  if (value === null || value === undefined) {
    return;
  }
  const text = String(value).replace(/\s+/g, " ").trim();
  addSearchValueSuggestion(suggestions, text);
  for (const part of text.split(/[,\s]+/)) {
    addSearchValueSuggestion(suggestions, part);
  }
}

function addSearchValueSuggestion(suggestions: Set<string>, value: string) {
  const trimmed = value.trim();
  if (trimmed.length < 2 || trimmed.length > MAX_SUGGESTION_LENGTH) {
    return;
  }
  if (/^[\W_]+$/.test(trimmed)) {
    return;
  }
  suggestions.add(trimmed);
}

function collectNamespacedTerm(namespaces: Record<string, string[]>, rawTerm: string) {
  const term = rawTerm.replace(/^[([{]+|[)\]},.;]+$/g, "");
  const separator = term.indexOf(":");
  if (separator <= 0 || separator === term.length - 1) {
    return;
  }
  const namespace = term.slice(0, separator).toLocaleLowerCase();
  const value = term.slice(separator + 1);
  addNamespaceValue(namespaces, namespace, value);
  addNamespaceValue(namespaces, "tag", `${namespace}:${value}`);
  if (namespace === "region") {
    addNamespaceValue(namespaces, "country", value);
    addNamespaceValue(namespaces, "tag", `country:${value}`);
  }
}

function collectEventTerm(events: string[], rawTerm: string) {
  const term = rawTerm.replace(/^[([{]+|[)\]},.;]+$/g, "").toLocaleLowerCase();
  if (
    term.startsWith("interval.") ||
    term.startsWith("vps.status.") ||
    term.startsWith("job.status:") ||
    term.startsWith("job.status.become_") ||
    term.startsWith("job.type:") ||
    term.startsWith("job.target.status:") ||
    term.startsWith("schedule.id:") ||
    term.startsWith("schedule.name:") ||
    term.startsWith("alert.severity:") ||
    term.startsWith("alert.category:") ||
    term.startsWith("alert.state:") ||
    [
      "server.on_start",
      "schedule.due",
      "schedule.dispatched",
      "schedule.failed",
      "vps.tag_changed",
      "job.created",
      "alert.open",
      "telemetry.rollup",
      "telemetry.network_rate",
      "telemetry.tunnel",
    ].includes(term)
  ) {
    events.push(term);
  }
}

function addNamespaceValue(namespaces: Record<string, string[]>, namespace: string, value: string) {
  const values = namespaces[namespace] ?? [];
  if (!values.includes(value)) {
    values.push(value);
  }
  namespaces[namespace] = values;
}
