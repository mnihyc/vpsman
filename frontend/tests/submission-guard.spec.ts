import { expect, test } from "@playwright/test";
import {
  beginSubmission,
  createSubmissionGuard,
  finishSubmission,
} from "../src/submissionGuard";

test("suppresses only pending and recently successful identical submissions", () => {
  const guard = createSubmissionGuard();

  expect(beginSubmission(guard, "refresh:one", 1_000)).toBe(true);
  expect(beginSubmission(guard, "refresh:one", 1_001)).toBe(false);
  finishSubmission(guard, "refresh:one", true, 1_100);

  expect(beginSubmission(guard, "refresh:one", 1_650)).toBe(false);
  expect(beginSubmission(guard, "refresh:two", 1_651)).toBe(true);
  finishSubmission(guard, "refresh:two", true, 1_652);
  expect(beginSubmission(guard, "refresh:one", 1_653)).toBe(true);
});

test("allows an immediate identical retry after failure", () => {
  const guard = createSubmissionGuard();

  expect(beginSubmission(guard, "apply:one", 2_000)).toBe(true);
  finishSubmission(guard, "apply:one", false, 2_010);
  expect(beginSubmission(guard, "apply:one", 2_011)).toBe(true);
});
