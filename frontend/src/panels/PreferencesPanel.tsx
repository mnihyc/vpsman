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
  Route,
  Save,
  ServerCog,
  Tags,
  TimerReset,
  Trash2,
  Wifi,
} from "lucide-react";
import { clearLocalStorageSelections } from "../localStorageSelections";
import { FRONTEND_BUILD_NUMBER } from "../buildInfo";
import { usePanelDisplaySettings } from "../panelDisplay";
import { DEFAULT_OPERATOR_PREFERENCES } from "../utils";
import {
  defaultFleetTagVisible,
  fleetTagVisible,
} from "../tagDisplay";
import type { OperatorPreferences, OperatorView, TagView } from "../types";

type PreferencesPanelProps = {
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

export function PreferencesPanel({ operator, tags }: PreferencesPanelProps) {
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
  const [tagVisibilityFilter, setTagVisibilityFilter] = useState("");
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
    const fleetTagVisibilityOverrides = normalizeFleetTagVisibilityOverrides(
      draft.fleet_tag_visibility_overrides,
    );
    const validationError =
      validateTimezone(timezone) ??
      validateDashboardLimits(
        draft.dashboard_resource_top_limit,
        draft.dashboard_network_top_limit,
      ) ??
      validateFleetTagVisibilityOverrides(fleetTagVisibilityOverrides);
    if (validationError) {
      setLocalError(validationError);
      return;
    }
    try {
      await updatePreferences({
        ...draft,
        dashboard_curve_exclusions: dashboardCurveExclusions,
        fleet_tag_visibility_overrides: fleetTagVisibilityOverrides,
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

  const operationalDefaultCount = [
    draft.gateway_server_public_key_hex,
    draft.gateway_endpoints.trim(),
    draft.tunnel_ipv4_allocation_pool_cidr.trim(),
    draft.tunnel_ipv6_allocation_pool_cidr.trim(),
    normalizeCurveExclusions(draft.dashboard_curve_exclusions).length > 0
      ? "curves"
      : "",
  ].filter(Boolean).length;

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
          <section className="preferenceScopeOverview" aria-label="Preferences scope overview">
            <PreferenceScopeTile
              detail="Timezone, language, display labels, flags, sidebar behavior, review prompt display, tag visibility, and output comparison affect this operator's console experience."
              label="Personal display"
              value="Operator scoped"
            />
            <PreferenceScopeTile
              detail="Saved views, table layouts, Home telemetry selectors, and expanded panels are browser-local and can be cleared without changing server preferences."
              label="Browser state"
              value="Local only"
            />
            <PreferenceScopeTile
              detail={`${operationalDefaultCount} operational default groups are temporarily stored per operator until backend fleet/system settings own shared defaults.`}
              label="Fleet/system defaults"
              value="Needs shared scope"
            />
          </section>

          <PreferenceSection
            description="Personal operator presentation choices. These should not change fleet behavior or another operator's console."
            title="Personal display preferences"
          >
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
                  onChange={(event) => setTagVisibilityFilter(event.target.value)}
                  placeholder="Filter tags"
                  value={tagVisibilityFilter}
                />
                <button
                  className="secondaryAction compactAction"
                  disabled={
                    Object.keys(draft.fleet_tag_visibility_overrides).length === 0
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
                  <span>Create tags before setting Fleet column visibility.</span>
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
                          onChange={(event) =>
                            setFleetTagVisibility(tag.name, event.target.checked)
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
              resetDisabled={draft.timezone === DEFAULT_OPERATOR_PREFERENCES.timezone}
              scope="Personal"
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
              description="Language is stored with the operator profile for future localization. English is the current console language."
              icon={<Languages size={18} />}
              onReset={() =>
                resetDraftPatch({
                  language: DEFAULT_OPERATOR_PREFERENCES.language,
                })
              }
              resetDisabled={draft.language === DEFAULT_OPERATOR_PREFERENCES.language}
              scope="Personal"
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
                  value={draft.review_prompt_mode}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      review_prompt_mode:
                        event.target.value === "overlay" ? "overlay" : "inline",
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
          </PreferenceSection>

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
          </PreferenceSection>

          <PreferenceSection
            description="Operational defaults currently stored per operator. These affect generated commands, topology workflows, or Home telemetry ranking and should move to shared fleet/system settings when backend scopes exist."
            title="Fleet and system defaults"
          >
            <PreferenceGroup
              description="Used to compose the one-line agent install command after identity import. Endpoints are label=host:port=priority, one per line."
              icon={<Wifi size={18} />}
              onReset={() =>
                resetDraftPatch({
                  gateway_endpoints:
                    DEFAULT_OPERATOR_PREFERENCES.gateway_endpoints,
                  gateway_server_public_key_hex:
                    DEFAULT_OPERATOR_PREFERENCES.gateway_server_public_key_hex,
                })
              }
              resetDisabled={
                draft.gateway_endpoints ===
                  DEFAULT_OPERATOR_PREFERENCES.gateway_endpoints &&
                draft.gateway_server_public_key_hex ===
                  DEFAULT_OPERATOR_PREFERENCES.gateway_server_public_key_hex
              }
              scope="Fleet/system"
              title="Gateway install defaults"
            >
              <PreferenceOperationalNotice>
                These are operator-stored defaults for install-command
                generation. Shared gateway endpoints and server keys belong in
                Suite config or fleet settings once backend scope is available.
              </PreferenceOperationalNotice>
              <label>
                <span>Server public key hex</span>
                <input
                  aria-label="Gateway server public key hex"
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      gateway_server_public_key_hex:
                        event.target.value.trim() || null,
                    }))
                  }
                  placeholder="64 hex characters"
                  value={draft.gateway_server_public_key_hex ?? ""}
                />
              </label>
              <label className="wideField">
                <span>Endpoints</span>
                <textarea
                  aria-label="Gateway install endpoints"
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      gateway_endpoints: event.target.value,
                    }))
                  }
                  placeholder={"primary=gw.example.com:9443=10\nbackup=gw2.example.com:9443=20"}
                  rows={3}
                  value={draft.gateway_endpoints}
                />
              </label>
            </PreferenceGroup>

            <PreferenceGroup
              description="Used to prefill tunnel endpoint allocation pools. Empty fields keep allocation manual."
              icon={<Route size={18} />}
              onReset={() =>
                resetDraftPatch({
                  tunnel_ipv4_allocation_pool_cidr:
                    DEFAULT_OPERATOR_PREFERENCES.tunnel_ipv4_allocation_pool_cidr,
                  tunnel_ipv6_allocation_pool_cidr:
                    DEFAULT_OPERATOR_PREFERENCES.tunnel_ipv6_allocation_pool_cidr,
                })
              }
              resetDisabled={
                draft.tunnel_ipv4_allocation_pool_cidr ===
                  DEFAULT_OPERATOR_PREFERENCES.tunnel_ipv4_allocation_pool_cidr &&
                draft.tunnel_ipv6_allocation_pool_cidr ===
                  DEFAULT_OPERATOR_PREFERENCES.tunnel_ipv6_allocation_pool_cidr
              }
              scope="Fleet/system"
              title="Tunnel allocation"
            >
              <PreferenceOperationalNotice>
                These pools prefill topology workflows. They are not personal
                display settings; shared pool policy should move to fleet
                topology settings when available.
              </PreferenceOperationalNotice>
              <label>
                <span>IPv4 pool CIDR</span>
                <input
                  aria-label="Tunnel IPv4 allocation pool CIDR"
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      tunnel_ipv4_allocation_pool_cidr: event.target.value,
                    }))
                  }
                  placeholder="No default; enter IPv4 pool CIDR"
                  value={draft.tunnel_ipv4_allocation_pool_cidr}
                />
              </label>
              <label>
                <span>IPv6 pool CIDR</span>
                <input
                  aria-label="Tunnel IPv6 allocation pool CIDR"
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      tunnel_ipv6_allocation_pool_cidr: event.target.value,
                    }))
                  }
                  placeholder="No default; enter IPv6 pool CIDR"
                  value={draft.tunnel_ipv6_allocation_pool_cidr}
                />
              </label>
            </PreferenceGroup>

            <PreferenceGroup
              description="Controls shared Home telemetry curve limits and exclusions. Selectors support *, provider:*, country:*, tag:*, name:*, id:*, or a raw tag."
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
              scope="Fleet/system"
              title="Home telemetry curves"
            >
              <PreferenceOperationalNotice>
                These choices change shared Home telemetry ranking for this
                operator. Shared observability policy belongs in fleet settings
                once backend support exists.
              </PreferenceOperationalNotice>
              <div className="preferenceInlineControls">
              <label>
                <span>Resource top count</span>
                <select
                  aria-label="Resource curve top count"
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
                <span>Network top count</span>
                <select
                  aria-label="Network top count"
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
                aria-label="Home telemetry curve exclusions"
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
                Applied by the control plane before top-N resource and network curves are
                selected.
              </span>
            </div>
            </PreferenceGroup>
          </PreferenceSection>

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
            className="iconOnlyButton preferenceCardReset"
            disabled={resetDisabled}
            onClick={onReset}
            title={`Reset ${title} to default`}
            type="button"
          >
            <RotateCcw size={15} />
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
  detail,
  label,
  value,
}: {
  detail: string;
  label: string;
  value: string;
}) {
  return (
    <div className="preferenceScopeTile">
      <small>{label}</small>
      <strong>{value}</strong>
      <p>{detail}</p>
    </div>
  );
}

function PreferenceOperationalNotice({ children }: { children: ReactNode }) {
  return (
    <div className="preferenceOperationalNotice">
      <ServerCog size={15} />
      <span>{children}</span>
    </div>
  );
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
