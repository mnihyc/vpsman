import { StateField } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet } from "@codemirror/view";
import { readExpressionComment, tokenizeSearchExpression } from "./searchExpression";

/** Reuse the expression lexer, including its string/regex boundaries, for styling. */
function highlightExpression(input: string): DecorationSet {
  const { tokens } = tokenizeSearchExpression(input, true);
  const meaningful = tokens.filter((token) => token.kind !== "comment");
  const decorations: ReturnType<Decoration["range"]>[] = [];
  function mark(from: number, to: number, kind: string) {
    if (to > from) {
      decorations.push(Decoration.mark({ class: `expressionSyntax${kind}` }).range(from, to));
    }
  }
  let meaningfulIndex = 0;
  for (const token of tokens) {
    const next = token.kind === "comment" ? undefined : meaningful[++meaningfulIndex];
    if (token.kind === "comment") {
      mark(token.start, token.end, "Comment");
    } else if (token.kind === "string" || token.kind === "regex") {
      mark(token.start, token.end, "String");
    } else if (token.kind !== "term") {
      mark(token.start, token.end, "Operator");
    } else {
      if (next && ["operator", "in", "not"].includes(next.kind)) {
        mark(token.start, token.end, "Field");
      } else if (token.namespace) {
        const separator = token.start + token.raw.indexOf(":");
        mark(token.start, separator, "Field");
        mark(separator, separator + 1, "Operator");
        mark(separator + 1, token.end, "String");
      } else if (/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(token.raw)) {
        mark(token.start, token.end, "Number");
      } else {
        mark(token.start, token.end, token.raw.includes(".") ? "Field" : "String");
      }
    }
  }
  return Decoration.set(decorations, true);
}

// Expression drafts are small; rescanning on edits keeps multiline comments and
// quoted fragments aligned with validation without maintaining a second grammar.
export const expressionHighlighting = highlightingField(highlightExpression);

/** Metric policy grammar has arithmetic and byte sizes, but no regex literals. */
function highlightMetricCondition(input: string): DecorationSet {
  const decorations: ReturnType<Decoration["range"]>[] = [];
  let index = 0;
  while (index < input.length) {
    const start = index;
    const comment = readExpressionComment(input, index);
    let kind: string;
    if (comment) {
      index = comment.end;
      kind = "Comment";
    } else if (/[A-Za-z0-9_.]/.test(input[index])) {
      index += 1;
      while (index < input.length && /[A-Za-z0-9_.]/.test(input[index])) index += 1;
      const raw = input.slice(start, index);
      kind = /^(and|or|not)$/i.test(raw) ? "Operator"
        : /^[0-9.]/.test(raw) ? "Number" : "Field";
    } else if (/[()+*/!~<>=&|\-]/.test(input[index])) {
      index += 1;
      if (/^(?:&&|\|\||!=|<=|>=|==)$/.test(input.slice(start, index + 1))) index += 1;
      kind = "Operator";
    } else {
      // Unsupported metric characters are left unstyled; server validation
      // remains authoritative and styling never changes the submitted text.
      index += 1;
      continue;
    }
    decorations.push(Decoration.mark({ class: `expressionSyntax${kind}` }).range(start, index));
  }
  return Decoration.set(decorations);
}

export const metricConditionHighlighting = highlightingField(highlightMetricCondition);

function highlightingField(highlight: (input: string) => DecorationSet) {
  return StateField.define<DecorationSet>({
    create: (state) => highlight(state.doc.toString()),
    update: (decorations, transaction) => transaction.docChanged
      ? highlight(transaction.state.doc.toString())
      : decorations,
    provide: (field) => EditorView.decorations.from(field),
  });
}
