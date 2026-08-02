import { expect, test } from "@playwright/test";
import { clientIdSuffix, formatVpsName } from "../src/utils";

test("generated VPS IDs remain complete in name labels", () => {
  expect(clientIdSuffix("v-1")).toBe("v-1");
  expect(clientIdSuffix("v-15")).toBe("v-15");
  expect(clientIdSuffix("v-222")).toBe("v-222");
  expect(clientIdSuffix("v-12345")).toBe("v-12345");
  expect(
    formatVpsName({ id: "v-222", display_name: "Singapore" }),
  ).toBe("Singapore (v-222)");
});

test("custom client IDs retain the compact suffix", () => {
  expect(clientIdSuffix("agent-sfo-01")).toBe("fo01");
});
