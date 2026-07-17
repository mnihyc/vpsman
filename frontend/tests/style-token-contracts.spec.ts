import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

const RUNTIME_OR_FALLBACK_TOKENS = new Set([
  "--console-sticky-offset",
  "--radix-context-menu-content-available-height",
]);

const UNSTYLED_BEHAVIOR_MARKERS = new Set([
  "actionDrawerInitialFocus",
  "jobEvidenceDetailActionFeedback",
  "policyDryRunValidationFeedback",
]);

function filesUnder(directory: string, suffix: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory()
      ? filesUnder(path, suffix)
      : entry.name.endsWith(suffix)
        ? [path]
        : [];
  });
}

test("every static console style token resolves to a declared or runtime value", () => {
  const styleDirectory = join(process.cwd(), "src", "styles");
  const css = readdirSync(styleDirectory)
    .filter((file) => file.endsWith(".css"))
    .map((file) => readFileSync(join(styleDirectory, file), "utf8"))
    .join("\n");
  const declared = new Set(
    [...css.matchAll(/(--[A-Za-z0-9_-]+)\s*:/g)].map((match) => match[1]),
  );
  const used = new Set(
    [...css.matchAll(/var\((--[A-Za-z0-9_-]+)/g)].map((match) => match[1]),
  );
  const unresolved = [...used]
    .filter(
      (token) => !declared.has(token) && !RUNTIME_OR_FALLBACK_TOKENS.has(token),
    )
    .sort();

  expect(unresolved).toEqual([]);
  expect(css).not.toMatch(/outline:[^;]*var\(--focus-ring\)/);
  expect(css).not.toMatch(
    /repeat\((?:auto-fit|auto-fill),\s*minmax\(\d+px,\s*1fr\)\)/,
  );
});

test("every literal class list has a style or an explicit behavior marker", () => {
  const sourceDirectory = join(process.cwd(), "src");
  const css = filesUnder(sourceDirectory, ".css")
    .map((file) => readFileSync(file, "utf8"))
    .join("\n");
  const declaredClasses = new Set(
    [...css.matchAll(/\.([_A-Za-z]+[_A-Za-z0-9-]*)/g)].map(
      (match) => match[1],
    ),
  );
  const unresolved: string[] = [];

  for (const file of filesUnder(sourceDirectory, ".tsx")) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(/className\s*=\s*"([^"]+)"/g)) {
      const classes = match[1].trim().split(/\s+/);
      if (
        !classes.some((name) => declaredClasses.has(name)) &&
        !classes.every((name) => UNSTYLED_BEHAVIOR_MARKERS.has(name))
      ) {
        unresolved.push(`${file.slice(sourceDirectory.length + 1)}: ${match[1]}`);
      }
    }
  }

  expect(unresolved.sort()).toEqual([]);
});
