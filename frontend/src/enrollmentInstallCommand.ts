export const DEFAULT_ENROLLMENT_INSTALL_COMMAND_TEMPLATE =
  "curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env VPSMAN_INSTALL_MODE={INSTALL_MODE} VPSMAN_ENROLLMENT_API_URL={API_URL} VPSMAN_ENROLLMENT_TOKEN={TOKEN} bash";

export const ENROLLMENT_INSTALL_TEMPLATE_VARIABLES = [
  "TOKEN",
  "API_URL",
  "INSTALL_MODE",
] as const;

type EnrollmentInstallTemplateVariable = (typeof ENROLLMENT_INSTALL_TEMPLATE_VARIABLES)[number];

export type EnrollmentInstallCommandValues = {
  apiUrl: string;
  installMode?: string;
  token?: string | null;
};

export type EnrollmentInstallCommandRender = {
  command: string | null;
  error: string | null;
  missing: string[];
};

const VARIABLE_PATTERN = /\{([^{}]+)\}/g;
const SUPPORTED_VARIABLES = new Set<string>(ENROLLMENT_INSTALL_TEMPLATE_VARIABLES);

export function renderEnrollmentInstallCommand(
  template: string,
  values: EnrollmentInstallCommandValues,
): EnrollmentInstallCommandRender {
  const source = template.trim() || DEFAULT_ENROLLMENT_INSTALL_COMMAND_TEMPLATE;
  if (hasMalformedTemplateBraces(source)) {
    return {
      command: null,
      error: "Invalid template braces",
      missing: [],
    };
  }
  const variables = templateVariables(source);
  const unknown = variables.filter((variable) => !SUPPORTED_VARIABLES.has(variable));
  if (unknown.length > 0) {
    return {
      command: null,
      error: `Unknown variable {${unknown[0]}}`,
      missing: [],
    };
  }
  if (!variables.includes("TOKEN")) {
    return {
      command: null,
      error: "Template must include {TOKEN}",
      missing: [],
    };
  }
  const missing = variables.filter((variable) => !valueForVariable(variable as EnrollmentInstallTemplateVariable, values));
  if (missing.length > 0) {
    return {
      command: null,
      error: missing.some((variable) => variable === "TOKEN")
        ? "Create token to generate command"
        : `Set {${missing[0]}} to generate command`,
      missing,
    };
  }
  return {
    command: source.replace(VARIABLE_PATTERN, (_match, variable: string) =>
      shellQuote(valueForVariable(variable as EnrollmentInstallTemplateVariable, values) ?? ""),
    ),
    error: null,
    missing: [],
  };
}

export function validateEnrollmentInstallCommandTemplate(template: string): string | null {
  const source = template.trim();
  if (!source) {
    return "Enrollment install command template is required";
  }
  if (hasMalformedTemplateBraces(source)) {
    return "Enrollment install command template has invalid braces";
  }
  if (source.length > 2000) {
    return "Enrollment install command template must be 2000 characters or less";
  }
  const unknown = templateVariables(source).filter((variable) => !SUPPORTED_VARIABLES.has(variable));
  if (unknown.length > 0) {
    return `Unknown enrollment install variable {${unknown[0]}}`;
  }
  return templateVariables(source).includes("TOKEN") ? null : "Enrollment install command template must include {TOKEN}";
}

function templateVariables(template: string): string[] {
  const variables = new Set<string>();
  for (const match of template.matchAll(VARIABLE_PATTERN)) {
    variables.add(match[1]);
  }
  return Array.from(variables);
}

function hasMalformedTemplateBraces(template: string): boolean {
  return template.replace(VARIABLE_PATTERN, "").includes("{") || template.replace(VARIABLE_PATTERN, "").includes("}");
}

function valueForVariable(variable: EnrollmentInstallTemplateVariable, values: EnrollmentInstallCommandValues): string | null {
  switch (variable) {
    case "TOKEN":
      return clean(values.token);
    case "API_URL":
      return clean(values.apiUrl);
    case "INSTALL_MODE":
      return clean(values.installMode ?? "root");
  }
}

function clean(value: string | null | undefined): string | null {
  const trimmed = value?.trim() ?? "";
  return trimmed ? trimmed : null;
}

function shellQuote(value: string): string {
  return `'${value.split("'").join("'\\''")}'`;
}
