import { useEffect } from "react";

const SENSITIVE_FIELD_PATTERN =
  /password|passphrase|secret|token|private|credential|verifier|salt|api[-_ ]?key/i;
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
  ".tokenChip",
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
    const updateControlTitle = (element: HTMLInputElement | HTMLTextAreaElement) => {
      if (element.dataset.valueTooltipSkip === "true") {
        clearGeneratedTitle(element);
        return;
      }
      if (element.title && element.dataset.valueTooltip !== "true") {
        return;
      }
      if (isSensitiveControl(element)) {
        clearGeneratedTitle(element);
        return;
      }
      const value = element.value.trim();
      const placeholder = element.placeholder.trim();
      const title = value || placeholder;
      if (title) {
        element.dataset.valueTooltip = "true";
        element.title = title;
      } else if (element.dataset.valueTooltip === "true") {
        clearGeneratedTitle(element);
      }
    };

    const refresh = () => {
      document
        .querySelectorAll<HTMLInputElement | HTMLTextAreaElement>(
          "input, textarea",
        )
        .forEach(updateControlTitle);
      document
        .querySelectorAll<HTMLElement>(TRUNCATED_TEXT_SELECTOR)
        .forEach(updateTextTitle);
    };
    const handleInput = (event: Event) => {
      const target = event.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
        if (target.dataset.valueTooltip === "true") {
          target.removeAttribute("title");
          delete target.dataset.valueTooltip;
        }
        updateControlTitle(target);
      }
    };
    let scheduledRefresh: number | null = null;
    const scheduleRefresh = () => {
      if (scheduledRefresh !== null) {
        return;
      }
      scheduledRefresh = window.requestAnimationFrame(() => {
        scheduledRefresh = null;
        refresh();
      });
    };
    const observer = new MutationObserver(scheduleRefresh);
    refresh();
    document.addEventListener("input", handleInput, true);
    document.addEventListener("change", handleInput, true);
    observer.observe(document.body, {
      characterData: true,
      childList: true,
      subtree: true,
    });
    return () => {
      document.removeEventListener("input", handleInput, true);
      document.removeEventListener("change", handleInput, true);
      if (scheduledRefresh !== null) {
        window.cancelAnimationFrame(scheduledRefresh);
      }
      observer.disconnect();
    };
  }, []);
}

function updateTextTitle(element: HTMLElement) {
  if (element.title && element.dataset.valueTooltip !== "true") {
    return;
  }
  const text = element.textContent?.trim() ?? "";
  if (!text) {
    clearGeneratedTitle(element);
    return;
  }
  element.dataset.valueTooltip = "true";
  element.title = text;
}

function clearGeneratedTitle(element: HTMLElement) {
  if (element.dataset.valueTooltip !== "true") {
    return;
  }
  element.removeAttribute("title");
  delete element.dataset.valueTooltip;
}

function isSensitiveControl(element: HTMLInputElement | HTMLTextAreaElement) {
  if (element instanceof HTMLInputElement && element.type === "password") {
    return true;
  }
  const descriptor = [
    element.name,
    element.id,
    element.autocomplete,
    element.getAttribute("aria-label"),
    element.placeholder,
    element.closest("label")?.textContent,
  ]
    .filter(Boolean)
    .join(" ");
  return SENSITIVE_FIELD_PATTERN.test(descriptor);
}
