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
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  quoteSelectorValue,
  removeTokenFromExpression,
  type SearchToken,
  termMatchTitle,
  tokenizeSearchExpression,
} from "../searchExpression";
import { clientIdSuffix, formatVpsName, type VpsNameDisplayMode } from "../utils";

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
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const editorRef = useRef<HTMLDivElement | null>(null);
  const wheelHandlerRef = useRef<((event: globalThis.WheelEvent) => void) | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [autocompleteOpen, setAutocompleteOpen] = useState(false);
  const [focused, setFocused] = useState(false);
  const [caretIndex, setCaretIndex] = useState(value.length);
  const parsed = parseSearchExpression(value);
  const displayTokens = useMemo(() => tokenizeForDisplay(value), [value]);
  const hasTokens = displayTokens.some((token) => token.kind === "term");
  const matchedAgents = agents && !parsed.error ? agentsMatchingExpression(agents, value) : [];
  const completion = useMemo(
    () => buildCompletion(value, caretIndex, agents ?? [], suggestions ?? [], vpsNameDisplayMode, Boolean(agents?.length)),
    [agents, caretIndex, suggestions, value, vpsNameDisplayMode],
  );
  const matchTitle = agents && !parsed.error ? agentListTitle(matchedAgents) : undefined;

  useLayoutEffect(() => {
    if (!focused || !editorRef.current) {
      return;
    }
    if (cleanEditorText(editorRef.current.textContent ?? "") !== value) {
      editorRef.current.textContent = value;
    }
    if (document.activeElement !== editorRef.current) {
      editorRef.current.focus({ preventScroll: true });
    }
    const nextCaretIndex = Math.min(caretIndex, editorTextLength(editorRef.current));
    setCaretOffset(editorRef.current, nextCaretIndex);
    scrollCaretIndexIntoView(editorRef.current, nextCaretIndex);
  }, [caretIndex, focused, value]);

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
    if (editorRef.current && wheelHandlerRef.current) {
      editorRef.current.removeEventListener("wheel", wheelHandlerRef.current, true);
      wheelHandlerRef.current = null;
    }
    editorRef.current = element;
    assignRef(inputRef, element);
    if (element) {
      const wheelHandler = (event: globalThis.WheelEvent) => {
        if (scrollEditorByWheelDelta(element, event.deltaX, event.deltaY)) {
          event.preventDefault();
        }
      };
      element.addEventListener("wheel", wheelHandler, { capture: true, passive: false });
      wheelHandlerRef.current = wheelHandler;
    }
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
    scrollCaretIndexIntoView(editorRef.current, offset);
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
        const nextCaretIndex = Math.min(getCaretOffset(editorRef.current), editorTextLength(editorRef.current));
        setCaretIndex(nextCaretIndex);
        scrollCaretIndexIntoView(editorRef.current, nextCaretIndex);
      }
    }, 0);
  }

  function applySuggestion(suggestion: CompletionOption) {
    const nextValue = applyCompletion(value, completion, suggestion);
    onChange(nextValue);
    setCaretIndex(completion.start + suggestion.value.length + 1);
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
              const nextCaretIndex = Math.min(getCaretOffset(editorRef.current), editorTextLength(editorRef.current));
              setCaretIndex(nextCaretIndex);
              scrollCaretIndexIntoView(editorRef.current, nextCaretIndex);
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
            ? null
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
              key={`${suggestion.value}:${suggestion.label}`}
              onMouseDown={(event) => {
                event.preventDefault();
                applySuggestion(suggestion);
              }}
              role="option"
              type="button"
            >
              <span>{suggestion.label}</span>
              {suggestion.detail ? <small>{suggestion.detail}</small> : null}
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
  filtered: CompletionOption[];
  fragment: string;
  start: number;
};

type CompletionOption = {
  detail?: string;
  label: string;
  matchText: string;
  namespace: string | null;
  selectorValue: string;
  value: string;
};

const ALWAYS_VISIBLE_VPS_SELECTOR_SUGGESTIONS = ["*", "id:*"];
const COMMON_VPS_STATUSES = [
  "online",
  "stale",
  "offline",
  "disconnected",
  "never",
  "revoked",
];
const COMMON_VPS_SELECTOR_SUGGESTIONS = [
  "untagged",
  ...COMMON_VPS_STATUSES.flatMap((status) => [
    `status:${status}`,
    `vps.status:${status}`,
  ]),
];

export function buildAgentSelectorSuggestionValues(agents: AgentView[]): string[] {
  const observedValues = new Set<string>();
  for (const value of ALWAYS_VISIBLE_VPS_SELECTOR_SUGGESTIONS) {
    observedValues.add(value);
  }
  if (agents.some((agent) => agent.tags.length === 0)) {
    observedValues.add("untagged");
  }
  for (const agent of agents) {
    if (agent.status) {
      observedValues.add(`status:${agent.status}`);
      observedValues.add(`vps.status:${agent.status}`);
    }
    for (const tag of agent.tags) {
      const lowerTag = tag.toLocaleLowerCase();
      observedValues.add(`tag:${quoteSelectorValue(tag)}`);
      observedValues.add(`vps.tag:${quoteSelectorValue(tag)}`);
      observedValues.add(`vps.tags:${quoteSelectorValue(tag)}`);
      if (isSimpleNamespacedTag(tag)) {
        observedValues.add(tag);
      }
      if (lowerTag.startsWith("provider:")) {
        const value = tag.slice("provider:".length);
        observedValues.add(`provider:${quoteSelectorValue(value)}`);
        observedValues.add(`vps.provider:${quoteSelectorValue(value)}`);
      }
      if (lowerTag.startsWith("country:")) {
        const value = tag.slice("country:".length);
        observedValues.add(`country:${quoteSelectorValue(value)}`);
        observedValues.add(`region:${quoteSelectorValue(value)}`);
        observedValues.add(`vps.country:${quoteSelectorValue(value)}`);
        observedValues.add(`vps.region:${quoteSelectorValue(value)}`);
      }
    }
  }
  return uniqueParseableSuggestions([
    ...Array.from(observedValues).sort((left, right) => left.localeCompare(right)),
    ...[...COMMON_VPS_SELECTOR_SUGGESTIONS].sort((left, right) => left.localeCompare(right)),
  ]);
}

function buildAgentSelectorSuggestions(agents: AgentView[]): CompletionOption[] {
  return buildAgentSelectorSuggestionValues(agents).map((value) => staticCompletionOption(value));
}

function buildCompletion(
  value: string,
  caretIndex: number,
  agents: AgentView[],
  suggestions: string[],
  mode: VpsNameDisplayMode,
  agentSuggestionsEnabled: boolean,
): CompletionState {
  const boundedCaret = Math.max(0, Math.min(caretIndex, value.length));
  const { fragment, start } = completionFragment(value, boundedCaret);
  const normalized = fragment.toLocaleLowerCase();
  const namespaceSeparator = normalized.indexOf(":");
  const allSuggestions = uniqueCompletionOptions([
    ...(agentSuggestionsEnabled ? buildAgentCompletionOptions(agents, fragment, mode) : []),
    ...(agentSuggestionsEnabled ? buildAgentSelectorSuggestions(agents) : []),
    ...suggestions.map((suggestion) => staticCompletionOption(suggestion)),
  ]);
  return {
    end: boundedCaret,
    filtered: normalized
      ? allSuggestions.filter((suggestion) => suggestionMatchesFragment(suggestion, normalized, namespaceSeparator))
      : allSuggestions.slice(0, 8),
    fragment,
    start,
  };
}

function applyCompletion(value: string, completion: CompletionState, suggestion: CompletionOption): string {
  return cleanEditorText(`${value.slice(0, completion.start)}${suggestion.value} ${value.slice(completion.end)}`);
}

function suggestionMatchesFragment(
  suggestion: CompletionOption,
  normalizedFragment: string,
  namespaceSeparator: number,
): boolean {
  if (namespaceSeparator < 0) {
    return suggestion.matchText.includes(normalizedFragment);
  }
  const namespace = normalizedFragment.slice(0, namespaceSeparator);
  const valueFragment = unquoteLeadingFragment(normalizedFragment.slice(namespaceSeparator + 1));
  if (!suggestion.namespace || suggestion.namespace !== namespace) {
    return false;
  }
  return valueFragment ? suggestion.selectorValue.includes(valueFragment) || suggestion.matchText.includes(valueFragment) : true;
}

function buildAgentCompletionOptions(
  agents: AgentView[],
  fragment: string,
  mode: VpsNameDisplayMode,
): CompletionOption[] {
  const normalized = fragment.trim().toLocaleLowerCase();
  const separator = normalized.indexOf(":");
  const namespace = separator >= 0 ? normalized.slice(0, separator) : null;
  const valueFragment = separator >= 0 ? unquoteLeadingFragment(normalized.slice(separator + 1)) : normalized;
  return agents
    .map((agent) => agentCompletionOption(agent, namespace, valueFragment, mode))
    .filter((option): option is CompletionOption => Boolean(option))
    .sort((left, right) => left.label.localeCompare(right.label) || left.value.localeCompare(right.value));
}

function agentCompletionOption(
  agent: AgentView,
  namespace: string | null,
  normalizedFragment: string,
  mode: VpsNameDisplayMode,
): CompletionOption | null {
  const displayName = agent.display_name.trim();
  const suffix = clientIdSuffix(agent.id) ?? "";
  const label = formatVpsName(agent, mode);
  const idMatchText = `${agent.id} ${suffix}`.toLocaleLowerCase();
  const nameMatchText = `${displayName} ${label}`.toLocaleLowerCase();
  if (namespace === "id") {
    if (normalizedFragment && !idMatchText.includes(normalizedFragment)) {
      return null;
    }
    return completionOption(`id:${agent.id}`, label, agentDetail(agent, "ID"), `${idMatchText} ${nameMatchText}`);
  }
  if (namespace === "name") {
    if (!displayName || (normalizedFragment && !nameMatchText.includes(normalizedFragment))) {
      return null;
    }
    return completionOption(`name:${quoteSelectorValue(displayName)}`, label, agentDetail(agent, "Name"), `${nameMatchText} ${idMatchText}`);
  }
  if (namespace) {
    return null;
  }
  const nameMatched = Boolean(displayName) && (!normalizedFragment || nameMatchText.includes(normalizedFragment));
  const idMatched = !normalizedFragment || idMatchText.includes(normalizedFragment);
  if (!nameMatched && !idMatched) {
    return null;
  }
  const useId = idMatched && (!nameMatched || agent.id.toLocaleLowerCase().startsWith(normalizedFragment) || suffix.toLocaleLowerCase() === normalizedFragment);
  const selector = useId ? `id:${agent.id}` : `name:${quoteSelectorValue(displayName)}`;
  return completionOption(selector, label, agentDetail(agent, useId ? "ID" : "Name"), `${nameMatchText} ${idMatchText}`);
}

function staticCompletionOption(value: string): CompletionOption {
  return completionOption(value, value, undefined, value);
}

function completionOption(value: string, label: string, detail: string | undefined, matchText: string): CompletionOption {
  const separator = value.indexOf(":");
  const selectorValue = separator >= 0 ? unquoteSelectorValue(value.slice(separator + 1)) : value;
  return {
    detail: detail ?? (label === value ? undefined : value),
    label,
    matchText: `${matchText} ${value}`.toLocaleLowerCase(),
    namespace: separator > 0 ? value.slice(0, separator).toLocaleLowerCase() : null,
    selectorValue: selectorValue.toLocaleLowerCase(),
    value,
  };
}

function uniqueCompletionOptions(options: CompletionOption[]): CompletionOption[] {
  const seen = new Set<string>();
  return options.filter((option) => {
    const key = option.value.toLocaleLowerCase();
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
    return true;
  });
}

function uniqueParseableSuggestions(values: string[]): string[] {
  const seen = new Set<string>();
  return values.filter((value) => {
    const key = value.toLocaleLowerCase();
    if (seen.has(key)) {
      return false;
    }
    if (parseSearchExpression(value).error) {
      return false;
    }
    seen.add(key);
    return true;
  });
}

function isSimpleNamespacedTag(tag: string): boolean {
  return /^[^\s()[\],=!<>|&~"']+:[^\s()[\],=!<>|&~"']+$/.test(tag);
}

function agentDetail(agent: AgentView, source: "ID" | "Name"): string {
  return `${source} · ${agent.id}${agent.status ? ` · ${agent.status}` : ""}`;
}

function unquoteSelectorValue(value: string): string {
  const trimmed = value.trim();
  if ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'"))) {
    return trimmed.slice(1, -1).replace(/\\(["'\\])/g, "$1");
  }
  return trimmed;
}

function unquoteLeadingFragment(value: string): string {
  return value.replace(/^["']/, "");
}

function completionFragment(value: string, caretIndex: number): { fragment: string; start: number } {
  let start = 0;
  let quote: string | null = null;
  let escaped = false;
  for (let index = 0; index < caretIndex; index += 1) {
    const char = value[index];
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === quote) {
        quote = null;
      }
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }
    if (/[\s()&|]/.test(char)) {
      start = index + 1;
    }
  }
  return { fragment: value.slice(start, caretIndex), start };
}

function agentListTitle(agents: AgentView[]): string {
  if (agents.length === 0) {
    return "0 matches";
  }
  return agents.map((agent) => `${agent.id} (${agent.display_name})`).join(", ");
}

function tokenizeForDisplay(input: string): DisplayToken[] {
  const parsed = tokenizeSearchExpression(input);
  if (!parsed.error) {
    return parsed.tokens;
  }
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

function scrollCaretIndexIntoView(editor: HTMLElement, caretIndex: number) {
  const maxScrollLeft = editor.scrollWidth - editor.clientWidth;
  if (maxScrollLeft <= 1) {
    editor.scrollLeft = 0;
    return;
  }
  const caretX = caretInlineOffset(editor, caretIndex);
  const leftGuard = editor.scrollLeft + 8;
  const rightGuard = editor.scrollLeft + editor.clientWidth - 12;
  if (caretX > rightGuard) {
    editor.scrollLeft = Math.min(maxScrollLeft, caretX - editor.clientWidth + 18);
  } else if (caretX < leftGuard) {
    editor.scrollLeft = Math.max(0, caretX - 18);
  }
}

function scrollEditorByWheelDelta(editor: HTMLElement, deltaX: number, deltaY: number): boolean {
  const maxScrollLeft = editor.scrollWidth - editor.clientWidth;
  if (maxScrollLeft <= 1) {
    return false;
  }
  const delta = Math.abs(deltaX) > Math.abs(deltaY) ? deltaX : deltaY;
  if (!delta) {
    return false;
  }
  const previousScrollLeft = editor.scrollLeft;
  editor.scrollLeft = Math.max(0, Math.min(maxScrollLeft, previousScrollLeft + delta));
  return editor.scrollLeft !== previousScrollLeft;
}

function caretInlineOffset(editor: HTMLElement, caretIndex: number): number {
  const text = cleanEditorText(editor.textContent ?? "");
  if (!text) {
    return 0;
  }
  const targetOffset = Math.max(0, Math.min(caretIndex, text.length));
  const textNode = firstTextNode(editor);
  if (!textNode) {
    return Math.round((targetOffset / text.length) * editor.scrollWidth);
  }
  const range = document.createRange();
  range.setStart(textNode, 0);
  range.setEnd(textNode, Math.min(targetOffset, textNode.textContent?.length ?? 0));
  const rect = range.getBoundingClientRect();
  const editorRect = editor.getBoundingClientRect();
  range.detach();
  if (rect.width > 0 || targetOffset > 0) {
    return Math.round(rect.right - editorRect.left + editor.scrollLeft);
  }
  return Math.round((targetOffset / text.length) * editor.scrollWidth);
}

function firstTextNode(root: HTMLElement): Text | null {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const node = walker.nextNode();
  return node instanceof Text ? node : null;
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
