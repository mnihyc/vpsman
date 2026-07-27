#!/usr/bin/env node

import { mkdir, open, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const COMPONENT = "frontend";
const LOCK_RETRIES = 400;
const LOCK_DELAY_MS = 25;

const buildDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(buildDir, "..");
const counterDir = process.env.VPSMAN_BUILD_NUMBER_DIR
  ? path.resolve(process.env.VPSMAN_BUILD_NUMBER_DIR)
  : path.join(repoRoot, "build", "build-numbers");
const counterPath = path.join(counterDir, `${COMPONENT}.txt`);

if (isGithubActions()) {
  const buildNumber = await readCounter(counterPath, false);
  console.log(`frontend build number ${buildNumber}`);
} else {
  const buildNumber = await incrementCounter(counterPath);
  console.log(`frontend build number ${buildNumber}`);
}

async function incrementCounter(filePath) {
  await mkdir(path.dirname(filePath), { recursive: true });
  const release = await acquireLock(`${filePath}.lock`);
  try {
    const current = await readCounter(filePath, true);
    if (current >= Number.MAX_SAFE_INTEGER) {
      throw new Error(`frontend build-number counter is exhausted: ${filePath}`);
    }
    const next = current + 1;
    await writeFile(filePath, `${next}\n`, "utf8");
    return next;
  } finally {
    await release();
  }
}

async function readCounter(filePath, allowMissing) {
  try {
    const value = await readFile(filePath, "utf8");
    const trimmed = value.trim();
    if (!/^[1-9]\d*$/.test(trimmed)) {
      throw new Error(
        `build-number counter ${filePath} must contain one positive integer`,
      );
    }
    const parsed = Number(trimmed);
    if (!Number.isSafeInteger(parsed)) {
      throw new Error(
        `build-number counter ${filePath} exceeds JavaScript's safe integer range`,
      );
    }
    return parsed;
  } catch (error) {
    if (
      allowMissing &&
      error &&
      typeof error === "object" &&
      "code" in error &&
      error.code === "ENOENT"
    ) {
      return 0;
    }
    throw error;
  }
}

async function acquireLock(lockPath) {
  for (let attempt = 0; attempt < LOCK_RETRIES; attempt += 1) {
    try {
      const handle = await open(lockPath, "wx");
      await handle.close();
      return async () => {
        await rm(lockPath, { force: true });
      };
    } catch (error) {
      if (error && typeof error === "object" && "code" in error && error.code === "EEXIST") {
        await sleep(LOCK_DELAY_MS);
        continue;
      }
      throw error;
    }
  }
  throw new Error(`timed out waiting for build-number lock ${lockPath}`);
}

function isGithubActions() {
  return (process.env.GITHUB_ACTIONS ?? "").toLowerCase() === "true";
}

function sleep(ms) {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}
