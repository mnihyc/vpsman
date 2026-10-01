import { quoteSelectorValue, tokenizeSearchExpression } from "./searchExpression";
import type { SearchCompletion } from "./components/SearchExpressionInput";
import type { JobHistoryRecord } from "./types";

export type JobSearchField = {
  name: string;
  aliases: string[];
  kind: string;
  description: string;
  operators: string[];
  examples: string[];
  values: string[];
  scope: "job" | "target";
};

export type JobSearchValue = { value: string; label: string };
export type JobSearchPage = {
  rows: JobHistoryRecord[];
  total: number;
  next_cursor: string | null;
  as_of: string;
};

export const JOB_SEARCH_HELP = 'Search all retained jobs. Combine conditions with &&, ||, ! and parentheses. Examples: type = runtime_config_sync && status = failed; created_at >= now-24h; duration > 30s; target = v-11. Use target_result = "id = v-11 && status = failed" to match one VPS result. Hover a hint for field details.';

const REMOTE_VALUE_FIELDS = new Set(["target", "actor", "actor_id", "schedule", "schedule_id"]);

/** Replaces only the active field/operator/value, preserving adjacent clauses. */
export function jobSearchCompletion(
  value: string,
  caret: number,
  fields: JobSearchField[],
  remoteValues: ReadonlyMap<string, JobSearchValue[]>,
): SearchCompletion {
  caret = Math.max(0, Math.min(value.length, caret));
  const tokens = tokenizeSearchExpression(value.slice(0, caret), true).tokens;
  const comment = tokens.find((token) => token.kind === "comment" && token.start < caret
    && (token.end > caret || (token.end === caret && (token.raw.startsWith("#") || !token.raw.endsWith("*/")))));
  if (comment) return { start: caret, end: caret, options: [] };
  let start = 0;
  tokens.forEach((token, index) => {
    if (["and", "or", "left_paren", "comment"].includes(token.kind)
      || (token.kind === "not" && (index === 0 || token.raw === "!" || token.raw === "~"
        || ["and", "or", "left_paren", "not"].includes(tokens[index - 1].kind)))) start = token.end;
  });
  start += value.slice(start, caret).match(/^\s*/)?.[0].length ?? 0;
  const fragment = value.slice(start, caret);
  const word = fragment.match(/^([a-z_][\w.]*)/i)?.[0] ?? "";
  const jobFields = fields.filter((field) => field.scope === "job");
  const name = word.replace(/^job\./i, "").toLowerCase();
  const field = jobFields.find((field) => field.name === name || field.aliases.includes(name));
  const tail = fragment.slice(word.length);
  const fullTokens = tokenizeSearchExpression(value, true).tokens;
  const currentToken = fullTokens.find((token) => token.start < caret && token.end >= caret
    && (token.kind === "string" || token.kind === "term" || token.kind === "regex"));
  const end = currentToken?.end ?? caret;
  if (!field || !tail) {
    return {
      start, end,
      options: jobFields.filter((field) => !word || [field.name, ...field.aliases].some((candidate) => candidate.startsWith(name)))
        .map((field) => ({
          value: field.name + " ", label: field.name, detail: field.kind,
          description: field.description, continueCompletion: true,
        })),
    };
  }
  const rest = tail.trimStart();
  const operator = rest.match(/^(not\s+in\b|in\b|!=|<=|>=|[=:<>])\s*/i);
  if (!operator) {
    return {
      start, end: caret,
      options: field.operators.filter((op) => op.startsWith(rest.toLowerCase()))
        .map((op) => ({
          value: word + " " + op + (op.endsWith("in") ? " [" : " "),
          label: op, detail: field.name, description: field.description, continueCompletion: true,
        })),
    };
  }
  let valueStart = start + word.length + tail.length - rest.length + operator[0].length;
  const membership = operator[1].toLowerCase().endsWith("in");
  if (membership) {
    const listTokens = tokenizeSearchExpression(value.slice(valueStart, caret), true).tokens;
    const boundaries = listTokens.filter((token) => token.kind === "comma" || token.kind === "left_bracket");
    const boundary = boundaries[boundaries.length - 1];
    if (boundary) valueStart += boundary.end;
  }
  valueStart += value.slice(valueStart, caret).match(/^\s*/)?.[0].length ?? 0;
  const raw = value.slice(valueStart, caret);
  const prefix = raw.replace(/^["']/, "").replace(/["']$/, "").toLowerCase();
  const lookup = REMOTE_VALUE_FIELDS.has(field.name) ? { field: field.name, prefix } : undefined;
  const known = lookup ? remoteValues.get(lookupKey(lookup)) ?? [] : [];
  const candidates = [
    ...known,
    ...field.values.map((item) => ({ value: item, label: item })),
    ...field.examples.map((item) => ({ value: item, label: item })),
  ];
  if (field.kind === "expression") {
    for (const target of fields.filter((item) => item.scope === "target")) {
      for (const sample of target.examples.slice(0, 1)) {
        candidates.push({ value: target.name + " = " + quoteSelectorValue(sample), label: target.name + " = " + quoteSelectorValue(sample) });
      }
    }
  }
  const seen = new Set<string>();
  // A bracket in a later clause does not close the list being edited.
  const followingBoundary = fullTokens.find((token) => token.start >= end
    && ["right_bracket", "and", "or", "right_paren"].includes(token.kind));
  return {
    start: valueStart, end, lookup,
    options: candidates.filter((item) => {
      if (seen.has(item.value)) return false;
      seen.add(item.value);
      return !prefix || item.value.toLowerCase().includes(prefix) || item.label.toLowerCase().includes(prefix);
    }).map((item) => ({
      value: (field.kind === "expression" ? JSON.stringify(item.value) : quoteSelectorValue(item.value))
        + (membership && followingBoundary?.kind !== "right_bracket" ? "]" : ""),
      label: item.label === item.value ? item.value : item.label + " · " + item.value,
      detail: field.name, description: field.description,
    })),
  };
}

export function lookupKey(lookup: NonNullable<SearchCompletion["lookup"]>): string {
  return JSON.stringify([lookup.field, lookup.prefix]);
}
