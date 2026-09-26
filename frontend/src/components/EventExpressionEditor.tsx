import { useId, useRef, useState, type ReactNode } from "react";
import type { EditorView } from "codemirror";
import { Maximize2, Minimize2 } from "lucide-react";
import { parseSearchExpression } from "../searchExpression";
import { ExpressionCodeEditor } from "./ExpressionCodeEditor";

/** Multiline event authoring; the existing expression grammar remains the owner. */
export function EventExpressionEditor({
  ariaLabel,
  value,
  onChange,
  suggestions,
  descriptions,
  validationError,
}: {
  ariaLabel: string;
  value: string;
  onChange: (value: string) => void;
  suggestions: readonly string[];
  descriptions: ReactNode;
  validationError?: string | null;
}) {
  const viewRef = useRef<EditorView | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [snippet, setSnippet] = useState("");
  const helpId = useId();
  const error = validationError ?? parseSearchExpression(value).error;

  function insertSnippet() {
    const view = viewRef.current;
    if (!view || !snippet) return;
    // Replace only the explicit selection; retain the rest of the operator's draft.
    view.dispatch(view.state.replaceSelection(snippet));
    view.focus();
    setSnippet("");
  }

  return (
    <div className={`eventExpressionEditor${expanded ? " expanded" : ""}`}>
      <div className="eventExpressionToolbar">
        <select
          aria-label={`${ariaLabel} snippet`}
          value={snippet}
          onChange={(event) => setSnippet(event.target.value)}
        >
          <option value="">Insert event or filter…</option>
          {suggestions.map((suggestion) => (
            <option key={suggestion} value={suggestion}>{suggestion}</option>
          ))}
        </select>
        <button
          className="secondaryAction compactAction"
          disabled={!snippet}
          onClick={insertSnippet}
          type="button"
        >
          Insert
        </button>
        <button
          className="secondaryAction compactAction"
          aria-expanded={expanded}
          onClick={() => setExpanded(!expanded)}
          type="button"
        >
          {expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
          {expanded ? "Compact" : "Expand"}
        </button>
      </div>
      <ExpressionCodeEditor
        ariaLabel={ariaLabel}
        describedBy={helpId}
        editorRef={viewRef}
        onChange={onChange}
        value={value}
      />
      <small className={error ? "status warn" : "mutedText"} id={helpId}>
        {error ?? "Enter inserts a new line · && means AND · || means OR · # line comments · /* block comments */"}
      </small>
      <details className="eventExpressionDescriptions">
        <summary>Descriptions</summary>
        <p>Comments act as whitespace: <code># comment</code> runs to the end of the line; <code>/* comment */</code> can span lines and does not nest. Comment markers inside quoted values or regexes remain literal. An expression must contain a condition, not just comments.</p>
        {descriptions}
      </details>
    </div>
  );
}
