import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  flexRender,
  getCoreRowModel,
  getPaginationRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type ColumnSizingState,
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
  Columns3,
  GripVertical,
  Search,
} from "lucide-react";

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
};

export type ConsoleDataGridAction<T> = {
  disabled?: (rows: T[]) => boolean;
  label: string;
  onSelect: (rows: T[]) => void;
  tone?: "danger" | "normal";
};

type ConsoleDataGridPreferences = {
  columnOrder?: string[];
  columnSizing?: ColumnSizingState;
  columnVisibility?: VisibilityState;
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
  onOpenRow,
  renderExpandedRow,
  rows,
  searchPlaceholder = "Search",
  storageKey,
  title,
}: {
  actions?: ConsoleDataGridAction<T>[];
  columns: ConsoleDataGridColumn<T>[];
  defaultColumnVisibility?: VisibilityState;
  defaultPageSize?: number;
  empty?: ReactNode;
  getRowId: (row: T) => string;
  itemLabel?: string;
  onOpenRow?: (row: T) => void;
  renderExpandedRow?: (row: T) => ReactNode;
  rows: T[];
  searchPlaceholder?: string;
  storageKey: string;
  title: string;
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
  const [globalFilter, setGlobalFilter] = useState("");
  const [pageSize, setPageSize] = useState(
    preferences.pageSize ?? defaultPageSize,
  );
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
  const [sorting, setSorting] = useState<SortingState>(
    preferences.sorting ?? [],
  );
  const normalizedFilter = globalFilter.trim().toLocaleLowerCase();
  const filteredRows = useMemo(() => {
    if (!normalizedFilter) {
      return rows;
    }
    return rows.filter((row) =>
      columns.some((column) => {
        const value = column.searchValue?.(row) ?? column.sortValue?.(row);
        return String(value ?? "")
          .toLocaleLowerCase()
          .includes(normalizedFilter);
      }),
    );
  }, [columns, normalizedFilter, rows]);
  const tableColumns = useMemo<ColumnDef<T>[]>(
    () => [
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
            onChange={table.getToggleAllPageRowsSelectedHandler()}
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
            aria-label={`Select ${title} row`}
            checked={row.getIsSelected()}
            onChange={row.getToggleSelectedHandler()}
            type="checkbox"
          />
        ),
      },
      ...(renderExpandedRow
        ? [
            {
              id: "__expand",
              size: 42,
              minSize: 42,
              maxSize: 42,
              enableHiding: false,
              header: "",
              cell: ({ row }: { row: Row<T> }) => {
                const open = Boolean(expandedRows[row.id]);
                return (
                  <button
                    aria-expanded={open}
                    aria-label={`${open ? "Collapse" : "Expand"} ${title} row`}
                    className="iconButton gridIconButton"
                    onClick={(event) => {
                      event.stopPropagation();
                      setExpandedRows((current) => ({
                        ...current,
                        [row.id]: !current[row.id],
                      }));
                    }}
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
        cell: ({ row }: { row: Row<T> }) => (
          <span
            className={
              column.align === "end"
                ? "gridCellContent alignEnd"
                : "gridCellContent"
            }
          >
            {column.cell(row.original)}
          </span>
        ),
      })),
    ],
    [columns, expandedRows, renderExpandedRow, title],
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
    enableMultiRowSelection: true,
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
  const pageCount = table.getPageCount() || 1;
  const currentPage = table.getState().pagination.pageIndex + 1;

  useEffect(() => {
    table.setPageSize(pageSize);
  }, [pageSize, table]);

  useEffect(() => {
    writeGridPreferences(storageKey, {
      columnOrder: effectiveColumnOrder,
      columnSizing,
      columnVisibility,
      pageSize,
      sorting,
    });
  }, [
    columnOrder,
    columnSizing,
    columnVisibility,
    effectiveColumnOrder,
    pageSize,
    sorting,
    storageKey,
  ]);

  function invokeAction(action: ConsoleDataGridAction<T>, sourceRows?: T[]) {
    const actionRows = sourceRows ?? selectedRows;
    if (actionRows.length === 0 || action.disabled?.(actionRows)) {
      return;
    }
    action.onSelect(actionRows);
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

  return (
    <div className="consoleDataGrid" aria-label={`${title} data grid`}>
      <div className="gridToolbar">
        <div className="gridCounts">
          <strong>{title}</strong>
          <span>
            {filteredRows.length} of {rows.length} {itemLabel}
          </span>
          <span>{selectedRows.length} selected</span>
        </div>
        <label className="gridSearch">
          <Search size={15} />
          <input
            aria-label={`${title} search`}
            onChange={(event) => setGlobalFilter(event.target.value)}
            placeholder={searchPlaceholder}
            type="search"
            value={globalFilter}
          />
        </label>
        <div className="gridToolbarActions">
          {actions.length > 0 && (
            <DropdownMenu.Root>
              <DropdownMenu.Trigger asChild>
                <button
                  className="secondaryAction compactAction"
                  disabled={selectedRows.length === 0}
                  type="button"
                >
                  <span>Selection</span>
                  <ChevronDown size={16} />
                </button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content align="end" className="consoleMenu">
                  {actions.map((action) => (
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
                      key={action.label}
                      onSelect={() => invokeAction(action)}
                    >
                      {action.label}
                    </DropdownMenu.Item>
                  ))}
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
          )}
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button
                aria-label={`${title} columns`}
                className="iconButton"
                type="button"
              >
                <Columns3 size={17} />
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content align="end" className="consoleMenu">
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
            type="button"
          >
            <ChevronRight size={16} />
          </button>
        </div>
      </div>
      {table.getRowModel().rows.length === 0 ? (
        (empty ?? (
          <div className="emptyState compactEmpty">
            No {itemLabel} match the current view.
          </div>
        ))
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
            {table.getRowModel().rows.map((row) => (
              <ContextMenu.Root key={row.id}>
                <ContextMenu.Trigger asChild>
                  <div>
                    <div
                      className={
                        row.getIsSelected() ? "gridRow selected" : "gridRow"
                      }
                      onClick={() => onOpenRow?.(row.original)}
                      role="row"
                    >
                      {row.getVisibleCells().map((cell) => (
                        <div
                          className="gridCell"
                          key={cell.id}
                          role="gridcell"
                          style={{ width: cell.column.getSize() }}
                        >
                          {flexRender(
                            cell.column.columnDef.cell,
                            cell.getContext(),
                          )}
                        </div>
                      ))}
                    </div>
                    {renderExpandedRow && expandedRows[row.id] && (
                      <div className="gridExpandedRow">
                        {renderExpandedRow(row.original)}
                      </div>
                    )}
                  </div>
                </ContextMenu.Trigger>
                {actions.length > 0 && (
                  <ContextMenu.Portal>
                    <ContextMenu.Content className="consoleMenu">
                      <ContextMenu.Label className="consoleMenuLabel">
                        Row actions
                      </ContextMenu.Label>
                      {actions.map((action) => (
                        <ContextMenu.Item
                          className={
                            action.tone === "danger"
                              ? "consoleMenuItem danger"
                              : "consoleMenuItem"
                          }
                          disabled={action.disabled?.([row.original])}
                          key={action.label}
                          onSelect={() => invokeAction(action, [row.original])}
                        >
                          {action.label}
                        </ContextMenu.Item>
                      ))}
                    </ContextMenu.Content>
                  </ContextMenu.Portal>
                )}
              </ContextMenu.Root>
            ))}
          </div>
        </div>
      )}
    </div>
  );
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
  return (
    <div
      className={isDragging ? "gridHeaderCell dragging" : "gridHeaderCell"}
      ref={setNodeRef}
      role="columnheader"
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        width: header.getSize(),
      }}
    >
      {canDrag && (
        <button
          aria-label={`Reorder ${String(header.column.columnDef.header)} column`}
          className="gridDragHandle"
          type="button"
          {...attributes}
          {...listeners}
        >
          <GripVertical size={14} />
        </button>
      )}
      {header.isPlaceholder ? null : (
        <button
          className={
            header.column.getCanSort()
              ? "gridHeaderButton sortable"
              : "gridHeaderButton"
          }
          onClick={header.column.getToggleSortingHandler()}
          type="button"
        >
          {flexRender(header.column.columnDef.header, header.getContext())}
          {header.column.getIsSorted() === "asc"
            ? " ↑"
            : header.column.getIsSorted() === "desc"
              ? " ↓"
              : ""}
        </button>
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

function reconcileColumnOrder(current: string[], defaults: string[]): string[] {
  const defaultSet = new Set(defaults);
  const kept = current.filter((id) => defaultSet.has(id));
  const missing = defaults.filter((id) => !kept.includes(id));
  return [...kept, ...missing];
}
