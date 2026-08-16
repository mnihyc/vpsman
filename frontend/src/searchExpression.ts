import type { AgentView, VpsRuleValueRecord } from "./types";
import {
  isVpsRuleKey,
  tryNormalizeVpsRuleValue,
  vpsRuleDefinition,
  vpsRuleOrderedIntegerValue,
} from "./vpsRules";

type ComparableValue = string | number;
type ComparisonOperator = "=" | "!=" | "<" | "<=" | ">" | ">=";

export type SearchExpression =
  | { type: "predicate"; predicate: SearchPredicate }
  | { type: "not"; expression: SearchExpression }
  | { type: "and"; left: SearchExpression; right: SearchExpression }
  | { type: "or"; left: SearchExpression; right: SearchExpression };

export type SearchPredicate =
  | { type: "bare"; value: string }
  | {
      type: "comparison";
      field: string;
      operator: ComparisonOperator;
      value: string;
    }
  | {
      type: "membership";
      field: string;
      negated: boolean;
      values: SearchListValue[];
    }
  | { type: "event"; value: string }
  | { type: "untagged" };

export type SearchListValue =
  | { type: "literal"; value: string }
  | { type: "regex"; value: string };

export type SearchToken = {
  end: number;
  kind:
    | "and"
    | "comma"
    | "in"
    | "left_bracket"
    | "left_paren"
    | "not"
    | "operator"
    | "or"
    | "regex"
    | "right_bracket"
    | "right_paren"
    | "string"
    | "term";
  namespace: string | null;
  raw: string;
  start: number;
  value: string;
};

export type SearchParseResult =
  | { expression: SearchExpression | null; error: null; tokens: SearchToken[] }
  | { expression: null; error: string; tokens: SearchToken[] };

export type SearchFields = {
  all: string[];
  events?: string[];
  fields?: Record<string, ComparableValue[]>;
  namespaces?: Record<string, string[]>;
  vpsRules?: readonly VpsRuleValueRecord[];
};

export type AgentSearchContext = {
  available: boolean;
  rulesByClient: ReadonlyMap<string, readonly VpsRuleValueRecord[]>;
};

export const VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE = "VPS rule data unavailable";

export function parseSearchExpression(input: string): SearchParseResult {
  const tokenResult = tokenizeSearchExpression(input);
  if (tokenResult.error) {
    return {
      expression: null,
      error: tokenResult.error,
      tokens: tokenResult.tokens,
    };
  }
  if (tokenResult.tokens.length === 0) {
    return { expression: null, error: null, tokens: [] };
  }
  const parser = new Parser(tokenResult.tokens);
  const expression = parser.parseOr();
  if (parser.error) {
    return {
      expression: null,
      error: parser.error,
      tokens: tokenResult.tokens,
    };
  }
  if (!parser.atEnd()) {
    return {
      expression: null,
      error: "Unexpected token after expression",
      tokens: tokenResult.tokens,
    };
  }
  const semanticError = validateVpsRuleExpression(expression);
  if (semanticError) {
    return {
      expression: null,
      error: semanticError,
      tokens: tokenResult.tokens,
    };
  }
  return { expression, error: null, tokens: tokenResult.tokens };
}

export function tokenizeSearchExpression(input: string): {
  error: string | null;
  tokens: SearchToken[];
} {
  const tokens: SearchToken[] = [];
  let index = 0;
  while (index < input.length) {
    const char = input[index];
    if (/\s/.test(char)) {
      index += 1;
      continue;
    }
    const simple = simpleTokenKind(char);
    if (simple) {
      tokens.push(
        token(simple, input.slice(index, index + 1), index, index + 1),
      );
      index += 1;
      continue;
    }
    if (char === "&" || char === "|") {
      const next = input[index + 1];
      if (next !== char) {
        return { error: "Use && or || for boolean operators", tokens };
      }
      tokens.push(
        token(
          char === "&" ? "and" : "or",
          input.slice(index, index + 2),
          index,
          index + 2,
        ),
      );
      index += 2;
      continue;
    }
    if (char === "~") {
      tokens.push(token("not", char, index, index + 1));
      index += 1;
      continue;
    }
    if (char === "!") {
      if (input[index + 1] === "=") {
        tokens.push(token("operator", "!=", index, index + 2));
        index += 2;
      } else {
        tokens.push(token("not", char, index, index + 1));
        index += 1;
      }
      continue;
    }
    if (char === "=" || char === "<" || char === ">") {
      const two = input.slice(index, index + 2);
      if (two === "<=" || two === ">=") {
        tokens.push(token("operator", two, index, index + 2));
        index += 2;
      } else {
        tokens.push(token("operator", char, index, index + 1));
        index += 1;
      }
      continue;
    }
    if (char === '"' || char === "'") {
      const quoted = readQuoted(input, index, char);
      if (quoted.error) {
        return { error: quoted.error, tokens };
      }
      tokens.push(
        token(
          "string",
          input.slice(index, quoted.end),
          index,
          quoted.end,
          quoted.value,
        ),
      );
      index = quoted.end;
      continue;
    }
    if (char === "/") {
      const regex = readRegex(input, index);
      if (regex.error) {
        return { error: regex.error, tokens };
      }
      tokens.push(
        token(
          "regex",
          input.slice(index, regex.end),
          index,
          regex.end,
          regex.value,
        ),
      );
      index = regex.end;
      continue;
    }
    const start = index;
    const term = readTerm(input, index);
    if (term.error) {
      return { error: term.error, tokens };
    }
    index = term.end;
    const raw = term.raw;
    const lower = term.value.toLowerCase();
    if (
      lower === "and" ||
      lower === "or" ||
      lower === "not" ||
      lower === "in"
    ) {
      tokens.push(
        token(lower as "and" | "or" | "not" | "in", raw, start, index),
      );
      continue;
    }
    const separator = term.value.indexOf(":");
    if (separator === 0) {
      return { error: "Selector namespace is empty", tokens };
    }
    if (separator === term.value.length - 1) {
      return { error: "Selector value is empty", tokens };
    }
    tokens.push(token("term", raw, start, index, term.value));
  }
  return { error: null, tokens };
}

export function evaluateSearchExpression(
  expression: SearchExpression | null,
  fields: SearchFields,
): boolean {
  if (!expression) {
    return true;
  }
  if (expression.type === "and") {
    return (
      evaluateSearchExpression(expression.left, fields) &&
      evaluateSearchExpression(expression.right, fields)
    );
  }
  if (expression.type === "or") {
    return (
      evaluateSearchExpression(expression.left, fields) ||
      evaluateSearchExpression(expression.right, fields)
    );
  }
  if (expression.type === "not") {
    return !evaluateSearchExpression(expression.expression, fields);
  }
  return predicateMatchesFields(expression.predicate, fields);
}

export function filterBySearchExpression<T>(
  items: T[],
  input: string,
  fieldsForItem: (item: T) => SearchFields,
): { error: string | null; items: T[]; tokens: SearchToken[] } {
  const parsed = parseSearchExpression(input);
  if (parsed.error) {
    return { error: parsed.error, items: [], tokens: parsed.tokens };
  }
  return {
    error: null,
    items: items.filter((item) =>
      evaluateSearchExpression(parsed.expression, fieldsForItem(item)),
    ),
    tokens: parsed.tokens,
  };
}

export function agentSearchFields(
  agent: AgentView,
  context?: AgentSearchContext,
): SearchFields {
  const providerTags = agent.tags.filter((tag) =>
    tag.toLocaleLowerCase().startsWith("provider:"),
  );
  const countryTags = agent.tags.filter((tag) =>
    tag.toLocaleLowerCase().startsWith("country:"),
  );
  const regionTags = agent.tags.filter((tag) =>
    tag.toLocaleLowerCase().startsWith("region:"),
  );
  const providerValues = providerTags.map((tag) =>
    tag.slice("provider:".length),
  );
  const countryValues = countryTags.map((tag) => tag.slice("country:".length));
  const regionValues = regionTags.map((tag) => tag.slice("region:".length));
  return {
    all: [agent.id, agent.display_name],
    fields: {
      "vps.id": [agent.id],
      "vps.display_name": [agent.display_name],
      "vps.name": [agent.display_name],
      "vps.status": [agent.status],
      "vps.tag": agent.tags,
      "vps.tags": agent.tags,
      "vps.provider": providerValues,
      "vps.country": countryValues,
      "vps.region": regionValues,
      "vps.last_seen_at": agent.last_seen_at ? [agent.last_seen_at] : [],
      "vps.internal_build_number": [agent.internal_build_number ?? 0],
      last_seen: agent.last_seen_at ? [agent.last_seen_at] : [],
      status: [agent.status],
    },
    namespaces: {
      country: countryTags.concat(countryValues),
      id: [agent.id],
      name: [agent.display_name],
      provider: providerTags.concat(providerValues),
      region: regionTags.concat(regionValues),
      status: [agent.status],
      tag: agent.tags,
      tags: agent.tags,
    },
    vpsRules: context?.available
      ? (context.rulesByClient.get(agent.id) ?? [])
      : undefined,
  };
}

export function agentsMatchingExpression(
  agents: AgentView[],
  input: string,
  context?: AgentSearchContext,
): AgentView[] {
  if (expressionReferencesVpsRules(input) && !context?.available) {
    throw new Error(VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE);
  }
  return filterBySearchExpression(agents, input, (agent) =>
    agentSearchFields(agent, context),
  ).items;
}

export function termMatchTitle(
  term: SearchToken,
  agents: AgentView[],
  context?: AgentSearchContext,
): string {
  if (term.kind !== "term" && term.kind !== "string" && term.kind !== "regex") {
    return term.raw;
  }
  const description = describeToken(term);
  const parsed = expressionForToken(term);
  if (expressionReferencesVpsRules(parsed) && !context?.available) {
    return `${description}. ${VPS_RULE_SEARCH_UNAVAILABLE_MESSAGE}`;
  }
  const matches = agents.filter((agent) =>
    evaluateSearchExpression(parsed, agentSearchFields(agent, context)),
  );
  const labels = matches
    .map((agent) => `${agent.id} (${agent.display_name}; ${agent.status})`)
    .join(", ");
  return `${description}. ${matches.length} matched target${matches.length === 1 ? "" : "s"}${labels ? `: ${labels}` : ""}`;
}

export function expressionReferencesVpsRules(
  inputOrExpression: string | SearchExpression | null,
): boolean {
  const expression =
    typeof inputOrExpression === "string"
      ? parseSearchExpression(inputOrExpression).expression
      : inputOrExpression;
  if (!expression) {
    return typeof inputOrExpression === "string"
      ? /(?:^|[^a-z0-9_.])vps\.rules(?::|\b)/i.test(inputOrExpression)
      : false;
  }
  if (expression.type === "and" || expression.type === "or") {
    return (
      expressionReferencesVpsRules(expression.left) ||
      expressionReferencesVpsRules(expression.right)
    );
  }
  if (expression.type === "not") {
    return expressionReferencesVpsRules(expression.expression);
  }
  const predicate = expression.predicate;
  if (predicate.type !== "comparison" && predicate.type !== "membership") {
    return false;
  }
  const field = canonicalField(predicate.field);
  return (
    field === "vps.rules" ||
    field.startsWith("vps.rules:") ||
    field.startsWith("vps.rules.")
  );
}

export function vpsRuleSearchUnavailable(
  input: string,
  context?: AgentSearchContext,
): boolean {
  return expressionReferencesVpsRules(input) && !context?.available;
}

export function addSelectorToExpression(
  expression: string,
  selector: string,
): string {
  const trimmedExpression = expression.trim();
  const trimmedSelector = selector.trim();
  if (!trimmedSelector) {
    return trimmedExpression;
  }
  if (!trimmedExpression) {
    return trimmedSelector;
  }
  const parsed = tokenizeSearchExpression(trimmedExpression);
  if (
    parsed.tokens.some(
      (candidate) =>
        candidate.kind === "term" && candidate.raw === trimmedSelector,
    )
  ) {
    return trimmedExpression;
  }
  return `${trimmedExpression} || ${trimmedSelector}`;
}

export function selectorExpressionForClientIds(clientIds: string[]): string {
  return Array.from(
    new Set(clientIds.map((clientId) => clientId.trim()).filter(Boolean)),
  )
    .map((clientId) => `id:${clientId}`)
    .join(" || ");
}

export function quoteSelectorValue(value: string): string {
  if (/^[^\s()[\],=!<>|&~"']+$/.test(value)) {
    return value;
  }
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

export function removeTokenFromExpression(
  expression: string,
  tokenToRemove: SearchToken,
): string {
  if (tokenToRemove.kind !== "term") {
    return expression;
  }
  const tokens = tokenizeSearchExpression(expression).tokens;
  const tokenIndex = tokens.findIndex(
    (candidate) =>
      candidate.start === tokenToRemove.start &&
      candidate.end === tokenToRemove.end,
  );
  let removeStart = tokenToRemove.start;
  let removeEnd = tokenToRemove.end;
  const previous = tokenIndex > 0 ? tokens[tokenIndex - 1] : null;
  const next = tokenIndex >= 0 ? tokens[tokenIndex + 1] : null;
  if (next?.kind === "and" || next?.kind === "or") {
    removeEnd = next.end;
  } else if (previous?.kind === "and" || previous?.kind === "or") {
    removeStart = previous.start;
  }
  return cleanExpressionSpacing(
    `${expression.slice(0, removeStart)} ${expression.slice(removeEnd)}`,
  );
}

function predicateMatchesFields(
  predicate: SearchPredicate,
  fields: SearchFields,
): boolean {
  if (predicate.type === "bare") {
    return fields.all.some((value) =>
      valueMatches(value, predicate.value, true),
    );
  }
  if (predicate.type === "event") {
    return (
      fields.events?.some(
        (event) => event.toLocaleLowerCase() === predicate.value,
      ) ?? false
    );
  }
  if (predicate.type === "untagged") {
    return Boolean(
      fields.fields?.["vps.tag"] && fields.fields["vps.tag"].length === 0,
    );
  }
  const values = fieldValues(fields, predicate.field);
  if (!values) {
    if (
      predicate.type === "membership" &&
      canonicalField(predicate.field) === "vps.tag" &&
      predicate.values.length === 1 &&
      predicate.values[0].type === "literal" &&
      predicate.values[0].value.includes(":")
    ) {
      return fields.all.some((value) =>
        valueMatches(value, predicate.values[0].value, true),
      );
    }
    return false;
  }
  if (predicate.type === "comparison") {
    if (canonicalField(predicate.field) === "vps.rules") {
      const matched = values.some((actual) =>
        vpsRuleLiteralMatches(String(actual), predicate.value),
      );
      return predicate.operator === "=" ? matched : !matched;
    }
    const keyedRule = keyedVpsRuleField(predicate.field);
    if (keyedRule) {
      const rule = fields.vpsRules?.find(
        (candidate) => candidate.key === keyedRule,
      );
      if (!rule) return false;
      if (isOrderedComparison(predicate.operator)) {
        return orderedVpsRuleMatches(rule, predicate.operator, predicate.value);
      }
      const expected = hasGlob(predicate.value)
        ? predicate.value
        : tryNormalizeVpsRuleValue(keyedRule, predicate.value);
      const actual = hasGlob(predicate.value)
        ? rule.value_raw
        : tryNormalizeVpsRuleValue(keyedRule, rule.value_raw);
      if (expected === null || actual === null) return false;
      const matched = vpsRuleLiteralMatches(actual, expected);
      return predicate.operator === "=" ? matched : !matched;
    }
    return values.some((actual) =>
      compareValue(actual, predicate.operator, predicate.value),
    );
  }
  if (canonicalField(predicate.field) === "vps.rules") {
    const matched = values.some((actual) =>
      predicate.values.some((expected) => {
        if (expected.type === "regex") {
          try {
            return new RegExp(expected.value).test(String(actual));
          } catch {
            return false;
          }
        }
        return vpsRuleLiteralMatches(String(actual), expected.value);
      }),
    );
    return predicate.negated ? !matched : matched;
  }
  const keyedRule = keyedVpsRuleField(predicate.field);
  if (keyedRule) {
    const rule = fields.vpsRules?.find(
      (candidate) => candidate.key === keyedRule,
    );
    if (!rule) return false;
    const matched = predicate.values.some((expected) => {
      if (expected.type === "regex") {
        try {
          return new RegExp(expected.value).test(rule.value_raw);
        } catch {
          return false;
        }
      }
      if (hasGlob(expected.value)) {
        return vpsRuleLiteralMatches(rule.value_raw, expected.value);
      }
      const actualValue = tryNormalizeVpsRuleValue(keyedRule, rule.value_raw);
      const expectedValue = tryNormalizeVpsRuleValue(keyedRule, expected.value);
      return Boolean(
        actualValue &&
        expectedValue &&
        asciiLowercase(actualValue) === asciiLowercase(expectedValue),
      );
    });
    return predicate.negated ? !matched : matched;
  }
  const matched = values.some((actual) =>
    listValuesMatch(String(actual), predicate.values),
  );
  return predicate.negated ? !matched : matched;
}

function compareValue(
  actual: ComparableValue,
  operator: ComparisonOperator,
  expected: string,
): boolean {
  if (operator === "=" || operator === "!=") {
    const matched =
      typeof actual === "number"
        ? Number(expected) === actual
        : valueMatches(String(actual), expected, false);
    return operator === "=" ? matched : !matched;
  }
  const actualNumber =
    typeof actual === "number" ? actual : timestampValue(String(actual));
  const expectedNumber = timestampValue(expected);
  if (actualNumber === null || expectedNumber === null) {
    return false;
  }
  if (operator === "<") return actualNumber < expectedNumber;
  if (operator === "<=") return actualNumber <= expectedNumber;
  if (operator === ">") return actualNumber > expectedNumber;
  return actualNumber >= expectedNumber;
}

function listValuesMatch(actual: string, values: SearchListValue[]): boolean {
  return values.some((value) => {
    if (value.type === "literal") {
      return valueMatches(actual, value.value, false);
    }
    try {
      return new RegExp(value.value).test(actual);
    } catch {
      return false;
    }
  });
}

function fieldValues(
  fields: SearchFields,
  field: string,
): ComparableValue[] | null {
  const canonical = canonicalField(field);
  if (canonical === "vps.rules") {
    return fields.vpsRules?.map((rule) => rule.key) ?? null;
  }
  const keyedRule = keyedVpsRuleField(canonical);
  if (keyedRule) {
    const rule = fields.vpsRules?.find(
      (candidate) => candidate.key === keyedRule,
    );
    return rule ? [rule.value_raw] : fields.vpsRules ? [] : null;
  }
  if (fields.fields?.[canonical]) {
    return fields.fields[canonical];
  }
  if (fields.namespaces?.[canonical]) {
    return fields.namespaces[canonical];
  }
  if (
    canonical.startsWith("vps.") &&
    fields.namespaces?.[canonical.slice("vps.".length)]
  ) {
    return fields.namespaces[canonical.slice("vps.".length)];
  }
  return null;
}

function keyedVpsRuleField(field: string): string | null {
  const canonical = canonicalField(field);
  const prefix = "vps.rules:";
  return canonical.startsWith(prefix) ? canonical.slice(prefix.length) : null;
}

function isOrderedComparison(operator: ComparisonOperator): boolean {
  return (
    operator === "<" ||
    operator === "<=" ||
    operator === ">" ||
    operator === ">="
  );
}

function validateVpsRuleExpression(
  expression: SearchExpression | null,
): string | null {
  if (!expression) return null;
  if (expression.type === "and" || expression.type === "or") {
    return (
      validateVpsRuleExpression(expression.left) ??
      validateVpsRuleExpression(expression.right)
    );
  }
  if (expression.type === "not") {
    return validateVpsRuleExpression(expression.expression);
  }
  const predicate = expression.predicate;
  if (predicate.type === "bare") {
    const raw = asciiLowercase(predicate.value);
    return raw === "vps.rules" || raw.startsWith("vps.rules.")
      ? "choose a VPS rule; use vps.rules:<key>, for example vps.rules:traffic.reset_day"
      : null;
  }
  if (predicate.type !== "comparison" && predicate.type !== "membership") {
    return null;
  }
  const field = canonicalField(predicate.field);
  if (field === "vps.rules") {
    if (predicate.type === "comparison") {
      if (predicate.operator !== "=") {
        return predicate.operator === "!="
          ? "vps.rules key absence uses !(vps.rules:<key>), not !="
          : "vps.rules key presence does not support ordered comparisons";
      }
      if (
        !hasGlob(predicate.value) &&
        predicate.value !== "*" &&
        !isVpsRuleKey(asciiLowercase(predicate.value))
      ) {
        return `unsupported VPS rule key \`${predicate.value}\``;
      }
    } else {
      if (predicate.negated) {
        return "vps.rules key absence uses unary ! around a presence expression, not `not in`";
      }
      const unsupported = predicate.values.find(
        (value) =>
          value.type === "literal" &&
          !hasGlob(value.value) &&
          !isVpsRuleKey(asciiLowercase(value.value)),
      );
      if (unsupported) {
        return `unsupported VPS rule key \`${unsupported.value}\``;
      }
    }
    return null;
  }
  if (field.startsWith("vps.rules.")) {
    return "VPS rule values use vps.rules:<key>, for example vps.rules:traffic.reset_day";
  }
  const key = keyedVpsRuleField(field);
  if (!key) return null;
  if (!isVpsRuleKey(key)) {
    return `unsupported VPS rule key \`${key}\``;
  }
  if (
    predicate.type === "comparison" &&
    !isOrderedComparison(predicate.operator)
  ) {
    if (
      !hasGlob(predicate.value) &&
      !tryNormalizeVpsRuleValue(key, predicate.value)
    ) {
      return `invalid value for VPS rule \`${key}\``;
    }
    return null;
  }
  if (predicate.type === "membership") {
    const invalid = predicate.values.find(
      (value) =>
        value.type === "literal" &&
        !hasGlob(value.value) &&
        !tryNormalizeVpsRuleValue(key, value.value),
    );
    return invalid ? `invalid value for VPS rule \`${key}\`` : null;
  }
  if (predicate.type !== "comparison") {
    return null;
  }
  const definition = vpsRuleDefinition(key);
  if (!definition?.orderedKind) {
    return `VPS rule \`${key}\` does not support ordered comparisons`;
  }
  if (!parseOrderedVpsRuleValue(definition.orderedKind, predicate.value)) {
    const requirement =
      definition.orderedKind === "day"
        ? "a day from 1 to 31"
        : definition.orderedKind === "bytes"
          ? "a positive byte size using B, KB, MB, GB, TB, KiB, MiB, GiB, or TiB"
          : definition.orderedKind === "speed"
            ? "a positive speed using bps, Kbps, Mbps, Gbps, or Tbps"
            : "a positive amount with currency and period, such as 50 USD/m";
    return `VPS rule \`${key}\` ordered comparison requires ${requirement}`;
  }
  return null;
}

function hasGlob(value: string): boolean {
  return value.includes("*") || value.includes("?");
}

function vpsRuleLiteralMatches(actual: string, expected: string): boolean {
  const normalizedActual = asciiLowercase(actual);
  const normalizedExpected = asciiLowercase(expected);
  return hasGlob(expected)
    ? globMatches(normalizedActual, normalizedExpected)
    : normalizedActual === normalizedExpected;
}

function asciiLowercase(value: string): string {
  return value.replace(/[A-Z]/g, (character) => character.toLowerCase());
}

type OrderedRuleValue =
  | { kind: "integer"; value: bigint }
  | { kind: "billing"; amount: bigint; currency: string; period: string };

function orderedVpsRuleMatches(
  rule: VpsRuleValueRecord,
  operator: ComparisonOperator,
  expected: string,
): boolean {
  const kind = vpsRuleDefinition(rule.key)?.orderedKind;
  if (!kind || rule.value_raw.trim() === "-1" || expected.trim() === "-1") {
    return false;
  }
  const actualValue = parseOrderedVpsRuleValue(kind, rule.value_raw, true);
  const expectedValue = parseOrderedVpsRuleValue(kind, expected);
  if (
    !actualValue ||
    !expectedValue ||
    actualValue.kind !== expectedValue.kind
  ) {
    return false;
  }
  if (actualValue.kind === "billing" && expectedValue.kind === "billing") {
    if (
      actualValue.currency !== expectedValue.currency ||
      actualValue.period !== expectedValue.period
    ) {
      return false;
    }
    return compareBigInt(actualValue.amount, operator, expectedValue.amount);
  }
  if (actualValue.kind === "integer" && expectedValue.kind === "integer") {
    return compareBigInt(actualValue.value, operator, expectedValue.value);
  }
  return false;
}

function compareBigInt(
  actual: bigint,
  operator: ComparisonOperator,
  expected: bigint,
): boolean {
  if (operator === "<") return actual < expected;
  if (operator === "<=") return actual <= expected;
  if (operator === ">") return actual > expected;
  if (operator === ">=") return actual >= expected;
  return operator === "=" ? actual === expected : actual !== expected;
}

function parseOrderedVpsRuleValue(
  kind: "billing_price" | "bytes" | "day" | "speed",
  input: string,
  storedValue = false,
): OrderedRuleValue | null {
  if (kind === "day") {
    const value = vpsRuleOrderedIntegerValue("traffic.reset_day", input);
    return value !== null ? { kind: "integer", value } : null;
  }
  if (kind === "bytes") {
    const value = vpsRuleOrderedIntegerValue("traffic.quota.total", input);
    return value !== null ? { kind: "integer", value } : null;
  }
  if (kind === "speed") {
    const value = vpsRuleOrderedIntegerValue("network.port_speed", input);
    return value !== null ? { kind: "integer", value } : null;
  }
  return parseBillingValue(input, storedValue);
}

function parseBillingValue(
  input: string,
  allowZero: boolean,
): OrderedRuleValue | null {
  const canonical = tryNormalizeVpsRuleValue("billing.price", input);
  if (!canonical || canonical === "-1") return null;
  const match = canonical.match(/^(\d+)\.(\d{2}) ([^/]+)\/(m|q|hy|y)$/);
  if (!match) return null;
  const whole = BigInt(match[1]);
  const fraction = match[2];
  const currencyInput = match[3];
  const upperCurrency = currencyInput.toUpperCase();
  const currency =
    currencyInput === "$"
      ? "USD"
      : currencyInput === "¥" || currencyInput === "￥"
        ? "CNY"
        : currencyInput === "€"
          ? "EUR"
          : currencyInput === "£"
            ? "GBP"
            : /^[A-Z]{3}$/.test(upperCurrency)
              ? upperCurrency
              : null;
  if (!currency) return null;
  const periodInput = match[4].toLowerCase();
  const period = periodInput === "h" ? "hy" : periodInput;
  const amount = whole * 100n + BigInt(fraction);
  if (!allowZero && amount === 0n) return null;
  return {
    amount,
    currency,
    kind: "billing",
    period,
  };
}

function shorthandPredicate(raw: string): SearchPredicate {
  const [namespace, ...rest] = raw.split(":");
  const value = rest.join(":");
  const lower = asciiLowercase(namespace);
  if (!value) {
    return { type: "bare", value: raw };
  }
  if (lower === "id")
    return { type: "comparison", field: "vps.id", operator: "=", value };
  if (lower === "name")
    return {
      type: "comparison",
      field: "vps.display_name",
      operator: "=",
      value,
    };
  if (lower === "status")
    return { type: "comparison", field: "vps.status", operator: "=", value };
  if (
    lower === "tag" ||
    lower === "tags" ||
    lower === "vps.tag" ||
    lower === "vps.tags"
  ) {
    return {
      type: "membership",
      field: "vps.tag",
      negated: false,
      values: [{ type: "literal", value }],
    };
  }
  if (lower === "provider") {
    return {
      type: "membership",
      field: "vps.tag",
      negated: false,
      values: [{ type: "literal", value: `provider:${value}` }],
    };
  }
  if (lower === "country") {
    return {
      type: "membership",
      field: "vps.tag",
      negated: false,
      values: [{ type: "literal", value: `country:${value}` }],
    };
  }
  if (lower === "region") {
    return {
      type: "membership",
      field: "vps.tag",
      negated: false,
      values: [{ type: "literal", value: `region:${value}` }],
    };
  }
  if (lower.startsWith("vps.")) {
    return {
      type: "comparison",
      field: canonicalField(lower),
      operator: "=",
      value,
    };
  }
  return {
    type: "membership",
    field: "vps.tag",
    negated: false,
    values: [{ type: "literal", value: `${namespace}:${value}` }],
  };
}

function canonicalField(field: string): string {
  const lower = asciiLowercase(field);
  if (
    lower === "id" ||
    lower === "client_id" ||
    lower === "vps.id" ||
    lower === "vps.client_id"
  )
    return "vps.id";
  if (
    lower === "name" ||
    lower === "display_name" ||
    lower === "vps.name" ||
    lower === "vps.display_name"
  )
    return "vps.display_name";
  if (lower === "status" || lower === "vps.status") return "vps.status";
  if (
    lower === "tag" ||
    lower === "tags" ||
    lower === "vps.tag" ||
    lower === "vps.tags"
  )
    return "vps.tag";
  if (
    lower === "last_seen" ||
    lower === "last_seen_at" ||
    lower === "vps.last_seen" ||
    lower === "vps.last_seen_at"
  )
    return "vps.last_seen_at";
  if (lower === "region" || lower === "vps.region") return "vps.region";
  return lower;
}

function isEventPredicate(raw: string): boolean {
  const lower = asciiLowercase(raw);
  return (
    lower.startsWith("interval.") ||
    lower.startsWith("vps.status.") ||
    lower.startsWith("vps.tag_event:") ||
    lower.startsWith("vps.tag_event.added:") ||
    lower.startsWith("vps.tag_event.removed:") ||
    lower.startsWith("job.status:") ||
    lower.startsWith("job.status.become_") ||
    lower.startsWith("job.type:") ||
    lower.startsWith("job.target.status:") ||
    lower.startsWith("schedule.id:") ||
    lower.startsWith("schedule.name:") ||
    lower.startsWith("alert.severity:") ||
    lower.startsWith("alert.category:") ||
    lower.startsWith("alert.state:") ||
    lower === "server.on_start" ||
    lower === "schedule.due" ||
    lower === "schedule.dispatched" ||
    lower === "schedule.failed" ||
    lower === "vps.tag_changed" ||
    lower === "job.created" ||
    lower === "alert.open" ||
    lower === "telemetry.rollup" ||
    lower === "telemetry.network_rate" ||
    lower === "telemetry.tunnel"
  );
}

function expressionForToken(term: SearchToken): SearchExpression {
  if (term.kind === "regex") {
    return {
      type: "predicate",
      predicate: { type: "bare", value: term.value },
    };
  }
  if (term.kind === "string") {
    return {
      type: "predicate",
      predicate: { type: "bare", value: term.value },
    };
  }
  return { type: "predicate", predicate: predicateFromTerm(term.value) };
}

function predicateFromTerm(raw: string): SearchPredicate {
  if (asciiLowercase(raw) === "untagged") {
    return { type: "untagged" };
  }
  if (isEventPredicate(raw)) {
    return { type: "event", value: asciiLowercase(raw) };
  }
  if (raw.includes(":")) {
    return shorthandPredicate(raw);
  }
  return { type: "bare", value: raw };
}

function describeToken(term: SearchToken): string {
  if (term.kind === "regex") {
    return `Regex list value /${term.value}/`;
  }
  if (term.kind === "string") {
    return `Quoted literal "${term.value}"`;
  }
  const predicate = predicateFromTerm(term.value);
  if (predicate.type === "event") return `Event predicate ${predicate.value}`;
  if (predicate.type === "untagged") return "VPS has metadata and no tags";
  if (predicate.type === "comparison")
    return `${predicate.field} ${predicate.operator} ${predicate.value}`;
  if (predicate.type === "membership")
    return `${predicate.field} ${predicate.negated ? "not in" : "in"} [${predicate.values.map((value) => value.value).join(", ")}]`;
  return `Bare text search "${predicate.value}"`;
}

function valueMatches(
  value: string,
  pattern: string,
  allowContains: boolean,
): boolean {
  const normalizedValue = value.toLocaleLowerCase();
  const normalizedPattern = pattern.toLocaleLowerCase();
  if (normalizedPattern.includes("*") || normalizedPattern.includes("?")) {
    return globMatches(normalizedValue, normalizedPattern);
  }
  return allowContains
    ? normalizedValue.includes(normalizedPattern)
    : normalizedValue === normalizedPattern;
}

function timestampValue(value: string): number | null {
  const numeric = Number(value);
  if (Number.isFinite(numeric)) {
    return numeric;
  }
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? Math.floor(timestamp / 1000) : null;
}

function globMatches(value: string, pattern: string): boolean {
  let valueIndex = 0;
  let patternIndex = 0;
  let starIndex = -1;
  let starValueIndex = 0;
  while (valueIndex < value.length) {
    if (
      patternIndex < pattern.length &&
      (pattern[patternIndex] === "?" ||
        pattern[patternIndex] === value[valueIndex])
    ) {
      valueIndex += 1;
      patternIndex += 1;
    } else if (patternIndex < pattern.length && pattern[patternIndex] === "*") {
      starIndex = patternIndex;
      patternIndex += 1;
      starValueIndex = valueIndex;
    } else if (starIndex >= 0) {
      patternIndex = starIndex + 1;
      starValueIndex += 1;
      valueIndex = starValueIndex;
    } else {
      return false;
    }
  }
  while (patternIndex < pattern.length && pattern[patternIndex] === "*") {
    patternIndex += 1;
  }
  return patternIndex === pattern.length;
}

function cleanExpressionSpacing(expression: string): string {
  return expression
    .replace(/\s+/g, " ")
    .replace(/\(\s+/g, "(")
    .replace(/\s+\)/g, ")")
    .replace(/\(\s*\)/g, "")
    .replace(/^\s*(?:&&|\|\|)\s*/g, "")
    .replace(/\s*(?:&&|\|\|)\s*$/g, "")
    .trim();
}

function token(
  kind: SearchToken["kind"],
  raw: string,
  start: number,
  end: number,
  value = raw,
): SearchToken {
  const separator = value.indexOf(":");
  return {
    end,
    kind,
    namespace:
      kind === "term" && separator > 0
        ? asciiLowercase(value.slice(0, separator))
        : null,
    raw,
    start,
    value,
  };
}

function simpleTokenKind(char: string): SearchToken["kind"] | null {
  if (char === "(") return "left_paren";
  if (char === ")") return "right_paren";
  if (char === "[") return "left_bracket";
  if (char === "]") return "right_bracket";
  if (char === ",") return "comma";
  return null;
}

function readQuoted(
  input: string,
  start: number,
  quote: string,
): { end: number; error: string | null; value: string } {
  let value = "";
  let escaped = false;
  for (let index = start + 1; index < input.length; index += 1) {
    const char = input[index];
    if (escaped) {
      value += char;
      escaped = false;
    } else if (char === "\\") {
      escaped = true;
    } else if (char === quote) {
      return { end: index + 1, error: null, value };
    } else {
      value += char;
    }
  }
  return { end: input.length, error: "Unterminated quoted value", value };
}

function readTerm(
  input: string,
  start: number,
): { end: number; error: string | null; raw: string; value: string } {
  let escaped = false;
  let quote: string | null = null;
  let raw = "";
  let value = "";
  for (let index = start; index < input.length; index += 1) {
    const char = input[index];
    if (!quote && isTermDelimiter(char)) {
      return { end: index, error: null, raw, value };
    }
    raw += char;
    if (quote) {
      if (escaped) {
        value += char;
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === quote) {
        quote = null;
      } else {
        value += char;
      }
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }
    value += char;
  }
  if (quote) {
    return {
      end: input.length,
      error: "Unterminated quoted value",
      raw,
      value,
    };
  }
  return { end: input.length, error: null, raw, value };
}

function isTermDelimiter(char: string): boolean {
  return /[\s()[\],=!<>|&~]/.test(char);
}

function readRegex(
  input: string,
  start: number,
): { end: number; error: string | null; value: string } {
  let value = "";
  let escaped = false;
  for (let index = start + 1; index < input.length; index += 1) {
    const char = input[index];
    if (escaped) {
      value += `\\${char}`;
      escaped = false;
    } else if (char === "\\") {
      escaped = true;
    } else if (char === "/") {
      const next = input[index + 1];
      if (next && /[A-Za-z]/.test(next)) {
        return {
          end: index + 1,
          error: "Regex flags are not supported",
          value,
        };
      }
      try {
        new RegExp(value);
      } catch {
        return { end: index + 1, error: "Invalid regex list value", value };
      }
      return { end: index + 1, error: null, value };
    } else {
      value += char;
    }
  }
  return { end: input.length, error: "Unterminated regex value", value };
}

class Parser {
  error: string | null = null;
  private position = 0;

  constructor(private readonly tokens: SearchToken[]) {}

  atEnd(): boolean {
    return this.position >= this.tokens.length;
  }

  parseOr(): SearchExpression | null {
    let expression = this.parseAnd();
    while (!this.error && this.peek()?.kind === "or") {
      this.position += 1;
      const right = this.parseAnd();
      if (!expression || !right) {
        this.error = "Operator is missing an operand";
        return null;
      }
      expression = { type: "or", left: expression, right };
    }
    return expression;
  }

  private parseAnd(): SearchExpression | null {
    let expression = this.parseNot();
    while (!this.error) {
      if (this.peek()?.kind === "and") {
        this.position += 1;
        const right = this.parseNot();
        if (!expression || !right) {
          this.error = "Operator is missing an operand";
          return null;
        }
        expression = { type: "and", left: expression, right };
        continue;
      }
      if (this.nextStartsPrimary()) {
        const right = this.parseNot();
        if (!expression || !right) {
          this.error = "Expression is incomplete";
          return null;
        }
        expression = { type: "and", left: expression, right };
        continue;
      }
      break;
    }
    return expression;
  }

  private parseNot(): SearchExpression | null {
    if (this.peek()?.kind === "not") {
      this.position += 1;
      const expression = this.parseNot();
      if (!expression) {
        this.error = "NOT is missing an operand";
        return null;
      }
      return { type: "not", expression };
    }
    return this.parsePrimary();
  }

  private parsePrimary(): SearchExpression | null {
    const token = this.advance();
    if (!token) {
      this.error = "Expression is incomplete";
      return null;
    }
    if (token.kind === "term") {
      const predicate = this.parsePredicate(token.value);
      return predicate ? { type: "predicate", predicate } : null;
    }
    if (token.kind === "left_paren") {
      const expression = this.parseOr();
      if (this.advance()?.kind !== "right_paren") {
        this.error = "Missing closing parenthesis";
        return null;
      }
      return expression;
    }
    this.error =
      token.kind === "right_paren"
        ? "Unexpected closing parenthesis"
        : "Operator is missing a left operand";
    return null;
  }

  private parsePredicate(raw: string): SearchPredicate | null {
    const operator = this.consumeComparisonOperator();
    if (operator) {
      const value = this.parseScalarValue();
      return value === null
        ? null
        : { type: "comparison", field: canonicalField(raw), operator, value };
    }
    if (this.peek()?.kind === "in") {
      this.position += 1;
      const values = this.parseListValues();
      return values
        ? {
            type: "membership",
            field: canonicalField(raw),
            negated: false,
            values,
          }
        : null;
    }
    if (
      this.peek()?.kind === "not" &&
      this.tokens[this.position + 1]?.kind === "in"
    ) {
      this.position += 2;
      const values = this.parseListValues();
      return values
        ? {
            type: "membership",
            field: canonicalField(raw),
            negated: true,
            values,
          }
        : null;
    }
    return predicateFromTerm(raw);
  }

  private parseScalarValue(): string | null {
    const token = this.advance();
    if (token?.kind === "term" || token?.kind === "string") {
      return token.value;
    }
    this.error = "Comparison is missing a scalar value";
    return null;
  }

  private parseListValues(): SearchListValue[] | null {
    if (this.advance()?.kind !== "left_bracket") {
      this.error = "Membership comparison is missing [";
      return null;
    }
    const values: SearchListValue[] = [];
    while (!this.error) {
      const token = this.advance();
      if (token?.kind === "term" || token?.kind === "string") {
        values.push({ type: "literal", value: token.value });
      } else if (token?.kind === "regex") {
        values.push({ type: "regex", value: token.value });
      } else {
        this.error =
          values.length === 0 && token?.kind === "right_bracket"
            ? "Membership list must not be empty"
            : "Membership list contains an invalid value";
        return null;
      }
      if (this.peek()?.kind === "comma") {
        this.position += 1;
        continue;
      }
      if (this.peek()?.kind === "right_bracket") {
        this.position += 1;
        return values;
      }
      this.error = "Membership list values must be comma-separated";
      return null;
    }
    return null;
  }

  private consumeComparisonOperator(): ComparisonOperator | null {
    const value = this.peek()?.value;
    if (
      this.peek()?.kind === "operator" &&
      (value === "=" ||
        value === "!=" ||
        value === "<" ||
        value === "<=" ||
        value === ">" ||
        value === ">=")
    ) {
      this.position += 1;
      return value;
    }
    return null;
  }

  private nextStartsPrimary(): boolean {
    const next = this.peek()?.kind;
    return next === "term" || next === "left_paren" || next === "not";
  }

  private advance(): SearchToken | null {
    const token = this.tokens[this.position] ?? null;
    this.position += token ? 1 : 0;
    return token;
  }

  private peek(): SearchToken | null {
    return this.tokens[this.position] ?? null;
  }
}
