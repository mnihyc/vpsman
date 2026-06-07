import type { AgentView } from "./types";

export type SearchExpression =
  | { type: "term"; namespace: string | null; raw: string; value: string }
  | { type: "and"; left: SearchExpression; right: SearchExpression }
  | { type: "or"; left: SearchExpression; right: SearchExpression };

export type SearchToken = {
  end: number;
  kind: "and" | "left_paren" | "or" | "right_paren" | "term";
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
  namespaces?: Record<string, string[]>;
};

export function parseSearchExpression(input: string): SearchParseResult {
  const tokenResult = tokenizeSearchExpression(input);
  if (tokenResult.error) {
    return { expression: null, error: tokenResult.error, tokens: tokenResult.tokens };
  }
  if (tokenResult.tokens.length === 0) {
    return { expression: null, error: null, tokens: [] };
  }
  const parser = new Parser(tokenResult.tokens);
  const expression = parser.parseOr();
  if (parser.error) {
    return { expression: null, error: parser.error, tokens: tokenResult.tokens };
  }
  if (!parser.atEnd()) {
    return { expression: null, error: "Unexpected token after expression", tokens: tokenResult.tokens };
  }
  return { expression, error: null, tokens: tokenResult.tokens };
}

export function tokenizeSearchExpression(input: string): { error: string | null; tokens: SearchToken[] } {
  const tokens: SearchToken[] = [];
  let index = 0;
  while (index < input.length) {
    const char = input[index];
    if (/\s/.test(char)) {
      index += 1;
      continue;
    }
    if (char === "(" || char === ")") {
      tokens.push({
        end: index + 1,
        kind: char === "(" ? "left_paren" : "right_paren",
        namespace: null,
        raw: char,
        start: index,
        value: char,
      });
      index += 1;
      continue;
    }
    if (char === "&" || char === "|") {
      const next = input[index + 1];
      if (next !== char) {
        return { error: "Use && or || for boolean operators", tokens };
      }
      tokens.push({
        end: index + 2,
        kind: char === "&" ? "and" : "or",
        namespace: null,
        raw: input.slice(index, index + 2),
        start: index,
        value: input.slice(index, index + 2),
      });
      index += 2;
      continue;
    }
    const start = index;
    while (index < input.length && !/[\s()&|]/.test(input[index])) {
      index += 1;
    }
    const raw = input.slice(start, index);
    const lower = raw.toLocaleLowerCase();
    if (lower === "and" || lower === "or") {
      tokens.push({
        end: index,
        kind: lower === "and" ? "and" : "or",
        namespace: null,
        raw,
        start,
        value: raw,
      });
      continue;
    }
    const separator = raw.indexOf(":");
    if (separator === 0) {
      return { error: "Selector namespace is empty", tokens };
    }
    if (separator === raw.length - 1) {
      return { error: "Selector value is empty", tokens };
    }
    tokens.push({
      end: index,
      kind: "term",
      namespace: separator > 0 ? raw.slice(0, separator).toLocaleLowerCase() : null,
      raw,
      start,
      value: separator > 0 ? raw.slice(separator + 1) : raw,
    });
  }
  return { error: null, tokens };
}

export function evaluateSearchExpression(expression: SearchExpression | null, fields: SearchFields): boolean {
  if (!expression) {
    return true;
  }
  if (expression.type === "and") {
    return evaluateSearchExpression(expression.left, fields) && evaluateSearchExpression(expression.right, fields);
  }
  if (expression.type === "or") {
    return evaluateSearchExpression(expression.left, fields) || evaluateSearchExpression(expression.right, fields);
  }
  return termMatchesFields(expression, fields);
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
    items: items.filter((item) => evaluateSearchExpression(parsed.expression, fieldsForItem(item))),
    tokens: parsed.tokens,
  };
}

export function agentSearchFields(agent: AgentView): SearchFields {
  const providerTags = agent.tags.filter((tag) => tag.toLocaleLowerCase().startsWith("provider:"));
  const countryTags = agent.tags.filter((tag) => tag.toLocaleLowerCase().startsWith("country:"));
  const providerValues = providerTags.map((tag) => tag.slice("provider:".length));
  const countryValues = countryTags.map((tag) => tag.slice("country:".length));
  return {
    all: [agent.id, agent.display_name],
    namespaces: {
      country: countryTags.concat(countryValues),
      id: [agent.id],
      name: [agent.display_name],
      provider: providerTags.concat(providerValues),
      region: countryTags.concat(countryValues),
      status: [agent.status],
      tag: agent.tags,
    },
  };
}

export function agentsMatchingExpression(agents: AgentView[], input: string): AgentView[] {
  return filterBySearchExpression(agents, input, agentSearchFields).items;
}

export function termMatchTitle(term: SearchToken, agents: AgentView[]): string {
  if (term.kind !== "term") {
    return term.raw;
  }
  const expression: SearchExpression = {
    type: "term",
    namespace: term.namespace,
    raw: term.raw,
    value: term.value,
  };
  const matches = agents.filter((agent) => evaluateSearchExpression(expression, agentSearchFields(agent)));
  const labels = matches.map((agent) => `${agent.id} (${agent.display_name})`).join(", ");
  return `${matches.length} match${matches.length === 1 ? "" : "es"}${labels ? `: ${labels}` : ""}`;
}

export function addSelectorToExpression(expression: string, selector: string): string {
  const trimmedExpression = expression.trim();
  const trimmedSelector = selector.trim();
  if (!trimmedSelector) {
    return trimmedExpression;
  }
  if (!trimmedExpression) {
    return trimmedSelector;
  }
  const parsed = tokenizeSearchExpression(trimmedExpression);
  if (parsed.tokens.some((token) => token.kind === "term" && token.raw === trimmedSelector)) {
    return trimmedExpression;
  }
  return `${trimmedExpression} || ${trimmedSelector}`;
}

export function selectorExpressionForClientIds(clientIds: string[]): string {
  return Array.from(new Set(clientIds.map((clientId) => clientId.trim()).filter(Boolean)))
    .map((clientId) => `id:${clientId}`)
    .join(" || ");
}

export function removeTokenFromExpression(expression: string, token: SearchToken): string {
  if (token.kind !== "term") {
    return expression;
  }
  const tokens = tokenizeSearchExpression(expression).tokens;
  const tokenIndex = tokens.findIndex((candidate) => candidate.start === token.start && candidate.end === token.end);
  let removeStart = token.start;
  let removeEnd = token.end;
  const previous = tokenIndex > 0 ? tokens[tokenIndex - 1] : null;
  const next = tokenIndex >= 0 ? tokens[tokenIndex + 1] : null;
  if (next?.kind === "and" || next?.kind === "or") {
    removeEnd = next.end;
  } else if (previous?.kind === "and" || previous?.kind === "or") {
    removeStart = previous.start;
  }
  return cleanExpressionSpacing(`${expression.slice(0, removeStart)} ${expression.slice(removeEnd)}`);
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

function termMatchesFields(term: Extract<SearchExpression, { type: "term" }>, fields: SearchFields): boolean {
  if (term.namespace) {
    const namespaceValues = fields.namespaces?.[term.namespace];
    if (!namespaceValues) {
      return fields.all.some((value) => valueMatches(value, term.raw, true));
    }
    return namespaceValues.some((value) => valueMatches(value, term.value, false));
  }
  return fields.all.some((value) => valueMatches(value, term.value, true));
}

function valueMatches(value: string, pattern: string, allowContains: boolean): boolean {
  const normalizedValue = value.toLocaleLowerCase();
  const normalizedPattern = pattern.toLocaleLowerCase();
  if (normalizedPattern.includes("*") || normalizedPattern.includes("?")) {
    return globMatches(normalizedValue, normalizedPattern);
  }
  return allowContains ? normalizedValue.includes(normalizedPattern) : normalizedValue === normalizedPattern;
}

function globMatches(value: string, pattern: string): boolean {
  let valueIndex = 0;
  let patternIndex = 0;
  let starIndex = -1;
  let starValueIndex = 0;
  while (valueIndex < value.length) {
    if (patternIndex < pattern.length && (pattern[patternIndex] === "?" || pattern[patternIndex] === value[valueIndex])) {
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
    let expression = this.parsePrimary();
    while (!this.error) {
      if (this.peek()?.kind === "and") {
        this.position += 1;
        const right = this.parsePrimary();
        if (!expression || !right) {
          this.error = "Operator is missing an operand";
          return null;
        }
        expression = { type: "and", left: expression, right };
        continue;
      }
      if (this.nextStartsPrimary()) {
        const right = this.parsePrimary();
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

  private parsePrimary(): SearchExpression | null {
    const token = this.advance();
    if (!token) {
      this.error = "Expression is incomplete";
      return null;
    }
    if (token.kind === "term") {
      return { type: "term", namespace: token.namespace, raw: token.raw, value: token.value };
    }
    if (token.kind === "left_paren") {
      const expression = this.parseOr();
      if (this.advance()?.kind !== "right_paren") {
        this.error = "Missing closing parenthesis";
        return null;
      }
      return expression;
    }
    this.error = token.kind === "right_paren" ? "Unexpected closing parenthesis" : "Operator is missing a left operand";
    return null;
  }

  private nextStartsPrimary(): boolean {
    const next = this.peek()?.kind;
    return next === "term" || next === "left_paren";
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
