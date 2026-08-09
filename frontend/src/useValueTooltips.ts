import { useEffect } from "react";

const SKIPPED_TOOLTIP_ELEMENT_SELECTOR =
  "[data-value-tooltip-skip='true'], [data-tooltip-sensitive='true']";
const NON_VISIBLE_TOOLTIP_DESCENDANT_SELECTOR =
  ".srOnly, .visuallyHidden, [aria-hidden='true']";
const generatedTooltipTitles = new WeakMap<HTMLElement, string>();
const pendingGeneratedTitleMutations = new WeakMap<HTMLElement, number>();

const SEMANTIC_TOOLTIP_SELECTOR = [
  "[data-tooltip-disabled-reason]",
  "[data-tooltip-empty-reason]",
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
  ".confirmationPromptBody dd",
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
  ".truncateValue",
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

        if (record.type === "attributes" && record.attributeName === "title") {
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
    const handleResize = () => {
      queueSubtree(document.body);
      scheduleRefresh();
    };
    updateTooltipSubtree(document.body);
    document.addEventListener("input", handleControlChange, true);
    document.addEventListener("change", handleControlChange, true);
    window.addEventListener("resize", handleResize);
    observer.observe(document.body, {
      attributeFilter: [
        "aria-hidden",
        "class",
        "data-tooltip-disabled-reason",
        "data-tooltip-empty-reason",
        "data-tooltip-sensitive",
        "data-value-tooltip-skip",
        "disabled",
        "style",
        "title",
      ],
      attributes: true,
      characterData: true,
      childList: true,
      subtree: true,
    });
    return () => {
      document.removeEventListener("input", handleControlChange, true);
      document.removeEventListener("change", handleControlChange, true);
      window.removeEventListener("resize", handleResize);
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

  const disabledReason = element.dataset.tooltipDisabledReason?.trim();
  if (disabledReason) {
    setGeneratedTitle(element, disabledReason);
    return;
  }
  const emptyReason = element.dataset.tooltipEmptyReason?.trim();
  if (emptyReason) {
    setGeneratedTitle(element, emptyReason);
    return;
  }
  clearGeneratedTitle(element);
}

function updateTruncatedTextTitle(element: HTMLElement) {
  if (shouldSkipElement(element)) {
    clearGeneratedTitle(element);
    return;
  }
  if (hasAuthoredTitle(element)) return;
  if (!isVisiblyShortened(element)) {
    clearGeneratedTitle(element);
    return;
  }
  const text = safeElementText(element);
  setGeneratedTitle(element, text || null);
}

function shouldSkipElement(element: HTMLElement) {
  return Boolean(element.closest(SKIPPED_TOOLTIP_ELEMENT_SELECTOR));
}

function isVisiblyShortened(element: HTMLElement) {
  const style = window.getComputedStyle(element);
  if (
    style.display === "none" ||
    style.visibility === "hidden" ||
    element.clientWidth === 0 ||
    element.clientHeight === 0
  ) {
    return false;
  }
  const horizontalOverflow = element.scrollWidth > element.clientWidth + 1;
  const verticalOverflow = element.scrollHeight > element.clientHeight + 1;
  const lineClamp = Number.parseInt(style.webkitLineClamp, 10);
  return (
    (style.textOverflow === "ellipsis" && horizontalOverflow) ||
    (Number.isFinite(lineClamp) && lineClamp > 0 && verticalOverflow)
  );
}

function safeElementText(element: Element | null): string {
  if (!(element instanceof HTMLElement)) return "";
  if (
    element.matches(SKIPPED_TOOLTIP_ELEMENT_SELECTOR) ||
    element.matches(NON_VISIBLE_TOOLTIP_DESCENDANT_SELECTOR)
  ) {
    return "";
  }
  const text = sanitizedDescendantText(element);
  return text;
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
        (node.matches(SKIPPED_TOOLTIP_ELEMENT_SELECTOR) ||
          node.matches(NON_VISIBLE_TOOLTIP_DESCENDANT_SELECTOR)))
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

function normalizeText(value: string) {
  return value.replace(/\s+/g, " ").trim();
}

function setGeneratedTitle(element: HTMLElement, title: string | null) {
  const normalized = normalizeText(title ?? "");
  if (!normalized) {
    clearGeneratedTitle(element);
    return;
  }
  if (hasAuthoredTitle(element)) return;
  const nextTitle = normalized;
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
  return (
    Boolean(element.getAttribute("title")) && !hasOwnedGeneratedTitle(element)
  );
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
