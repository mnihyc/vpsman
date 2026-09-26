import { expect, test } from "@playwright/test";
import { EditorState } from "@codemirror/state";
import { expressionHighlighting, metricConditionHighlighting } from "../src/expressionHighlighting";
import {
  evaluateSearchExpression,
  filterBySearchExpression,
  parseSearchExpression,
  quoteSelectorValue,
  tokenizeSearchExpression,
} from "../src/searchExpression";
import {
  buildParseableSearchValueSuggestions,
  searchFieldsForSearchValues,
} from "../src/components/searchSuggestions";
import { scheduleEventExpressionValidationMessage } from "../src/eventExpression";
import { selectorForTemplateScope } from "../src/panels/automation/RunbooksPanel";

function highlights(state: EditorState, field = expressionHighlighting) {
  const result: Array<{ text: string; className: string }> = [];
  state.field(field).between(0, state.doc.length, (from, to, decoration) => {
    result.push({ text: state.sliceDoc(from, to), className: decoration.spec.class });
  });
  return result;
}

test("CodeMirror styles comment, field, operator, number, string and regex spans without rewriting source", () => {
  const source = '# Operator note\nevent.kind = "job.status" /* schedule\ncompletion */ && job.exit_code >= 0 && vps.tag in [/^a#b\\/\\*c$/]';
  const state = EditorState.create({ doc: source, extensions: [expressionHighlighting] });
  expect(state.doc.toString()).toBe(source);
  expect(parseSearchExpression(source).error).toBeNull();
  expect(highlights(state)).toEqual(expect.arrayContaining([
    { text: "# Operator note", className: "expressionSyntaxComment" },
    { text: "event.kind", className: "expressionSyntaxField" },
    { text: '"job.status"', className: "expressionSyntaxString" },
    { text: "/* schedule\ncompletion */", className: "expressionSyntaxComment" },
    { text: ">=", className: "expressionSyntaxOperator" },
    { text: "0", className: "expressionSyntaxNumber" },
    { text: "/^a#b\\/\\*c$/", className: "expressionSyntaxString" },
  ]));
  expect(highlights(state).filter((span) => span.className === "expressionSyntaxComment")).toHaveLength(2);
});

test("metric condition highlighting retains division, byte literals and non-nested comments", () => {
  const source = "# quota ratio\ntraffic.used.total/* numerator */ / traffic.quota.total >= 0.8 && traffic.used.total + 1GiB > 2GiB";
  let state = EditorState.create({ doc: source, extensions: [metricConditionHighlighting] });
  expect(state.doc.toString()).toBe(source);
  const spans = highlights(state, metricConditionHighlighting);
  expect(spans).toEqual(expect.arrayContaining([
    { text: "# quota ratio", className: "expressionSyntaxComment" },
    { text: "/* numerator */", className: "expressionSyntaxComment" },
    { text: "/", className: "expressionSyntaxOperator" },
    { text: "traffic.quota.total", className: "expressionSyntaxField" },
    { text: "0.8", className: "expressionSyntaxNumber" },
    { text: "1GiB", className: "expressionSyntaxNumber" },
  ]));
  expect(spans.some((span) => span.className === "expressionSyntaxString")).toBe(false);
  state = state.update({ changes: { from: state.doc.length, insert: " /* unfinished / #" } }).state;
  expect(highlights(state, metricConditionHighlighting).at(-1)).toEqual({
    text: "/* unfinished / #", className: "expressionSyntaxComment",
  });
});

test("CodeMirror updates incomplete multiline comments while retaining the draft", () => {
  const source = 'alert.triggered /* explain\nalert.resolved';
  let state = EditorState.create({ doc: source, extensions: [expressionHighlighting] });
  expect(parseSearchExpression(source).error).toBe("Unterminated block comment");
  expect(highlights(state).at(-1)).toEqual({
    text: "/* explain\nalert.resolved",
    className: "expressionSyntaxComment",
  });
  const addition = "\n*/ && alert.category:traffic # retain note";
  state = state.update({ changes: { from: state.doc.length, insert: addition } }).state;
  expect(state.doc.toString()).toBe(source + addition);
  expect(scheduleEventExpressionValidationMessage(state.doc.toString())).toBeNull();
  expect(highlights(state).filter((span) => span.className === "expressionSyntaxComment")).toEqual([
    { text: "/* explain\nalert.resolved\n*/", className: "expressionSyntaxComment" },
    { text: "# retain note", className: "expressionSyntaxComment" },
  ]);
  state = state.update({ changes: { from: 0, to: "alert.triggered".length } }).state;
  expect(parseSearchExpression(state.doc.toString()).error).not.toBeNull();
});

test("comment markers stay literal in quoted shorthand values and regexes", () => {
  const source = 'name:"alpha#/*literal*/" && vps.tag in [/^a#b\\/\\*c$/]';
  const state = EditorState.create({ doc: source, extensions: [expressionHighlighting] });
  expect(highlights(state).filter((span) => span.className === "expressionSyntaxComment")).toEqual([]);
  const parsed = parseSearchExpression(source);
  expect(parsed.error).toBeNull();
  expect(evaluateSearchExpression(parsed.expression, {
    all: [],
    fields: { "vps.display_name": ["alpha#/*literal*/"], "vps.tag": ["a#b/*c"] },
  })).toBe(true);
  for (const unfinished of ['name:"alpha#/*literal', 'name = "alpha#/*literal', 'vps.tag in [/^alpha#literal']) {
    const draft = EditorState.create({ doc: unfinished, extensions: [expressionHighlighting] });
    expect(parseSearchExpression(unfinished).error).not.toBeNull();
    expect(highlights(draft).filter((span) => span.className === "expressionSyntaxComment")).toEqual([]);
  }
});

test("comments separate tokens and retain source offsets across CRLF and Unicode", () => {
  const source = "/* 説明 */status/* column */=online# first\r\n&&tag:edge";
  const tokens = tokenizeSearchExpression(source).tokens;
  expect(tokens.map((token) => token.raw)).toEqual(["status", "=", "online", "&&", "tag:edge"]);
  for (const token of tokens) {
    expect(source.slice(token.start, token.end)).toBe(token.raw);
  }
  expect(parseSearchExpression(source).expression).toEqual(parseSearchExpression("status = online && tag:edge").expression);
  expect(parseSearchExpression("/* not nested /* */status:online").error).toBeNull();
});

test("comment-only, malformed and operator-splitting expressions cannot match everything", () => {
  const items = [{ status: "online" }, { status: "offline" }];
  for (const expression of ["# note", "/* note */", "# one\n/* two */", "/*", "status:online /*", "status >/* note */= 2", "status:online &/* note */& tag:edge"]) {
    const parsed = parseSearchExpression(expression);
    expect(parsed.error, expression).not.toBeNull();
    const filtered = filterBySearchExpression(items, expression, (item) => ({ all: [item.status] }));
    expect(filtered.error, expression).not.toBeNull();
    expect(filtered.items, expression).toEqual([]);
  }
  expect(parseSearchExpression(" \r\n ")).toEqual({ expression: null, error: null, tokens: [] });
  expect(scheduleEventExpressionValidationMessage("# no alert lifecycle condition")).not.toBeNull();
});

test("generated literal values quote comment markers instead of broadening a match", () => {
  for (const value of ["alpha#beta", "alpha/*beta*/", "# heading", "x/*unfinished"]) {
    const expression = `name:${quoteSelectorValue(value)}`;
    const parsed = parseSearchExpression(expression);
    expect(parsed.error, expression).toBeNull();
    expect(evaluateSearchExpression(parsed.expression, {
      all: [], fields: { "vps.display_name": [value] },
    })).toBe(true);
    expect(evaluateSearchExpression(parsed.expression, {
      all: [], fields: { "vps.display_name": [value.split(/[#/]/)[0]] },
    })).toBe(false);
  }
  const rows = ["alpha#beta", "alpha/*beta*/", "alpha"];
  const suggestions = buildParseableSearchValueSuggestions(rows, (value) => [value], (value) => searchFieldsForSearchValues([value]));
  expect(suggestions).not.toContain("alpha#beta");
  expect(suggestions).not.toContain("alpha/*beta*/");
  expect(suggestions).toContain("alpha");
});

test("runbook scope selectors keep comment-like scope values literal", () => {
  for (const scope_kind of ["provider", "tag", "client"] as const) {
    const namespace = scope_kind === "client" ? "id" : scope_kind;
    for (const value of ["edge#note", "edge/*note*/", 'edge"#\\note']) {
      for (const scope_value of [value, `${namespace}:${value}`]) {
        const selector = selectorForTemplateScope({ scope_kind, scope_value });
        const parsed = parseSearchExpression(selector);
        expect(parsed.error, selector).toBeNull();
        const fields = (literal: string) => ({
          all: [],
          fields: scope_kind === "client"
            ? { "vps.id": [literal] }
            : { "vps.tag": [scope_kind === "provider" ? `provider:${literal}` : literal] },
        });
        expect(evaluateSearchExpression(parsed.expression, fields(value))).toBe(true);
        expect(evaluateSearchExpression(parsed.expression, fields("edge"))).toBe(false);
      }
    }
  }
  expect(selectorForTemplateScope({ scope_kind: "global", scope_value: null })).toBe("");
  expect(selectorForTemplateScope({ scope_kind: "tag", scope_value: "edge" })).toBe("tag:edge");
});
