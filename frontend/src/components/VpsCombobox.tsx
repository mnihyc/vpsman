import { ChevronDown, Search } from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import { createPortal } from "react-dom";
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
  const menuRef = useRef<HTMLDivElement | null>(null);
  const skipBlurCommitRef = useRef(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [focused, setFocused] = useState(false);
  const [menuStyle, setMenuStyle] = useState<CSSProperties | null>(null);
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
    setActiveIndex(query.trim() ? 0 : -1);
  }, [query]);

  useEffect(() => {
    if (!open) {
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
      commitQuery();
    }
    document.addEventListener("pointerdown", handleDocumentPointerDown, true);
    return () => document.removeEventListener("pointerdown", handleDocumentPointerDown, true);
  });

  useLayoutEffect(() => {
    if (!open || disabled) {
      setMenuStyle(null);
      return;
    }
    const updateMenuPosition = () => {
      const container = containerRef.current;
      if (!container) {
        return;
      }
      const rect = container.getBoundingClientRect();
      const margin = 8;
      const gap = 4;
      const viewportWidth = window.innerWidth;
      const viewportHeight = window.innerHeight;
      const below = Math.max(0, viewportHeight - rect.bottom - gap - margin);
      const above = Math.max(0, rect.top - gap - margin);
      const minimumHeight = Math.min(120, Math.max(0, viewportHeight - margin * 2));
      const openAbove = below < minimumHeight && above > below;
      const available = openAbove ? above : below;
      const maxHeight = Math.min(
        240,
        Math.max(available, minimumHeight),
        Math.max(0, viewportHeight - margin * 2),
      );
      const width = Math.min(rect.width, Math.max(0, viewportWidth - margin * 2));
      const left = Math.min(
        Math.max(rect.left, margin),
        Math.max(margin, viewportWidth - width - margin),
      );
      const desiredTop = openAbove
        ? rect.top - gap - maxHeight
        : rect.bottom + gap;
      const top = Math.min(
        Math.max(desiredTop, margin),
        Math.max(margin, viewportHeight - maxHeight - margin),
      );
      setMenuStyle({
        left,
        maxHeight,
        top,
        width,
      });
    };
    updateMenuPosition();
    window.addEventListener("resize", updateMenuPosition);
    window.addEventListener("scroll", updateMenuPosition, true);
    return () => {
      window.removeEventListener("resize", updateMenuPosition);
      window.removeEventListener("scroll", updateMenuPosition, true);
    };
  }, [disabled, open, filtered.length]);

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
      setActiveIndex((current) =>
        Math.min(current + 1, Math.max(filtered.length - 1, 0)),
      );
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
      {open && !disabled && menuStyle
        ? createPortal(
          <div
            className="vpsComboboxMenu"
            ref={menuRef}
            role="listbox"
            style={menuStyle}
          >
          {filtered.length > 0 ? (
            filtered.slice(0, 10).map((option, index) => (
              <button
                aria-selected={index === activeIndex}
                className={index === activeIndex ? "active" : undefined}
                key={option.id}
                title={`${option.label} ${option.detail}`.trim()}
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
            <span className="vpsComboboxEmpty" title="No VPS matches this search.">No VPS matches this search.</span>
          )}
          </div>,
          document.body,
        )
        : null}
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
