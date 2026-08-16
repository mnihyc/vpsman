import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  buildAgentSelectorSuggestionValues,
  buildVpsRuleCompletionSuggestions,
  genericCallerSuggestions,
  shouldSuppressVpsRuleCompletionError,
  vpsRulesCategorySuggestion,
} from "../src/components/SearchExpressionInput";
import {
  buildParseableSearchValueSuggestions,
  isParseableSearchSuggestion,
  searchFieldsForSearchValues,
} from "../src/components/searchSuggestions";
import {
  agentsMatchingExpression,
  evaluateSearchExpression,
  expressionReferencesVpsRules,
  filterBySearchExpression,
  parseSearchExpression,
  termMatchTitle,
  tokenizeSearchExpression,
  type SearchFields,
} from "../src/searchExpression";
import type { AgentView, VpsRuleValueRecord } from "../src/types";
import {
  defaultFleetTagVisible,
  isProviderTag,
  isRegionTag,
  regionTagValue,
} from "../src/tagDisplay";
import {
  normalizeVpsRuleValue,
  providerProductLabel,
  tryNormalizeVpsRuleValue,
  VPS_RULE_KEYS,
} from "../src/vpsRules";
import { WEBHOOK_EXPRESSION_SUGGESTIONS } from "../src/webhookExpressionSuggestions";

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
  parseable_suggestions?: string[];
};

const fixturePath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../crates/common/tests/fixtures/expression-cases.json",
);
const fixture = JSON.parse(
  readFileSync(fixturePath, "utf8"),
) as ExpressionFixture;

type VpsRuleFixture = {
  contexts: Record<
    string,
    {
      rules: Array<{
        json: VpsRuleValueRecord["value_json"];
        key: string;
        raw: string;
      }>;
    }
  >;
  expression_cases: FixtureCase[];
  invalid_expressions: Array<{ error_contains: string; expression: string }>;
  invalid_normalization_cases: Array<{
    error_contains: string;
    input?: string;
    input_parts?: {
      count: number;
      prefix: string;
      repeat: string;
      suffix: string;
    };
    key: string;
    name: string;
  }>;
  normalization_cases: Array<{
    canonical: string;
    input: string;
    key: string;
    name: string;
  }>;
};

const vpsRuleFixturePath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../crates/common/tests/fixtures/vps-rule-cases.json",
);
const vpsRuleFixture = JSON.parse(
  readFileSync(vpsRuleFixturePath, "utf8"),
) as VpsRuleFixture;

function normalizationFixtureInput(
  testCase: VpsRuleFixture["invalid_normalization_cases"][number],
): string {
  if (testCase.input !== undefined) return testCase.input;
  const parts = testCase.input_parts;
  if (!parts) throw new Error(`${testCase.name}: fixture input is missing`);
  return `${parts.prefix}${parts.repeat.repeat(parts.count)}${parts.suffix}`;
}

test("shared expression fixture cases match frontend evaluator", () => {
  const contexts = fixture.contexts;
  for (const testCase of fixture.cases) {
    const parsed = parseSearchExpression(testCase.expression);
    expect(parsed.error, testCase.name).toBeNull();
    const actual = Object.entries(contexts)
      .filter(([, context]) =>
        evaluateSearchExpression(parsed.expression, fieldsForContext(context)),
      )
      .map(([name]) => name)
      .sort();
    expect(actual, testCase.name).toEqual([...testCase.matches].sort());
  }
});

test("shared VPS rule normalization fixture matches the frontend editor normalizer", () => {
  for (const testCase of vpsRuleFixture.normalization_cases) {
    expect(
      normalizeVpsRuleValue(testCase.key, testCase.input),
      testCase.name,
    ).toBe(testCase.canonical);
  }
  for (const testCase of vpsRuleFixture.invalid_normalization_cases) {
    expect(
      tryNormalizeVpsRuleValue(
        testCase.key,
        normalizationFixtureInput(testCase),
      ),
      testCase.name,
    ).toBeNull();
  }
});

test("provider and product presentation keeps each optional identity explicit", () => {
  expect(providerProductLabel("Northwind", "Storage-Box 4")).toBe(
    "Northwind · Storage-Box 4",
  );
  expect(providerProductLabel("Northwind", null)).toBe("Northwind");
  expect(providerProductLabel(null, "Storage-Box 4")).toBe(
    "provider unset · Storage-Box 4",
  );
  expect(providerProductLabel(null, null)).toBe("");
  expect(providerProductLabel(null, null, "provider unset")).toBe(
    "provider unset",
  );
  expect(providerProductLabel("Same", "Same")).toBe("Same · Same");
  expect(tryNormalizeVpsRuleValue("product.name", "A\ud800B")).toBeNull();
  expect(normalizeVpsRuleValue("product.name", "\ufeffBox\ufeff")).toBe(
    "\ufeffBox\ufeff",
  );
  expect(isProviderTag("provider:Northwind")).toBe(true);
  expect(defaultFleetTagVisible("provider:Northwind")).toBe(false);
  expect(isProviderTag("provider=Northwind")).toBe(true);
  expect(defaultFleetTagVisible("provider=Northwind")).toBe(false);
  expect(isRegionTag("region:FRA")).toBe(true);
  expect(regionTagValue(["country:DE", "region:FRA"])).toBe("FRA");
  expect(defaultFleetTagVisible("region:FRA")).toBe(false);
});

test("shared VPS rule expressions match the frontend evaluator", () => {
  for (const testCase of vpsRuleFixture.expression_cases) {
    const parsed = parseSearchExpression(testCase.expression);
    expect(parsed.error, testCase.name).toBeNull();
    const actual = Object.entries(vpsRuleFixture.contexts)
      .filter(([, context]) =>
        evaluateSearchExpression(parsed.expression, {
          all: [],
          vpsRules: context.rules.map(vpsRuleRecord),
        }),
      )
      .map(([name]) => name)
      .sort();
    expect(actual, testCase.name).toEqual([...testCase.matches].sort());
  }
});

test("shared invalid VPS rule expressions return matching guidance", () => {
  for (const testCase of vpsRuleFixture.invalid_expressions) {
    expect(
      parseSearchExpression(testCase.expression).error,
      testCase.expression,
    ).toContain(testCase.error_contains);
  }
});

test("missing VPS-rule evidence cannot silently evaluate as all or zero", () => {
  const agent = agentFromContext({
    vps: {
      display_name: "edge-one",
      id: "edge-one",
      status: "online",
      tags: [],
    },
  });
  expect(() => agentsMatchingExpression([agent], "vps.rules:*")).toThrow(
    "VPS rule data unavailable",
  );
});

test("VPS rule autocomplete progressively reveals scoped details only", () => {
  const category = vpsRulesCategorySuggestion();
  expect(category).toMatchObject({ label: "VPS rules…", value: "vps.rules:" });
  expect(
    buildAgentSelectorSuggestionValues([]).some((value) =>
      value.startsWith("vps.rules"),
    ),
  ).toBe(false);
  const genericAgentSuggestions = buildAgentSelectorSuggestionValues([
    agentFromContext({
      vps: {
        display_name: "edge-one",
        id: "edge-one",
        status: "online",
        tags: ["role:edge", "vps.rules:traffic.reset_day"],
      },
    }),
  ]);
  expect(genericAgentSuggestions).toContain("role:edge");
  expect(
    genericAgentSuggestions.some((value) =>
      value.toLocaleLowerCase().includes("vps.rules"),
    ),
  ).toBe(false);
  expect(buildVpsRuleCompletionSuggestions("vps", 3, [])).toEqual([]);

  const scoped = buildVpsRuleCompletionSuggestions("vps.rules:", 10, []);
  for (const key of VPS_RULE_KEYS) {
    expect(scoped.map((option) => option.value)).toContain(`vps.rules:${key}`);
  }
  expect(scoped.map((option) => option.label)).toContain("Any configured rule");
  expect(
    buildVpsRuleCompletionSuggestions(
      "status:online vps.rules:",
      "status:online vps.rules:".length,
      [],
    ).map((option) => option.value),
  ).toContain("vps.rules:traffic.reset_day");

  expect(
    genericCallerSuggestions([
      "status:online",
      "vps.rules",
      "vps.rules:traffic.quota.total >= 1TB",
      "tag:edge && vps.rules:billing.price = *USD*",
      "tag:vps.rules:traffic.reset_day",
      "vps.tag:vps.rules:network.port_speed",
    ]),
  ).toEqual(["status:online"]);

  expect(expressionReferencesVpsRules("vps.ruleship:edge")).toBe(false);
  expect(expressionReferencesVpsRules("tag:vps.ruleship:edge")).toBe(false);
});

test("an incomplete VPS-rule prefix suppresses errors only during active completion", () => {
  const value = "vps.rules:";
  expect(
    shouldSuppressVpsRuleCompletionError(value, value.length, true, true),
  ).toBe(true);
  expect(
    shouldSuppressVpsRuleCompletionError(value, value.length, true, false),
  ).toBe(false);
  expect(
    shouldSuppressVpsRuleCompletionError(value, value.length, false, false),
  ).toBe(false);
});

test("VPS rule autocomplete ranks canonical observed values and withholds them when unavailable", () => {
  const rows = [
    vpsRuleRecord({
      key: "traffic.quota.total",
      raw: "4TB",
      json: { bytes: 4_000_000_000_000 },
    }),
    vpsRuleRecord(
      {
        key: "traffic.quota.total",
        raw: "4TB",
        json: { bytes: 4_000_000_000_000 },
      },
      "two",
    ),
    vpsRuleRecord(
      {
        key: "traffic.quota.total",
        raw: "750GB",
        json: { bytes: 750_000_000_000 },
      },
      "three",
    ),
  ];
  const scoped = buildVpsRuleCompletionSuggestions(
    "vps.rules:traffic.quota.total",
    "vps.rules:traffic.quota.total".length,
    rows,
  );
  const equalityValues = scoped.filter((option) =>
    option.value.includes(" = "),
  );
  expect(equalityValues[0]?.value).toBe("vps.rules:traffic.quota.total = 4TB");
  expect(equalityValues[0]?.detail).toContain("2 VPSs");
  expect(scoped.map((option) => option.value)).toContain(
    "vps.rules:traffic.quota.total >= 1TB",
  );

  const billingCycle = vpsRuleRecord({
    key: "billing.cycle",
    raw: "06-15",
    json: { day: 15, month: 6, display: "06-15" },
  });
  expect(
    buildVpsRuleCompletionSuggestions(
      "vps.rules:billing.cycle",
      "vps.rules:billing.cycle".length,
      [billingCycle],
    ).map((option) => option.value),
  ).toContain("vps.rules:billing.cycle = 06-15");

  const productName = vpsRuleRecord({
    key: "product.name",
    raw: "Storage-Box 4",
    json: { display: "Storage-Box 4", name: "Storage-Box 4" },
  });
  const productSuggestions = buildVpsRuleCompletionSuggestions(
    "vps.rules:product.name",
    "vps.rules:product.name".length,
    [productName],
  ).map((option) => option.value);
  expect(productSuggestions).toContain(
    'vps.rules:product.name = "Storage-Box 4"',
  );
  expect(productSuggestions).toContain("vps.rules:product.name = LN.V2.HKGv3");
  expect(productSuggestions).not.toContain(
    expect.stringMatching(/vps\.rules:product\.name\s+[<>]=?/),
  );

  const unavailable = buildVpsRuleCompletionSuggestions(
    "vps.rules:",
    10,
    rows,
    false,
  );
  expect(unavailable).toEqual([
    expect.objectContaining({
      disabled: true,
      label: "VPS rule data unavailable",
    }),
  ]);
  expect(unavailable[0]?.value).not.toContain("traffic.quota.total");
});

test("quoted name selector matches display names with spaces", () => {
  const parsed = parseSearchExpression('name:"edge alpha 01"');
  expect(parsed.error).toBeNull();
  expect(
    evaluateSearchExpression(
      parsed.expression,
      fieldsForContext({
        vps: {
          display_name: "edge alpha 01",
          id: "agent-8f3c",
          status: "online",
          tags: ["provider:alpha", "country:us"],
        },
      }),
    ),
  ).toBe(true);
  expect(
    evaluateSearchExpression(
      parsed.expression,
      fieldsForContext({
        vps: {
          display_name: "edge beta 01",
          id: "agent-7e2a",
          status: "online",
          tags: ["provider:beta", "country:us"],
        },
      }),
    ),
  ).toBe(false);
});

test("agent selector autocomplete values parse and matching values rank before unmatched common values", () => {
  const contexts = Object.values(fixture.contexts);
  const agents = contexts.map(agentFromContext);
  const suggestions = buildAgentSelectorSuggestionValues(agents);
  expect(suggestions).toContain("*");
  expect(suggestions).toContain("id:*");
  expect(suggestions).toContain("status:online");
  expect(suggestions).toContain("status:never");

  for (const suggestion of suggestions) {
    const parsed = parseSearchExpression(suggestion);
    expect(parsed.error, suggestion).toBeNull();
  }
  expect(suggestions.indexOf("status:online")).toBeLessThan(
    suggestions.indexOf("status:never"),
  );
});

test("selector chip help describes that predicate rather than the full expression", () => {
  const agents = [
    agentFromContext({
      vps: {
        display_name: "edge-sin-01",
        id: "201",
        status: "online",
        tags: [],
      },
    }),
    agentFromContext({
      vps: {
        display_name: "core-fra-01",
        id: "202",
        status: "online",
        tags: [],
      },
    }),
  ];
  const token = tokenizeSearchExpression("id:201 OR id:202").tokens[0]!;
  const title = termMatchTitle(token, agents);

  expect(title).toContain("1 matched target: 201 (edge-sin-01; online)");
  expect(title).not.toContain("202 (core-fra-01; online)");
});

test("webhook expression autocomplete values are accepted event predicates", () => {
  for (const suggestion of WEBHOOK_EXPRESSION_SUGGESTIONS) {
    const parsed = parseSearchExpression(suggestion);
    expect(parsed.error, suggestion).toBeNull();
    expect(
      evaluateSearchExpression(parsed.expression, {
        all: [],
        events: [suggestion.toLocaleLowerCase()],
      }),
      suggestion,
    ).toBe(true);
  }
});

test("shared advertised autocomplete suggestions parse in the frontend parser", () => {
  for (const suggestion of fixture.parseable_suggestions ?? []) {
    expect(isParseableSearchSuggestion(suggestion), suggestion).toBe(true);
  }
});

test("generic table autocomplete values keep parseable unmatched expressions below matches", () => {
  const rows = [
    {
      values: [
        "selector id:agent-sfo-01 tag:edge provider:alpha",
        "https://hooks.example/vpsman",
      ],
    },
    {
      values: [
        "schedule.failed alert.category:network telemetry.tunnel status:retired",
        "state:enabled",
      ],
    },
  ];
  const valuesForRow = (row: (typeof rows)[number]) => row.values;
  const fieldsForRow = (row: (typeof rows)[number]) => {
    const fields = searchFieldsForSearchValues(valuesForRow(row));
    if (row.values.some((value) => String(value).includes("status:retired"))) {
      return {
        ...fields,
        fields: { ...fields.fields, "vps.status": ["online"] },
        namespaces: { ...fields.namespaces, status: ["online"] },
      };
    }
    return fields;
  };
  const suggestions = buildParseableSearchValueSuggestions(
    rows,
    valuesForRow,
    fieldsForRow,
  );
  expect(suggestions).toContain("id:agent-sfo-01");
  expect(suggestions).toContain("tag:edge");
  expect(suggestions).toContain("provider:alpha");
  expect(suggestions).toContain("schedule.failed");
  expect(suggestions).toContain("alert.category:network");
  expect(suggestions).toContain("status:retired");

  const nonMatchingSuggestionIndexes: number[] = [];
  for (const suggestion of suggestions) {
    const result = filterBySearchExpression(rows, suggestion, fieldsForRow);
    expect(result.error, suggestion).toBeNull();
    if (result.items.length === 0) {
      nonMatchingSuggestionIndexes.push(suggestions.indexOf(suggestion));
    }
  }
  expect(nonMatchingSuggestionIndexes.length).toBeGreaterThan(0);
  expect(Math.min(...nonMatchingSuggestionIndexes)).toBeGreaterThan(
    suggestions.indexOf("alert.category:network"),
  );
});

test("generic table autocomplete computes each row's search fields once", () => {
  const rows = Array.from({ length: 20 }, (_, index) => ({
    values: [
      `event-${index}`,
      `target:${index}`,
      `status:${index % 2 ? "online" : "offline"}`,
    ],
  }));
  let fieldBuilds = 0;

  const suggestions = buildParseableSearchValueSuggestions(
    rows,
    (row) => row.values,
    (row) => {
      fieldBuilds += 1;
      return searchFieldsForSearchValues(row.values);
    },
  );

  expect(suggestions.length).toBeGreaterThan(0);
  expect(fieldBuilds).toBe(rows.length);
});

function fieldsForContext(context: FixtureContext): SearchFields {
  const agent = agentFromContext(context);
  const providerTags = agent.tags.filter((tag) =>
    tag.toLocaleLowerCase().startsWith("provider:"),
  );
  const countryTags = agent.tags.filter((tag) =>
    tag.toLocaleLowerCase().startsWith("country:"),
  );
  const regionTags = agent.tags.filter((tag) =>
    tag.toLocaleLowerCase().startsWith("region:"),
  );
  const providerValues = providerTags.map((tag) =>
    tag.slice("provider:".length),
  );
  const countryValues = countryTags.map((tag) => tag.slice("country:".length));
  const regionValues = regionTags.map((tag) => tag.slice("region:".length));
  return {
    all: [agent.id, agent.display_name],
    events: (context.event_predicates ?? []).map((event) =>
      event.toLocaleLowerCase(),
    ),
    fields: {
      "alert.category": stringValues(context.alert?.category),
      "alert.severity": stringValues(context.alert?.severity),
      "alert.state": stringValues(context.alert?.state),
      "job.status": stringValues(context.job?.status),
      "job.target.status": stringValues(
        (context.job?.target as Record<string, unknown> | undefined)?.status,
      ),
      "job.type": stringValues(context.job?.type),
      "vps.country": countryValues,
      "vps.display_name": [agent.display_name],
      "vps.id": [agent.id],
      "vps.internal_build_number": [agent.internal_build_number ?? 0],
      "vps.last_seen_at": agent.last_seen_at ? [agent.last_seen_at] : [],
      "vps.provider": providerValues,
      "vps.region": regionValues,
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
      region: regionTags.concat(regionValues),
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

function vpsRuleRecord(
  rule: { json: VpsRuleValueRecord["value_json"]; key: string; raw: string },
  clientId = "one",
): VpsRuleValueRecord {
  return {
    client_id: clientId,
    key: rule.key,
    parsed_display: rule.raw,
    source_id: null,
    source_kind: "operator",
    state: "valid",
    updated_at: "2026-01-01T00:00:00Z",
    updated_by: null,
    validation_errors: [],
    value_json: rule.json,
    value_raw: rule.raw,
  };
}
