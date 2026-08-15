import { Search, X } from "lucide-react";
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ChangeEvent,
  type ClipboardEvent,
  type KeyboardEvent,
  type MutableRefObject,
  type MouseEvent,
  type Ref,
  type SyntheticEvent,
} from "react";
import { createPortal } from "react-dom";
import type { AgentView, VpsRuleValueRecord } from "../types";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  agentsMatchingExpression,
  expressionReferencesVpsRules,
  parseSearchExpression,
  quoteSelectorValue,
  removeTokenFromExpression,
  type SearchToken,
  termMatchTitle,
  tokenizeSearchExpression,
} from "../searchExpression";
import { useVpsRuleSearchContext } from "../vpsRuleSearchContext";
import {
  VPS_RULE_FIELD_DEFINITIONS,
  isVpsRuleKey,
  vpsRuleDefinition,
} from "../vpsRules";
import {
  clientIdSuffix,
  formatVpsName,
  type VpsNameDisplayMode,
} from "../utils";

type SearchExpressionInputProps = {
  agents?: AgentView[];
  ariaLabel: string;
  className?: string;
  disabled?: boolean;
  inputId?: string;
  inputRef?: Ref<HTMLElement>;
  metaDescription?: string | null;
  onChange: (value: string) => void;
  placeholder: string;
  showMatchCount?: boolean;
  showVerificationMessage?: boolean;
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
  disabled = false,
  inputId,
  inputRef,
  metaDescription,
  onChange,
  placeholder,
  showMatchCount = false,
  showVerificationMessage = false,
  suggestions,
  value,
  verification = "neutral",
  verificationMessage,
}: SearchExpressionInputProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const vpsRuleSearch = useVpsRuleSearchContext();
  const editorRef = useRef<HTMLInputElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const previewRef = useRef<HTMLDivElement | null>(null);
  const [autocompleteOpen, setAutocompleteOpen] = useState(false);
  const [autocompleteStyle, setAutocompleteStyle] =
    useState<CSSProperties | null>(null);
  const [activeSuggestionIndex, setActiveSuggestionIndex] = useState(0);
  const [focused, setFocused] = useState(false);
  const [caretIndex, setCaretIndex] = useState(value.length);
  const generatedId = useId().replace(/:/g, "");
  const editorId = inputId ?? `search-expression-${generatedId}`;
  const autocompleteId = `${editorId}-suggestions`;
  const metaId = `${editorId}-status`;
  const parsed = parseSearchExpression(value);
  const referencesVpsRules = expressionReferencesVpsRules(value);
  const ruleEvidenceUnavailable =
    referencesVpsRules && !vpsRuleSearch.available;
  const displayTokens = useMemo(() => tokenizeForDisplay(value), [value]);
  const hasTokens = displayTokens.some((token) => token.kind === "term");
  const completingVpsRules = shouldSuppressVpsRuleCompletionError(
    value,
    caretIndex,
    focused,
    autocompleteOpen,
  );
  const visibleParseError =
    parsed.error && !completingVpsRules ? parsed.error : null;
  const matchedAgents =
    agents && !visibleParseError && !ruleEvidenceUnavailable
      ? agentsMatchingExpression(agents, value, vpsRuleSearch)
      : [];
  const completion = useMemo(
    () =>
      buildCompletion(
        value,
        caretIndex,
        agents ?? [],
        suggestions ?? [],
        vpsNameDisplayMode,
        Boolean(agents),
        vpsRuleSearch.rules,
        vpsRuleSearch.available,
      ),
    [
      agents,
      caretIndex,
      suggestions,
      value,
      vpsNameDisplayMode,
      vpsRuleSearch.available,
      vpsRuleSearch.rules,
    ],
  );
  const visibleSuggestions = completion.filtered.slice(
    0,
    completion.ruleScoped ? 20 : 8,
  );
  const matchTitle =
    agents && !parsed.error ? agentListTitle(matchedAgents) : undefined;
  const matchSummary = visibleParseError
    ? visibleParseError
    : ruleEvidenceUnavailable
      ? "VPS rule data unavailable"
      : completingVpsRules && parsed.error
        ? "Choose a VPS rule"
        : `${matchedAgents.length}/${agents?.length ?? 0}`;
  const metaText =
    visibleParseError || ruleEvidenceUnavailable
      ? matchSummary
      : (verificationMessage ?? matchSummary);
  const metaTitle = visibleParseError
    ? visibleParseError
    : ruleEvidenceUnavailable
      ? "VPS rule data is unavailable or you do not have config:read access"
      : (metaDescription ?? verificationMessage ?? matchTitle ?? matchSummary);
  const showVisibleMeta = Boolean(
    ruleEvidenceUnavailable ||
    (visibleParseError && referencesVpsRules) ||
    (showMatchCount && agents) ||
    (showVerificationMessage && verificationMessage),
  );
  const showMeta = showVisibleMeta || Boolean(verificationMessage);
  const autocompleteVisible =
    !disabled &&
    focused &&
    autocompleteOpen &&
    visibleSuggestions.length > 0 &&
    completion.fragment.trim().length > 0;
  const activeSuggestion = visibleSuggestions[activeSuggestionIndex] ?? null;
  const activeSuggestionId =
    autocompleteVisible && activeSuggestion
      ? `${autocompleteId}-option-${activeSuggestionIndex}`
      : undefined;

  useEffect(() => {
    if (!focused && !autocompleteOpen) {
      return;
    }
    function handleDocumentPointerDown(event: PointerEvent) {
      const container = containerRef.current;
      const menu = menuRef.current;
      if (
        !container ||
        !event.target ||
        container.contains(event.target as Node) ||
        menu?.contains(event.target as Node)
      ) {
        return;
      }
      setAutocompleteOpen(false);
      setFocused(false);
    }
    document.addEventListener("pointerdown", handleDocumentPointerDown, true);
    return () =>
      document.removeEventListener(
        "pointerdown",
        handleDocumentPointerDown,
        true,
      );
  }, [autocompleteOpen, focused]);

  useLayoutEffect(() => {
    if (!autocompleteVisible) {
      setAutocompleteStyle(null);
      return;
    }
    const updateAutocompletePosition = () => {
      const container = containerRef.current;
      if (!container) {
        return;
      }
      const rect = container.getBoundingClientRect();
      const margin = 8;
      const gap = 4;
      const viewportWidth = window.innerWidth;
      const viewportHeight = window.innerHeight;
      if (
        rect.bottom <= margin ||
        rect.top >= viewportHeight - margin ||
        rect.right <= margin ||
        rect.left >= viewportWidth - margin
      ) {
        setAutocompleteOpen(false);
        return;
      }
      const below = Math.max(0, viewportHeight - rect.bottom - gap - margin);
      const above = Math.max(0, rect.top - gap - margin);
      const requiredHeight = Math.min(
        240,
        Math.max(40, visibleSuggestions.length * 42 + 8),
      );
      const openAbove = below < requiredHeight && above > below;
      const available = openAbove ? above : below;
      const maxHeight = Math.min(
        240,
        available,
        Math.max(0, viewportHeight - margin * 2),
      );
      const width = Math.min(
        Math.max(rect.width, 180),
        Math.max(0, viewportWidth - margin * 2),
      );
      const left = Math.min(
        Math.max(rect.left, margin),
        Math.max(margin, viewportWidth - width - margin),
      );
      setAutocompleteStyle({
        ...(openAbove
          ? { bottom: Math.max(viewportHeight - rect.top + gap, margin) }
          : {
              top: Math.min(
                Math.max(rect.bottom + gap, margin),
                Math.max(margin, viewportHeight - maxHeight - margin),
              ),
            }),
        left,
        maxHeight,
        width,
      });
    };
    updateAutocompletePosition();
    window.addEventListener("resize", updateAutocompletePosition);
    window.addEventListener("scroll", updateAutocompletePosition, true);
    return () => {
      window.removeEventListener("resize", updateAutocompletePosition);
      window.removeEventListener("scroll", updateAutocompletePosition, true);
    };
  }, [
    autocompleteVisible,
    completion.filtered.length,
    completion.fragment,
    visibleSuggestions.length,
  ]);

  useEffect(() => {
    setCaretIndex((current) => Math.min(current, value.length));
  }, [value]);

  useEffect(() => {
    setActiveSuggestionIndex((current) =>
      visibleSuggestions.length === 0
        ? 0
        : Math.min(current, visibleSuggestions.length - 1),
    );
  }, [visibleSuggestions.length]);

  useEffect(() => {
    if (!activeSuggestionId) {
      return;
    }
    document
      .getElementById(activeSuggestionId)
      ?.scrollIntoView({ block: "nearest" });
  }, [activeSuggestionId]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }
    const handleWheel = (event: globalThis.WheelEvent) => {
      const editor = editorRef.current;
      const preview = previewRef.current;
      const primaryView = preview ?? editor;
      if (
        !primaryView ||
        !scrollEditorByWheelDelta(primaryView, event.deltaX, event.deltaY)
      ) {
        return;
      }
      if (preview && editor) {
        editor.scrollLeft = Math.min(
          editor.scrollWidth - editor.clientWidth,
          preview.scrollLeft,
        );
      }
      event.preventDefault();
    };
    container.addEventListener("wheel", handleWheel, { passive: false });
    return () => container.removeEventListener("wheel", handleWheel);
  }, []);

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
      const boundedCaret = Math.max(
        0,
        Math.min(nextCaretIndex, editor.value.length),
      );
      editor.focus({ preventScroll: true });
      editor.setSelectionRange(boundedCaret, boundedCaret);
      scrollCaretIndexIntoView(editor, boundedCaret);
      setCaretIndex(boundedCaret);
    }, 0);
  }

  function syncInputCaret(editor: HTMLInputElement) {
    const nextCaretIndex = Math.min(
      editor.selectionStart ?? editor.value.length,
      editor.value.length,
    );
    setCaretIndex(nextCaretIndex);
    scrollCaretIndexIntoView(editor, nextCaretIndex);
  }

  function commitInputValue(event: ChangeEvent<HTMLInputElement>) {
    const editor = event.currentTarget;
    const nextValue = cleanEditorText(editor.value);
    const nextCaretIndex = Math.min(
      editor.selectionStart ?? nextValue.length,
      nextValue.length,
    );
    setCaretIndex(nextCaretIndex);
    setActiveSuggestionIndex(0);
    setAutocompleteOpen(true);
    setFocused(true);
    if (nextValue !== value) {
      onChange(nextValue);
    }
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      (event.key === "ArrowDown" || event.key === "ArrowUp") &&
      visibleSuggestions.length > 0
    ) {
      event.preventDefault();
      setAutocompleteOpen(true);
      setActiveSuggestionIndex((current) =>
        event.key === "ArrowDown"
          ? (current + 1) % visibleSuggestions.length
          : (current - 1 + visibleSuggestions.length) %
            visibleSuggestions.length,
      );
      return;
    }
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
      if (
        autocompleteVisible &&
        activeSuggestion &&
        completion.fragment.trim()
      ) {
        applySuggestion(activeSuggestion);
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
      return;
    }
  }

  function handlePaste(event: ClipboardEvent<HTMLInputElement>) {
    event.preventDefault();
    const editor = event.currentTarget;
    const start = editor.selectionStart ?? value.length;
    const end = editor.selectionEnd ?? start;
    const pastedText = cleanEditorText(
      event.clipboardData.getData("text/plain"),
    );
    const nextValue = cleanEditorText(
      `${value.slice(0, start)}${pastedText}${value.slice(end)}`,
    );
    const nextCaretIndex = Math.min(
      start + pastedText.length,
      nextValue.length,
    );
    setCaretIndex(nextCaretIndex);
    setActiveSuggestionIndex(0);
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

  function applySuggestion(suggestion: CompletionOption) {
    if (suggestion.disabled) {
      return;
    }
    const nextValue = applyCompletion(value, completion, suggestion);
    const nextCaretIndex = completion.start + suggestion.value.length;
    onChange(nextValue);
    setCaretIndex(nextCaretIndex);
    setAutocompleteOpen(Boolean(suggestion.continueCompletion));
    focusInputAt(nextCaretIndex);
  }

  return (
    <div
      aria-disabled={disabled}
      className={`searchExpressionInput ${className} ${verification} ${focused ? "editing" : "previewing"} ${
        hasTokens ? "hasTokens" : "empty"
      } ${disabled ? "disabled" : ""}`.trim()}
      ref={containerRef}
      onMouseDown={(event) => {
        if (disabled) {
          return;
        }
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
              if (disabled) {
                return;
              }
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
                disabled={disabled}
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
          aria-activedescendant={activeSuggestionId}
          aria-autocomplete="list"
          aria-controls={autocompleteId}
          aria-describedby={showMeta ? metaId : undefined}
          aria-errormessage={
            showMeta &&
            (visibleParseError ||
              ruleEvidenceUnavailable ||
              verification === "invalid")
              ? metaId
              : undefined
          }
          aria-expanded={autocompleteVisible}
          aria-invalid={
            Boolean(visibleParseError) ||
            ruleEvidenceUnavailable ||
            verification === "invalid"
          }
          aria-label={ariaLabel}
          autoCapitalize="none"
          autoComplete="off"
          autoCorrect="off"
          className="searchExpressionEditor"
          disabled={disabled}
          id={editorId}
          name={editorId}
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
            setActiveSuggestionIndex(0);
            setFocused(true);
            syncInputCaret(event.currentTarget);
          }}
          onKeyDown={handleKeyDown}
          onKeyUp={handleSelectionUpdate}
          onMouseUp={handlePointerUpdate}
          onPaste={handlePaste}
          onSelect={handleSelectionUpdate}
          placeholder={placeholder}
          ref={bindEditor}
          role="combobox"
          spellCheck={false}
          tabIndex={0}
          type="text"
          value={value}
        />
      </div>
      {autocompleteVisible && autocompleteStyle
        ? createPortal(
            <div
              aria-label={`${ariaLabel} suggestions`}
              className="searchExpressionAutocomplete"
              id={autocompleteId}
              ref={menuRef}
              role="listbox"
              style={autocompleteStyle}
            >
              {visibleSuggestions.map((suggestion, index) => (
                <button
                  aria-selected={activeSuggestionIndex === index}
                  disabled={suggestion.disabled}
                  id={`${autocompleteId}-option-${index}`}
                  key={`${suggestion.value}:${suggestion.label}`}
                  onClick={() => applySuggestion(suggestion)}
                  onMouseDown={(event) => {
                    event.preventDefault();
                  }}
                  onMouseMove={() => setActiveSuggestionIndex(index)}
                  role="option"
                  tabIndex={-1}
                  type="button"
                >
                  <span className="truncateValue">{suggestion.label}</span>
                  {suggestion.detail ? (
                    <small>{suggestion.detail}</small>
                  ) : null}
                </button>
              ))}
            </div>,
            document.body,
          )
        : null}
      {showVisibleMeta ? (
        <span
          className={
            visibleParseError || ruleEvidenceUnavailable
              ? "searchExpressionMeta errorText"
              : "searchExpressionMeta"
          }
          id={metaId}
          aria-label={metaDescription ?? metaText}
          title={metaTitle}
        >
          {metaText}
        </span>
      ) : verificationMessage ? (
        <span className="srOnly" id={metaId}>
          {verificationMessage}
        </span>
      ) : null}
      <span aria-live="polite" className="srOnly">
        {autocompleteVisible
          ? `${visibleSuggestions.length} search suggestion${visibleSuggestions.length === 1 ? "" : "s"}`
          : ""}
      </span>
    </div>
  );
}

function TokenFragment({
  agents,
  disabled,
  expression,
  onChange,
  token,
  trailingSpace,
}: {
  agents?: AgentView[];
  disabled: boolean;
  expression: string;
  onChange: (value: string) => void;
  token: DisplayToken;
  trailingSpace: boolean;
}) {
  return (
    <>
      <SearchExpressionTokenView
        agents={agents}
        disabled={disabled}
        expression={expression}
        onChange={onChange}
        token={token}
      />
      {trailingSpace ? " " : null}
    </>
  );
}

function SearchExpressionTokenView({
  agents,
  disabled,
  expression,
  onChange,
  token,
}: {
  agents?: AgentView[];
  disabled: boolean;
  expression: string;
  onChange: (value: string) => void;
  token: DisplayToken;
}) {
  const vpsRuleSearch = useVpsRuleSearchContext();
  if (token.kind !== "term") {
    return <span className="searchExpressionOperator">{token.raw}</span>;
  }
  return (
    <span
      className="searchExpressionChip"
      title={agents ? termMatchTitle(token, agents, vpsRuleSearch) : token.raw}
    >
      <span>{token.raw}</span>
      <button
        aria-label={`Remove ${token.raw}`}
        contentEditable={false}
        disabled={disabled}
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
  ruleScoped: boolean;
};

type CompletionOption = {
  continueCompletion?: boolean;
  detail?: string;
  disabled?: boolean;
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

export function buildAgentSelectorSuggestionValues(
  agents: AgentView[],
): string[] {
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
    ...Array.from(observedValues).sort((left, right) =>
      left.localeCompare(right),
    ),
    ...[...COMMON_VPS_SELECTOR_SUGGESTIONS].sort((left, right) =>
      left.localeCompare(right),
    ),
  ]).filter((value) => !containsReservedVpsRuleNamespace(value));
}

function containsReservedVpsRuleNamespace(value: string): boolean {
  return /(?:^|[^a-z0-9_])vps\.rules(?=$|[:.])/i.test(value);
}

function buildAgentSelectorSuggestions(
  agents: AgentView[],
): CompletionOption[] {
  return buildAgentSelectorSuggestionValues(agents).map((value) =>
    staticCompletionOption(value),
  );
}

function buildCompletion(
  value: string,
  caretIndex: number,
  agents: AgentView[],
  suggestions: string[],
  mode: VpsNameDisplayMode,
  agentSuggestionsEnabled: boolean,
  vpsRuleValues: readonly VpsRuleValueRecord[],
  vpsRuleEvidenceAvailable: boolean,
): CompletionState {
  const boundedCaret = Math.max(0, Math.min(caretIndex, value.length));
  const ruleScope = vpsRuleCompletionScope(value, boundedCaret);
  if (ruleScope) {
    return {
      end: boundedCaret,
      filtered: buildVpsRuleScopedCompletionOptions(
        ruleScope,
        vpsRuleValues,
        vpsRuleEvidenceAvailable,
      ),
      fragment: ruleScope.fragment,
      ruleScoped: true,
      start: ruleScope.start,
    };
  }
  const { fragment, start } = completionFragment(value, boundedCaret);
  const normalized = fragment.toLocaleLowerCase();
  const namespaceSeparator = normalized.indexOf(":");
  const allSuggestions = uniqueCompletionOptions([
    ...(agentSuggestionsEnabled ? [vpsRulesCategoryOption()] : []),
    ...(agentSuggestionsEnabled
      ? buildAgentCompletionOptions(agents, fragment, mode)
      : []),
    ...(agentSuggestionsEnabled ? buildAgentSelectorSuggestions(agents) : []),
    ...genericCallerSuggestions(suggestions).map((suggestion) =>
      staticCompletionOption(suggestion),
    ),
  ]);
  return {
    end: boundedCaret,
    filtered: normalized
      ? allSuggestions.filter((suggestion) =>
          suggestionMatchesFragment(suggestion, normalized, namespaceSeparator),
        )
      : allSuggestions.slice(0, 8),
    fragment,
    ruleScoped: false,
    start,
  };
}

export function genericCallerSuggestions(suggestions: string[]): string[] {
  return suggestions.filter(
    (suggestion) => !containsReservedVpsRuleNamespace(suggestion),
  );
}

type VpsRuleCompletionScope = {
  fragment: string;
  keyFragment: string;
  negated: boolean;
  operator: "=" | "!=" | "<" | "<=" | ">" | ">=" | null;
  rhs: string;
  start: number;
};

export type VpsRuleCompletionSuggestion = {
  detail?: string;
  disabled?: boolean;
  label: string;
  value: string;
};

export function buildVpsRuleCompletionSuggestions(
  value: string,
  caretIndex: number,
  rows: readonly VpsRuleValueRecord[],
  available = true,
): VpsRuleCompletionSuggestion[] {
  const scope = vpsRuleCompletionScope(value, caretIndex);
  return scope
    ? buildVpsRuleScopedCompletionOptions(scope, rows, available).map(
        ({ detail, disabled, label, value }) => ({
          detail,
          disabled,
          label,
          value,
        }),
      )
    : [];
}

function isActiveVpsRuleCompletion(value: string, caretIndex: number): boolean {
  return Boolean(vpsRuleCompletionScope(value, caretIndex));
}

export function shouldSuppressVpsRuleCompletionError(
  value: string,
  caretIndex: number,
  focused: boolean,
  autocompleteOpen: boolean,
): boolean {
  return (
    focused && autocompleteOpen && isActiveVpsRuleCompletion(value, caretIndex)
  );
}

function vpsRuleCompletionScope(
  value: string,
  caretIndex: number,
): VpsRuleCompletionScope | null {
  const beforeCaret = value.slice(
    0,
    Math.max(0, Math.min(caretIndex, value.length)),
  );
  let start = 0;
  const delimiter = /&&|\|\||\b(?:and|or)\b|\(/gi;
  for (const match of beforeCaret.matchAll(delimiter)) {
    start = (match.index ?? 0) + match[0].length;
  }
  const scopedPrefixStart = lastUnquotedVpsRulePrefixStart(beforeCaret);
  if (scopedPrefixStart !== null) {
    start = Math.max(start, scopedPrefixStart);
  }
  while (/\s/.test(beforeCaret[start] ?? "")) start += 1;
  const fragment = beforeCaret.slice(start);
  const match = fragment.match(
    /^(!\s*)?vps\.rules:([^\s=!<>()[\],&|~]*)(?:\s*(>=|<=|!=|=|>|<)\s*(.*))?$/i,
  );
  if (!match) return null;
  return {
    fragment,
    keyFragment: match[2].toLowerCase(),
    negated: Boolean(match[1]),
    operator: (match[3] as VpsRuleCompletionScope["operator"]) ?? null,
    rhs: match[4] ?? "",
    start,
  };
}

function lastUnquotedVpsRulePrefixStart(value: string): number | null {
  const lower = value.toLowerCase();
  const prefix = "vps.rules:";
  let escaped = false;
  let lastStart: number | null = null;
  let quote: string | null = null;
  for (let index = 0; index <= value.length - prefix.length; index += 1) {
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
    if (!lower.startsWith(prefix, index)) continue;
    if (index > 0 && !/[\s(!&|]/.test(value[index - 1])) continue;
    let candidate = index;
    let previous = index;
    while (previous > 0 && /\s/.test(value[previous - 1])) previous -= 1;
    if (previous > 0 && value[previous - 1] === "!") {
      candidate = previous - 1;
    }
    lastStart = candidate;
    index += prefix.length - 1;
  }
  return lastStart;
}

function vpsRulesCategoryOption(): CompletionOption {
  const category = vpsRulesCategorySuggestion();
  return {
    ...completionOption(
      category.value,
      category.label,
      category.detail,
      "vps rules configured configuration",
    ),
    continueCompletion: true,
  };
}

export function vpsRulesCategorySuggestion(): VpsRuleCompletionSuggestion {
  return {
    detail: "Search directly configured VPS rules",
    label: "VPS rules…",
    value: "vps.rules:",
  };
}

function buildVpsRuleScopedCompletionOptions(
  scope: VpsRuleCompletionScope,
  rows: readonly VpsRuleValueRecord[],
  available: boolean,
): CompletionOption[] {
  if (!available) {
    return [
      {
        ...completionOption(
          scope.fragment,
          "VPS rule data unavailable",
          "Requires complete rule evidence and config:read access",
          "unavailable config read",
        ),
        disabled: true,
      },
    ];
  }
  const key = scope.keyFragment;
  if (!isVpsRuleKey(key)) {
    const prefix = scope.negated ? "!vps.rules:" : "vps.rules:";
    const generic = scope.negated
      ? []
      : [
          ruleCompletionOption(
            "vps.rules:*",
            "Any configured rule",
            "At least one directly configured rule",
          ),
          ruleCompletionOption(
            "vps.rules:traffic.*",
            "Any traffic rule",
            "Configured key wildcard",
          ),
          ruleCompletionOption(
            "vps.rules in [/^traffic\\./]",
            "Traffic rule key regex",
            "Case-sensitive regular expression",
          ),
        ];
    const keys = VPS_RULE_FIELD_DEFINITIONS.filter(
      (definition) =>
        !key ||
        definition.key.includes(key) ||
        definition.label.toLowerCase().includes(key),
    ).map((definition) =>
      ruleCompletionOption(
        `${prefix}${definition.key}`,
        definition.label,
        `${scope.negated ? "Rule is absent" : "Configured rule"} · ${definition.key}`,
      ),
    );
    const normalizedFragment = key.toLowerCase();
    return uniqueCompletionOptions([...generic, ...keys]).filter(
      (option) =>
        !normalizedFragment || option.matchText.includes(normalizedFragment),
    );
  }

  const definition = vpsRuleDefinition(key)!;
  const base = `${scope.negated ? "!" : ""}vps.rules:${key}`;
  const presence = ruleCompletionOption(
    base,
    scope.negated ? `${definition.label} is absent` : definition.label,
    `${scope.negated ? "Rule is absent" : "Configured rule"} · ${key}`,
  );
  if (scope.negated) return [presence];

  const observed = observedRuleValues(rows, key);
  const operator = scope.operator;
  const rhs = unquoteLeadingFragment(scope.rhs.trim().toLowerCase());
  const valueOptions = observed
    .filter(({ value }) =>
      operator && isOrderedRuleOperator(operator) ? value !== "-1" : true,
    )
    .filter(({ value }) => !rhs || value.toLocaleLowerCase().includes(rhs))
    .slice(0, 20)
    .map(({ count, value }) => {
      const selectedOperator = operator ?? "=";
      return ruleCompletionOption(
        `${base} ${selectedOperator} ${quoteSelectorValue(value)}`,
        `${selectedOperator} ${value}`,
        `${count} VPS${count === 1 ? "" : "s"} · canonical configured value`,
      );
    });
  if (operator) {
    const example = orderedRuleExample(key);
    return uniqueCompletionOptions([
      ...valueOptions,
      ...(example && isOrderedRuleOperator(operator)
        ? [
            ruleCompletionOption(
              `${base} ${operator} ${quoteSelectorValue(example)}`,
              `${operator} ${example}`,
              `${definition.label} example`,
            ),
          ]
        : []),
    ]);
  }

  const example = orderedRuleExample(key);
  return uniqueCompletionOptions([
    presence,
    ...valueOptions,
    ...(example && definition.orderedKind
      ? [
          ruleCompletionOption(
            `${base} >= ${quoteSelectorValue(example)}`,
            `At least ${example}`,
            `Typed ${definition.label.toLocaleLowerCase()} comparison`,
          ),
        ]
      : []),
    ruleCompletionOption(
      `${base} = "*"`,
      "Value wildcard…",
      "Match the canonical configured value",
    ),
    ruleCompletionOption(
      `${base} in [/.*/]`,
      "Value regex…",
      "Case-sensitive canonical-value regex",
    ),
  ]);
}

function observedRuleValues(
  rows: readonly VpsRuleValueRecord[],
  key: string,
): Array<{ count: number; value: string }> {
  const counts = new Map<string, number>();
  for (const row of rows) {
    if (row.key === key) {
      counts.set(row.value_raw, (counts.get(row.value_raw) ?? 0) + 1);
    }
  }
  return Array.from(counts, ([value, count]) => ({ count, value })).sort(
    (left, right) =>
      right.count - left.count || left.value.localeCompare(right.value),
  );
}

function isOrderedRuleOperator(operator: string): boolean {
  return (
    operator === "<" ||
    operator === "<=" ||
    operator === ">" ||
    operator === ">="
  );
}

function orderedRuleExample(key: string): string | null {
  if (key === "billing.price") return "50 USD/m";
  if (key === "network.port_speed") return "1 Gbps";
  if (key === "traffic.reset_day") return "15";
  if (key.startsWith("traffic.quota.")) return "1TB";
  return null;
}

function ruleCompletionOption(
  value: string,
  label: string,
  detail: string,
): CompletionOption {
  return completionOption(value, label, detail, `${label} ${detail}`);
}

function applyCompletion(
  value: string,
  completion: CompletionState,
  suggestion: CompletionOption,
): string {
  const suffix = value.slice(completion.end);
  const separator = suffix && !/^\s/.test(suffix) ? " " : "";
  return cleanEditorText(
    `${value.slice(0, completion.start)}${suggestion.value}${separator}${suffix}`,
  );
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
  const valueFragment = unquoteLeadingFragment(
    normalizedFragment.slice(namespaceSeparator + 1),
  );
  if (!suggestion.namespace || suggestion.namespace !== namespace) {
    return false;
  }
  return valueFragment
    ? suggestion.selectorValue.includes(valueFragment) ||
        suggestion.matchText.includes(valueFragment)
    : true;
}

function buildAgentCompletionOptions(
  agents: AgentView[],
  fragment: string,
  mode: VpsNameDisplayMode,
): CompletionOption[] {
  const normalized = fragment.trim().toLocaleLowerCase();
  const separator = normalized.indexOf(":");
  const namespace = separator >= 0 ? normalized.slice(0, separator) : null;
  const valueFragment =
    separator >= 0
      ? unquoteLeadingFragment(normalized.slice(separator + 1))
      : normalized;
  return agents
    .map((agent) =>
      agentCompletionOption(agent, namespace, valueFragment, mode),
    )
    .filter((option): option is CompletionOption => Boolean(option))
    .sort(
      (left, right) =>
        left.label.localeCompare(right.label) ||
        left.value.localeCompare(right.value),
    );
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
    return completionOption(
      `id:${agent.id}`,
      label,
      agentDetail(agent, "ID"),
      `${idMatchText} ${nameMatchText}`,
    );
  }
  if (namespace === "name") {
    if (
      !displayName ||
      (normalizedFragment && !nameMatchText.includes(normalizedFragment))
    ) {
      return null;
    }
    return completionOption(
      `name:${quoteSelectorValue(displayName)}`,
      label,
      agentDetail(agent, "Name"),
      `${nameMatchText} ${idMatchText}`,
    );
  }
  if (namespace) {
    return null;
  }
  const nameMatched =
    Boolean(displayName) &&
    (!normalizedFragment || nameMatchText.includes(normalizedFragment));
  const idMatched =
    !normalizedFragment || idMatchText.includes(normalizedFragment);
  if (!nameMatched && !idMatched) {
    return null;
  }
  const useId =
    idMatched &&
    (!nameMatched ||
      agent.id.toLocaleLowerCase().startsWith(normalizedFragment) ||
      suffix.toLocaleLowerCase() === normalizedFragment);
  const selector = useId
    ? `id:${agent.id}`
    : `name:${quoteSelectorValue(displayName)}`;
  return completionOption(
    selector,
    label,
    agentDetail(agent, useId ? "ID" : "Name"),
    `${nameMatchText} ${idMatchText}`,
  );
}

function staticCompletionOption(value: string): CompletionOption {
  return completionOption(value, value, undefined, value);
}

function completionOption(
  value: string,
  label: string,
  detail: string | undefined,
  matchText: string,
): CompletionOption {
  const separator = value.indexOf(":");
  const selectorValue =
    separator >= 0 ? unquoteSelectorValue(value.slice(separator + 1)) : value;
  return {
    detail: detail ?? (label === value ? undefined : value),
    label,
    matchText: `${matchText} ${value}`.toLocaleLowerCase(),
    namespace:
      separator > 0 ? value.slice(0, separator).toLocaleLowerCase() : null,
    selectorValue: selectorValue.toLocaleLowerCase(),
    value,
  };
}

function uniqueCompletionOptions(
  options: CompletionOption[],
): CompletionOption[] {
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
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1).replace(/\\(["'\\])/g, "$1");
  }
  return trimmed;
}

function unquoteLeadingFragment(value: string): string {
  return value.replace(/^["']/, "");
}

function completionFragment(
  value: string,
  caretIndex: number,
): { fragment: string; start: number } {
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
  return agents
    .map((agent) => `${agent.id} (${agent.display_name})`)
    .join(", ");
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
      tokens.push(
        createDisplayToken(
          char === "(" ? "left_paren" : "right_paren",
          input.slice(index, index + 1),
          index,
          index + 1,
        ),
      );
      index += 1;
      continue;
    }
    if (char === "&" || char === "|") {
      const end = input[index + 1] === char ? index + 2 : index + 1;
      tokens.push(
        createDisplayToken(
          char === "&" ? "and" : "or",
          input.slice(index, end),
          index,
          end,
        ),
      );
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
      tokens.push(
        createDisplayToken(lower === "and" ? "and" : "or", raw, start, index),
      );
    } else {
      tokens.push(createTermToken(raw, start, index));
    }
  }
  return tokens;
}

function createDisplayToken(
  kind: DisplayToken["kind"],
  raw: string,
  start: number,
  end: number,
): DisplayToken {
  return {
    end,
    kind,
    namespace: null,
    raw,
    start,
    value: raw,
  };
}

function createTermToken(
  raw: string,
  start: number,
  end: number,
): DisplayToken {
  const separator = raw.indexOf(":");
  return {
    end,
    kind: "term",
    namespace:
      separator > 0 ? raw.slice(0, separator).toLocaleLowerCase() : null,
    raw,
    start,
    value: separator > 0 ? raw.slice(separator + 1) : raw,
  };
}

function cleanEditorText(text: string): string {
  return text
    .replace(/\u00a0/g, " ")
    .replace(/\s+/g, " ")
    .trimStart();
}

function scrollCaretIndexIntoView(
  editor: HTMLInputElement,
  caretIndex: number,
) {
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

function scrollEditorByWheelDelta(
  editor: HTMLElement,
  deltaX: number,
  deltaY: number,
): boolean {
  const maxScrollLeft = editor.scrollWidth - editor.clientWidth;
  if (maxScrollLeft <= 1) {
    return false;
  }
  const delta = Math.abs(deltaX) > Math.abs(deltaY) ? deltaX : deltaY;
  if (!delta) {
    return false;
  }
  const previousScrollLeft = editor.scrollLeft;
  editor.scrollLeft = Math.max(
    0,
    Math.min(maxScrollLeft, previousScrollLeft + delta),
  );
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
