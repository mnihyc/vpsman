import { defineConfig } from "vite";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const apiTarget = process.env.VPSMAN_API_PROXY ?? "http://127.0.0.1:8080";
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const frontendBuildNumber = readBuildNumber("frontend");
const releaseTag = resolveReleaseTag();
const apiProxy = {
  "/api": apiTarget,
  "/health": apiTarget,
  "/ws": {
    target: apiTarget,
    ws: true,
  },
};

export default defineConfig({
  define: {
    __VPSMAN_FRONTEND_BUILD_NUMBER__: JSON.stringify(frontendBuildNumber),
    __VPSMAN_RELEASE_TAG__: JSON.stringify(releaseTag),
  },
  preview: {
    proxy: apiProxy,
  },
  server: {
    proxy: apiProxy,
  },
});

function resolveReleaseTag(): string {
  const configured =
    process.env.VPSMAN_RELEASE_TAG ??
    (process.env.GITHUB_REF_TYPE === "tag"
      ? process.env.GITHUB_REF_NAME ?? ""
      : "");
  if (!configured) {
    return "";
  }
  const match =
    /^v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(
      configured,
    );
  const nonCanonicalNumericIdentifier =
    match?.[1]
      ?.split(".")
      .some(
        (part) => /^\d+$/.test(part) && part.length > 1 && part.startsWith("0"),
      ) ?? false;
  if (!match || nonCanonicalNumericIdentifier) {
    throw new Error(
      "VPSMAN_RELEASE_TAG/GITHUB_REF_NAME must be an exact canonical vX.Y.Z tag",
    );
  }
  return configured;
}

function readBuildNumber(component: string): string {
  const counterDir = process.env.VPSMAN_BUILD_NUMBER_DIR
    ? path.resolve(process.env.VPSMAN_BUILD_NUMBER_DIR)
    : path.join(repoRoot, "build", "build-numbers");
  const counterPath = path.join(counterDir, `${component}.txt`);
  try {
    const value = fs.readFileSync(counterPath, "utf8").trim();
    if (!/^[1-9]\d*$/.test(value)) {
      throw new Error(
        `build-number counter ${counterPath} must contain one positive integer`,
      );
    }
    return value;
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") {
      throw new Error(`build-number counter is missing: ${counterPath}`);
    }
    throw error;
  }
}
