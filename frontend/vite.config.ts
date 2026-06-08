import { defineConfig } from "vite";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const apiTarget = process.env.VPSMAN_API_PROXY ?? "http://127.0.0.1:18080";
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const frontendBuildNumber = readBuildNumber("frontend");

export default defineConfig({
  define: {
    __VPSMAN_FRONTEND_BUILD_NUMBER__: JSON.stringify(frontendBuildNumber),
  },
  server: {
    proxy: {
      "/api": apiTarget,
      "/health": apiTarget,
      "/ws": {
        target: apiTarget,
        ws: true,
      },
    },
  },
});

function readBuildNumber(component: string): string {
  const counterDir = process.env.VPSMAN_BUILD_NUMBER_DIR
    ? path.resolve(process.env.VPSMAN_BUILD_NUMBER_DIR)
    : path.join(repoRoot, "build", "build-numbers");
  const counterPath = path.join(counterDir, `${component}.txt`);
  try {
    const value = fs.readFileSync(counterPath, "utf8").trim();
    return /^\d+$/.test(value) ? value : "0";
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") {
      return "0";
    }
    throw error;
  }
}
