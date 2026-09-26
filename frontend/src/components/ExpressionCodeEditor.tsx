import { useEffect, useRef, type MutableRefObject } from "react";
import { Compartment } from "@codemirror/state";
import { placeholder as editorPlaceholder } from "@codemirror/view";
import { basicSetup, EditorView } from "codemirror";
import { expressionHighlighting, metricConditionHighlighting } from "../expressionHighlighting";

/** Shared CodeMirror surface; the field owner retains validation and draft state. */
export function ExpressionCodeEditor({
  ariaLabel,
  className = "",
  describedBy,
  editorRef,
  mode = "search",
  onChange,
  placeholder = "",
  value,
}: {
  ariaLabel: string;
  className?: string;
  describedBy?: string;
  editorRef?: MutableRefObject<EditorView | null>;
  mode?: "search" | "metric";
  onChange: (value: string) => void;
  placeholder?: string;
  value: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const internalViewRef = useRef<EditorView | null>(null);
  const viewRef = editorRef ?? internalViewRef;
  const configurationRef = useRef(new Compartment());
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  function configuration() {
    return [
      mode === "metric" ? metricConditionHighlighting : expressionHighlighting,
      editorPlaceholder(placeholder),
      EditorView.contentAttributes.of({
        "aria-label": ariaLabel,
        "aria-multiline": "true",
        ...(describedBy ? { "aria-describedby": describedBy } : {}),
      }),
    ];
  }

  useEffect(() => {
    if (!containerRef.current) return;
    const view = new EditorView({
      doc: value,
      parent: containerRef.current,
      extensions: [
        basicSetup,
        EditorView.lineWrapping,
        configurationRef.current.of(configuration()),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) onChangeRef.current(update.state.doc.toString());
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
    viewRef.current?.dispatch({
      effects: configurationRef.current.reconfigure(configuration()),
    });
  }, [ariaLabel, describedBy, mode, placeholder]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || view.state.doc.toString() === value) return;
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
  }, [value]);

  return <div className={`eventExpressionCodeMirror ${className}`.trim()} ref={containerRef} />;
}
