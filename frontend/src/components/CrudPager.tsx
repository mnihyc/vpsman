import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import type { KeyboardEvent, ReactNode } from "react";
import { SearchExpressionInput } from "./SearchExpressionInput";
import {
  buildParseableSearchValueSuggestions,
  searchFieldsForSearchValues,
} from "./searchSuggestions";
import { filterBySearchExpression } from "../searchExpression";

export type CrudSearchField<T> = {
  label: string;
  value: (item: T) => string | number | boolean | null | undefined;
};

export type CrudPageState = {
  currentPage: number;
  filteredCount: number;
  pageCount: number;
  pageSize: number;
  totalCount: number;
};

type CrudPagerPreferences = {
  field?: string;
  page?: number;
  pageSize?: number;
  query?: string;
};

export function CrudPager<T>({
  children,
  defaultField = "__all",
  empty,
  fields,
  itemLabel = "rows",
  items,
  pageSize = 10,
  pageSizeOptions,
  storageKey,
  title,
}: {
  children: (items: T[], state: CrudPageState) => ReactNode;
  defaultField?: string;
  empty?: ReactNode;
  fields: CrudSearchField<T>[];
  itemLabel?: string;
  items: T[];
  pageSize?: number;
  pageSizeOptions?: number[];
  storageKey?: string;
  title: string;
}) {
  const resolvedStorageKey = storageKey ?? `vpsman.crudPager.${title}`;
  const [initialPreferences] = useState(() => readCrudPagerPreferences(resolvedStorageKey));
  const sizeOptions = useMemo(() => buildPageSizeOptions(pageSize, pageSizeOptions), [pageSize, pageSizeOptions]);
  const [query, setQuery] = useState(initialPreferences.query ?? "");
  const [field, setField] = useState(initialPreferences.field ?? defaultField);
  const [page, setPage] = useState(initialPreferences.page ?? 1);
  const searchInputRef = useRef<HTMLElement | null>(null);
  const [activePageSize, setActivePageSize] = useState(() =>
    sizeOptions.includes(initialPreferences.pageSize ?? pageSize) ? (initialPreferences.pageSize ?? pageSize) : pageSize,
  );
  const fallbackField =
    defaultField === "__all" || fields.some((candidate) => candidate.label === defaultField) ? defaultField : "__all";
  const effectiveField = field === "__all" || fields.some((candidate) => candidate.label === field) ? field : fallbackField;
  const activeFields = useMemo(
    () => (effectiveField === "__all" ? fields : fields.filter((candidate) => candidate.label === effectiveField)),
    [effectiveField, fields],
  );
  const searchValuesForItem = (item: T) =>
    activeFields.map((candidate) => candidate.value(item));
  const searchFieldsForItem = (item: T) =>
    searchFieldsForSearchValues(searchValuesForItem(item));
  const filteredItems = useMemo(() => {
    return filterBySearchExpression(items, query, searchFieldsForItem).items;
  }, [activeFields, items, query]);
  const searchSuggestions = useMemo(
    () =>
      buildParseableSearchValueSuggestions(
        items,
        searchValuesForItem,
        searchFieldsForItem,
      ),
    [activeFields, items],
  );
  const pageCount = Math.max(1, Math.ceil(filteredItems.length / activePageSize));
  const currentPage = Math.min(page, pageCount);
  const pagedItems = filteredItems.slice((currentPage - 1) * activePageSize, currentPage * activePageSize);

  useEffect(() => {
    setPage(1);
  }, [activePageSize, field, query]);

  useEffect(() => {
    if (page > pageCount) {
      setPage(pageCount);
    }
  }, [page, pageCount]);

  useEffect(() => {
    if (field !== effectiveField) {
      setField(effectiveField);
    }
  }, [effectiveField, field]);

  useEffect(() => {
    writeCrudPagerPreferences(resolvedStorageKey, {
      field: effectiveField,
      page: currentPage,
      pageSize: activePageSize,
      query,
    });
  }, [activePageSize, currentPage, effectiveField, query, resolvedStorageKey]);

  const state: CrudPageState = {
    currentPage,
    filteredCount: filteredItems.length,
    pageCount,
    pageSize: activePageSize,
    totalCount: items.length,
  };

  function handlePagerKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) {
      return;
    }
    if (event.key === "/" && document.activeElement !== searchInputRef.current) {
      event.preventDefault();
      searchInputRef.current?.focus();
      return;
    }
    if (
      event.target instanceof HTMLInputElement ||
      event.target instanceof HTMLSelectElement ||
      (event.target instanceof HTMLElement &&
        (event.target.isContentEditable || Boolean(event.target.closest(".searchExpressionInput"))))
    ) {
      return;
    }
    if (event.key === "ArrowLeft" || event.key === "PageUp") {
      event.preventDefault();
      setPage((value) => Math.max(1, value - 1));
    } else if (event.key === "ArrowRight" || event.key === "PageDown") {
      event.preventDefault();
      setPage((value) => Math.min(pageCount, value + 1));
    } else if (event.key === "Home") {
      event.preventDefault();
      setPage(1);
    } else if (event.key === "End") {
      event.preventDefault();
      setPage(pageCount);
    }
  }

  return (
    <div
      aria-label={`${title} pageable table`}
      className="crudPager"
      onKeyDown={handlePagerKeyDown}
      tabIndex={0}
    >
      <div className="crudToolbar" aria-label={`${title} table controls`}>
        <div className="crudCounts">
          <strong>{title}</strong>
          <span>
            {state.filteredCount} of {state.totalCount} {itemLabel}
          </span>
          <span>
            Page {state.currentPage} / {state.pageCount}
          </span>
        </div>
        <div className="crudSearch">
          <select aria-label={`${title} search field`} value={effectiveField} onChange={(event) => setField(event.target.value)}>
            <option value="__all">All fields</option>
            {fields.map((candidate) => (
              <option key={candidate.label} value={candidate.label}>
                {candidate.label}
              </option>
            ))}
          </select>
          <SearchExpressionInput
            ariaLabel={`${title} search`}
            className="compact"
            inputRef={searchInputRef}
            onChange={setQuery}
            placeholder="Search"
            suggestions={searchSuggestions}
            value={query}
          />
        </div>
        <div className="crudPageActions">
          <label className="crudPageSize">
            <span>Rows</span>
            <select
              aria-label={`${title} page size`}
              onChange={(event) => setActivePageSize(Number(event.target.value))}
              value={activePageSize}
            >
              {sizeOptions.map((option) => (
                <option key={option} value={option}>
                  {option}
                </option>
              ))}
            </select>
          </label>
          <button
            aria-label={`${title} previous page`}
            className="iconButton"
            disabled={currentPage <= 1}
            onClick={() => setPage((value) => Math.max(1, value - 1))}
            type="button"
          >
            <ChevronLeft size={16} />
          </button>
          <button
            aria-label={`${title} next page`}
            className="iconButton"
            disabled={currentPage >= pageCount}
            onClick={() => setPage((value) => Math.min(pageCount, value + 1))}
            type="button"
          >
            <ChevronRight size={16} />
          </button>
        </div>
      </div>
      {pagedItems.length === 0 ? empty : children(pagedItems, state)}
    </div>
  );
}

function buildPageSizeOptions(pageSize: number, pageSizeOptions: number[] | undefined): number[] {
  const options = pageSizeOptions ?? [pageSize, 10, 25, 50, 100];
  return Array.from(new Set(options.filter((option) => Number.isFinite(option) && option > 0).map((option) => Math.floor(option)))).sort(
    (left, right) => left - right,
  );
}

function readCrudPagerPreferences(storageKey: string): CrudPagerPreferences {
  if (typeof window === "undefined") {
    return {};
  }
  try {
    const raw = window.localStorage.getItem(storageKey);
    if (!raw) {
      return {};
    }
    const parsed = JSON.parse(raw) as CrudPagerPreferences;
    if (typeof parsed !== "object" || parsed === null) {
      return {};
    }
    return {
      field: typeof parsed.field === "string" ? parsed.field : undefined,
      page: toPositiveInteger(parsed.page),
      pageSize: toPositiveInteger(parsed.pageSize),
      query: typeof parsed.query === "string" ? parsed.query : undefined,
    };
  } catch {
    return {};
  }
}

function writeCrudPagerPreferences(storageKey: string, preferences: Required<CrudPagerPreferences>) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.localStorage.setItem(storageKey, JSON.stringify(preferences));
  } catch {
    // Best-effort UI preference only; quota/privacy failures must not break the table.
  }
}

function toPositiveInteger(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.floor(value) : undefined;
}
