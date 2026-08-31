import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import {
  Activity,
  Flag,
  LayoutPanelTop,
  Languages,
  ListChecks,
  MapPin,
  RotateCcw,
  RefreshCw,
  Route,
  Ruler,
  Save,
  ServerCog,
  Tags,
  TimerReset,
  Trash2,
  Wifi,
} from "lucide-react";
import { clearLocalStorageSelections } from "../localStorageSelections";
import { ActionFeedback } from "../components/ActionFeedback";
import { FRONTEND_BUILD_NUMBER } from "../buildInfo";
import { usePanelDisplaySettings } from "../panelDisplay";
import { DEFAULT_OPERATOR_PREFERENCES } from "../utils";
import { defaultFleetTagVisible, fleetTagVisible } from "../tagDisplay";
import type { OperatorPreferences, OperatorView, TagView } from "../types";

type PreferencesPanelProps = {
  error: string | null;
  loading: boolean;
  onRefreshSources: () => void;
  onSelectView: (view: "Access" | "System", subpage?: string) => void;
  operator: OperatorView | null;
  tags: TagView[];
};

const COMMON_TIMEZONES = [
  "UTC",
  "America/Los_Angeles",
  "America/New_York",
  "Europe/London",
  "Europe/Berlin",
  "Asia/Singapore",
  "Asia/Tokyo",
];

const DASHBOARD_TOP_LIMIT_OPTIONS = [3, 5, 8, 12, 16];
const DRAFT_VALIDATION_ERROR_ID = "preferences-draft-validation-error";

type PreferenceScopeTab = "browser" | "personal" | "system";

export function PreferencesPanel({
  error,
  loading,
  onRefreshSources,
  onSelectView,
  operator,
  tags,
}: PreferencesPanelProps) {
  const {
    preferences,
    preferencesError,
    preferencesSaving,
    updatePreferences,
  } = usePanelDisplaySettings();
  const operatorId = operator?.id ?? null;
  const operatorIdRef = useRef(operatorId);
  operatorIdRef.current = operatorId;
  const [draft, setDraft] = useState<OperatorPreferences>(preferences);
  const synchronizedPreferenceSourceRef = useRef({
    operatorId,
    preferences,
  });
  const [localSelectionMessage, setLocalSelectionMessage] = useState<
    string | null
  >(null);
  const [activeScope, setActiveScope] =
    useState<PreferenceScopeTab>("personal");
  const [tagVisibilityFilter, setTagVisibilityFilter] = useState("");
  const savePendingRef = useRef(false);
  const [localSavePending, setLocalSavePending] = useState(false);
  const browserTimezone = useMemo(
    () =>
      Intl.DateTimeFormat().resolvedOptions().timeZone || "local browser time",
    [],
  );
  const timezonePreview = useMemo(
    () => previewTimezone(draft.timezone || browserTimezone),
    [browserTimezone, draft.timezone],
  );
  const filteredVisibilityTags = useMemo(() => {
    const filter = tagVisibilityFilter.trim().toLowerCase();
    return filter
      ? tags.filter((tag) => tag.name.toLowerCase().includes(filter))
      : tags;
  }, [tagVisibilityFilter, tags]);
  const visibleFleetTagCount = useMemo(
    () =>
      tags.filter((tag) =>
        fleetTagVisible(tag.name, draft.fleet_tag_visibility_overrides),
      ).length,
    [draft.fleet_tag_visibility_overrides, tags],
  );
  const dirty = !operatorPreferencesEqual(draft, preferences);
  const saveInFlight = preferencesSaving || localSavePending;
  const timezoneValidationError = validateTimezone(
    draft.timezone?.trim() || null,
  );
  const draftValidationError =
    timezoneValidationError ??
    validateDashboardLimits(
      draft.dashboard_resource_top_limit,
      draft.dashboard_network_top_limit,
    ) ??
    validateFleetTagVisibilityOverrides(
      normalizeFleetTagVisibilityOverrides(
        draft.fleet_tag_visibility_overrides,
      ),
    );
  const saveDisabled = !dirty || saveInFlight;

  useEffect(() => {
    const previousSource = synchronizedPreferenceSourceRef.current;
    synchronizedPreferenceSourceRef.current = { operatorId, preferences };
    setDraft((current) =>
      previousSource.operatorId !== operatorId ||
      operatorPreferencesEqual(current, previousSource.preferences)
        ? preferences
        : current,
    );
  }, [operatorId, preferences]);

  async function savePreferences(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!dirty || saveInFlight || savePendingRef.current) {
      return;
    }
    const submittedDraft = draft;
    const submittedOperatorId = operatorId;
    const timezone = submittedDraft.timezone?.trim() || null;
    const dashboardCurveExclusions = normalizeCurveExclusions(
      submittedDraft.dashboard_curve_exclusions,
    );
    const fleetTagVisibilityOverrides = normalizeFleetTagVisibilityOverrides(
      submittedDraft.fleet_tag_visibility_overrides,
    );
    if (draftValidationError) {
      return;
    }
    const submittedPreferences: OperatorPreferences = {
      ...submittedDraft,
      dashboard_curve_exclusions: dashboardCurveExclusions,
      fleet_tag_visibility_overrides: fleetTagVisibilityOverrides,
      timezone,
    };
    savePendingRef.current = true;
    setLocalSavePending(true);
    try {
      const savedPreferences = await updatePreferences(submittedPreferences);
      if (
        savedPreferences !== null &&
        operatorIdRef.current === submittedOperatorId
      ) {
        setDraft((current) =>
          operatorPreferencesEqual(current, submittedDraft)
            ? savedPreferences
            : current,
        );
      }
    } catch {
      // The shared preference context exposes the API error for rendering.
    } finally {
      savePendingRef.current = false;
      setLocalSavePending(false);
    }
  }

  function resetPreferences() {
    setDraft(preferences);
  }

  function resetLocalSelections() {
    const cleared = clearLocalStorageSelections();
    setLocalSelectionMessage(
      cleared === 0
        ? "Local console selections are already at defaults."
        : `Cleared ${cleared} local console selection${cleared === 1 ? "" : "s"}. Reloading defaults...`,
    );
    if (cleared > 0) {
      window.setTimeout(() => window.location.reload(), 250);
    }
  }

  function setFleetTagVisibility(tag: string, visible: boolean) {
    setDraft((current) => {
      const nextOverrides = { ...current.fleet_tag_visibility_overrides };
      if (visible === defaultFleetTagVisible(tag)) {
        delete nextOverrides[tag];
      } else {
        nextOverrides[tag] = visible;
      }
      return {
        ...current,
        fleet_tag_visibility_overrides: nextOverrides,
      };
    });
  }

  function resetFleetTagVisibility() {
    setDraft((current) => ({
      ...current,
      fleet_tag_visibility_overrides: {},
    }));
  }

  function resetDraftPatch(patch: Partial<OperatorPreferences>) {
    setDraft((current) => ({
      ...current,
      ...patch,
    }));
  }

  const changedPreferenceCount = preferenceChangedCount(draft, preferences);

  return (
    <div className="workspace singleColumn preferencesWorkspace">
      <section className="fleetPanel preferencesPanel">
        <div className="sectionHeader">
          <div>
            <h2>Operator preferences</h2>
            <span>
              {operator
                ? `${operator.username} / ${operator.role}`
                : "Current authenticated operator"}
            </span>
          </div>
          <div className="headerActionStack">
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                loading
                  ? "Preference sources are already loading"
                  : dirty
                    ? "Save or reset unsaved preference changes before refreshing"
                    : undefined
              }
              disabled={loading || dirty}
              onClick={onRefreshSources}
              title="Refresh the current operator profile and tag registry"
              type="button"
            >
              <RefreshCw size={14} />
              <span>{loading ? "Loading" : "Refresh"}</span>
            </button>
            <span
              className={
                dirty ? "consoleStatusBadge warning" : "consoleStatusBadge ok"
              }
            >
              {dirty ? "Unsaved changes" : "Saved"}
            </span>
          </div>
        </div>
        <ActionFeedback
          className="localActionFeedback"
          message={
            error ?? (loading ? "Refreshing operator and tag registry" : null)
          }
          tone={error ? "danger" : "progress"}
        />

        <form className="preferencesForm" onSubmit={savePreferences}>
          <section
            className="preferenceScopeOverview"
            aria-label="Preferences scope overview"
          >
            <PreferenceScopeTile
              active={activeScope === "personal"}
              detail="Timezone, language, display labels, flags, sidebar behavior, review prompt display, tag visibility, Home chart presentation, and output comparison affect this operator's console experience."
              label="Personal display"
              onSelect={() => setActiveScope("personal")}
              value="Personal — stored for this operator"
            />
            <PreferenceScopeTile
              active={activeScope === "browser"}
              detail="Saved views, table layouts, Home telemetry selectors, and expanded panels are browser-local and can be cleared without changing server preferences."
              label="Browser state"
              onSelect={() => setActiveScope("browser")}
              value="Browser — stored on this device"
            />
            <PreferenceScopeTile
              active={activeScope === "system"}
              detail="Gateway install material and tunnel allocation pools belong to shared system workflows, not personal display preferences."
              label="System-linked defaults"
              onSelect={() => setActiveScope("system")}
              value="System — shared workflow settings"
            />
          </section>

          <section
            className={`preferenceStickySaveBar ${dirty ? "dirty" : ""}`}
            aria-label="Preferences sticky save bar"
          >
            <div>
              <strong>
                {dirty
                  ? `${changedPreferenceCount} changed setting${changedPreferenceCount === 1 ? "" : "s"}`
                  : "No preference changes"}
              </strong>
              <span>
                {activeScope === "system"
                  ? "System-linked defaults are routed to Suite Config and Access workflows."
                  : dirty
                    ? "Save applies only the operator preference draft."
                    : "Personal and browser-local controls are separated from shared system defaults."}
              </span>
              <ActionFeedback
                className="preferenceDraftValidation"
                id={DRAFT_VALIDATION_ERROR_ID}
                message={draftValidationError}
                tone="warning"
              />
            </div>
            <div className="buttonCluster">
              <button
                className="secondaryAction compactAction"
                data-tooltip-disabled-reason={preferenceResetDisabledReason(
                  dirty,
                  saveInFlight,
                )}
                disabled={!dirty || saveInFlight}
                onClick={resetPreferences}
                type="button"
              >
                <RotateCcw size={16} />
                <span>Reset draft</span>
              </button>
              <button
                aria-describedby={
                  draftValidationError ? DRAFT_VALIDATION_ERROR_ID : undefined
                }
                className="primaryAction compactAction"
                data-tooltip-disabled-reason={preferenceSaveDisabledReason(
                  dirty,
                  saveInFlight,
                  draftValidationError,
                )}
                disabled={saveDisabled}
                type="submit"
              >
                <Save size={16} />
                <span>{saveInFlight ? "Saving" : "Save changes"}</span>
              </button>
            </div>
          </section>

          {activeScope === "personal" && (
            <PreferenceSection
              description="Personal operator presentation choices. These do not change fleet behavior or another operator's console."
              title="Personal display preferences"
            >
              <PreferenceGroup
                description="Country columns show a compact flag plus code when enabled; turn this off for code-only compact rows such as US, DE, or JP."
                icon={<Flag size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    show_country_flags:
                      DEFAULT_OPERATOR_PREFERENCES.show_country_flags,
                  })
                }
                resetDisabled={
                  draft.show_country_flags ===
                  DEFAULT_OPERATOR_PREFERENCES.show_country_flags
                }
                scope="Personal"
                title="Country flags"
              >
                <label className="checkLine inlineCheck">
                  <input
                    checked={draft.show_country_flags}
                    name="show_country_flags"
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        show_country_flags: event.target.checked,
                      }))
                    }
                    type="checkbox"
                  />
                  <span>Show flag next to country code</span>
                </label>
              </PreferenceGroup>

              <PreferenceGroup
                description="Fleet / Instances keeps the original single-line country cell by default. Country + region adds the independent region tag on a second line without resizing the column. Full location remains available in tooltips and details."
                icon={<MapPin size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    fleet_location_display_mode:
                      DEFAULT_OPERATOR_PREFERENCES.fleet_location_display_mode,
                  })
                }
                resetDisabled={
                  draft.fleet_location_display_mode ===
                  DEFAULT_OPERATOR_PREFERENCES.fleet_location_display_mode
                }
                scope="Personal"
                title="Fleet table location"
              >
                <label>
                  <span>Location display</span>
                  <select
                    aria-label="Fleet table location"
                    name="fleet_location_display_mode"
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        fleet_location_display_mode:
                          event.target.value === "country_region"
                            ? "country_region"
                            : "country_only",
                      }))
                    }
                    value={draft.fleet_location_display_mode}
                  >
                    <option value="country_only">Country only</option>
                    <option value="country_region">Country + region</option>
                  </select>
                </label>
              </PreferenceGroup>

              <PreferenceGroup
                description="Controls displayed byte quantities and byte-per-second rates across the console. Decimal is the default and uses powers of 1000; Binary uses powers of 1024. Stored byte values and explicitly configured rule units do not change."
                icon={<Ruler size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    byte_unit_display_mode:
                      DEFAULT_OPERATOR_PREFERENCES.byte_unit_display_mode,
                  })
                }
                resetDisabled={
                  draft.byte_unit_display_mode ===
                  DEFAULT_OPERATOR_PREFERENCES.byte_unit_display_mode
                }
                scope="Personal"
                title="Byte units"
              >
                <label>
                  <span>Unit system</span>
                  <select
                    aria-label="Byte unit system"
                    name="byte_unit_display_mode"
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        byte_unit_display_mode:
                          event.target.value === "binary"
                            ? "binary"
                            : "decimal",
                      }))
                    }
                    value={draft.byte_unit_display_mode}
                  >
                    <option value="decimal">Decimal · KB, MB, GB, TB</option>
                    <option value="binary">Binary · KiB, MiB, GiB, TiB</option>
                  </select>
                </label>
              </PreferenceGroup>

              <PreferenceGroup
                description="Controls how VPS labels are rendered in tables, drawers, and action previews."
                icon={<ServerCog size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    vps_name_display_mode:
                      DEFAULT_OPERATOR_PREFERENCES.vps_name_display_mode,
                  })
                }
                resetDisabled={
                  draft.vps_name_display_mode ===
                  DEFAULT_OPERATOR_PREFERENCES.vps_name_display_mode
                }
                scope="Personal"
                title="VPS name format"
              >
                <label>
                  <span>Name display</span>
                  <select
                    name="vps_name_display_mode"
                    value={draft.vps_name_display_mode}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        vps_name_display_mode:
                          event.target.value === "name"
                            ? "name"
                            : "name_id_suffix",
                      }))
                    }
                  >
                    <option value="name_id_suffix">
                      Name with client ID suffix
                    </option>
                    <option value="name">Name only</option>
                  </select>
                </label>
              </PreferenceGroup>

              <PreferenceGroup
                description="Controls which registry tags render inside the Fleet Tags column for this operator."
                icon={<Tags size={18} />}
                onReset={resetFleetTagVisibility}
                resetDisabled={
                  Object.keys(draft.fleet_tag_visibility_overrides).length === 0
                }
                scope="Personal"
                title="Fleet tag visibility"
              >
                <div className="preferenceTagVisibilityToolbar">
                  <input
                    aria-label="Filter Fleet tag visibility"
                    name="fleet_tag_visibility_filter"
                    onChange={(event) =>
                      setTagVisibilityFilter(event.target.value)
                    }
                    placeholder="Filter tags"
                    value={tagVisibilityFilter}
                  />
                  <button
                    className="secondaryAction compactAction"
                    data-tooltip-disabled-reason="Fleet tag visibility already uses its default settings."
                    disabled={
                      Object.keys(draft.fleet_tag_visibility_overrides)
                        .length === 0
                    }
                    onClick={resetFleetTagVisibility}
                    type="button"
                  >
                    <RotateCcw size={14} />
                    <span>Reset</span>
                  </button>
                </div>
                <div className="preferenceHint">
                  <strong>{visibleFleetTagCount} shown</strong>
                  <span>{tags.length - visibleFleetTagCount} hidden</span>
                </div>
                {tags.length === 0 ? (
                  <div className="preferenceHint">
                    <strong>No registry tags</strong>
                    <span>
                      Create tags before setting Fleet column visibility.
                    </span>
                  </div>
                ) : (
                  <div className="preferenceTagVisibilityList">
                    {filteredVisibilityTags.map((tag) => {
                      const checked = fleetTagVisible(
                        tag.name,
                        draft.fleet_tag_visibility_overrides,
                      );
                      const defaultVisible = defaultFleetTagVisible(tag.name);
                      return (
                        <label className="tagVisibilityLine" key={tag.name}>
                          <input
                            checked={checked}
                            name="fleet_tag_visibility"
                            onChange={(event) =>
                              setFleetTagVisibility(
                                tag.name,
                                event.target.checked,
                              )
                            }
                            type="checkbox"
                          />
                          <span className="tags">
                            <em>{tag.name}</em>
                          </span>
                          <small>
                            {tag.clients.length} VPS
                            {tag.clients.length === 1 ? "" : "s"} / default{" "}
                            {defaultVisible ? "shown" : "hidden"}
                          </small>
                        </label>
                      );
                    })}
                    {filteredVisibilityTags.length === 0 && (
                      <div className="preferenceHint">
                        <strong>No matching tags</strong>
                        <span>{tagVisibilityFilter.trim()}</span>
                      </div>
                    )}
                  </div>
                )}
              </PreferenceGroup>

              <PreferenceGroup
                description="Times remain ISO UTC in the API; this only changes how the console renders timestamps."
                icon={<TimerReset size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    timezone: DEFAULT_OPERATOR_PREFERENCES.timezone,
                  })
                }
                resetDisabled={
                  draft.timezone === DEFAULT_OPERATOR_PREFERENCES.timezone
                }
                scope="Personal"
                title="Timezone"
              >
                <label>
                  <span>Display timezone</span>
                  <input
                    aria-describedby={
                      timezoneValidationError
                        ? DRAFT_VALIDATION_ERROR_ID
                        : undefined
                    }
                    aria-invalid={Boolean(timezoneValidationError)}
                    list="operator-timezones"
                    name="timezone"
                    placeholder={browserTimezone}
                    value={draft.timezone ?? ""}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        timezone: event.target.value.trim()
                          ? event.target.value
                          : null,
                      }))
                    }
                  />
                  <datalist id="operator-timezones">
                    {COMMON_TIMEZONES.map((timezone) => (
                      <option key={timezone} value={timezone} />
                    ))}
                  </datalist>
                </label>
                <div className="preferenceHint">
                  <strong>
                    {draft.timezone ? draft.timezone : "Browser timezone"}
                  </strong>
                  <span>{timezonePreview}</span>
                </div>
              </PreferenceGroup>

              <PreferenceGroup
                description="Language is stored with the operator profile for future localization. English is the current console language."
                icon={<Languages size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    language: DEFAULT_OPERATOR_PREFERENCES.language,
                  })
                }
                resetDisabled={
                  draft.language === DEFAULT_OPERATOR_PREFERENCES.language
                }
                scope="Personal"
                title="Language"
              >
                <label>
                  <span>Console language</span>
                  <select
                    name="language"
                    value={draft.language}
                    onChange={() =>
                      setDraft((current) => ({
                        ...current,
                        language: "en",
                      }))
                    }
                  >
                    <option value="en">English</option>
                  </select>
                </label>
              </PreferenceGroup>

              <PreferenceGroup
                description="Choose how left-sidebar subpanels open before any local manual expand/collapse overrides."
                icon={<LayoutPanelTop size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    sidebar_subpanel_default:
                      DEFAULT_OPERATOR_PREFERENCES.sidebar_subpanel_default,
                  })
                }
                resetDisabled={
                  draft.sidebar_subpanel_default ===
                  DEFAULT_OPERATOR_PREFERENCES.sidebar_subpanel_default
                }
                scope="Personal"
                title="Sidebar subpanels"
              >
                <label>
                  <span>Default expansion</span>
                  <select
                    name="sidebar_subpanel_default"
                    value={draft.sidebar_subpanel_default}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        sidebar_subpanel_default:
                          event.target.value === "all" ? "all" : "active",
                      }))
                    }
                  >
                    <option value="active">Active section expanded</option>
                    <option value="all">All sections expanded</option>
                  </select>
                </label>
              </PreferenceGroup>

              <PreferenceGroup
                description="Choose whether reviewed action prompts stay inline in the page or open as overlay dialogs. This is a personal display preference; it does not weaken required review, privilege, or audit workflows."
                icon={<LayoutPanelTop size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    review_prompt_mode:
                      DEFAULT_OPERATOR_PREFERENCES.review_prompt_mode,
                  })
                }
                resetDisabled={
                  draft.review_prompt_mode ===
                  DEFAULT_OPERATOR_PREFERENCES.review_prompt_mode
                }
                scope="Personal"
                title="Review prompts"
              >
                <label>
                  <span>Prompt display</span>
                  <select
                    aria-label="Review prompt display mode"
                    name="review_prompt_mode"
                    value={draft.review_prompt_mode}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        review_prompt_mode:
                          event.target.value === "overlay"
                            ? "overlay"
                            : "inline",
                      }))
                    }
                  >
                    <option value="inline">Inline in page</option>
                    <option value="overlay">Overlay dialog</option>
                  </select>
                </label>
                <div className="preferenceHint preferenceHintStack">
                  <strong>
                    {draft.review_prompt_mode === "overlay"
                      ? "Overlay dialog"
                      : "Inline in page"}
                  </strong>
                  <span>
                    Inline keeps the review beside the form. Overlay brings the
                    frozen review snapshot to the front when the page is dense.
                  </span>
                </div>
              </PreferenceGroup>

              <PreferenceGroup
                description="Controls how bulk job result groups are compared before individual target output chunks are shown."
                icon={<ListChecks size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    bulk_output_compare_mode:
                      DEFAULT_OPERATOR_PREFERENCES.bulk_output_compare_mode,
                  })
                }
                resetDisabled={
                  draft.bulk_output_compare_mode ===
                  DEFAULT_OPERATOR_PREFERENCES.bulk_output_compare_mode
                }
                scope="Personal"
                title="Bulk execution summaries"
              >
                <label>
                  <span>Default comparison</span>
                  <select
                    aria-label="Bulk output comparison default"
                    name="bulk_output_compare_mode"
                    value={draft.bulk_output_compare_mode}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        bulk_output_compare_mode:
                          event.target.value === "text" ? "text" : "binary",
                      }))
                    }
                  >
                    <option value="binary">Binary exact</option>
                    <option value="text">Text normalized</option>
                  </select>
                </label>
                <div className="preferenceHint preferenceHintStack">
                  <strong>
                    {draft.bulk_output_compare_mode === "text"
                      ? "Text normalized"
                      : "Binary exact"}
                  </strong>
                  <span>
                    Binary exact compares bytes and is safest for security,
                    checksums, generated files, and command output where
                    whitespace matters. Text normalized is only for human log
                    review when line endings and trailing whitespace are noise.
                  </span>
                </div>
              </PreferenceGroup>

              <PreferenceGroup
                description="Controls this operator's Home resource/network curve ranking and exclusions. Fleet-wide observability policy belongs in shared system settings, not here."
                icon={<Activity size={18} />}
                onReset={() =>
                  resetDraftPatch({
                    dashboard_curve_exclusions:
                      DEFAULT_OPERATOR_PREFERENCES.dashboard_curve_exclusions,
                    dashboard_network_top_limit:
                      DEFAULT_OPERATOR_PREFERENCES.dashboard_network_top_limit,
                    dashboard_resource_top_limit:
                      DEFAULT_OPERATOR_PREFERENCES.dashboard_resource_top_limit,
                  })
                }
                resetDisabled={
                  draft.dashboard_network_top_limit ===
                    DEFAULT_OPERATOR_PREFERENCES.dashboard_network_top_limit &&
                  draft.dashboard_resource_top_limit ===
                    DEFAULT_OPERATOR_PREFERENCES.dashboard_resource_top_limit &&
                  draft.dashboard_curve_exclusions.length === 0
                }
                scope="Personal"
                title="Home chart presentation"
              >
                <div className="preferenceInlineControls">
                  <label>
                    <span>Resource top count</span>
                    <select
                      aria-label="Resource curve top count"
                      name="dashboard_resource_top_limit"
                      value={draft.dashboard_resource_top_limit}
                      onChange={(event) =>
                        setDraft((current) => ({
                          ...current,
                          dashboard_resource_top_limit: Number(
                            event.target.value,
                          ),
                        }))
                      }
                    >
                      {DASHBOARD_TOP_LIMIT_OPTIONS.map((value) => (
                        <option key={value} value={value}>
                          {value}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label>
                    <span>Network top count</span>
                    <select
                      aria-label="Network top count"
                      name="dashboard_network_top_limit"
                      value={draft.dashboard_network_top_limit}
                      onChange={(event) =>
                        setDraft((current) => ({
                          ...current,
                          dashboard_network_top_limit: Number(
                            event.target.value,
                          ),
                        }))
                      }
                    >
                      {DASHBOARD_TOP_LIMIT_OPTIONS.map((value) => (
                        <option key={value} value={value}>
                          {value}
                        </option>
                      ))}
                    </select>
                  </label>
                </div>
                <label>
                  <span>Curve exclusions</span>
                  <textarea
                    aria-label="Home telemetry curve exclusions"
                    name="dashboard_curve_exclusions"
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        dashboard_curve_exclusions: splitCurveExclusions(
                          event.target.value,
                        ),
                      }))
                    }
                    placeholder={
                      "provider:test\ncountry:lab\nname:lab\nid:agent-dev-"
                    }
                    rows={5}
                    value={draft.dashboard_curve_exclusions.join("\n")}
                  />
                </label>
                <div className="preferenceHint">
                  <strong>
                    {
                      normalizeCurveExclusions(draft.dashboard_curve_exclusions)
                        .length
                    }{" "}
                    exclusions
                  </strong>
                  <span>
                    Applied before this operator's top-N resource and network
                    curves are selected.
                  </span>
                </div>
              </PreferenceGroup>
            </PreferenceSection>
          )}

          {activeScope === "browser" && (
            <PreferenceSection
              description="Browser-only state that affects this device, not the operator record or other consoles."
              title="Local browser state"
            >
              <PreferenceGroup
                description="Clears browser-only console state such as Home telemetry selectors, saved fleet views, table layout, paging, column visibility, and expanded panels."
                icon={<Trash2 size={18} />}
                scope="Browser"
                title="Local console selections"
              >
                <div className="preferenceResetRow">
                  <div className="preferenceHint preferenceHintStack">
                    <strong>
                      Server preferences, signed-in session, and Privilege Vault
                      are preserved.
                    </strong>
                    <span>
                      After clearing, this page reloads and reads default local
                      selections without changing route.
                    </span>
                  </div>
                  <button
                    className="secondaryAction"
                    onClick={resetLocalSelections}
                    type="button"
                  >
                    <Trash2 size={18} />
                    <span>Clear local selections</span>
                  </button>
                </div>
                <ActionFeedback
                  className="localActionFeedback preferencesSelectionActionFeedback"
                  message={localSelectionMessage}
                  tone="success"
                />
              </PreferenceGroup>
            </PreferenceSection>
          )}

          {activeScope === "system" && (
            <PreferenceSection
              description="Operational defaults are managed in their owning workflows. Preferences links to them but does not edit them as personal display settings."
              title="System-linked defaults"
            >
              <SystemLinkedPreferenceRow
                icon={<Wifi size={18} />}
                title="Gateway install material"
                scope="System / Access"
                detail="Gateway runtime binds and its private-key file live in Suite Config. Reusable agent-installer endpoints, server public key, and install mode are edited under Access / Gateway sessions."
                primaryAction="Open gateway settings"
                onPrimary={() => onSelectView("Access", "gateway_sessions")}
                secondaryAction="Open VPS identities"
                onSecondary={() => onSelectView("Access", "vps_identities")}
              />
              <SystemLinkedPreferenceRow
                icon={<Route size={18} />}
                title="Tunnel allocation pools"
                scope="System / Suite Config"
                detail="Open System / Suite Config, choose Network, and edit the shared IPv4 and IPv6 tunnel allocation pools. Advanced TOML is not required."
                primaryAction="Open Suite Config"
                onPrimary={() => onSelectView("System", "suite_config")}
              />
            </PreferenceSection>
          )}

          <ActionFeedback
            className="localActionFeedback preferencesActionFeedback"
            message={preferencesError}
            tone="danger"
          />

          <div className="preferencesActions">
            <button
              className="secondaryAction"
              data-tooltip-disabled-reason={preferenceResetDisabledReason(
                dirty,
                saveInFlight,
              )}
              disabled={!dirty || saveInFlight}
              onClick={resetPreferences}
              type="button"
            >
              <RotateCcw size={18} />
              <span>Reset</span>
            </button>
            <button
              aria-describedby={
                draftValidationError ? DRAFT_VALIDATION_ERROR_ID : undefined
              }
              className="primaryAction"
              data-tooltip-disabled-reason={preferenceSaveDisabledReason(
                dirty,
                saveInFlight,
                draftValidationError,
              )}
              disabled={saveDisabled}
              type="submit"
            >
              <Save size={18} />
              <span>{saveInFlight ? "Saving" : "Save preferences"}</span>
            </button>
          </div>
          <p
            className="preferenceBuildNote"
            title={`Frontend build number: ${FRONTEND_BUILD_NUMBER}.`}
          >
            Console build {FRONTEND_BUILD_NUMBER}
          </p>
        </form>
      </section>
    </div>
  );
}

function PreferenceGroup({
  children,
  description,
  icon,
  onReset,
  resetDisabled,
  scope,
  title,
}: {
  children: ReactNode;
  description: string;
  icon: ReactNode;
  onReset?: () => void;
  resetDisabled?: boolean;
  scope: "Browser" | "Fleet/system" | "Personal";
  title: string;
}) {
  return (
    <section className="preferenceGroup">
      <div className="preferenceGroupHeader">
        <span className="preferenceIcon">{icon}</span>
        <div>
          <div className="preferenceTitleRow">
            <h3>{title}</h3>
            <span className={`preferenceScopeBadge ${scopeClass(scope)}`}>
              {scope}
            </span>
          </div>
          <p>{description}</p>
        </div>
        {onReset && (
          <button
            aria-label={`Reset ${title} to default`}
            className="secondaryAction compactAction preferenceCardReset"
            data-tooltip-disabled-reason={`${title} already uses its default setting.`}
            disabled={resetDisabled}
            onClick={onReset}
            title={
              resetDisabled
                ? `${title} already uses its default setting.`
                : `Reset ${title} to default.`
            }
            type="button"
          >
            <RotateCcw size={15} />
            <span>Reset</span>
          </button>
        )}
      </div>
      <div className="preferenceControls">{children}</div>
    </section>
  );
}

function PreferenceSection({
  children,
  description,
  title,
}: {
  children: ReactNode;
  description: string;
  title: string;
}) {
  return (
    <section className="preferenceSection" aria-label={title}>
      <div className="preferenceSectionHeader">
        <h3>{title}</h3>
        <p>{description}</p>
      </div>
      <div className="preferenceSectionGrid">{children}</div>
    </section>
  );
}

function PreferenceScopeTile({
  active,
  detail,
  label,
  onSelect,
  value,
}: {
  active: boolean;
  detail: string;
  label: string;
  onSelect: () => void;
  value: string;
}) {
  return (
    <button
      aria-pressed={active}
      className={`preferenceScopeTile ${active ? "active" : ""}`}
      onClick={onSelect}
      type="button"
    >
      <small>{label}</small>
      <strong>{value}</strong>
      <p>{detail}</p>
    </button>
  );
}

function preferenceResetDisabledReason(dirty: boolean, saveInFlight: boolean) {
  return saveInFlight
    ? "The preference draft cannot be reset while it is being saved."
    : dirty
      ? "The preference draft can be reset."
      : "The preference draft has no unsaved changes to reset.";
}

function preferenceSaveDisabledReason(
  dirty: boolean,
  saveInFlight: boolean,
  validationError: string | null,
) {
  if (saveInFlight) {
    return "Preferences are already being saved.";
  }
  if (validationError) {
    return `Preferences cannot be saved until the draft is valid: ${validationError}`;
  }
  return dirty
    ? "Preference changes are ready to save."
    : "There are no preference changes to save.";
}

function SystemLinkedPreferenceRow({
  detail,
  icon,
  onPrimary,
  onSecondary,
  primaryAction,
  scope,
  secondaryAction,
  title,
}: {
  detail: string;
  icon: ReactNode;
  onPrimary: () => void;
  onSecondary?: () => void;
  primaryAction: string;
  scope: string;
  secondaryAction?: string;
  title: string;
}) {
  return (
    <article className="systemLinkedPreferenceRow">
      <span className="preferenceIcon">{icon}</span>
      <div>
        <div className="preferenceTitleRow">
          <h3>{title}</h3>
          <span className="preferenceScopeBadge operational">{scope}</span>
        </div>
        <p>{detail}</p>
      </div>
      <div className="systemLinkedPreferenceActions">
        <button
          className="primaryAction compactAction"
          onClick={onPrimary}
          type="button"
        >
          {primaryAction}
        </button>
        {secondaryAction && onSecondary ? (
          <button
            className="secondaryAction compactAction"
            onClick={onSecondary}
            type="button"
          >
            {secondaryAction}
          </button>
        ) : null}
      </div>
    </article>
  );
}

function preferenceChangedCount(
  draft: OperatorPreferences,
  saved: OperatorPreferences,
): number {
  return (
    Object.keys(DEFAULT_OPERATOR_PREFERENCES) as Array<
      keyof OperatorPreferences
    >
  ).filter((key) => JSON.stringify(draft[key]) !== JSON.stringify(saved[key]))
    .length;
}

function operatorPreferencesEqual(
  left: OperatorPreferences,
  right: OperatorPreferences,
): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function scopeClass(scope: "Browser" | "Fleet/system" | "Personal"): string {
  if (scope === "Fleet/system") {
    return "operational";
  }
  if (scope === "Browser") {
    return "browser";
  }
  return "personal";
}

function splitCurveExclusions(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function normalizeCurveExclusions(values: string[]): string[] {
  const normalized: string[] = [];
  for (const value of values) {
    const trimmed = value.trim();
    if (
      !trimmed ||
      trimmed.length > 128 ||
      normalized.includes(trimmed) ||
      normalized.length >= 50
    ) {
      continue;
    }
    normalized.push(trimmed);
  }
  return normalized;
}

function normalizeFleetTagVisibilityOverrides(
  values: Record<string, boolean>,
): Record<string, boolean> {
  const normalized: Record<string, boolean> = {};
  for (const [tag, visible] of Object.entries(values)) {
    const trimmed = tag.trim();
    if (
      !isValidPreferenceTagName(trimmed) ||
      Object.keys(normalized).length >= 500
    ) {
      continue;
    }
    normalized[trimmed] = visible;
  }
  return normalized;
}

function validateFleetTagVisibilityOverrides(
  values: Record<string, boolean>,
): string | null {
  const entries = Object.keys(values);
  if (entries.length > 500) {
    return "Fleet tag visibility has too many overrides.";
  }
  if (entries.some((tag) => !isValidPreferenceTagName(tag))) {
    return "Fleet tag visibility contains an invalid tag.";
  }
  return null;
}

function isValidPreferenceTagName(tag: string): boolean {
  return (
    tag.length > 0 &&
    tag.length <= 128 &&
    !tag.startsWith("id:") &&
    !tag.startsWith("name:") &&
    /^[A-Za-z0-9_.:-]+$/.test(tag)
  );
}

function validateDashboardLimits(
  resourceTopLimit: number,
  networkTopLimit: number,
): string | null {
  if (
    !Number.isInteger(resourceTopLimit) ||
    resourceTopLimit < 3 ||
    resourceTopLimit > 16
  ) {
    return "Resource curve top count must be between 3 and 16";
  }
  if (
    !Number.isInteger(networkTopLimit) ||
    networkTopLimit < 3 ||
    networkTopLimit > 16
  ) {
    return "Network top count must be between 3 and 16";
  }
  return null;
}

function validateTimezone(timezone: string | null): string | null {
  if (!timezone) {
    return null;
  }
  try {
    new Intl.DateTimeFormat(undefined, { timeZone: timezone }).format(
      new Date(),
    );
    return null;
  } catch {
    return "Timezone must be a valid IANA identifier such as UTC or America/Los_Angeles";
  }
}

function previewTimezone(timezone: string): string {
  try {
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: timezone,
    }).format(new Date());
  } catch {
    return "Invalid timezone";
  }
}
