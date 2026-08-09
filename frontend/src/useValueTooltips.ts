import { useEffect } from "react";

const SENSITIVE_FIELD_PATTERN =
  /password|passphrase|secret|token|private|credential|verifier|salt|api[-_ ]?key|\botp\b|totp|one[-_ ]?time|authenticator|verification code|setup key|enrollment key|\brecovery\b/i;
const PROTECTED_TOOLTIP_DESCENDANT_SELECTOR =
  "pre, code, .srOnly, .visuallyHidden, [aria-hidden='true'], [data-value-tooltip-skip='true'], [data-tooltip-sensitive='true']";
const UNSAFE_ARIA_LABEL_PATTERN = /\b(?:https?|wss?):\/\//i;
const generatedTooltipTitles = new WeakMap<HTMLElement, string>();
const pendingGeneratedTitleMutations = new WeakMap<HTMLElement, number>();

const SEMANTIC_TOOLTIP_SELECTOR = [
  "input:not([type='hidden'])",
  "textarea",
  "select",
  "button",
  "a[href]",
  "[role='button']",
  "[role='link']",
  "[role='tab']",
  "[role='menuitem']",
  "label",
  "legend",
  "summary",
  "dt",
  "dd",
  "th",
  "td",
  "[role='cell']",
  "[role='columnheader']",
  ".metric",
  ".metricCard",
  ".consoleStatusBadge",
  ".status",
  ".statusPill",
  ".gridCounts > *",
  ".gridPageLabel",
  ".consoleInlineDetailGrid > span",
  ".vpsFactRow",
  ".vpsResourceFact",
  ".topologyMetric",
  ".timeSeriesCoverage",
  ".timeSeriesLegendActions > span",
  "[aria-label]:not(svg):not([aria-hidden='true'])",
].join(",");

const TRUNCATED_TEXT_SELECTOR = [
  ".accessQueueRow span",
  ".accessQueueRow strong",
  ".accessSelectionPanel span",
  ".authReasonCell small",
  ".authReasonCell strong",
  ".backupEvidenceRow small",
  ".backupMigrationRoute small",
  ".backupQuickAction small",
  ".backupRecordLink small",
  ".breadcrumb",
  ".bulkComparisonRowTitle span",
  ".bulkComparisonVariantLabel",
  ".bulkEvidenceBox strong",
  ".bulkSummaryClients span",
  ".bulkSummaryList summary span:last-child",
  ".bulkTagPreviewStats small",
  ".bulkTagPreviewStats strong",
  ".consoleInlineDetailGrid > span > span",
  ".consoleInlineDetailGrid > span > strong",
  ".commandPaletteResultText small",
  ".commandPaletteResultText strong",
  ".compactSelectorText",
  ".compactTopologyNote span",
  ".copyHashButton span",
  ".dashboardAlertText small",
  ".dashboardAlertText strong",
  ".dashboardChartHeader > span",
  ".dashboardListRow span:last-child",
  ".dashboardTrafficRow small",
  ".dashboardTrafficRow strong",
  ".executionFailureReason small",
  ".executionResultStats strong",
  ".executionSummaryStats strong",
  ".fileActionButton span",
  ".fileActionGroupHeader span",
  ".fileActionStack > span",
  ".fileBrowserStateStrip small",
  ".fileCommandHeader span",
  ".fileCurrentDirectoryHeader span",
  ".fileEditorToolbar span",
  ".fileTransferHandoffPanel dd",
  ".fileTransferHandoffPanel span",
  ".fileTreeRow span",
  ".fleetAlertRow > small:not(.fleetAlertReason)",
  ".fleetPolicyRows small",
  ".fleetPolicyRows strong",
  ".gridCellContent",
  ".gridMobilePrimary",
  ".gridMobileState",
  ".gridMobileFieldValue",
  ".groupSummaryStrip small",
  ".historyPrimary .deliveryErrorText",
  ".historyPrimary small",
  ".historyPrimary strong",
  ".instance small",
  ".jobEvidenceOutputRow small",
  ".jobEvidenceOutputRow strong",
  ".jobStatusCell .status",
  ".latencyCurveTitle small",
  ".linkLikeButton",
  ".migrationCheckItem small",
  ".migrationPlanSummary strong",
  ".miniTableRow span:not(.status)",
  ".miniTableRow strong",
  ".mobilePageMenu summary span",
  ".mobileSavedViewMenu summary span",
  ".navItem span",
  ".monoValue",
  ".operatorRecordName small",
  ".outputMeta small",
  ".outputMeta strong",
  ".pageHeaderContext span",
  ".processSupervisorSummaryStrip small",
  ".privilegeStatus span",
  ".restoreArchiveSummary strong",
  ".restoreReadOnlyField strong",
  ".ruleCard small",
  ".ruleCard span",
  ".scheduleRunsCell small",
  ".scheduleRunsCell strong",
  ".scheduleRunMenuItem span",
  ".scopeMeta small",
  ".scopeMeta strong",
  ".searchExpressionAutocomplete button small",
  ".searchExpressionMeta",
  ".status",
  ".statusPill",
  ".subnavItem",
  ".systemCapacityConfigLinks button small",
  ".systemConfigFieldMeta dd",
  ".systemConfigFieldMeta summary span",
  ".systemConfigPath",
  ".systemConfigSearch span",
  ".systemConfigSideNav button span",
  ".systemConfigStatusItem small",
  ".systemConfigStepper li strong",
  ".targetImpactGroupHeader span",
  ".targetSelectorHeader span",
  ".templateToolbarStatus",
  ".terminalActiveHeader > div:first-child > span",
  ".terminalFocusOverlay header span",
  ".terminalSessionContext small",
  ".terminalSummaryStrip small",
  ".topologyMetric small",
  ".networkWorkflowList small",
  ".topologyNetworkTestGroupHeader strong",
  ".topologyNetworkTrendChartHeader span",
  ".topologyTagList span",
  ".totpStepList li strong",
  ".transferFocusBanner small",
  ".transferLifecycleSummary small",
  ".transferProgressCell small",
  ".transferRetryReview dd",
  ".vpsComboboxEmpty",
  ".vpsComboboxMenu small",
  ".vpsComboboxMenu strong",
  ".vpsMonitorSignal span",
  ".vpsMonitorTags span",
  ".vpsRulesPreviewFinalAction small",
  ".vpsRulesPreviewFinalAction strong",
].join(",");

export function useValueTooltips() {
  useEffect(() => {
    const pendingElements = new Set<HTMLElement>();
    const pendingSubtrees = new Set<HTMLElement>();
    let scheduledRefresh: number | null = null;

    const queueElementAndAncestors = (element: HTMLElement | null) => {
      let current = element;
      while (current) {
        pendingElements.add(current);
        current = current.parentElement;
      }
    };
    const queueSubtree = (element: HTMLElement) => {
      pendingSubtrees.add(element);
      queueElementAndAncestors(element.parentElement);
    };
    const flushPending = () => {
      scheduledRefresh = null;
      const subtreeRoots = Array.from(pendingSubtrees).filter((candidate) => {
        let ancestor = candidate.parentElement;
        while (ancestor) {
          if (pendingSubtrees.has(ancestor)) return false;
          ancestor = ancestor.parentElement;
        }
        return true;
      });
      const elements = Array.from(pendingElements);
      pendingSubtrees.clear();
      pendingElements.clear();
      subtreeRoots.forEach(updateTooltipSubtree);
      elements.forEach((element) => {
        if (element.isConnected) updateTooltipElement(element);
      });
    };
    const scheduleRefresh = () => {
      if (scheduledRefresh !== null) return;
      scheduledRefresh = window.requestAnimationFrame(flushPending);
    };
    const handleControlChange = (event: Event) => {
      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        target instanceof HTMLSelectElement
      ) {
        clearGeneratedTitle(target);
        updateSemanticTitle(target);
        queueElementAndAncestors(target.parentElement);
        scheduleRefresh();
      }
    };
    const handleMutations = (records: MutationRecord[]) => {
      records.forEach((record) => {
        const target =
          record.target instanceof HTMLElement
            ? record.target
            : record.target.parentElement;
        if (!target) return;

        if (record.type === "childList") {
          pendingElements.add(target);
          queueElementAndAncestors(target.parentElement);
          record.addedNodes.forEach((node) => {
            if (node instanceof HTMLElement) queueSubtree(node);
          });
          return;
        }

        if (
          record.type === "attributes" &&
          record.attributeName === "title"
        ) {
          if (consumeGeneratedTitleMutation(target)) return;
          releaseGeneratedTitleOwnership(target);
          pendingElements.add(target);
          queueElementAndAncestors(target.parentElement);
          return;
        }

        if (
          record.type === "attributes" &&
          [
            "aria-hidden",
            "class",
            "data-tooltip-sensitive",
            "data-value-tooltip-skip",
          ].includes(record.attributeName ?? "")
        ) {
          queueSubtree(target);
          return;
        }

        pendingElements.add(target);
        queueElementAndAncestors(target.parentElement);
      });
      scheduleRefresh();
    };
    const observer = new MutationObserver(handleMutations);
    updateTooltipSubtree(document.body);
    document.addEventListener("input", handleControlChange, true);
    document.addEventListener("change", handleControlChange, true);
    observer.observe(document.body, {
      attributeFilter: [
        "aria-disabled",
        "aria-hidden",
        "aria-label",
        "autocomplete",
        "checked",
        "class",
        "data-tooltip-disabled-reason",
        "data-tooltip-empty-reason",
        "data-tooltip-label",
        "data-tooltip-sensitive",
        "data-value-tooltip-skip",
        "disabled",
        "id",
        "name",
        "placeholder",
        "selected",
        "title",
        "type",
        "value",
      ],
      attributes: true,
      characterData: true,
      childList: true,
      subtree: true,
    });
    return () => {
      document.removeEventListener("input", handleControlChange, true);
      document.removeEventListener("change", handleControlChange, true);
      if (scheduledRefresh !== null) {
        window.cancelAnimationFrame(scheduledRefresh);
      }
      pendingElements.clear();
      pendingSubtrees.clear();
      observer.disconnect();
    };
  }, []);
}

function updateTooltipSubtree(root: HTMLElement) {
  updateTooltipElement(root);
  root
    .querySelectorAll<HTMLElement>(TRUNCATED_TEXT_SELECTOR)
    .forEach(updateTruncatedTextTitle);
  root
    .querySelectorAll<HTMLElement>(SEMANTIC_TOOLTIP_SELECTOR)
    .forEach(updateSemanticTitle);
}

function updateTooltipElement(element: HTMLElement) {
  const matchesTruncated = element.matches(TRUNCATED_TEXT_SELECTOR);
  const matchesSemantic = element.matches(SEMANTIC_TOOLTIP_SELECTOR);
  if (matchesTruncated) updateTruncatedTextTitle(element);
  if (matchesSemantic) updateSemanticTitle(element);
  if (!matchesTruncated && !matchesSemantic) clearGeneratedTitle(element);
}

function updateSemanticTitle(element: HTMLElement) {
  if (shouldSkipElement(element)) {
    clearGeneratedTitle(element);
    return;
  }
  if (hasAuthoredTitle(element)) return;

  if (
    element instanceof HTMLInputElement ||
    element instanceof HTMLTextAreaElement ||
    element instanceof HTMLSelectElement
  ) {
    updateControlTitle(element);
    return;
  }

  const text = safeElementText(element);
  const label = semanticLabel(element, text);
  const unavailable =
    element.hasAttribute("disabled") || element.getAttribute("aria-disabled") === "true";
  if (unavailable) {
    setGeneratedTitle(
      element,
      element.dataset.tooltipDisabledReason ??
        `${label || "This control"} is unavailable in the current state.`,
    );
    return;
  }
  if (text === "-") {
    setGeneratedTitle(
      element,
      element.dataset.tooltipEmptyReason ?? emptyValueTitle(element),
    );
    return;
  }

  const ariaLabel = safeAuthoredAriaLabel(element);
  if (element.matches("button, [role='button'], [role='tab'], [role='menuitem']")) {
    setGeneratedTitle(element, label ? `Activate ${label}.` : null);
    return;
  }
  if (element.matches("a[href], [role='link']")) {
    setGeneratedTitle(element, label ? `Open ${label}.` : null);
    return;
  }
  if (element.matches("summary")) {
    setGeneratedTitle(element, label ? `Expand or collapse ${label}.` : null);
    return;
  }
  if (element.matches("label")) {
    setGeneratedTitle(element, label ? `${label} field.` : null);
    return;
  }
  if (element.matches("legend")) {
    setGeneratedTitle(element, label ? `${label} field group.` : null);
    return;
  }
  if (element.matches("th, [role='columnheader']")) {
    setGeneratedTitle(element, label ? `${label} column.` : null);
    return;
  }
  if (element.matches("td, [role='cell']")) {
    setGeneratedTitle(element, tableCellTitle(element, text));
    return;
  }
  if (element.matches("dt")) {
    const value = safeElementText(element.nextElementSibling);
    setGeneratedTitle(element, label ? `${label}: ${value || "no available value"}.` : null);
    return;
  }
  if (element.matches("dd")) {
    setGeneratedTitle(element, descriptionValueTitle(element, text));
    return;
  }
  if (element.matches(".consoleStatusBadge, .status, .statusPill")) {
    setGeneratedTitle(element, text ? `Status: ${text}.` : null);
    return;
  }
  if (element.matches(".gridCounts > *, .gridPageLabel")) {
    setGeneratedTitle(element, text ? `Grid summary: ${text}.` : null);
    return;
  }
  if (
    element.matches(
      ".metric, .metricCard, .consoleInlineDetailGrid > span, .vpsFactRow, .vpsResourceFact, .topologyMetric, .timeSeriesCoverage, .timeSeriesLegendActions > span",
    )
  ) {
    setGeneratedTitle(element, text ? `Current value: ${text}.` : ariaLabel || null);
    return;
  }
  setGeneratedTitle(element, ariaLabel || null);
}

function updateControlTitle(
  element: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement,
) {
  if (isSensitiveControl(element)) {
    clearGeneratedTitle(element);
    return;
  }
  const label = controlLabel(element);
  if (element.hasAttribute("disabled") || element.getAttribute("aria-disabled") === "true") {
    setGeneratedTitle(
      element,
      element.dataset.tooltipDisabledReason ??
        `${label || "This field"} is unavailable in the current state.`,
    );
    return;
  }
  if (element instanceof HTMLSelectElement) {
    const selected = safeElementText(element.selectedOptions[0] ?? null);
    setGeneratedTitle(
      element,
      selected === "-"
        ? element.dataset.tooltipEmptyReason ??
            `${label || "This field"} has no available value.`
        : selected
          ? `${label || "Selected value"}: ${selected}.`
          : `${label || "This field"} has no selected value.`,
    );
    return;
  }
  if (element instanceof HTMLTextAreaElement) {
    setGeneratedTitle(
      element,
      `${label || "Multiline field"}; current multiline content is excluded from tooltips.`,
    );
    return;
  }
  if (element instanceof HTMLInputElement && ["checkbox", "radio"].includes(element.type)) {
    setGeneratedTitle(
      element,
      `${label || "This option"}: ${element.checked ? "selected" : "not selected"}.`,
    );
    return;
  }
  const value = element.value.trim();
  const placeholder = element.placeholder.trim();
  setGeneratedTitle(
    element,
    value === "-"
      ? element.dataset.tooltipEmptyReason ??
          `${label || "This field"} has no available value.`
      : value
        ? `${label || "Current value"}: ${value}.`
        : placeholder
          ? `${label ? `${label} accepted format` : "Accepted format"}: ${placeholder}.`
          : label
            ? `${label} field.`
            : null,
  );
}

function updateTruncatedTextTitle(element: HTMLElement) {
  if (shouldSkipElement(element)) {
    clearGeneratedTitle(element);
    return;
  }
  if (hasAuthoredTitle(element)) return;
  const text = safeElementText(element);
  setGeneratedTitle(
    element,
    text === "-"
      ? element.dataset.tooltipEmptyReason ?? emptyValueTitle(element)
      : text || null,
  );
}

function shouldSkipElement(element: HTMLElement) {
  return Boolean(element.closest(PROTECTED_TOOLTIP_DESCENDANT_SELECTOR));
}

function safeElementText(element: Element | null): string {
  if (!(element instanceof HTMLElement)) return "";
  if (element.matches(PROTECTED_TOOLTIP_DESCENDANT_SELECTOR)) return "";
  const text = sanitizedDescendantText(element);
  return text || safeAuthoredAriaLabel(element);
}

function sanitizedDescendantText(
  element: HTMLElement,
  excludedElement?: Element,
) {
  const text: string[] = [];
  const visit = (node: Node) => {
    if (
      node === excludedElement ||
      (node !== element &&
      node instanceof Element &&
      node.matches(PROTECTED_TOOLTIP_DESCENDANT_SELECTOR))
    ) {
      return;
    }
    if (node.nodeType === Node.TEXT_NODE) {
      text.push(node.textContent ?? "");
      return;
    }
    node.childNodes.forEach(visit);
  };
  visit(element);
  return normalizeText(text.join(" "));
}

function safeAuthoredAriaLabel(element: HTMLElement) {
  return safeTooltipLabel(element.getAttribute("aria-label"));
}

function safeTooltipLabel(value: string | null | undefined) {
  const label = normalizeText(value ?? "");
  if (
    !label ||
    SENSITIVE_FIELD_PATTERN.test(label) ||
    UNSAFE_ARIA_LABEL_PATTERN.test(label)
  ) {
    return "";
  }
  return label;
}

function normalizeText(value: string) {
  return value.replace(/\s+/g, " ").trim();
}

function semanticLabel(element: HTMLElement, text: string) {
  const nestedControl = element.matches("label")
    ? element.querySelector<HTMLElement>(
        "input[aria-label], textarea[aria-label], select[aria-label]",
      )
    : null;
  const nestedControlLabel = nestedControl
    ? safeAuthoredAriaLabel(nestedControl)
    : "";
  return normalizeText(
    safeTooltipLabel(element.dataset.tooltipLabel) ||
      (safeAuthoredAriaLabel(element) || nestedControlLabel || text),
  );
}

function controlLabel(
  element: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement,
) {
  const dataLabel = safeTooltipLabel(element.dataset.tooltipLabel);
  if (dataLabel) return dataLabel;
  const ariaLabel = safeAuthoredAriaLabel(element);
  if (ariaLabel) return ariaLabel;
  if (element.id) {
    const explicitLabel = document.querySelector<HTMLLabelElement>(
      `label[for="${CSS.escape(element.id)}"]`,
    );
    const text = safeElementText(explicitLabel);
    if (text) return text;
  }
  const wrappingLabel = element.closest<HTMLLabelElement>("label");
  if (!wrappingLabel) return "";
  return (
    sanitizedDescendantText(wrappingLabel, element) ||
    safeAuthoredAriaLabel(wrappingLabel)
  );
}

function emptyValueTitle(element: HTMLElement) {
  const context = nearestValueLabel(element);
  return `${context || "This value"} has no available value.`;
}

function nearestValueLabel(element: HTMLElement): string {
  const explicit =
    safeTooltipLabel(element.dataset.tooltipLabel) ||
    safeAuthoredAriaLabel(element);
  if (explicit && explicit !== "-") return explicit;
  if (element.matches("dd")) {
    const term = safeElementText(element.previousElementSibling);
    if (term) return term;
  }
  const parent = element.parentElement;
  if (parent) {
    const candidate = parent.querySelector<HTMLElement>(
      ":scope > small:first-child, :scope > span:first-child, :scope > b:first-child, :scope > strong:first-child, :scope > dt:first-child",
    );
    const text = safeElementText(candidate);
    if (text && text !== "-") return text;
  }
  const header = tableCellHeader(element);
  return header || "This value";
}

function descriptionValueTitle(element: HTMLElement, text: string) {
  const label = safeElementText(element.previousElementSibling);
  if (text === "-") return `${label || "This field"} has no available value.`;
  return label && text ? `${label}: ${text}.` : text || null;
}

function tableCellTitle(element: HTMLElement, text: string) {
  const header = tableCellHeader(element);
  if (text === "-") return `${header || "This field"} has no available value.`;
  return header && text ? `${header}: ${text}.` : text || null;
}

function tableCellHeader(element: HTMLElement): string {
  const cell = element.closest<HTMLElement>("td, [role='cell']") ?? element;
  const row = cell.parentElement;
  const cells = row ? Array.from(row.children) : [];
  const index = cells.indexOf(cell);
  if (index < 0) return "";
  const table = cell.closest("table, [role='grid']");
  const headers = table?.querySelectorAll<HTMLElement>("th, [role='columnheader']");
  return safeElementText(headers?.[index] ?? null);
}

function setGeneratedTitle(element: HTMLElement, title: string | null) {
  const normalized = normalizeText(title ?? "");
  if (!normalized) {
    clearGeneratedTitle(element);
    return;
  }
  if (hasAuthoredTitle(element)) return;
  const nextTitle = normalized === "-" ? emptyValueTitle(element) : normalized;
  if (
    generatedTooltipTitles.get(element) === nextTitle &&
    element.getAttribute("title") === nextTitle &&
    element.dataset.valueTooltip === "true"
  ) {
    return;
  }
  generatedTooltipTitles.set(element, nextTitle);
  if (element.dataset.valueTooltip !== "true") {
    element.dataset.valueTooltip = "true";
  }
  if (element.getAttribute("title") !== nextTitle) {
    trackGeneratedTitleMutation(element);
    element.setAttribute("title", nextTitle);
  }
}

function clearGeneratedTitle(element: HTMLElement) {
  const generatedTitle = generatedTooltipTitles.get(element);
  generatedTooltipTitles.delete(element);
  if (
    generatedTitle !== undefined &&
    element.getAttribute("title") === generatedTitle
  ) {
    trackGeneratedTitleMutation(element);
    element.removeAttribute("title");
  }
  if (element.dataset.valueTooltip === "true") {
    delete element.dataset.valueTooltip;
  }
}

function hasOwnedGeneratedTitle(element: HTMLElement) {
  const generatedTitle = generatedTooltipTitles.get(element);
  return (
    generatedTitle !== undefined &&
    element.getAttribute("title") === generatedTitle
  );
}

function hasAuthoredTitle(element: HTMLElement) {
  return Boolean(element.getAttribute("title")) && !hasOwnedGeneratedTitle(element);
}

function releaseGeneratedTitleOwnership(element: HTMLElement) {
  if (!generatedTooltipTitles.has(element)) return;
  generatedTooltipTitles.delete(element);
  if (element.dataset.valueTooltip === "true") {
    delete element.dataset.valueTooltip;
  }
}

function trackGeneratedTitleMutation(element: HTMLElement) {
  pendingGeneratedTitleMutations.set(
    element,
    (pendingGeneratedTitleMutations.get(element) ?? 0) + 1,
  );
}

function consumeGeneratedTitleMutation(element: HTMLElement) {
  const pending = pendingGeneratedTitleMutations.get(element) ?? 0;
  if (pending === 0) return false;
  if (pending === 1) {
    pendingGeneratedTitleMutations.delete(element);
  } else {
    pendingGeneratedTitleMutations.set(element, pending - 1);
  }
  return true;
}

function isSensitiveControl(
  element: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement,
) {
  if (element instanceof HTMLInputElement && element.type === "password") {
    return true;
  }
  const descriptor = [
    element.name,
    element.id,
    element.autocomplete,
    element.getAttribute("aria-label"),
    element instanceof HTMLSelectElement ? "" : element.placeholder,
    element.closest("label")?.textContent,
  ]
    .filter(Boolean)
    .join(" ");
  return SENSITIVE_FIELD_PATTERN.test(descriptor);
}
