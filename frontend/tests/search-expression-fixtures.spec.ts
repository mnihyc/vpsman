import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { evaluateSearchExpression, parseSearchExpression, type SearchFields } from "../src/searchExpression";
import type { AgentView } from "../src/types";

type FixtureCase = {
  expression: string;
  matches: string[];
  name: string;
};

type FixtureContext = {
  alert?: Record<string, unknown>;
  event_predicates?: string[];
  job?: Record<string, unknown>;
  vps: {
    display_name: string;
    id: string;
    internal_build_number?: number | null;
    last_seen_at?: string | null;
    status: string;
    tags: string[];
  };
};

type ExpressionFixture = {
  cases: FixtureCase[];
  contexts: Record<string, FixtureContext>;
};

const fixturePath = resolve(dirname(fileURLToPath(import.meta.url)), "../../fixtures/expression-cases.json");
const fixture = JSON.parse(readFileSync(fixturePath, "utf8")) as ExpressionFixture;

test("shared expression fixture cases match frontend evaluator", () => {
  const contexts = fixture.contexts;
  for (const testCase of fixture.cases) {
    const parsed = parseSearchExpression(testCase.expression);
    expect(parsed.error, testCase.name).toBeNull();
    const actual = Object.entries(contexts)
      .filter(([, context]) => evaluateSearchExpression(parsed.expression, fieldsForContext(context)))
      .map(([name]) => name)
      .sort();
    expect(actual, testCase.name).toEqual([...testCase.matches].sort());
  }
});

function fieldsForContext(context: FixtureContext): SearchFields {
  const agent = agentFromContext(context);
  const providerTags = agent.tags.filter((tag) => tag.toLocaleLowerCase().startsWith("provider:"));
  const countryTags = agent.tags.filter((tag) => tag.toLocaleLowerCase().startsWith("country:"));
  const providerValues = providerTags.map((tag) => tag.slice("provider:".length));
  const countryValues = countryTags.map((tag) => tag.slice("country:".length));
  return {
    all: [agent.id, agent.display_name],
    events: (context.event_predicates ?? []).map((event) => event.toLocaleLowerCase()),
    fields: {
      "alert.category": stringValues(context.alert?.category),
      "alert.severity": stringValues(context.alert?.severity),
      "alert.state": stringValues(context.alert?.state),
      "job.status": stringValues(context.job?.status),
      "job.target.status": stringValues((context.job?.target as Record<string, unknown> | undefined)?.status),
      "job.type": stringValues(context.job?.type),
      "vps.country": countryValues,
      "vps.display_name": [agent.display_name],
      "vps.id": [agent.id],
      "vps.internal_build_number": [agent.internal_build_number ?? 0],
      "vps.last_seen_at": agent.last_seen_at ? [agent.last_seen_at] : [],
      "vps.provider": providerValues,
      "vps.status": [agent.status],
      "vps.tag": agent.tags,
      "vps.tags": agent.tags,
      last_seen: agent.last_seen_at ? [agent.last_seen_at] : [],
      status: [agent.status],
    },
    namespaces: {
      country: countryTags.concat(countryValues),
      id: [agent.id],
      name: [agent.display_name],
      provider: providerTags.concat(providerValues),
      region: countryTags.concat(countryValues),
      status: [agent.status],
      tag: agent.tags,
      tags: agent.tags,
    },
  };
}

function agentFromContext(context: FixtureContext): AgentView {
  return {
    capabilities: {
      can_apply_process_limits: false,
      can_attempt_privileged_ops: false,
      can_manage_runtime_tunnels: false,
      privilege_mode: "unknown",
    },
    display_name: context.vps.display_name,
    id: context.vps.id,
    internal_build_number: context.vps.internal_build_number ?? 1,
    last_ip: null,
    last_seen_at: context.vps.last_seen_at ?? null,
    registration_ip: null,
    stale_reason: null,
    stale_since: null,
    status: context.vps.status,
    tags: context.vps.tags,
  };
}

function stringValues(value: unknown): string[] {
  return typeof value === "string" ? [value] : [];
}
