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

    const computed = getComputedStyle(textarea);
    // Mirror only text layout. Browser wrapping then determines each logical
    // line's height without measuring or guessing character widths per line.
    for (const property of [
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
    ]) {
      mirror.style.setProperty(property, computed.getPropertyValue(property));
    }
    gutter.style.fontSize = computed.fontSize;
    const gutterWidth = gutter.getBoundingClientRect().width;
    // Existing form fields use symmetric horizontal padding. Keep that text
    // inset after reserving the gutter, leaving all other native styles intact.
    const inset = parseFloat(computed.paddingRight);
    textarea.style.paddingLeft = `${gutterWidth + inset}px`;
    gutter.style.top = `${textarea.offsetTop + parseFloat(computed.borderTopWidth)}px`;
    gutter.style.left = `${textarea.offsetLeft + parseFloat(computed.borderLeftWidth)}px`;
    gutter.style.height = `${textarea.clientHeight}px`;
    gutter.style.borderTopLeftRadius = computed.borderTopLeftRadius;
    gutter.style.borderBottomLeftRadius = computed.borderBottomLeftRadius;
    // clientWidth rounds to an integer. Retain the CSS width's fractional part
    // or a word at a wrapping boundary can shift subsequent gutter numbers.
    const viewportWidth =
      parseFloat(computed.width) - textarea.offsetWidth + textarea.clientWidth;
    mirror.style.width = `${Math.max(0, viewportWidth - gutterWidth - 2 * inset)}px`;
    mirror.style.top = computed.paddingTop;
    syncScroll();
  }, [syncScroll]);

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
