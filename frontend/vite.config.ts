import { defineConfig, type Plugin } from "vite";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const apiTarget = process.env.VPSMAN_API_PROXY ?? "http://127.0.0.1:8080";
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const frontendBuildNumber = readBuildNumber("frontend");
const releaseTag = resolveReleaseTag();
const repositoryGitAvailable = repositoryHasOwnGitRoot();
const sourceCommit = resolveSourceCommit(releaseTag, repositoryGitAvailable);
verifyReleaseTagCommit(releaseTag, sourceCommit, repositoryGitAvailable);
const installerBytes =
  releaseTag || !sourceCommit || !repositoryGitAvailable
    ? null
    : readInstallerAtCommit(sourceCommit);
const installerSha256 = installerBytes
  ? createHash("sha256").update(installerBytes).digest("hex")
  : "";
const installerAssetName = !installerBytes
  ? ""
  : `install-agent-${sourceCommit}-${installerSha256}.sh`;
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
    __VPSMAN_INSTALLER_ASSET_NAME__: JSON.stringify(installerAssetName),
    __VPSMAN_INSTALLER_SHA256__: JSON.stringify(installerSha256),
    __VPSMAN_RELEASE_TAG__: JSON.stringify(releaseTag),
    __VPSMAN_SOURCE_COMMIT__: JSON.stringify(sourceCommit),
  },
  plugins: installerBytes
    ? [sourceInstallerAssetPlugin(installerAssetName, installerBytes)]
    : [],
  preview: {
    proxy: apiProxy,
  },
  server: {
    proxy: apiProxy,
  },
});

function repositoryHasOwnGitRoot(): boolean {
  let discoveredRoot: string;
  try {
    discoveredRoot = execFileSync("git", ["rev-parse", "--show-toplevel"], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    return (
      fs.realpathSync(discoveredRoot) === fs.realpathSync(repoRoot)
    );
  } catch {
    return false;
  }
}

function resolveSourceCommit(
  releaseTag: string,
  repositoryGitAvailable: boolean,
): string {
  const configured =
    process.env.VPSMAN_SOURCE_COMMIT ?? process.env.GITHUB_SHA;
  if (configured !== undefined) {
    return normalizeSourceCommit(configured);
  }
  if (!repositoryGitAvailable) {
    if (releaseTag) {
      throw new Error(
        "tagged frontend build requires an explicit source commit or a full Git checkout",
      );
    }
    return "";
  }
  let discovered: string;
  try {
    discovered = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    if (releaseTag) {
      throw new Error(
        "tagged frontend build requires an explicit source commit or a full Git checkout",
      );
    }
    return "";
  }
  return normalizeSourceCommit(discovered);
}

function normalizeSourceCommit(configured: string): string {
  if (!/^[0-9a-fA-F]{40}$/.test(configured)) {
    throw new Error(
      "VPSMAN_SOURCE_COMMIT/GITHUB_SHA must be an exact 40-character Git commit",
    );
  }
  return configured.toLowerCase();
}

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

function verifyReleaseTagCommit(
  releaseTag: string,
  sourceCommit: string,
  repositoryGitAvailable: boolean,
): void {
  if (!releaseTag) {
    return;
  }
  if (!repositoryGitAvailable) {
    throw new Error(
      `tagged frontend build requires ${releaseTag} in a full Git checkout`,
    );
  }
  let taggedCommit: string;
  try {
    taggedCommit = execFileSync(
      "git",
      ["rev-parse", `${releaseTag}^{commit}`],
      { cwd: repoRoot, encoding: "utf8" },
    )
      .trim()
      .toLowerCase();
  } catch {
    throw new Error(
      `tagged frontend build requires ${releaseTag} in a full Git checkout`,
    );
  }
  if (taggedCommit !== sourceCommit) {
    throw new Error(
      `release tag ${releaseTag} resolves to ${taggedCommit}, not source commit ${sourceCommit}`,
    );
  }
}

function readInstallerAtCommit(sourceCommit: string): Uint8Array | null {
  try {
    return execFileSync(
      "git",
      ["show", `${sourceCommit}:deploy/install-agent.sh`],
      { cwd: repoRoot, stdio: ["ignore", "pipe", "ignore"] },
    );
  } catch {
    return null;
  }
}

function sourceInstallerAssetPlugin(
  assetName: string,
  contents: Uint8Array,
): Plugin {
  let outDir = "";
  const route = `/${assetName}`;
  return {
    name: "vpsman-source-installer-asset",
    configResolved(config) {
      outDir = config.build.outDir;
    },
    configureServer(server) {
      server.middlewares.use(route, (request, response, next) => {
        if (request.method !== "GET" && request.method !== "HEAD") {
          next();
          return;
        }
        response.statusCode = 200;
        response.setHeader("Cache-Control", "public, max-age=31536000, immutable");
        response.setHeader("Content-Length", contents.byteLength.toString());
        response.setHeader("Content-Type", "text/x-shellscript; charset=utf-8");
        if (request.method === "HEAD") {
          response.end();
          return;
        }
        response.end(contents);
      });
    },
    closeBundle() {
      fs.mkdirSync(outDir, { recursive: true });
      fs.writeFileSync(path.join(outDir, assetName), contents, { mode: 0o755 });
    },
  };
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
