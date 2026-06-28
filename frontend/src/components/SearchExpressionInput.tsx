import { Search, X } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type ClipboardEvent,
  type KeyboardEvent,
  type MutableRefObject,
  type MouseEvent,
  type Ref,
  type SyntheticEvent,
  type WheelEvent,
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
  const editorRef = useRef<HTMLInputElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const previewRef = useRef<HTMLDivElement | null>(null);
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

  useEffect(() => {
    setCaretIndex((current) => Math.min(current, value.length));
  }, [value]);

  function bindEditor(element: HTMLInputElement | null) {
    editorRef.current = element;
    assignRef(inputRef, element);
  }

  function focusInputAt(nextCaretIndex: number) {
    window.setTimeout(() => {
      const editor = editorRef.current;
      if (!editor) {
        return;
      }
      const boundedCaret = Math.max(0, Math.min(nextCaretIndex, editor.value.length));
      editor.focus({ preventScroll: true });
      editor.setSelectionRange(boundedCaret, boundedCaret);
      scrollCaretIndexIntoView(editor, boundedCaret);
      setCaretIndex(boundedCaret);
    }, 0);
  }

  function syncInputCaret(editor: HTMLInputElement) {
    const nextCaretIndex = Math.min(editor.selectionStart ?? editor.value.length, editor.value.length);
    setCaretIndex(nextCaretIndex);
    scrollCaretIndexIntoView(editor, nextCaretIndex);
  }

  function commitInputValue(event: ChangeEvent<HTMLInputElement>) {
    const editor = event.currentTarget;
    const nextValue = cleanEditorText(editor.value);
    const nextCaretIndex = Math.min(editor.selectionStart ?? nextValue.length, nextValue.length);
    setCaretIndex(nextCaretIndex);
    setAutocompleteOpen(true);
    setFocused(true);
    if (nextValue !== value) {
      onChange(nextValue);
    }
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (!event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
      if (event.key === "Home") {
        event.preventDefault();
        focusInputAt(0);
        return;
      }
      if (event.key === "End") {
        event.preventDefault();
        focusInputAt(event.currentTarget.value.length);
        return;
      }
    }
    if (event.key === "Enter") {
      event.preventDefault();
      if (completion.filtered.length > 0 && completion.fragment.trim()) {
        applySuggestion(completion.filtered[0]);
      } else {
        const nextValue = value.trim();
        onChange(nextValue);
        setCaretIndex(nextValue.length);
        focusInputAt(nextValue.length);
      }
      return;
    }
    if (event.key === "Escape") {
      setAutocompleteOpen(false);
      setFocused(false);
      editorRef.current?.blur();
    }
  }

  function handlePaste(event: ClipboardEvent<HTMLInputElement>) {
    event.preventDefault();
    const editor = event.currentTarget;
    const start = editor.selectionStart ?? value.length;
    const end = editor.selectionEnd ?? start;
    const pastedText = cleanEditorText(event.clipboardData.getData("text/plain"));
    const nextValue = cleanEditorText(`${value.slice(0, start)}${pastedText}${value.slice(end)}`);
    const nextCaretIndex = Math.min(start + pastedText.length, nextValue.length);
    setCaretIndex(nextCaretIndex);
    setAutocompleteOpen(true);
    setFocused(true);
    if (nextValue !== value) {
      onChange(nextValue);
    }
    focusInputAt(nextCaretIndex);
  }

  function handlePointerUpdate(event: MouseEvent<HTMLInputElement>) {
    const editor = event.currentTarget;
    window.setTimeout(() => syncInputCaret(editor), 0);
  }

  function handleSelectionUpdate(event: SyntheticEvent<HTMLInputElement>) {
    syncInputCaret(event.currentTarget);
  }

  function handleWheel(event: WheelEvent<HTMLInputElement>) {
    if (scrollExpressionViewsByWheelDelta(event.deltaX, event.deltaY)) {
      event.preventDefault();
    }
  }

  function handleContainerWheel(event: WheelEvent<HTMLDivElement>) {
    if (event.defaultPrevented) {
      return;
    }
    if (scrollExpressionViewsByWheelDelta(event.deltaX, event.deltaY)) {
      event.preventDefault();
    }
  }

  function scrollExpressionViewsByWheelDelta(deltaX: number, deltaY: number): boolean {
    const editor = editorRef.current;
    const preview = previewRef.current;
    const editorScrolled = editor
      ? scrollEditorByWheelDelta(editor, deltaX, deltaY)
      : false;
    const previewScrolled = preview
      ? scrollEditorByWheelDelta(preview, deltaX, deltaY)
      : false;
    if (editor && preview) {
      preview.scrollLeft = editor.scrollLeft;
    }
    return editorScrolled || previewScrolled;
  }

  function applySuggestion(suggestion: CompletionOption) {
    const nextValue = applyCompletion(value, completion, suggestion);
    const nextCaretIndex = completion.start + suggestion.value.length;
    onChange(nextValue);
    setCaretIndex(nextCaretIndex);
    setAutocompleteOpen(false);
    focusInputAt(nextCaretIndex);
  }

  return (
    <div
      className={`searchExpressionInput ${className} ${verification} ${focused ? "editing" : "previewing"} ${
        hasTokens ? "hasTokens" : "empty"
      }`.trim()}
      ref={containerRef}
      onWheel={handleContainerWheel}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          event.preventDefault();
          editorRef.current?.focus();
        }
      }}
    >
      <Search size={16} />
      <div className="searchExpressionBody">
        {!focused && hasTokens && (
          <div
            className="searchExpressionPreview"
            ref={previewRef}
            onMouseDown={(event) => {
              if ((event.target as Element).closest("button")) {
                return;
              }
              event.preventDefault();
              editorRef.current?.focus();
            }}
          >
            {displayTokens.map((token, index) => (
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
        )}
        <input
          aria-label={ariaLabel}
          autoCapitalize="none"
          autoComplete="off"
          autoCorrect="off"
          className="searchExpressionEditor"
          id={inputId}
          onBlur={() =>
            window.setTimeout(() => {
              if (document.activeElement !== editorRef.current) {
                setAutocompleteOpen(false);
                setFocused(false);
              }
            }, 120)
          }
          onChange={commitInputValue}
          onClick={handlePointerUpdate}
          onFocus={(event) => {
            setAutocompleteOpen(true);
            setFocused(true);
            syncInputCaret(event.currentTarget);
          }}
          onKeyDown={handleKeyDown}
          onKeyUp={handleSelectionUpdate}
          onMouseUp={handlePointerUpdate}
          onPaste={handlePaste}
          onSelect={handleSelectionUpdate}
          onWheel={handleWheel}
          placeholder={placeholder}
          ref={bindEditor}
          role="searchbox"
          spellCheck={false}
          tabIndex={0}
          type="text"
          value={value}
        />
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
  const suffix = value.slice(completion.end);
  const separator = suffix && !/^\s/.test(suffix) ? " " : "";
  return cleanEditorText(`${value.slice(0, completion.start)}${suggestion.value}${separator}${suffix}`);
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

function scrollCaretIndexIntoView(editor: HTMLInputElement, caretIndex: number) {
  const maxScrollLeft = editor.scrollWidth - editor.clientWidth;
  if (maxScrollLeft <= 1) {
    editor.scrollLeft = 0;
    return;
  }
  if (caretIndex <= 1) {
    editor.scrollLeft = 0;
  } else if (caretIndex >= editor.value.length - 1) {
    editor.scrollLeft = maxScrollLeft;
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
