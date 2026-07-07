import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

function source(relativePath: string) {
  return readFileSync(join(process.cwd(), "src", relativePath), "utf8");
}

test("keeps action feedback in dedicated local containers", () => {
  const accessPanel = source("panels/AccessPanel.tsx");
  expect(accessPanel).not.toMatch(
    /<span>\{revokeError\s*\?\?\s*"Block the current VPS gateway key"\}<\/span>/,
  );
  expect(accessPanel).toContain("accessRevokeActionFeedback");

  const systemPanel = source("panels/SystemPanel.tsx");
  expect(systemPanel).not.toMatch(
    /reviewPending\s*\?\s*"Preparing review"\s*:\s*`\$\{sessions\.length\} recent sessions`/,
  );
  expect(systemPanel).not.toContain(
    '{configError && <div className="panelError">{configError}</div>}',
  );
  expect(systemPanel).not.toContain(
    '{configMessage && <div className="panelSuccess">{configMessage}</div>}',
  );
  expect(systemPanel).toContain("systemSessionActionFeedback");
  expect(systemPanel).toContain("systemConfigActionFeedback");

  const jobsPanel = source("panels/JobsPanel.tsx");
  expect(jobsPanel).not.toMatch(
    /approvalActionError\s*&&\s*\(\s*<div className="panelError"/,
  );
  expect(jobsPanel).toContain("approvalActionFeedback");

  const processSupervisorPanel = source(
    "panels/jobs/ProcessSupervisorInventoryPanel.tsx",
  );
  expect(processSupervisorPanel).not.toContain("processActionNotice");
  expect(processSupervisorPanel).toContain("processActionFeedback");
});
