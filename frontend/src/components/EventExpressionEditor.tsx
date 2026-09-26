import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { basicSetup, EditorView } from "codemirror";
import { Maximize2, Minimize2 } from "lucide-react";
import { parseSearchExpression } from "../searchExpression";

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
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const [expanded, setExpanded] = useState(false);
  const [snippet, setSnippet] = useState("");
  const helpId = useId();
  const error = validationError ?? parseSearchExpression(value).error;

  useEffect(() => {
    if (!containerRef.current) return;
    const view = new EditorView({
      doc: value,
      parent: containerRef.current,
      extensions: [
        basicSetup,
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({
          "aria-label": ariaLabel,
          "aria-multiline": "true",
          "aria-describedby": helpId,
        }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChangeRef.current(update.state.doc.toString());
          }
        }),
      ],
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || view.state.doc.toString() === value) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value },
    });
  }, [value]);

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
      <div className="eventExpressionCodeMirror" ref={containerRef} />
      <small className={error ? "status warn" : "mutedText"} id={helpId}>
        {error ?? "Enter inserts a new line · && means AND · || means OR · parentheses group conditions"}
      </small>
      <details className="eventExpressionDescriptions">
        <summary>Descriptions</summary>
        {descriptions}
      </details>
    </div>
  );
}
