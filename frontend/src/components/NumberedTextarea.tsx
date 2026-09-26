import {
  forwardRef,
  useCallback,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type TextareaHTMLAttributes,
} from "react";

const textLayoutProperties = [
  "font-family",
  "font-size",
  "font-weight",
  "font-style",
  "line-height",
  "letter-spacing",
  "word-spacing",
  "tab-size",
  "text-indent",
  "text-transform",
  "white-space",
  "overflow-wrap",
  "word-break",
] as const;

const pendingGeometry = new Map<
  HTMLTextAreaElement,
  { gutter: HTMLDivElement; mirror: HTMLDivElement }
>();
let geometryFrame = 0;

function flushGeometry() {
  geometryFrame = 0;
  // Mounting many editors must not alternate a layout write and forced layout
  // read for each field. Keep the same native measurements, grouped by phase.
  const fields = [...pendingGeometry]
    .filter(([textarea]) => textarea.isConnected)
    .map(([textarea, { gutter, mirror }]) => {
      const computed = getComputedStyle(textarea);
      return {
        textarea,
        gutter,
        mirror,
        computed,
        fontSize: computed.fontSize,
        inset: parseFloat(computed.paddingRight),
        typography: textLayoutProperties.map(
          (property) => [property, computed.getPropertyValue(property)] as const,
        ),
      };
    });
  pendingGeometry.clear();

  // Mirror only text layout. Browser wrapping determines logical line heights.
  for (const field of fields) {
    for (const [property, value] of field.typography) {
      field.mirror.style.setProperty(property, value);
    }
    field.gutter.style.fontSize = field.fontSize;
  }
  const widths = fields.map((field) => ({
    ...field,
    gutterWidth: field.gutter.getBoundingClientRect().width,
  }));
  for (const { textarea, gutterWidth, inset } of widths) {
    // Existing fields have symmetric horizontal padding. Preserve that inset.
    textarea.style.paddingLeft = `${gutterWidth + inset}px`;
  }
  const geometry = widths.map((field) => {
    const { textarea, computed, gutterWidth, inset } = field;
    // clientWidth rounds to an integer. Preserve the CSS width's fraction to
    // keep wrapping boundaries identical between the textarea and its mirror.
    const viewportWidth =
      parseFloat(computed.width) - textarea.offsetWidth + textarea.clientWidth;
    return {
      ...field,
      top: textarea.offsetTop + parseFloat(computed.borderTopWidth),
      left: textarea.offsetLeft + parseFloat(computed.borderLeftWidth),
      height: textarea.clientHeight,
      borderTopLeftRadius: computed.borderTopLeftRadius,
      borderBottomLeftRadius: computed.borderBottomLeftRadius,
      mirrorWidth: Math.max(0, viewportWidth - gutterWidth - 2 * inset),
      mirrorTop: computed.paddingTop,
      scrollTop: textarea.scrollTop,
    };
  });
  for (const field of geometry) {
    field.gutter.style.top = `${field.top}px`;
    field.gutter.style.left = `${field.left}px`;
    field.gutter.style.height = `${field.height}px`;
    field.gutter.style.borderTopLeftRadius = field.borderTopLeftRadius;
    field.gutter.style.borderBottomLeftRadius = field.borderBottomLeftRadius;
    field.mirror.style.width = `${field.mirrorWidth}px`;
    field.mirror.style.top = field.mirrorTop;
    field.mirror.style.transform = `translateY(${-field.scrollTop}px)`;
  }
}

/** A native textarea with a gutter for logical lines, not soft-wrapped rows. */
export const NumberedTextarea = forwardRef<
  HTMLTextAreaElement,
  TextareaHTMLAttributes<HTMLTextAreaElement>
>(function NumberedTextarea(
  { value, defaultValue, onChange, onScroll, ...props },
  forwardedRef,
) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const mirrorRef = useRef<HTMLDivElement>(null);
  const [localValue, setLocalValue] = useState(String(defaultValue ?? ""));
  const lines = String(value ?? localValue).replace(/\r\n?/g, "\n").split("\n");

  useImperativeHandle(forwardedRef, () => textareaRef.current!, []);

  const syncScroll = useCallback(() => {
    if (mirrorRef.current && textareaRef.current) {
      mirrorRef.current.style.transform = `translateY(${-textareaRef.current.scrollTop}px)`;
    }
  }, []);

  const syncGeometry = useCallback(() => {
    const textarea = textareaRef.current;
    const gutter = gutterRef.current;
    const mirror = mirrorRef.current;
    if (!textarea || !gutter || !mirror) return;

    pendingGeometry.set(textarea, { gutter, mirror });
    if (!geometryFrame) geometryFrame = requestAnimationFrame(flushGeometry);
  }, []);

  // Also runs after controlled edits are normalized/rejected by their owner.
  useLayoutEffect(syncGeometry);
  useLayoutEffect(() => {
    const textarea = textareaRef.current!;
    const observer = new ResizeObserver(syncGeometry);
    observer.observe(textarea);
    window.addEventListener("resize", syncGeometry);
    let resetFrame = 0;
    const onReset = () => {
      resetFrame = requestAnimationFrame(() => {
        setLocalValue(textarea.value);
        syncGeometry();
      });
    };
    const form = textarea.form;
    form?.addEventListener("reset", onReset);
    return () => {
      pendingGeometry.delete(textarea);
      if (!pendingGeometry.size) {
        cancelAnimationFrame(geometryFrame);
        geometryFrame = 0;
      }
      observer.disconnect();
      window.removeEventListener("resize", syncGeometry);
      form?.removeEventListener("reset", onReset);
      cancelAnimationFrame(resetFrame);
    };
  }, [syncGeometry]);

  return (
    <div
      className="numberedTextarea"
      style={
        { "--textarea-line-digits": String(lines.length).length } as CSSProperties
      }
    >
      <textarea
        {...props}
        ref={textareaRef}
        value={value}
        defaultValue={defaultValue}
        onChange={(event) => {
          setLocalValue(event.currentTarget.value);
          onChange?.(event);
        }}
        onScroll={(event) => {
          syncScroll();
          onScroll?.(event);
        }}
      />
      <div className="numberedTextareaGutter" ref={gutterRef} aria-hidden="true">
        <div className="numberedTextareaMirror" ref={mirrorRef}>
          {lines.map((line, index) => (
            <div className="numberedTextareaLine" key={index}>
              <div className="numberedTextareaNumber">{index + 1}</div>
              {line || "\u200b"}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
});
