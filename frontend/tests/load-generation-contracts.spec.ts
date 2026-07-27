import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

function hook(name: string) {
  return readFileSync(
    join(process.cwd(), "src", "hooks", `${name}.ts`),
    "utf8",
  );
}

function source(path: string) {
  return readFileSync(join(process.cwd(), "src", ...path.split("/")), "utf8");
}

function callbackSection(source: string, name: string): string {
  const start = source.indexOf(`const ${name} = useCallback`);
  expect(start, `${name} callback exists`).toBeGreaterThanOrEqual(0);
  const end = source.indexOf("\n  const ", start + 1);
  return source.slice(start, end === -1 ? source.length : end);
}

test("loads port-forwarding data on entry without duplicating Audit terminal loads", () => {
  const dashboardData = hook("useDashboardData");
  const networkBranch = dashboardData.slice(
    dashboardData.indexOf('activeView === "Network"'),
    dashboardData.indexOf('activeView === "Backups"'),
  );
  expect(networkBranch).toContain(
    "void portForwarding.loadPortForwardRules();",
  );

  const auditBranch = dashboardData.slice(
    dashboardData.indexOf('activeView === "Audit"'),
    dashboardData.indexOf('activeView === "Access"'),
  );
  expect(auditBranch).toContain("void jobs.loadJobs();");
  expect(auditBranch).not.toContain("jobs.loadTerminalSessions()");
});

test("keeps only the newest shared hook refresh and loading completion", () => {
  const jobs = hook("useJobsData");
  expect(jobs).toContain("const jobsLoadGeneration = useRef(0)");
  expect(jobs).toContain(
    "jobsLoadGeneration.current !== generation",
  );
  expect(jobs).toMatch(
    /jobsLoadGeneration\.current === generation &&\s*currentApiToken\.current === apiToken/,
  );
  expect(jobs).toContain("terminalSessionsLoadGeneration");
  expect(jobs).toContain("agentUpdateReleasesLoadGeneration");

  const audits = hook("useAuditData");
  expect(audits).toContain("const auditLoadGeneration = useRef(0)");
  expect(audits).toContain(
    "auditLoadGeneration.current !== generation",
  );
  expect(audits).toMatch(
    /auditLoadGeneration\.current === generation &&\s*currentApiToken\.current === apiToken/,
  );

  const access = hook("useAccessData");
  expect(access).toContain("const accessLoadGeneration = useRef(0)");
  expect(access.match(/accessLoadGeneration\.current !== generation/g)).toHaveLength(
    5,
  );
  expect(access.match(/accessLoadGeneration\.current === generation/g)).toHaveLength(
    2,
  );
});

test("forces a post-mutation inventory read and orders runtime apply refreshes", () => {
  const inventory = hook("useInventoryData");
  expect(inventory).toContain(
    "const loadTagInventory = useCallback(async (forceFresh = false)",
  );
  expect(inventory).toContain(
    "!forceFresh &&",
  );
  expect(inventory).toContain(
    "() => loadTagInventory(true)",
  );
  expect(inventory).toContain("refreshTagInventoryAfterMutation");
  expect(inventory).toContain("runtimeConfigApplyLoadGeneration");
  expect(inventory).toContain(
    "currentApiToken.current !== apiToken",
  );
});

test("distinguishes unavailable evidence from successful empty results", () => {
  const inventory = hook("useInventoryData");
  expect(inventory).toContain("runtimeConfigApplyEvidenceAvailable");
  expect(inventory).toContain("runtimeConfigApplyLoading");
  expect(inventory).toContain("setRuntimeConfigApplyEvidenceAvailable(true)");
  expect(inventory).toContain("setRuntimeConfigApplyEvidenceAvailable(false)");

  const topologyGraph = readFileSync(
    join(
      process.cwd(),
      "src",
      "panels",
      "topology",
      "TopologyGraphPanel.tsx",
    ),
    "utf8",
  );
  expect(topologyGraph).toContain('evidenceState === "loading"');
  expect(topologyGraph).toContain('evidenceState === "unavailable"');
  expect(topologyGraph).toContain('return "unknown"');
  expect(topologyGraph).toContain('return "not applied"');

  const home = readFileSync(
    join(process.cwd(), "src", "panels", "HomePanel.tsx"),
    "utf8",
  );
  expect(home).toContain("jobsEvidenceAvailable");
  expect(home).toContain("backupsEvidenceAvailable");
  expect(home).toContain(
    "No recent issues found in available evidence; some home sources are unavailable.",
  );
});

test("gates Home and runtime-config health claims on loaded evidence", () => {
  for (const [hookName, evidenceName] of [
    ["useJobsData", "jobsEvidenceAvailable"],
    ["useBackupsData", "backupsEvidenceAvailable"],
    ["useAuditData", "auditEvidenceAvailable"],
    ["useSchedulesData", "schedulesEvidenceAvailable"],
  ] as const) {
    const hookSource = hook(hookName);
    expect(hookSource).toContain(evidenceName);
    expect(hookSource).toContain(
      `set${evidenceName[0].toUpperCase()}${evidenceName.slice(1)}(false)`,
    );
    expect(hookSource).toMatch(
      new RegExp(`return \\{[\\s\\S]*${evidenceName}`),
    );
  }

  const app = source("App.tsx");
  expect(app).toContain("!homeEvidenceLoading");
  expect(app).toContain("dashboard.dashboardOverview !== null");
  expect(app).toContain("dashboard.jobsEvidenceAvailable");
  expect(app).toContain("dashboard.backupsEvidenceAvailable");
  expect(app).toContain("dashboard.auditEvidenceAvailable");
  expect(app).toContain("dashboard.schedulesEvidenceAvailable");
  expect(app).toContain("dashboard.systemDashboard !== null");

  const config = source("panels/ConfigPanel.tsx");
  expect(config).toContain('runtimeConfigEvidenceState === "available"');
  expect(config).toContain('inventoryEvidenceState === "available"');
  expect(config).toContain("fleetConfigEvidenceAvailable");
  expect(config).toContain("trustedRuntimeConfigApplyStates");
  expect(config).toContain("completeSummaryEvidence");
  expect(config).toContain("Health, drift, and zero-value claims remain unknown");
  expect(config).toContain("Evidence incomplete");

  const inventory = hook("useInventoryData");
  expect(inventory).toContain("tagInventoryEvidenceAvailable");
  expect(inventory).toContain("setTagInventoryEvidenceAvailable(false)");

  const fleet = hook("useFleetData");
  expect(fleet).toContain("configPolicyEvidenceAvailable");
  expect(fleet).toContain("setConfigPolicyEvidenceAvailable(false)");

  const vpsDetail = source("panels/VpsDetailPanel.tsx");
  expect(vpsDetail).toContain(
    'runtimeConfigEvidenceState === "available"',
  );
  expect(vpsDetail).toContain("Apply state unavailable");
  expect(vpsDetail).toContain("cached state is not treated as current");
});

test("does not derive unscoped alert totals from agent inventory", () => {
  const app = source("App.tsx");
  const shellCounts = app.slice(
    app.indexOf("const shellAlertCounts"),
    app.indexOf("const homeScopedRecords"),
  );
  expect(shellCounts).toContain("!hasFleetScope");
  expect(shellCounts).not.toContain("dashboard.agents");
  expect(app).toContain(
    "(!hasFleetScope || dashboard.fleetCoreEvidenceAvailable)",
  );
  expect(app).toContain(
    "fleetAlertsEvidenceAvailable={scopedFleetAlertsEvidenceAvailable}",
  );
});

test("guards direct-request unauthorized handlers with the current token", () => {
  const jobs = hook("useJobsData");
  const guard = callbackSection(jobs, "rethrowDirectRequestError");
  expect(guard.indexOf("currentApiToken.current !== apiToken")).toBeLessThan(
    guard.indexOf("isApiUnauthorized(error)"),
  );
  for (const callbackName of [
    "loadJobRollout",
    "loadHostProcessInventory",
    "loadHostServiceInventory",
    "loadHostStorageInventory",
    "loadHostPackageUpdatePlans",
    "loadHostPackageUpdatePlan",
    "loadJobTargets",
    "loadJob",
    "loadJobOutputs",
    "downloadFileDownloadBundle",
    "downloadJobOutputArchive",
    "downloadJobTargetStatuses",
    "loadJobOutputComparison",
    "downloadJobOutputChunk",
    "downloadJobOutputStream",
    "downloadFileDownloadForClient",
    "downloadFileTransferHandoff",
    "saveFileTransferHandoff",
    "downloadFileTransferSource",
    "loadTerminalReplay",
    "previewArtifactCleanup",
  ]) {
    expect(callbackSection(jobs, callbackName)).toContain(
      "rethrowDirectRequestError(error)",
    );
  }

  const backupDownload = callbackSection(
    hook("useBackupsData"),
    "downloadBackupArtifact",
  );
  expect(backupDownload.indexOf("apiTokenRef.current === apiToken")).toBeLessThan(
    backupDownload.indexOf("isApiUnauthorized(error)"),
  );
});

test("isolates refresh coalescing and logout feedback by auth generation", () => {
  const dashboard = hook("useDashboardData");
  for (const callbackName of [
    "forceAuthRequired",
    "handleAuth",
    "clearSession",
  ]) {
    expect(callbackSection(dashboard, callbackName)).toContain(
      "refreshAuthRef.current = null",
    );
  }
  const clearSession = callbackSection(dashboard, "clearSession");
  expect(clearSession).toContain(
    "authGenerationRef.current === logoutGeneration",
  );
  expect(clearSession).toContain("Signed out locally");
  expect(clearSession.indexOf("clearStoredAuthSession()")).toBeLessThan(
    clearSession.indexOf('apiPost<void>("/api/v1/auth/logout"'),
  );
});

test("orders dashboard, system, schedule, and port-forward refreshes", () => {
  const dashboardOverview = hook("useDashboardOverviewData");
  expect(dashboardOverview).toMatch(
    /sequence !== loadSequence\.current \|\|[\s\S]{0,100}requestKey !== desiredRequestKey\.current/,
  );
  const clearDashboard = dashboardOverview.slice(
    dashboardOverview.indexOf("const clearDashboardOverview"),
    dashboardOverview.indexOf("return {"),
  );
  expect(clearDashboard).toContain("loadSequence.current += 1");
  expect(clearDashboard).toContain("setDashboardOverviewLoading(false)");

  const system = hook("useSystemData");
  expect(system).toContain("systemDashboardLoadGeneration");
  expect(system).toContain("suiteConfigLoadGeneration");

  const schedules = hook("useSchedulesData");
  expect(schedules).toContain("schedulesLoadGeneration.current !== generation");
  expect(schedules).toMatch(
    /schedulesLoadGeneration\.current === generation &&\s*currentApiToken\.current === apiToken/,
  );

  const portForwarding = hook("usePortForwardingData");
  expect(portForwarding).toContain(
    "portForwardLoadGeneration.current !== generation",
  );
  expect(portForwarding).toMatch(
    /portForwardLoadGeneration\.current === generation &&\s*currentApiToken\.current === apiToken/,
  );
});

test("exposes bounded session resets for every scoped data hook", () => {
  const clearContracts = [
    ["useAccessData", "clearAccess"],
    ["useDashboardOverviewData", "clearDashboardOverview"],
    ["useJobsData", "clearJobs"],
    ["useAuditData", "clearAudits"],
    ["useInventoryData", "clearInventory"],
    ["useSystemData", "clearSystem"],
    ["useSchedulesData", "clearSchedules"],
    ["usePortForwardingData", "clearPortForwarding"],
  ] as const;

  for (const [hookName, clearName] of clearContracts) {
    const source = hook(hookName);
    expect(source).toContain(clearName);
    expect(source).toMatch(new RegExp(`return \\{[\\s\\S]*${clearName}`));
  }
});

test("preflights the current token before changing loader generations or state", () => {
  const contracts = [
    ["useAccessData", "loadCurrentOperatorProfile", "accessLoadGeneration"],
    ["useAccessData", "loadCurrentOperator", "accessLoadGeneration"],
    ["useAuditData", "loadAudits", "auditLoadGeneration"],
    ["useInventoryData", "loadTagInventory", "tagInventoryLoadGeneration"],
    ["useInventoryData", "loadSourceTemplates", "sourceTemplatesLoadGeneration"],
    [
      "useInventoryData",
      "loadRuntimeConfigApplyStates",
      "runtimeConfigApplyLoadGeneration",
    ],
    ["useJobsData", "loadJobs", "jobsLoadGeneration"],
    [
      "useJobsData",
      "loadAgentUpdateReleases",
      "agentUpdateReleasesLoadGeneration",
    ],
    ["useJobsData", "loadJobRollouts", "jobRolloutsLoadGeneration"],
    [
      "useJobsData",
      "loadTerminalSessions",
      "terminalSessionsLoadGeneration",
    ],
    ["useJobsData", "loadServerJobs", "serverJobsLoadGeneration"],
    ["useSchedulesData", "loadSchedules", "schedulesLoadGeneration"],
    [
      "useSystemData",
      "loadSystemDashboard",
      "systemDashboardLoadGeneration",
    ],
    ["useSystemData", "loadSuiteConfig", "suiteConfigLoadGeneration"],
    [
      "usePortForwardingData",
      "loadPortForwardRules",
      "portForwardLoadGeneration",
    ],
  ] as const;

  for (const [hookName, callbackName, generationName] of contracts) {
    const section = callbackSection(hook(hookName), callbackName);
    const tokenGuard = section.indexOf(
      "currentApiToken.current !== apiToken",
    );
    const generationChange = section.indexOf(
      `${generationName}.current + 1`,
    );
    expect(tokenGuard, `${callbackName} token guard`).toBeGreaterThanOrEqual(0);
    expect(
      tokenGuard,
      `${callbackName} preflights before generation`,
    ).toBeLessThan(generationChange);
  }
});

test("invalidates older reads before applying direct mutation responses", () => {
  const accessPreferences = callbackSection(
    hook("useAccessData"),
    "updateOperatorPreferences",
  );
  expect(accessPreferences).toContain("preferencesMutationGeneration");
  expect(accessPreferences).toContain(
    "currentApiToken.current !== apiToken",
  );
  expect(accessPreferences).toContain("await loadCurrentOperatorProfile()");

  const tagOrder = callbackSection(
    hook("useInventoryData"),
    "updateTagOrder",
  );
  expect(tagOrder).toContain("tagOrderMutationGeneration");
  expect(tagOrder).toContain("await refreshTagInventoryAfterMutation()");

  for (const callbackName of [
    "upsertCommandTemplate",
    "deleteCommandTemplate",
  ]) {
    const commandTemplate = callbackSection(
      hook("useJobsData"),
      callbackName,
    );
    expect(commandTemplate).toContain("commandTemplateMutationGeneration");
    expect(commandTemplate).toContain(
      "commandTemplatesLoadGeneration.current += 1",
    );
    expect(commandTemplate).toContain(
      "currentApiToken.current !== apiToken",
    );
  }
});

test("keeps source-template and command-template errors source scoped", () => {
  const inventory = hook("useInventoryData");
  expect(inventory).toContain(
    "const tagInventoryError = useRef<string | null>(null)",
  );
  expect(inventory).toContain(
    "const sourceTemplatesError = useRef<string | null>(null)",
  );
  const sourceTemplates = callbackSection(inventory, "loadSourceTemplates");
  expect(sourceTemplates).toContain("sourceTemplatesError.current = null");
  expect(sourceTemplates).toContain("publishTagsError()");

  const jobs = hook("useJobsData");
  expect(jobs).toContain(
    "const commandTemplatesError = useRef<string | null>(null)",
  );
  expect(jobs).toContain(
    "commandTemplatesError.current = settledSourceFailure",
  );
  expect(callbackSection(jobs, "upsertCommandTemplate")).toContain(
    "commandTemplatesError.current = null",
  );
});

test("guards mutation follow-up refreshes against an old token", () => {
  const contracts = [
    ["useSchedulesData", "createSchedule", "loadSchedules()"],
    ["useSystemData", "updateSuiteConfig", "loadSuiteConfig()"],
    [
      "usePortForwardingData",
      "createPortForwardRule",
      "refreshAfterMutation()",
    ],
    [
      "useInventoryData",
      "assignTag",
      "refreshTagInventoryAfterMutation()",
    ],
    ["useJobsData", "createJob", "loadJobs()"],
    ["useAuditData", "upsertHistoryRetentionPolicy", "loadAudits()"],
    ["useAccessData", "createOperator", "loadCurrentOperator()"],
  ] as const;

  for (const [hookName, callbackName, refreshCall] of contracts) {
    const section = callbackSection(hook(hookName), callbackName);
    const tokenGuard = section.indexOf(
      "currentApiToken.current !== apiToken",
    );
    expect(tokenGuard, `${callbackName} token guard`).toBeGreaterThanOrEqual(0);
    expect(tokenGuard, `${callbackName} guards refresh`).toBeLessThan(
      section.indexOf(refreshCall),
    );
  }
});
