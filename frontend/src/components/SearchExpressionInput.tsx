import { Search, X } from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type KeyboardEvent,
  type MutableRefObject,
  type MouseEvent,
  type Ref,
} from "react";
import type { AgentView } from "../types";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  removeTokenFromExpression,
  type SearchToken,
  termMatchTitle,
} from "../searchExpression";

type SearchExpressionInputProps = {
  agents?: AgentView[];
  ariaLabel: string;
  className?: string;
  inputId?: string;
  inputRef?: Ref<HTMLElement>;
  onChange: (value: string) => void;
  placeholder: string;
  showMatchCount?: boolean;
  suggestions?: string[];
  value: string;
  verification?: "checking" | "invalid" | "neutral" | "valid";
  verificationMessage?: string | null;
};

type DisplayToken = SearchToken;

export function SearchExpressionInput({
  agents,
  ariaLabel,
  className = "",
  inputId,
  inputRef,
  onChange,
  placeholder,
  showMatchCount = false,
  suggestions,
  value,
  verification = "neutral",
  verificationMessage,
}: SearchExpressionInputProps) {
  const editorRef = useRef<HTMLDivElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [autocompleteOpen, setAutocompleteOpen] = useState(false);
  const [focused, setFocused] = useState(false);
  const [caretIndex, setCaretIndex] = useState(value.length);
  const parsed = parseSearchExpression(value);
  const displayTokens = useMemo(() => tokenizeForDisplay(value), [value]);
  const hasTokens = displayTokens.some((token) => token.kind === "term");
  const matchedAgents = agents && !parsed.error ? agentsMatchingExpression(agents, value) : [];
  const completion = useMemo(
    () => buildCompletion(value, value.length, suggestions ?? buildAgentSuggestions(agents ?? [])),
    [agents, suggestions, value],
  );
  const matchTitle = agents && !parsed.error ? agentListTitle(matchedAgents) : undefined;

  useLayoutEffect(() => {
    if (!focused || !editorRef.current) {
      return;
    }
    if (document.activeElement !== editorRef.current) {
      editorRef.current.focus({ preventScroll: true });
    }
    setCaretOffset(editorRef.current, Math.min(caretIndex, editorTextLength(editorRef.current)));
  }, [caretIndex, displayTokens, focused, value]);

  useEffect(() => {
    if (!focused && !autocompleteOpen) {
      return;
    }
    function handleDocumentPointerDown(event: PointerEvent) {
      const container = containerRef.current;
      if (!container || !event.target || container.contains(event.target as Node)) {
        return;
      }
      setAutocompleteOpen(false);
      setFocused(false);
    }
    document.addEventListener("pointerdown", handleDocumentPointerDown, true);
    return () => document.removeEventListener("pointerdown", handleDocumentPointerDown, true);
  }, [autocompleteOpen, focused]);

  function bindEditor(element: HTMLDivElement | null) {
    editorRef.current = element;
    assignRef(inputRef, element);
  }

  function prepareEditorForTyping() {
    const editor = editorRef.current;
    if (editor && cleanEditorText(editor.textContent ?? "") !== value) {
      editor.textContent = value;
    }
  }

  function commitEditorText() {
    if (!editorRef.current) {
      return;
    }
    const nextValue = cleanEditorText(editorRef.current.textContent ?? "");
    const offset = Math.min(getCaretOffset(editorRef.current), nextValue.length);
    setCaretIndex(nextValue !== value && nextValue.startsWith(value) ? nextValue.length : offset);
    setAutocompleteOpen(true);
    setFocused(true);
    if (nextValue !== value) {
      onChange(nextValue);
    }
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Enter") {
      event.preventDefault();
      if (completion.filtered.length > 0 && completion.fragment.trim()) {
        applySuggestion(completion.filtered[0]);
      } else {
        onChange(value.trim());
        setCaretIndex(value.trim().length);
      }
      return;
    }
    if (event.key === "Escape") {
      setAutocompleteOpen(false);
      setFocused(false);
      editorRef.current?.blur();
    }
  }

  function handlePaste(event: ClipboardEvent<HTMLDivElement>) {
    event.preventDefault();
    insertPlainText(cleanEditorText(event.clipboardData.getData("text/plain")));
    commitEditorText();
  }

  function handlePointerUpdate(_: MouseEvent<HTMLDivElement>) {
    window.setTimeout(() => {
      if (editorRef.current) {
        setCaretIndex(Math.min(getCaretOffset(editorRef.current), value.length));
      }
    }, 0);
  }

  function applySuggestion(suggestion: string) {
    const nextValue = applyCompletion(value, completion, suggestion);
    onChange(nextValue);
    setCaretIndex(completion.start + suggestion.length + 1);
    window.setTimeout(() => editorRef.current?.focus(), 0);
  }

  return (
    <div
      className={`searchExpressionInput ${className} ${verification} ${focused ? "editing" : "previewing"} ${
        hasTokens ? "hasTokens" : "empty"
      }`.trim()}
      ref={containerRef}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          event.preventDefault();
          editorRef.current?.focus();
        }
      }}
    >
      <Search size={16} />
      <div className="searchExpressionBody">
        <div
          aria-label={ariaLabel}
          className="searchExpressionEditor"
          contentEditable
          data-placeholder={placeholder}
          id={inputId}
          key={value}
          onBlur={() =>
            window.setTimeout(() => {
              if (document.activeElement !== editorRef.current) {
                setAutocompleteOpen(false);
                setFocused(false);
              }
            }, 120)
          }
          onClick={handlePointerUpdate}
          onFocus={() => {
            prepareEditorForTyping();
            setAutocompleteOpen(true);
            setFocused(true);
            if (editorRef.current) {
              setCaretIndex(Math.min(getCaretOffset(editorRef.current), value.length));
            }
          }}
          onInput={commitEditorText}
          onKeyDown={handleKeyDown}
          onKeyUp={() => {
            if (editorRef.current) {
              setCaretIndex(Math.min(getCaretOffset(editorRef.current), value.length));
            }
          }}
          onMouseUp={handlePointerUpdate}
          onPaste={handlePaste}
          ref={bindEditor}
          role="searchbox"
          spellCheck={false}
          suppressContentEditableWarning
          tabIndex={0}
        >
          {focused
            ? value
            : displayTokens.map((token, index) => (
                <TokenFragment
                  agents={agents}
                  expression={value}
                  key={`${token.start}-${token.end}-${token.raw}`}
                  onChange={onChange}
                  token={token}
                  trailingSpace={index < displayTokens.length - 1}
                />
              ))}
        </div>
      </div>
      {(focused || autocompleteOpen) && completion.filtered.length > 0 && completion.fragment.trim() && (
        <div className="searchExpressionAutocomplete" role="listbox">
          {completion.filtered.slice(0, 8).map((suggestion) => (
            <button
              key={suggestion}
              onMouseDown={(event) => {
                event.preventDefault();
                applySuggestion(suggestion);
              }}
              role="option"
              type="button"
            >
              {suggestion}
            </button>
          ))}
        </div>
      )}
      {showMatchCount && agents && (
        <span className={parsed.error ? "searchExpressionMeta errorText" : "searchExpressionMeta"} title={matchTitle}>
          {verificationMessage ?? (parsed.error ? parsed.error : `${matchedAgents.length}/${agents.length}`)}
        </span>
      )}
    </div>
  );
}

function TokenFragment({
  agents,
  expression,
  onChange,
  token,
  trailingSpace,
}: {
  agents?: AgentView[];
  expression: string;
  onChange: (value: string) => void;
  token: DisplayToken;
  trailingSpace: boolean;
}) {
  return (
    <>
      <SearchExpressionTokenView agents={agents} expression={expression} onChange={onChange} token={token} />
      {trailingSpace ? " " : null}
    </>
  );
}

function SearchExpressionTokenView({
  agents,
  expression,
  onChange,
  token,
}: {
  agents?: AgentView[];
  expression: string;
  onChange: (value: string) => void;
  token: DisplayToken;
}) {
  if (token.kind !== "term") {
    return <span className="searchExpressionOperator">{token.raw}</span>;
  }
  return (
    <span className="searchExpressionChip" title={agents ? termMatchTitle(token, agents, expression) : token.raw}>
      <span>{token.raw}</span>
      <button
        aria-label={`Remove ${token.raw}`}
        contentEditable={false}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          onChange(removeTokenFromExpression(expression, token));
        }}
        onMouseDown={(event) => event.preventDefault()}
        type="button"
      >
        <X size={12} />
      </button>
    </span>
  );
}

type CompletionState = {
  end: number;
  filtered: string[];
  fragment: string;
  start: number;
};

function buildAgentSuggestions(agents: AgentView[]): string[] {
  const values = new Set<string>();
  for (const agent of agents) {
    values.add(`id:${agent.id}`);
    if (agent.display_name) {
      values.add(`name:${agent.display_name}`);
    }
    for (const tag of agent.tags) {
      values.add(`tag:${tag}`);
      if (tag.startsWith("provider:")) {
        values.add(tag);
      }
      if (tag.startsWith("country:")) {
        values.add(tag);
      }
    }
  }
  values.add("id:*");
  return Array.from(values).sort((left, right) => left.localeCompare(right));
}

function buildCompletion(value: string, caretIndex: number, suggestions: string[]): CompletionState {
  const boundedCaret = Math.max(0, Math.min(caretIndex, value.length));
  const beforeCaret = value.slice(0, boundedCaret);
  const match = beforeCaret.match(/(?:^|[\s(])([^\s()&|]*)$/);
  const fragment = match?.[1] ?? "";
  const start = match ? boundedCaret - fragment.length : boundedCaret;
  const normalized = fragment.toLocaleLowerCase();
  const namespaceSeparator = normalized.indexOf(":");
  return {
    end: boundedCaret,
    filtered: normalized
      ? suggestions.filter((suggestion) => suggestionMatchesFragment(suggestion, normalized, namespaceSeparator))
      : suggestions.slice(0, 8),
    fragment,
    start,
  };
}

function applyCompletion(value: string, completion: CompletionState, suggestion: string): string {
  return cleanEditorText(`${value.slice(0, completion.start)}${suggestion} ${value.slice(completion.end)}`);
}

function suggestionMatchesFragment(suggestion: string, normalizedFragment: string, namespaceSeparator: number): boolean {
  const normalizedSuggestion = suggestion.toLocaleLowerCase();
  if (namespaceSeparator < 0) {
    return normalizedSuggestion.includes(normalizedFragment);
  }
  const namespace = normalizedFragment.slice(0, namespaceSeparator);
  const valueFragment = normalizedFragment.slice(namespaceSeparator + 1);
  const suggestionSeparator = normalizedSuggestion.indexOf(":");
  if (suggestionSeparator < 0 || normalizedSuggestion.slice(0, suggestionSeparator) !== namespace) {
    return false;
  }
  const suggestionValue = normalizedSuggestion.slice(suggestionSeparator + 1);
  return valueFragment ? suggestionValue.includes(valueFragment) : true;
}

function agentListTitle(agents: AgentView[]): string {
  if (agents.length === 0) {
    return "0 matches";
  }
  return agents.map((agent) => `${agent.id} (${agent.display_name})`).join(", ");
}

function tokenizeForDisplay(input: string): DisplayToken[] {
  const tokens: DisplayToken[] = [];
  let index = 0;
  while (index < input.length) {
    const char = input[index];
    if (/\s/.test(char)) {
      index += 1;
      continue;
    }
    if (char === "(" || char === ")") {
      tokens.push(createDisplayToken(char === "(" ? "left_paren" : "right_paren", input.slice(index, index + 1), index, index + 1));
      index += 1;
      continue;
    }
    if (char === "&" || char === "|") {
      const end = input[index + 1] === char ? index + 2 : index + 1;
      tokens.push(createDisplayToken(char === "&" ? "and" : "or", input.slice(index, end), index, end));
      index = end;
      continue;
    }
    const start = index;
    while (index < input.length && !/[\s()&|]/.test(input[index])) {
      index += 1;
    }
    const raw = input.slice(start, index);
    const lower = raw.toLocaleLowerCase();
    if (lower === "and" || lower === "or") {
      tokens.push(createDisplayToken(lower === "and" ? "and" : "or", raw, start, index));
    } else {
      tokens.push(createTermToken(raw, start, index));
    }
  }
  return tokens;
}

function createDisplayToken(kind: DisplayToken["kind"], raw: string, start: number, end: number): DisplayToken {
  return {
    end,
    kind,
    namespace: null,
    raw,
    start,
    value: raw,
  };
}

function createTermToken(raw: string, start: number, end: number): DisplayToken {
  const separator = raw.indexOf(":");
  return {
    end,
    kind: "term",
    namespace: separator > 0 ? raw.slice(0, separator).toLocaleLowerCase() : null,
    raw,
    start,
    value: separator > 0 ? raw.slice(separator + 1) : raw,
  };
}

function cleanEditorText(text: string): string {
  return text.replace(/\u00a0/g, " ").replace(/\s+/g, " ").trimStart();
}

function editorTextLength(editor: HTMLElement): number {
  return cleanEditorText(editor.textContent ?? "").length;
}

function getCaretOffset(editor: HTMLElement): number {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0) {
    return editorTextLength(editor);
  }
  const range = selection.getRangeAt(0);
  if (!editor.contains(range.endContainer)) {
    return editorTextLength(editor);
  }
  const prefix = range.cloneRange();
  prefix.selectNodeContents(editor);
  prefix.setEnd(range.endContainer, range.endOffset);
  return cleanEditorText(prefix.toString()).length;
}

function setCaretOffset(editor: HTMLElement, offset: number) {
  const targetOffset = Math.max(0, offset);
  const walker = document.createTreeWalker(editor, NodeFilter.SHOW_TEXT);
  let traversed = 0;
  let node = walker.nextNode();
  while (node) {
    const text = node.textContent ?? "";
    const nextTraversed = traversed + text.length;
    if (targetOffset <= nextTraversed) {
      const range = document.createRange();
      range.setStart(node, Math.max(0, Math.min(text.length, targetOffset - traversed)));
      range.collapse(true);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
      return;
    }
    traversed = nextTraversed;
    node = walker.nextNode();
  }
  const range = document.createRange();
  range.selectNodeContents(editor);
  range.collapse(false);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function insertPlainText(text: string) {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0) {
    return;
  }
  const range = selection.getRangeAt(0);
  range.deleteContents();
  range.insertNode(document.createTextNode(text));
  range.collapse(false);
  selection.removeAllRanges();
  selection.addRange(range);
}

function assignRef<T>(ref: Ref<T> | undefined, value: T | null) {
  if (!ref) {
    return;
  }
  if (typeof ref === "function") {
    ref(value);
  } else {
    (ref as MutableRefObject<T | null>).current = value;
  }
}
