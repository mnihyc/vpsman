import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  flexRender,
  getCoreRowModel,
  getPaginationRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type ColumnSizingState,
  type Cell,
  type Header,
  type Row,
  type RowSelectionState,
  type SortingState,
  type VisibilityState,
} from "@tanstack/react-table";
import * as ContextMenu from "@radix-ui/react-context-menu";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  horizontalListSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronRight as ChevronRightIcon,
  CheckSquare,
  Columns3,
  GripVertical,
  X,
} from "lucide-react";
import { SearchExpressionInput } from "./SearchExpressionInput";
import {
  buildParseableSearchValueSuggestions,
  searchFieldsForSearchValues,
} from "./searchSuggestions";
import {
  filterBySearchExpression,
  type SearchFields,
} from "../searchExpression";

export type ConsoleDataGridColumn<T> = {
  align?: "end" | "start";
  cell: (row: T) => ReactNode;
  enableHiding?: boolean;
  header: string;
  id: string;
  minSize?: number;
  searchValue?: (row: T) => string | number | boolean | null | undefined;
  size?: number;
  sortValue?: (row: T) => string | number | boolean | null | undefined;
  stickyEnd?: boolean;
};

export type ConsoleDataGridAction<T> = {
  description?: (rows: T[]) => string;
  disabled?: (rows: T[]) => boolean;
  expandRow?: boolean;
  hidden?: (rows: T[]) => boolean;
  icon?: ReactNode;
  label: string;
  onSelect: (rows: T[]) => void;
  separatorBefore?: boolean;
  tone?: "danger" | "normal";
};

type ConsoleDataGridPreferences = {
  columnOrder?: string[];
  columnSizing?: ColumnSizingState;
  columnVisibility?: VisibilityState;
  globalFilter?: string;
  pageSize?: number;
  sorting?: SortingState;
};

export function ConsoleDataGrid<T>({
  actions = [],
  columns,
  defaultPageSize = 10,
  defaultColumnVisibility,
  empty,
  getRowId,
  itemLabel = "rows",
  mobileFieldLayout = "auto",
  mobileRowActionLimit = 3,
  expandOnRowClick,
  mobileLayout = "cards",
  onExpandedRowChange,
  onOpenRow,
  openRowOnClick = true,
  openRowLabel = "Open",
  openRowTitle,
  showMobileOpenRowAction = true,
  onSelectionChange,
  renderExpandedRow,
  renderSelectionPanel,
  rowActions = [],
  rows,
  selectable = true,
  singleExpandedRow = false,
  searchPlaceholder = "Search",
  storageKey,
  title,
  toolbarActions,
}: {
  actions?: ConsoleDataGridAction<T>[];
  columns: ConsoleDataGridColumn<T>[];
  defaultColumnVisibility?: VisibilityState;
  defaultPageSize?: number;
  empty?: ReactNode;
  expandOnRowClick?: boolean;
  getRowId: (row: T) => string;
  itemLabel?: string;
  mobileFieldLayout?: "auto" | "stacked";
  mobileRowActionLimit?: number;
  mobileLayout?: "cards" | "table";
  onExpandedRowChange?: (row: T | null) => void;
  onOpenRow?: (row: T) => void;
  openRowOnClick?: boolean;
  openRowLabel?: string;
  openRowTitle?: (row: T) => string;
  showMobileOpenRowAction?: boolean;
  onSelectionChange?: (rows: T[]) => void;
  renderExpandedRow?: (row: T) => ReactNode;
  renderSelectionPanel?: (rows: T[]) => ReactNode;
  rowActions?: ConsoleDataGridAction<T>[];
  rows: T[];
  selectable?: boolean;
  singleExpandedRow?: boolean;
  searchPlaceholder?: string;
  storageKey: string;
  title: string;
  toolbarActions?: ReactNode;
}) {
  const [preferences] = useState(() => readGridPreferences(storageKey));
  const [columnSizing, setColumnSizing] = useState<ColumnSizingState>(
    preferences.columnSizing ?? {},
  );
  const [columnVisibility, setColumnVisibility] = useState<VisibilityState>(
    preferences.columnVisibility ?? defaultColumnVisibility ?? {},
  );
  const [columnOrder, setColumnOrder] = useState<string[]>(
    preferences.columnOrder ?? [],
  );
  const [expandedRows, setExpandedRows] = useState<Record<string, boolean>>({});
  const [globalFilter, setGlobalFilter] = useState(
    preferences.globalFilter ?? "",
  );
  const [pageSize, setPageSize] = useState(
    preferences.pageSize ?? defaultPageSize,
  );
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
  const [sorting, setSorting] = useState<SortingState>(
    preferences.sorting ?? [],
  );
  const isMobileGrid = useMediaQuery("(max-width: 640px)");
  const showMobileCards = isMobileGrid && mobileLayout === "cards";
  const expandedRowsRef = useRef(expandedRows);
  const onExpandedRowChangeRef = useRef(onExpandedRowChange);
  const renderExpandedRowRef = useRef(renderExpandedRow);
  const singleExpandedRowRef = useRef(singleExpandedRow);
  expandedRowsRef.current = expandedRows;
  onExpandedRowChangeRef.current = onExpandedRowChange;
  renderExpandedRowRef.current = renderExpandedRow;
  singleExpandedRowRef.current = singleExpandedRow;
  const hasExpandedRows = Boolean(renderExpandedRow);
  const rowClickExpands = expandOnRowClick ?? hasExpandedRows;
  const searchValuesForRow = (row: T) =>
    columns.map(
      (column) => column.searchValue?.(row) ?? column.sortValue?.(row),
    );
  const searchFieldsForRow = (row: T): SearchFields =>
    searchFieldsForSearchValues(searchValuesForRow(row));
  const filteredRows = useMemo(() => {
    return filterBySearchExpression(rows, globalFilter, searchFieldsForRow)
      .items;
  }, [columns, globalFilter, rows]);
  const gridSearchSuggestions = useMemo(
    () =>
      buildParseableSearchValueSuggestions(
        rows,
        searchValuesForRow,
        searchFieldsForRow,
      ),
    [columns, rows],
  );
  const dataColumnsById = useMemo(
    () => new Map(columns.map((column) => [column.id, column])),
    [columns],
  );
  const tableColumns = useMemo<ColumnDef<T>[]>(
    () => [
      ...(selectable
        ? [
            {
              id: "__select",
              size: 42,
              minSize: 42,
              maxSize: 42,
              enableHiding: false,
              header: ({ table }) => (
                <input
                  aria-label={`Select all ${title}`}
                  checked={table.getIsAllPageRowsSelected()}
                  onChange={(event) => {
                    table.getRowModel().rows.forEach((row) => {
                      row.toggleSelected(event.currentTarget.checked);
                    });
                  }}
                  ref={(input) => {
                    if (input) {
                      input.indeterminate = table.getIsSomePageRowsSelected();
                    }
                  }}
                  type="checkbox"
                />
              ),
              cell: ({ row }) => (
                <input
                  aria-label={`Select ${title} row ${getRowId(row.original)}`}
                  checked={row.getIsSelected()}
                  onClick={(event) => event.stopPropagation()}
                  onChange={row.getToggleSelectedHandler()}
                  type="checkbox"
                />
              ),
            } satisfies ColumnDef<T>,
          ]
        : []),
      ...(hasExpandedRows
        ? [
            {
              id: "__expand",
              size: 42,
              minSize: 42,
              maxSize: 42,
              enableHiding: false,
              header: "",
              cell: ({ row }: { row: Row<T> }) => {
                const open = Boolean(expandedRowsRef.current[row.id]);
                return (
                  <button
                    aria-expanded={open}
                    aria-label={`${open ? "Collapse" : "Expand"} ${title} row ${getRowId(
                      row.original,
                    )}`}
                    className="iconButton gridIconButton"
                    onClick={(event) => {
                      event.stopPropagation();
                      toggleExpandedRow(row.id, row.original);
                    }}
                    title={
                      open
                        ? `Collapse ${title} row details.`
                        : `Expand ${title} row details.`
                    }
                    type="button"
                  >
                    {open ? (
                      <ChevronDown size={16} />
                    ) : (
                      <ChevronRightIcon size={16} />
                    )}
                  </button>
                );
              },
            } satisfies ColumnDef<T>,
          ]
        : []),
      ...columns.map((column) => ({
        id: column.id,
        accessorFn: (row: T) =>
          column.sortValue?.(row) ?? column.searchValue?.(row) ?? "",
        header: column.header,
        minSize: column.minSize ?? 96,
        size: column.size ?? 160,
        enableHiding: column.enableHiding ?? true,
        meta: { stickyEnd: column.stickyEnd === true },
        cell: ({ row }: { row: Row<T> }) => (
          <span
            className={
              column.align === "end"
                ? "gridCellContent alignEnd"
                : "gridCellContent"
            }
            title={tooltipFromValue(
              column.searchValue?.(row.original) ??
                column.sortValue?.(row.original),
            )}
          >
            {column.cell(row.original)}
          </span>
        ),
      })),
    ],
    [columns, hasExpandedRows, selectable, title],
  );
  const defaultColumnOrder = useMemo(
    () =>
      tableColumns
        .map((column) => column.id)
        .filter((id): id is string => Boolean(id)),
    [tableColumns],
  );
  const effectiveColumnOrder = useMemo(
    () => reconcileColumnOrder(columnOrder, defaultColumnOrder),
    [columnOrder, defaultColumnOrder],
  );
  const sortableColumnIds = useMemo(
    () => columns.map((column) => column.id),
    [columns],
  );
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );
  const table = useReactTable({
    columnResizeMode: "onChange",
    columns: tableColumns,
    data: filteredRows,
    enableMultiRowSelection: selectable,
    enableRowSelection: selectable,
    getCoreRowModel: getCoreRowModel(),
    getPaginationRowModel: getPaginationRowModel(),
    getRowId,
    getSortedRowModel: getSortedRowModel(),
    onColumnSizingChange: setColumnSizing,
    onColumnVisibilityChange: setColumnVisibility,
    onColumnOrderChange: setColumnOrder,
    onRowSelectionChange: setRowSelection,
    onSortingChange: setSorting,
    state: {
      columnSizing,
      columnOrder: effectiveColumnOrder,
      columnVisibility,
      rowSelection,
      sorting,
    },
  });
  const selectedRows = table
    .getSelectedRowModel()
    .rows.map((row) => row.original);
  const selectedRowSignature = selectedRows.map(getRowId).join("\u001f");
  const visibleSelectionActions = actions.filter(
    (action) => !action.hidden?.(selectedRows),
  );
  const contextRowActions = rowActions.length > 0 ? rowActions : actions;
  const showContextSelectionActions = false;
  const pageCount = table.getPageCount() || 1;
  const currentPage = table.getState().pagination.pageIndex + 1;
  const currentPageRows = table.getRowModel().rows;
  const selectedPageRowCount = currentPageRows.filter((row) =>
    row.getIsSelected(),
  ).length;
  const allCurrentPageRowsSelected =
    currentPageRows.length > 0 &&
    selectedPageRowCount === currentPageRows.length;

  useEffect(() => {
    table.setPageSize(pageSize);
  }, [pageSize, table]);

  useEffect(() => {
    writeGridPreferences(storageKey, {
      columnOrder: effectiveColumnOrder,
      columnSizing,
      columnVisibility,
      globalFilter,
      pageSize,
      sorting,
    });
  }, [
    columnOrder,
    columnSizing,
    columnVisibility,
    effectiveColumnOrder,
    globalFilter,
    pageSize,
    sorting,
    storageKey,
  ]);

  useEffect(() => {
    onSelectionChange?.(selectedRows);
    // Use row IDs as the dependency so parent selection summaries do not churn
    // on every table render with referentially fresh row objects.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onSelectionChange, selectedRowSignature]);

  function toggleExpandedRow(rowId: string, row?: T) {
    if (!renderExpandedRowRef.current) {
      return;
    }
    setExpandedRows((current) => {
      const nextOpen = !current[rowId];
      onExpandedRowChangeRef.current?.(nextOpen ? (row ?? null) : null);
      if (singleExpandedRowRef.current) {
        return nextOpen ? { [rowId]: true } : {};
      }
      return {
        ...current,
        [rowId]: nextOpen,
      };
    });
  }

  function openExpandedRow(row: T) {
    if (!renderExpandedRowRef.current) {
      return;
    }
    const rowId = getRowId(row);
    onExpandedRowChangeRef.current?.(row);
    setExpandedRows((current) => {
      if (singleExpandedRowRef.current) {
        return { [rowId]: true };
      }
      return {
        ...current,
        [rowId]: true,
      };
    });
  }

  function invokeAction(action: ConsoleDataGridAction<T>, sourceRows?: T[]) {
    const actionRows = sourceRows ?? selectedRows;
    if (actionRows.length === 0 || action.disabled?.(actionRows)) {
      return;
    }
    if (action.expandRow && actionRows.length === 1) {
      openExpandedRow(actionRows[0]);
    }
    action.onSelect(actionRows);
  }

  function setCurrentPageRowsSelected(selected: boolean) {
    const pageRowIds = table.getRowModel().rows.map((row) => row.id);
    setRowSelection((current) => {
      const next = { ...current };
      for (const rowId of pageRowIds) {
        if (selected) {
          next[rowId] = true;
        } else {
          delete next[rowId];
        }
      }
      return next;
    });
  }

  function actionDescription(action: ConsoleDataGridAction<T>, rows: T[]) {
    return action.description?.(rows) ?? action.label;
  }

  function rowDataCells(row: Row<T>) {
    return row
      .getVisibleCells()
      .filter(
        (cell) =>
          cell.column.id !== "__select" && cell.column.id !== "__expand",
      );
  }

  function tooltipForCell(cell: Cell<T, unknown>, row: T): string | undefined {
    const column = dataColumnsById.get(cell.column.id);
    return tooltipFromValue(
      column?.searchValue?.(row) ?? column?.sortValue?.(row) ?? cell.getValue(),
    );
  }

  function renderMobileCard(row: Row<T>) {
    const rowId = getRowId(row.original);
    const dataCells = rowDataCells(row);
    const primaryCell = dataCells[0] ?? null;
    const stateCell =
      dataCells.find((cell, index) => {
        if (index === 0) return false;
        return /status|state|result|health|verification|audit|output/i.test(
          cellHeaderLabel(cell),
        );
      }) ??
      dataCells[1] ??
      null;
    const primaryRowActions = rowActions
      .filter((action) => !action.hidden?.([row.original]))
      .slice(0, Math.max(1, Math.min(6, Math.trunc(mobileRowActionLimit))));
    const showOpenRowAction = Boolean(onOpenRow && showMobileOpenRowAction);
    const hasCardActions =
      primaryRowActions.length > 0 ||
      showOpenRowAction ||
      Boolean(renderExpandedRow);
    const detailCells = dataCells.filter((cell) => {
      if (cell.id === primaryCell?.id || cell.id === stateCell?.id) {
        return false;
      }
      return !(
        hasCardActions &&
        /^(action|actions|decision|open)$/i.test(cellHeaderLabel(cell))
      );
    });
    const cardClassName = [
      "gridMobileCard",
      mobileFieldLayout === "stacked" ? "stackedFields" : "",
      row.getIsSelected() ? "selected" : "",
    ]
      .filter(Boolean)
      .join(" ");

    return (
      <div
        aria-label={`${title} mobile card ${rowId}`}
        className={cardClassName}
        role="group"
      >
        <div className="gridMobileCardHeader">
          {selectable ? (
            <input
              aria-label={`Select ${title} row ${rowId}`}
              checked={row.getIsSelected()}
              onClick={(event) => event.stopPropagation()}
              onChange={row.getToggleSelectedHandler()}
              type="checkbox"
            />
          ) : null}
          <div
            className="gridMobilePrimary"
            title={primaryCell ? tooltipForCell(primaryCell, row.original) : rowId}
          >
            {primaryCell ? (
              flexRender(
                primaryCell.column.columnDef.cell,
                primaryCell.getContext(),
              )
            ) : (
              <strong>{rowId}</strong>
            )}
          </div>
          {stateCell ? (
            <div
              className="gridMobileState"
              title={tooltipForCell(stateCell, row.original)}
            >
              {flexRender(
                stateCell.column.columnDef.cell,
                stateCell.getContext(),
              )}
            </div>
          ) : null}
        </div>

        {detailCells.length > 0 ? (
          <div className="gridMobileFields">
            {detailCells.map((cell) => (
              <div
                className="gridMobileField"
                key={cell.id}
                title={tooltipForCell(cell, row.original)}
              >
                <span title={cellHeaderLabel(cell)}>{cellHeaderLabel(cell)}</span>
                <div
                  className="gridMobileFieldValue"
                  title={tooltipForCell(cell, row.original)}
                >
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </div>
              </div>
            ))}
          </div>
        ) : null}

        {(primaryRowActions.length > 0 || showOpenRowAction || renderExpandedRow) && (
          <div className="gridMobileActions">
            {primaryRowActions.map((action) => {
              const sourceRows = [row.original];
              return (
                <button
                  className={
                    action.tone === "danger"
                      ? "secondaryAction compactAction danger"
                      : "secondaryAction compactAction"
                  }
                  disabled={action.disabled?.(sourceRows)}
                  key={action.label}
                  onClick={(event) => {
                    event.stopPropagation();
                    invokeAction(action, sourceRows);
                  }}
                  title={actionDescription(action, sourceRows)}
                  type="button"
                >
                  {action.icon}
                  <span>{action.label}</span>
                </button>
              );
            })}
            {showOpenRowAction && onOpenRow ? (
              <button
                aria-label={`${openRowLabel} ${title} row ${rowId}`}
                className="secondaryAction compactAction"
                onClick={(event) => {
                  event.stopPropagation();
                  onOpenRow(row.original);
                }}
                title={openRowTitle?.(row.original) ?? openRowLabel}
                type="button"
              >
                {openRowLabel}
              </button>
            ) : null}
            {renderExpandedRow ? (
              <button
                aria-label={`${expandedRows[row.id] ? "Hide" : "Show"} details for ${title} row ${rowId}`}
                aria-expanded={Boolean(expandedRows[row.id])}
                className="secondaryAction compactAction"
                onClick={(event) => {
                  event.stopPropagation();
                  toggleExpandedRow(row.id, row.original);
                }}
                title={
                  expandedRows[row.id]
                    ? `Hide ${title} row details.`
                    : `Show ${title} row details.`
                }
                type="button"
              >
                {expandedRows[row.id] ? "Hide details" : "Details"}
              </button>
            ) : null}
          </div>
        )}
      </div>
    );
  }

  function handleColumnDragEnd(event: DragEndEvent) {
    const activeId = String(event.active.id);
    const overId = event.over ? String(event.over.id) : "";
    if (
      !overId ||
      activeId === overId ||
      !sortableColumnIds.includes(activeId) ||
      !sortableColumnIds.includes(overId)
    ) {
      return;
    }
    setColumnOrder((current) => {
      const next = reconcileColumnOrder(
        current.length > 0 ? current : effectiveColumnOrder,
        defaultColumnOrder,
      );
      const oldIndex = next.indexOf(activeId);
      const newIndex = next.indexOf(overId);
      if (oldIndex < 0 || newIndex < 0) {
        return next;
      }
      return arrayMove(next, oldIndex, newIndex);
    });
  }

  function renderEmptyContent() {
    if (rows.length > 0 && globalFilter.trim()) {
      return (
        <div className="emptyState compactEmpty">
          <strong>No matching {itemLabel}</strong>
          <span>Try another search or clear the current search.</span>
          <button
            className="secondaryAction compactAction"
            onClick={() => setGlobalFilter("")}
            type="button"
          >
            <X size={14} />
            <span>Clear search</span>
          </button>
        </div>
      );
    }
    const emptyContent = empty ?? `No ${itemLabel} match the current view.`;
    if (typeof emptyContent === "string" || typeof emptyContent === "number") {
      return <div className="emptyState compactEmpty">{emptyContent}</div>;
    }
    return emptyContent;
  }

  return (
    <div className="consoleDataGrid" aria-label={`${title} data grid`}>
      <div className="gridToolbar">
        <div className="gridCounts">
          <strong>{title}</strong>
          <span>
            {filteredRows.length} of {rows.length} {itemLabel}
          </span>
          {selectable && <span>{selectedRows.length} selected</span>}
        </div>
        <SearchExpressionInput
          ariaLabel={`${title} search`}
          className="gridSearch compact"
          onChange={setGlobalFilter}
          placeholder={searchPlaceholder}
          suggestions={gridSearchSuggestions}
          value={globalFilter}
        />
        <div className="gridToolbarActions">
          {toolbarActions}
          {selectable && currentPageRows.length > 0 && (
            <button
              aria-label={`${
                allCurrentPageRowsSelected ? "Clear" : "Select"
              } visible ${title}`}
              className="secondaryAction compactAction"
              onClick={() =>
                setCurrentPageRowsSelected(!allCurrentPageRowsSelected)
              }
              title={
                allCurrentPageRowsSelected
                  ? `Clear the ${selectedPageRowCount} visible selected ${itemLabel}.`
                  : `Select the ${currentPageRows.length} visible ${itemLabel} on this page.`
              }
              type="button"
            >
              <CheckSquare size={16} />
              <span>
                {allCurrentPageRowsSelected ? "Clear visible" : "Select visible"}
              </span>
            </button>
          )}
          {selectable && visibleSelectionActions.length > 0 && (
            <DropdownMenu.Root>
              <DropdownMenu.Trigger asChild>
                <button
                  className="secondaryAction compactAction"
                  disabled={selectedRows.length === 0}
                  title={
                    selectedRows.length === 0
                      ? "Select table rows to use actions."
                      : `Open actions for ${selectedRows.length} selected ${
                          selectedRows.length === 1 ? "row" : "rows"
                        }.`
                  }
                  type="button"
                >
                  <span>Actions</span>
                  <ChevronDown size={16} />
                </button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content
                  align="end"
                  className="consoleMenu"
                  collisionPadding={12}
                  loop
                  sideOffset={6}
                >
                  {visibleSelectionActions.map((action, index) => {
                    const description = actionDescription(action, selectedRows);
                    return (
                      <Fragment key={action.label}>
                        {action.separatorBefore && index > 0 && (
                          <DropdownMenu.Separator className="consoleMenuSeparator" />
                        )}
                        <DropdownMenu.Item
                          className={
                            action.tone === "danger"
                              ? "consoleMenuItem danger"
                              : "consoleMenuItem"
                          }
                          disabled={
                            selectedRows.length === 0 ||
                            action.disabled?.(selectedRows)
                          }
                          onSelect={() => invokeAction(action)}
                          title={description}
                        >
                          {action.icon && (
                            <span className="consoleMenuIcon" aria-hidden>
                              {action.icon}
                            </span>
                          )}
                          <span>{action.label}</span>
                        </DropdownMenu.Item>
                      </Fragment>
                    );
                  })}
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
          )}
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button
                aria-label={`${title} columns`}
                className="secondaryAction compactAction columnChooserButton"
                title={`Choose visible fields for ${title}.`}
                type="button"
              >
                <Columns3 size={17} />
                <span>Fields</span>
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                align="end"
                className="consoleMenu"
                collisionPadding={12}
                loop
                sideOffset={6}
              >
                {table
                  .getAllLeafColumns()
                  .filter((column) => column.getCanHide())
                  .map((column) => (
                    <DropdownMenu.CheckboxItem
                      checked={column.getIsVisible()}
                      className="consoleMenuItem"
                      key={column.id}
                      onCheckedChange={(checked) =>
                        column.toggleVisibility(Boolean(checked))
                      }
                    >
                      {String(column.columnDef.header)}
                    </DropdownMenu.CheckboxItem>
                  ))}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
          <label className="gridPageSize">
            <span>Rows</span>
            <select
              aria-label={`${title} page size`}
              onChange={(event) => setPageSize(Number(event.target.value))}
              value={pageSize}
            >
              {[defaultPageSize, 10, 25, 50, 100]
                .filter(
                  (value, index, values) => values.indexOf(value) === index,
                )
                .sort((left, right) => left - right)
                .map((value) => (
                  <option key={value} value={value}>
                    {value}
                  </option>
                ))}
            </select>
          </label>
          <button
            aria-label={`${title} previous page`}
            className="iconButton"
            disabled={!table.getCanPreviousPage()}
            onClick={() => table.previousPage()}
            title={
              table.getCanPreviousPage()
                ? `Go to the previous ${title} page.`
                : `Already on the first ${title} page.`
            }
            type="button"
          >
            <ChevronLeft size={16} />
          </button>
          <span className="gridPageLabel">
            {currentPage} / {pageCount}
          </span>
          <button
            aria-label={`${title} next page`}
            className="iconButton"
            disabled={!table.getCanNextPage()}
            onClick={() => table.nextPage()}
            title={
              table.getCanNextPage()
                ? `Go to the next ${title} page.`
                : `Already on the last ${title} page.`
            }
            type="button"
          >
            <ChevronRight size={16} />
          </button>
        </div>
      </div>
      {table.getRowModel().rows.length === 0 ? (
        renderEmptyContent()
      ) : (
        <div className="gridTable" role="grid">
          <div className="gridHeaderGroup" role="rowgroup">
            {table.getHeaderGroups().map((headerGroup) => (
              <DndContext
                collisionDetection={closestCenter}
                key={headerGroup.id}
                onDragEnd={handleColumnDragEnd}
                sensors={sensors}
              >
                <SortableContext
                  items={sortableColumnIds}
                  strategy={horizontalListSortingStrategy}
                >
                  <div className="gridRow gridHeaderRow" role="row">
                    {headerGroup.headers.map((header) => (
                      <SortableHeaderCell
                        canDrag={sortableColumnIds.includes(header.column.id)}
                        header={header}
                        key={header.id}
                      />
                    ))}
                  </div>
                </SortableContext>
              </DndContext>
            ))}
          </div>
          <div className="gridBody" role="rowgroup">
            {table.getRowModel().rows.map((row) => {
              const visibleContextRowActions = contextRowActions.filter(
                (action) => !action.hidden?.([row.original]),
              );
              const rowIsActionable =
                rowClickExpands || Boolean(openRowOnClick && onOpenRow);
              return (
              <ContextMenu.Root key={row.id}>
                <ContextMenu.Trigger asChild>
                  <div className="gridRecord">
                    {showMobileCards ? (
                      renderMobileCard(row)
                    ) : (
                      <div
                        aria-expanded={
                          renderExpandedRow
                            ? Boolean(expandedRows[row.id])
                            : undefined
                        }
                        className={
                          row.getIsSelected() ? "gridRow selected" : "gridRow"
                        }
                        onClick={() => {
                          if (rowClickExpands) {
                            toggleExpandedRow(row.id, row.original);
                            return;
                          }
                          if (openRowOnClick) {
                            onOpenRow?.(row.original);
                          }
                        }}
                        onKeyDown={(event) => {
                          if (
                            !rowIsActionable ||
                            event.target !== event.currentTarget ||
                            (event.key !== "Enter" && event.key !== " ")
                          ) {
                            return;
                          }
                          event.preventDefault();
                          if (rowClickExpands) {
                            toggleExpandedRow(row.id, row.original);
                          } else if (openRowOnClick) {
                            onOpenRow?.(row.original);
                          }
                        }}
                        role="row"
                        tabIndex={rowIsActionable ? 0 : undefined}
                      >
                        {row.getVisibleCells().map((cell) => (
                          <div
                            className={
                              gridColumnSticksToEnd(cell.column)
                                ? "gridCell stickyEndColumn"
                                : "gridCell"
                            }
                            key={cell.id}
                            role="gridcell"
                            style={gridColumnStyle(cell.column)}
                            title={tooltipForCell(cell, row.original)}
                          >
                            {flexRender(
                              cell.column.columnDef.cell,
                              cell.getContext(),
                            )}
                          </div>
                        ))}
                      </div>
                    )}
                    {renderExpandedRow && expandedRows[row.id] && (
                      <div className="gridExpandedRow">
                        <button
                          aria-label={`Close ${title} row details`}
                          className="iconButton gridExpandedClose"
                          onClick={(event) => {
                            event.stopPropagation();
                            toggleExpandedRow(row.id, row.original);
                          }}
                          title={`Close ${title} row details`}
                          type="button"
                        >
                          <X size={15} />
                        </button>
                        <div className="gridExpandedContent">
                          {renderExpandedRow(row.original)}
                        </div>
                      </div>
                    )}
                  </div>
                </ContextMenu.Trigger>
                {(visibleContextRowActions.length > 0 ||
                  showContextSelectionActions) && (
                  <ContextMenu.Portal>
                    <ContextMenu.Content
                      className="consoleMenu"
                      collisionPadding={12}
                      loop
                    >
                      {visibleContextRowActions.length > 0 && (
                        <>
                          <ContextMenu.Label className="consoleMenuLabel">
                            Row actions
                          </ContextMenu.Label>
                          {visibleContextRowActions.map((action) => {
                            const sourceRows = [row.original];
                            return (
                              <ContextMenu.Item
                                className={
                                  action.tone === "danger"
                                    ? "consoleMenuItem danger"
                                    : "consoleMenuItem"
                                }
                                disabled={action.disabled?.(sourceRows)}
                                key={`row:${action.label}`}
                                onSelect={() =>
                                  invokeAction(action, sourceRows)
                                }
                                title={actionDescription(action, sourceRows)}
                              >
                                {action.icon && (
                                  <span className="consoleMenuIcon" aria-hidden>
                                    {action.icon}
                                  </span>
                                )}
                                <span>{action.label}</span>
                              </ContextMenu.Item>
                            );
                          })}
                        </>
                      )}
                      {showContextSelectionActions && (
                        <>
                          <ContextMenu.Label className="consoleMenuLabel">
                            Actions
                          </ContextMenu.Label>
                          {actions.map((action, index) => (
                            <Fragment key={`selection:${action.label}`}>
                              {action.separatorBefore && index > 0 && (
                                <ContextMenu.Separator className="consoleMenuSeparator" />
                              )}
                              <ContextMenu.Item
                                className={
                                  action.tone === "danger"
                                    ? "consoleMenuItem danger"
                                    : "consoleMenuItem"
                                }
                                disabled={
                                  selectedRows.length === 0 ||
                                  action.disabled?.(selectedRows)
                                }
                                onSelect={() => invokeAction(action)}
                                title={actionDescription(action, selectedRows)}
                              >
                                {action.icon && (
                                  <span className="consoleMenuIcon" aria-hidden>
                                    {action.icon}
                                  </span>
                                )}
                                <span>{action.label}</span>
                              </ContextMenu.Item>
                            </Fragment>
                          ))}
                        </>
                      )}
                    </ContextMenu.Content>
                  </ContextMenu.Portal>
                )}
              </ContextMenu.Root>
              );
            })}
          </div>
        </div>
      )}
      {selectable && renderSelectionPanel && selectedRows.length > 0 && (
        <div className="gridSelectionPanel">
          {renderSelectionPanel(selectedRows)}
        </div>
      )}
    </div>
  );
}

function cellHeaderLabel<T>(cell: Cell<T, unknown>) {
  const header = cell.column.columnDef.header;
  return typeof header === "string" ? header : cell.column.id;
}

function useMediaQuery(query: string) {
  const [matches, setMatches] = useState(() =>
    typeof window === "undefined" ? false : window.matchMedia(query).matches,
  );

  useEffect(() => {
    if (typeof window === "undefined") {
      return;
    }
    const media = window.matchMedia(query);
    const handleChange = () => setMatches(media.matches);
    handleChange();
    media.addEventListener("change", handleChange);
    return () => media.removeEventListener("change", handleChange);
  }, [query]);

  return matches;
}

function readGridPreferences(storageKey: string): ConsoleDataGridPreferences {
  if (typeof window === "undefined") {
    return {};
  }
  try {
    const raw = window.localStorage.getItem(storageKey);
    if (!raw) {
      return {};
    }
    const parsed = JSON.parse(raw) as ConsoleDataGridPreferences;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function writeGridPreferences(
  storageKey: string,
  preferences: ConsoleDataGridPreferences,
) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.localStorage.setItem(storageKey, JSON.stringify(preferences));
  } catch {
    // Best-effort local UI preference only.
  }
}

function tooltipFromValue(value: unknown): string | undefined {
  if (value === null || value === undefined) {
    return undefined;
  }
  const text = String(value).trim();
  return text ? text : undefined;
}

function SortableHeaderCell<T>({
  canDrag,
  header,
}: {
  canDrag: boolean;
  header: Header<T, unknown>;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    disabled: !canDrag,
    id: header.column.id,
  });
  const headerClassName = [
    "gridHeaderCell",
    gridColumnSticksToEnd(header.column) ? "stickyEndColumn" : "",
    isDragging ? "dragging" : "",
  ]
    .filter(Boolean)
    .join(" ");
  const headerDefinition = header.column.columnDef.header;
  const headerTitle =
    typeof headerDefinition === "string" ? headerDefinition : "";

  return (
    <div
      className={headerClassName}
      ref={setNodeRef}
      role="columnheader"
      style={{
        ...gridColumnStyle(header.column),
        transform: CSS.Transform.toString(transform),
        transition,
      }}
    >
      {canDrag && (
        <button
          aria-label={`Reorder ${headerTitle || header.column.id} column`}
          className="gridDragHandle"
          title={`Reorder ${headerTitle || header.column.id} column`}
          type="button"
          {...attributes}
          {...listeners}
        >
          <GripVertical size={14} />
        </button>
      )}
      {header.isPlaceholder ? null : header.column.getCanSort() ? (
        <button
          className="gridHeaderButton sortable"
          onClick={header.column.getToggleSortingHandler()}
          title={headerTitle || undefined}
          type="button"
        >
          {flexRender(header.column.columnDef.header, header.getContext())}
          {header.column.getIsSorted() === "asc"
            ? " ↑"
            : header.column.getIsSorted() === "desc"
              ? " ↓"
              : ""}
        </button>
      ) : (
        <div className="gridHeaderButton" title={headerTitle || undefined}>
          {flexRender(header.column.columnDef.header, header.getContext())}
        </div>
      )}
      {header.column.getCanResize() && (
        <div
          className={
            header.column.getIsResizing()
              ? "gridResizeHandle active"
              : "gridResizeHandle"
          }
          onDoubleClick={() => header.column.resetSize()}
          onMouseDown={header.getResizeHandler()}
          onTouchStart={header.getResizeHandler()}
        />
      )}
    </div>
  );
}

function gridColumnStyle<T>(column: Header<T, unknown>["column"]) {
  const size = column.getSize();
  const minSize = column.columnDef.minSize ?? size;
  const maxSize = column.columnDef.maxSize;
  const fixed = maxSize != null && maxSize <= minSize;

  return {
    flex: fixed ? `0 0 ${size}px` : `1 1 ${size}px`,
    minWidth: minSize,
    width: size,
  };
}

function gridColumnSticksToEnd<T>(column: Header<T, unknown>["column"]): boolean {
  return Boolean((column.columnDef.meta as { stickyEnd?: boolean } | undefined)?.stickyEnd);
}

function reconcileColumnOrder(current: string[], defaults: string[]): string[] {
  const defaultSet = new Set(defaults);
  const kept = current.filter((id) => defaultSet.has(id));
  const missing = defaults.filter((id) => !kept.includes(id));
  return [...kept, ...missing];
}
