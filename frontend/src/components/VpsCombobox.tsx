import { ChevronDown, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { clientIdSuffix } from "../utils";

export type VpsComboboxOption = {
  display_name?: string | null;
  id: string;
  status?: string | null;
  tags?: string[];
};

type VpsComboboxProps = {
  agents: VpsComboboxOption[];
  allowUnknownId?: boolean;
  ariaLabel: string;
  className?: string;
  disabled?: boolean;
  excludeIds?: string[];
  onChange: (value: string) => void;
  placeholder?: string;
  value: string;
};

export function VpsCombobox({
  agents,
  allowUnknownId = false,
  ariaLabel,
  className = "",
  disabled = false,
  excludeIds = [],
  onChange,
  placeholder = "Search VPS name or ID",
  value,
}: VpsComboboxProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const skipBlurCommitRef = useRef(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [focused, setFocused] = useState(false);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState(() => displayValue(value, agents));
  const options = useMemo(
    () => searchableOptions(agents, excludeIds, value),
    [agents, excludeIds, value],
  );
  const filtered = useMemo(() => filterOptions(options, query), [options, query]);

  useEffect(() => {
    if (!focused) {
      setQuery(displayValue(value, agents));
    }
  }, [agents, focused, value]);

  useEffect(() => {
    setActiveIndex(0);
  }, [query]);

  useEffect(() => {
    if (!open) {
      return;
    }
    function handleDocumentPointerDown(event: PointerEvent) {
      const container = containerRef.current;
      if (!container || !event.target || container.contains(event.target as Node)) {
        return;
      }
      commitQuery();
    }
    document.addEventListener("pointerdown", handleDocumentPointerDown, true);
    return () => document.removeEventListener("pointerdown", handleDocumentPointerDown, true);
  });

  function selectOption(option: SearchableVpsOption) {
    skipBlurCommitRef.current = true;
    onChange(option.id);
    setQuery(option.label);
    setOpen(false);
    setFocused(false);
    window.setTimeout(() => inputRef.current?.blur(), 0);
  }

  function commitQuery() {
    const trimmed = query.trim();
    setOpen(false);
    setFocused(false);
    if (!trimmed) {
      onChange("");
      setQuery("");
      return;
    }
    const exact = exactOption(options, trimmed);
    if (exact) {
      selectOption(exact);
      return;
    }
    if (filtered.length === 1) {
      selectOption(filtered[0]);
      return;
    }
    if (allowUnknownId) {
      onChange(trimmed);
      setQuery(displayValue(trimmed, agents));
      return;
    }
    setQuery(displayValue(value, agents));
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setOpen(true);
      setActiveIndex((current) => Math.min(current + 1, Math.max(filtered.length - 1, 0)));
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setOpen(true);
      setActiveIndex((current) => Math.max(current - 1, 0));
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      if (open && filtered[activeIndex]) {
        selectOption(filtered[activeIndex]);
      } else {
        commitQuery();
      }
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
      setQuery(displayValue(value, agents));
      inputRef.current?.blur();
    }
  }

  return (
    <div
      className={`vpsCombobox ${className} ${disabled ? "disabled" : ""}`.trim()}
      ref={containerRef}
    >
      <Search size={15} />
      <input
        aria-autocomplete="list"
        aria-expanded={open}
        aria-label={ariaLabel}
        autoComplete="off"
        disabled={disabled}
        onBlur={() =>
          window.setTimeout(() => {
            if (skipBlurCommitRef.current) {
              skipBlurCommitRef.current = false;
              return;
            }
            commitQuery();
          }, 120)
        }
        onChange={(event) => {
          setQuery(event.target.value);
          setOpen(true);
          setFocused(true);
        }}
        onFocus={() => {
          setFocused(true);
          setOpen(true);
          inputRef.current?.select();
        }}
        onKeyDown={handleKeyDown}
        placeholder={placeholder}
        ref={inputRef}
        role="combobox"
        spellCheck={false}
        value={query}
      />
      <ChevronDown size={15} />
      {open && !disabled && (
        <div className="vpsComboboxMenu" role="listbox">
          {filtered.length > 0 ? (
            filtered.slice(0, 10).map((option, index) => (
              <button
                aria-selected={index === activeIndex}
                className={index === activeIndex ? "active" : undefined}
                key={option.id}
                onMouseDown={(event) => {
                  event.preventDefault();
                  selectOption(option);
                }}
                role="option"
                type="button"
              >
                <strong>{option.label}</strong>
                <small>{option.detail}</small>
              </button>
            ))
          ) : (
            <span className="vpsComboboxEmpty">No VPS matches this search.</span>
          )}
        </div>
      )}
    </div>
  );
}

type SearchableVpsOption = {
  detail: string;
  id: string;
  label: string;
  searchText: string;
};

function searchableOptions(
  agents: VpsComboboxOption[],
  excludeIds: string[],
  currentValue: string,
): SearchableVpsOption[] {
  const excluded = new Set(excludeIds.filter((id) => id && id !== currentValue));
  return agents
    .filter((agent) => !excluded.has(agent.id))
    .map((agent) => {
      const label = optionLabel(agent);
      const suffix = clientIdSuffix(agent.id) ?? "";
      const detailParts = [agent.id, agent.status].filter(Boolean);
      return {
        detail: detailParts.join(" · "),
        id: agent.id,
        label,
        searchText: [
          agent.id,
          suffix,
          agent.display_name ?? "",
          label,
          agent.status ?? "",
          ...(agent.tags ?? []),
        ]
          .join(" ")
          .toLocaleLowerCase(),
      };
    })
    .sort((left, right) => left.label.localeCompare(right.label) || left.id.localeCompare(right.id));
}

function filterOptions(options: SearchableVpsOption[], query: string): SearchableVpsOption[] {
  const normalized = query.trim().toLocaleLowerCase();
  if (!normalized) {
    return options;
  }
  return options.filter((option) => option.searchText.includes(normalized));
}

function exactOption(options: SearchableVpsOption[], query: string): SearchableVpsOption | null {
  const normalized = query.trim().toLocaleLowerCase();
  return (
    options.find((option) =>
      option.id.toLocaleLowerCase() === normalized ||
      option.label.toLocaleLowerCase() === normalized ||
      clientIdSuffix(option.id)?.toLocaleLowerCase() === normalized,
    ) ?? null
  );
}

function displayValue(
  value: string,
  agents: VpsComboboxOption[],
): string {
  const selected = agents.find((agent) => agent.id === value);
  return selected ? optionLabel(selected) : value;
}

function optionLabel(agent: VpsComboboxOption): string {
  const name = agent.display_name?.trim();
  if (!name) {
    return agent.id;
  }
  const suffix = clientIdSuffix(agent.id);
  return suffix ? `${name} (${suffix})` : name;
}
