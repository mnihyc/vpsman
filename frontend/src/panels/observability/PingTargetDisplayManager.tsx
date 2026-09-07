import { useEffect, useMemo, useState } from "react";
import {
  closestCenter,
  DndContext,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import * as Popover from "@radix-ui/react-popover";
import { GripVertical, RotateCcw, Save, X } from "lucide-react";
import {
  comparePingTargetDisplayOrder,
  defaultPingTargetColor,
  pingTargetColor,
} from "../../pingTargetDisplay";
import type { PingTargetView } from "../../types";
import { PingColorWheel } from "./PingColorWheel";

export type PingTargetDisplayDraft = { target_id: string; color: string };

function canonicalColor(value: string): string | null {
  const hex = value.trim();
  if (/^#[0-9a-f]{6}$/i.test(hex)) return hex.toUpperCase();
  if (/^#[0-9a-f]{3}$/i.test(hex)) {
    return `#${[...hex.slice(1)].map((digit) => digit + digit).join("")}`.toUpperCase();
  }
  return null;
}

function savedDraft(targets: PingTargetView[]): PingTargetDisplayDraft[] {
  return [...targets]
    .sort(
      (a, b) =>
        comparePingTargetDisplayOrder(a, b) || a.name.localeCompare(b.name),
    )
    .map((target) => ({
      target_id: target.id,
      color: pingTargetColor(target.id, target).toUpperCase(),
    }));
}

function sameDraft(
  left: PingTargetDisplayDraft[],
  right: PingTargetDisplayDraft[],
): boolean {
  return (
    left.length === right.length &&
    left.every(
      (row, index) =>
        row.target_id === right[index].target_id &&
        row.color === right[index].color,
    )
  );
}

export function PingTargetDisplayManager({
  targets,
  disabled,
  onSave,
}: {
  targets: PingTargetView[];
  disabled: boolean;
  onSave: (draft: PingTargetDisplayDraft[]) => Promise<void>;
}) {
  const [editor, setEditor] = useState(() => ({
    base: savedDraft(targets),
    draft: savedDraft(targets),
  }));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const byId = useMemo(
    () => new Map(targets.map((target) => [target.id, target])),
    [targets],
  );
  const dirty = !sameDraft(editor.base, editor.draft);
  const valid = editor.draft.every((row) => canonicalColor(row.color) !== null);
  const interactionDisabled = disabled || saving;
  const sensors = useSensors(
    // Match Groups: a small pointer movement distinguishes a drag from a click.
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  useEffect(() => {
    const next = savedDraft(targets);
    setEditor((current) => {
      if (sameDraft(current.base, current.draft))
        return { base: next, draft: next };
      const live = new Set(next.map((row) => row.target_id));
      const staged = new Set(current.draft.map((row) => row.target_id));
      return {
        base: next,
        // A definition refresh must not discard staged order or typed colors.
        draft: [
          ...current.draft.filter((row) => live.has(row.target_id)),
          ...next.filter((row) => !staged.has(row.target_id)),
        ],
      };
    });
  }, [targets]);

  function changeColor(targetId: string, color: string) {
    setError(null);
    setEditor((current) => ({
      ...current,
      draft: current.draft.map((row) =>
        row.target_id === targetId ? { ...row, color } : row,
      ),
    }));
  }

  async function save() {
    if (!dirty || !valid || interactionDisabled) return;
    setSaving(true);
    setError(null);
    try {
      await onSave(
        editor.draft.map((row) => ({
          ...row,
          color: canonicalColor(row.color)!,
        })),
      );
      setEditor((current) => ({ base: current.draft, draft: current.draft }));
      setAnnouncement("Ping display order and colors saved.");
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : "Display settings could not be saved.",
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <section
      className="tagOrderPanel pingDisplayPanel"
      aria-label="Manage Ping display order and colors"
      aria-busy={saving}
    >
      <div className="tagOrderPanelHeader">
        <div>
          <strong>Manage display order and colors</strong>
          <span>
            Drag targets to arrange Ping charts and legends. Display only;
            probes and primary assignments stay unchanged.
          </span>
        </div>
        <span
          className={`consoleStatusBadge ${error ? "danger" : dirty || saving ? "warning" : "ok"}`}
          role="status"
        >
          {saving ? "Saving" : dirty ? "Unsaved changes" : "Saved"}
        </span>
      </div>
      <div className="tagOrderSaveBar">
        <span>Stage changes, then save them once for all viewers.</span>
        <div className="buttonCluster">
          <button
            className="secondaryAction compactAction"
            type="button"
            disabled={!dirty || interactionDisabled}
            onClick={() => {
              setEditor((current) => ({ ...current, draft: current.base }));
              setError(null);
            }}
          >
            <RotateCcw size={15} aria-hidden="true" /> Revert
          </button>
          <button
            className="primaryAction compactAction"
            type="button"
            disabled={!dirty || !valid || interactionDisabled}
            onClick={() => void save()}
          >
            <Save size={15} aria-hidden="true" /> Save display
          </button>
        </div>
      </div>
      {error && (
        <div className="formError" role="alert">
          {error}
        </div>
      )}
      <span className="srOnly" role="status" aria-live="polite">
        {announcement}
      </span>
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={({ active, over }) => {
          if (interactionDisabled || !over || active.id === over.id) return;
          setEditor((current) => {
            const from = current.draft.findIndex(
              (row) => row.target_id === active.id,
            );
            const to = current.draft.findIndex(
              (row) => row.target_id === over.id,
            );
            if (from < 0 || to < 0) return current;
            setAnnouncement(
              `${byId.get(String(active.id))?.name ?? "Target"} moved to position ${to + 1}. Save to apply.`,
            );
            return { ...current, draft: arrayMove(current.draft, from, to) };
          });
        }}
      >
        <SortableContext
          items={editor.draft.map((row) => row.target_id)}
          strategy={verticalListSortingStrategy}
        >
          <div
            className="pingDisplayList"
            role="list"
            aria-label="Ping target display order"
          >
            {editor.draft.map((row) => {
              const target = byId.get(row.target_id);
              return target ? (
                <PingDisplayRow
                  key={row.target_id}
                  target={target}
                  color={row.color}
                  disabled={interactionDisabled}
                  onColor={(color) => changeColor(row.target_id, color)}
                />
              ) : null;
            })}
          </div>
        </SortableContext>
      </DndContext>
      {targets.length === 0 && (
        <div className="emptyState compactEmptyState">
          Create Ping targets before arranging their display.
        </div>
      )}
    </section>
  );
}

function PingDisplayRow({
  target,
  color,
  disabled,
  onColor,
}: {
  target: PingTargetView;
  color: string;
  disabled: boolean;
  onColor: (color: string) => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: target.id, disabled });
  const normalized = canonicalColor(color);
  return (
    <div
      ref={setNodeRef}
      role="listitem"
      className={`pingDisplayRow${isDragging ? " dragging" : ""}`}
      style={{ transform: CSS.Transform.toString(transform), transition }}
    >
      <button
        {...attributes}
        {...listeners}
        ref={setActivatorNodeRef}
        type="button"
        className="tagOrderHandle"
        disabled={disabled}
        aria-label={`Move ${target.name}`}
        title="Drag to reorder, or press Space then use arrow keys"
      >
        <GripVertical size={18} aria-hidden="true" />
      </button>
      <span className="pingDisplayIdentity">
        <strong>{target.name}</strong>
        <small>
          {target.probe_kind.toUpperCase()} · {target.host}
        </small>
      </span>
      <div className="pingDisplayColor">
        <Popover.Root>
          <Popover.Trigger asChild>
            <button
              className="pingColorSwatch"
              type="button"
              disabled={disabled}
              aria-label={`Choose color for ${target.name}`}
              style={{
                backgroundColor:
                  normalized ?? pingTargetColor(target.id, target),
              }}
            />
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Content
              className="pingColorPalette"
              align="end"
              sideOffset={8}
              collisionPadding={12}
              aria-label={`Color palette for ${target.name}`}
            >
              <div className="pingColorPaletteHeader">
                <strong>Target color</strong>
                <Popover.Close className="pingColorClose" aria-label="Close color palette">
                  <X size={16} aria-hidden="true" />
                </Popover.Close>
              </div>
              <PingColorWheel
                color={normalized ?? pingTargetColor(target.id, target)}
                onChange={onColor}
              />
              <button
                type="button"
                className="pingColorDefault"
                onClick={() =>
                  onColor(defaultPingTargetColor(target.id).toUpperCase())
                }
              >
                Default color
              </button>
            </Popover.Content>
          </Popover.Portal>
        </Popover.Root>
        <input
          className="pingColorHex"
          aria-label={`Color for ${target.name}`}
          aria-invalid={!normalized}
          aria-describedby={
            !normalized ? `ping-color-error-${target.id}` : undefined
          }
          type="text"
          value={color}
          disabled={disabled}
          spellCheck={false}
          onChange={(event) => onColor(event.target.value)}
          onBlur={() => {
            if (normalized) onColor(normalized);
          }}
          title="Hex color: #RGB or #RRGGBB"
        />
        {!normalized && (
          <small
            className="pingColorError"
            id={`ping-color-error-${target.id}`}
          >
            Use #RGB or #RRGGBB.
          </small>
        )}
      </div>
    </div>
  );
}
