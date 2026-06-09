import {
  useEffect,
  useMemo,
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
  RotateCcw,
  Save,
  ServerCog,
  TimerReset,
  Trash2,
} from "lucide-react";
import { clearLocalStorageSelections } from "../localStorageSelections";
import { FRONTEND_BUILD_NUMBER } from "../buildInfo";
import { usePanelDisplaySettings } from "../panelDisplay";
import type { OperatorPreferences, OperatorView } from "../types";

type PreferencesPanelProps = {
  operator: OperatorView | null;
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

export function PreferencesPanel({ operator }: PreferencesPanelProps) {
  const {
    preferences,
    preferencesError,
    preferencesSaving,
    updatePreferences,
  } = usePanelDisplaySettings();
  const [draft, setDraft] = useState<OperatorPreferences>(preferences);
  const [localError, setLocalError] = useState<string | null>(null);
  const [localSelectionMessage, setLocalSelectionMessage] = useState<
    string | null
  >(null);
  const browserTimezone = useMemo(
    () =>
      Intl.DateTimeFormat().resolvedOptions().timeZone || "local browser time",
    [],
  );
  const timezonePreview = useMemo(
    () => previewTimezone(draft.timezone || browserTimezone),
    [browserTimezone, draft.timezone],
  );
  const dirty = JSON.stringify(draft) !== JSON.stringify(preferences);

  useEffect(() => {
    setDraft(preferences);
  }, [preferences]);

  async function savePreferences(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setLocalError(null);
    const timezone = draft.timezone?.trim() || null;
    const dashboardCurveExclusions = normalizeCurveExclusions(
      draft.dashboard_curve_exclusions,
    );
    const validationError =
      validateTimezone(timezone) ??
      validateDashboardLimits(
        draft.dashboard_resource_top_limit,
        draft.dashboard_network_top_limit,
      );
    if (validationError) {
      setLocalError(validationError);
      return;
    }
    try {
      await updatePreferences({
        ...draft,
        dashboard_curve_exclusions: dashboardCurveExclusions,
        timezone,
      });
    } catch {
      // The shared preference context exposes the API error for rendering.
    }
  }

  function resetPreferences() {
    setLocalError(null);
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

  return (
    <div className="workspace singleColumn preferencesWorkspace">
      <section className="fleetPanel preferencesPanel">
        <div className="sectionHeader">
          <div>
            <h2>Operator preferences</h2>
            <span>
              {operator
                ? `${operator.username} / ${operator.role}`
                : "Current authenticated operator"}{" "}
              / Console build {FRONTEND_BUILD_NUMBER}
            </span>
          </div>
          <span
            className={
              dirty ? "consoleStatusBadge warning" : "consoleStatusBadge ok"
            }
          >
            {dirty ? "Unsaved changes" : "Saved"}
          </span>
        </div>

        <form className="preferencesForm" onSubmit={savePreferences}>
          <PreferenceGroup
            description="Controls how VPS labels are rendered in tables, drawers, and action previews."
            icon={<ServerCog size={18} />}
            title="VPS name format"
          >
            <label>
              <span>Name display</span>
              <select
                value={draft.vps_name_display_mode}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    vps_name_display_mode:
                      event.target.value === "name" ? "name" : "name_id_suffix",
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
            description="Country columns show a compact flag glyph plus code when enabled; turn this off for code-only compact rows such as US, DE, or JP."
            icon={<Flag size={18} />}
            title="Country flags"
          >
            <label className="checkLine inlineCheck">
              <input
                checked={draft.show_country_flags}
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
            description="Times remain ISO UTC in the API; this only changes how the console renders timestamps."
            icon={<TimerReset size={18} />}
            title="Timezone"
          >
            <label>
              <span>Display timezone</span>
              <input
                list="operator-timezones"
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
            description="Language is stored server-side for future localization. English is the current console language."
            icon={<Languages size={18} />}
            title="Language"
          >
            <label>
              <span>Console language</span>
              <select
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
            title="Sidebar subpanels"
          >
            <label>
              <span>Default expansion</span>
              <select
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
            description="Clears browser-only console state such as dashboard selectors, saved fleet views, table layout, paging, column visibility, and expanded panels."
            icon={<Trash2 size={18} />}
            title="Local console selections"
          >
            <div className="preferenceResetRow">
              <div className="preferenceHint">
                <strong>
                  Server preferences and encrypted vaults are preserved.
                </strong>
                <span>
                  After clearing, the console reloads and reads default local
                  selections.
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
            {localSelectionMessage && (
              <p className="preferencesNotice">{localSelectionMessage}</p>
            )}
          </PreferenceGroup>

          <PreferenceGroup
            description="Controls how bulk job result groups are compared before individual target output chunks are shown."
            icon={<ListChecks size={18} />}
            title="Bulk execution summaries"
          >
            <label>
              <span>Default comparison</span>
              <select
                aria-label="Bulk output comparison default"
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
            <div className="preferenceHint">
              <strong>
                {draft.bulk_output_compare_mode === "text"
                  ? "Text normalized"
                  : "Binary exact"}
              </strong>
              <span>
                Binary is safest for correctness; text mode normalizes line
                endings and trailing whitespace for command output review.
              </span>
            </div>
          </PreferenceGroup>

          <PreferenceGroup
            description="Controls server-side dashboard curve limits and exclusions. Selectors support provider:*, country:*, tag:*, name:*, id:*, or a raw tag."
            icon={<Activity size={18} />}
            title="Dashboard curves"
          >
            <div className="preferenceInlineControls">
              <label>
                <span>Resource top VPS</span>
                <select
                  aria-label="Resource curve top VPS"
                  value={draft.dashboard_resource_top_limit}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      dashboard_resource_top_limit: Number(event.target.value),
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
                <span>Network top VPS</span>
                <select
                  aria-label="Network top VPS"
                  value={draft.dashboard_network_top_limit}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      dashboard_network_top_limit: Number(event.target.value),
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
                aria-label="Dashboard curve exclusions"
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    dashboard_curve_exclusions: splitCurveExclusions(
                      event.target.value,
                    ),
                  }))
                }
                placeholder={
                  "provider:test\ncountry:lab\nname:canary\nid:agent-dev-"
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
                Applied server-side before top-N resource and network curves are
                selected.
              </span>
            </div>
          </PreferenceGroup>

          {(localError || preferencesError) && (
            <p className="preferencesError">{localError ?? preferencesError}</p>
          )}

          <div className="preferencesActions">
            <button
              className="secondaryAction"
              disabled={!dirty || preferencesSaving}
              onClick={resetPreferences}
              type="button"
            >
              <RotateCcw size={18} />
              <span>Reset</span>
            </button>
            <button
              className="primaryAction"
              disabled={!dirty || preferencesSaving}
              type="submit"
            >
              <Save size={18} />
              <span>{preferencesSaving ? "Saving" : "Save preferences"}</span>
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}

function PreferenceGroup({
  children,
  description,
  icon,
  title,
}: {
  children: ReactNode;
  description: string;
  icon: ReactNode;
  title: string;
}) {
  return (
    <section className="preferenceGroup">
      <div className="preferenceGroupHeader">
        <span className="preferenceIcon">{icon}</span>
        <div>
          <h3>{title}</h3>
          <p>{description}</p>
        </div>
      </div>
      <div className="preferenceControls">{children}</div>
    </section>
  );
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

function validateDashboardLimits(
  resourceTopLimit: number,
  networkTopLimit: number,
): string | null {
  if (
    !Number.isInteger(resourceTopLimit) ||
    resourceTopLimit < 3 ||
    resourceTopLimit > 16
  ) {
    return "Resource curve top VPS must be between 3 and 16";
  }
  if (
    !Number.isInteger(networkTopLimit) ||
    networkTopLimit < 3 ||
    networkTopLimit > 16
  ) {
    return "Network top VPS must be between 3 and 16";
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
