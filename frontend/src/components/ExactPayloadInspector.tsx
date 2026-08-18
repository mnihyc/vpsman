import type { ReactNode } from "react";

export type ExactPayloadInspectorItem = {
  detail: string;
  error?: string | null;
  label: string;
  value: string;
};

export function ExactPayloadInspector({
  ariaLabel,
  exactValue,
  exactValueLabel,
  footer,
  help,
  items,
  summary,
  summaryIsError = false,
  title,
}: {
  ariaLabel: string;
  exactValue: string;
  exactValueLabel: string;
  footer?: ReactNode;
  help: string;
  items: ExactPayloadInspectorItem[];
  summary: string;
  summaryIsError?: boolean;
  title: string;
}) {
  return (
    <section aria-label={ariaLabel} className="exactPayloadInspector">
      <header className="exactPayloadInspectorHeader">
        <span className="exactPayloadInspectorTitle">
          <span className="fieldLabelWithHelp">
            <strong>{title}</strong>
            <span
              aria-label={`${title} help`}
              className="fieldHelpIcon"
              role="img"
              tabIndex={0}
              title={help}
            >
              ?
            </span>
          </span>
          <small role={summaryIsError ? "alert" : undefined}>{summary}</small>
        </span>
        <code
          aria-label={exactValueLabel}
          className="exactPayloadSerialized"
          tabIndex={0}
        >
          {exactValue}
        </code>
      </header>
      <ol className="exactPayloadElements">
        {items.map((item, index) => (
          <li
            className={item.error ? "invalid" : undefined}
            key={`${index}:${item.label}:${item.value}`}
          >
            <span className="exactPayloadIndex">{item.label}</span>
            <code aria-label={`${item.label} exact value`} tabIndex={0}>
              {item.value}
            </code>
            <small role={item.error ? "alert" : undefined}>
              {item.error ?? item.detail}
            </small>
          </li>
        ))}
      </ol>
      {footer ? <small className="mutedText">{footer}</small> : null}
    </section>
  );
}

export function ArgvInspector({
  ariaLabel,
  argv,
  elementDetail,
  elementError,
  error,
  footer,
  help,
  summary,
  title,
}: {
  ariaLabel: string;
  argv: string[];
  elementDetail: (value: string, index: number) => string;
  elementError?: (value: string, index: number) => string | null;
  error?: string | null;
  footer?: ReactNode;
  help: string;
  summary?: string;
  title: string;
}) {
  const countSummary = `${argv.length} ordered JSON ${argv.length === 1 ? "element" : "elements"}`;
  const items: ExactPayloadInspectorItem[] = argv.map((value, index) => ({
    detail: elementDetail(value, index),
    error: elementError?.(value, index) ?? null,
    label: `argv[${index}]`,
    value,
  }));
  if (items.length === 0) {
    items.push({
      detail: error
        ? "Correct the authoring syntax to inspect the ordered payload."
        : "Enter a command to preview its ordered direct argv payload.",
      error,
      label: "argv",
      value: error ? "Payload unavailable" : "No arguments yet",
    });
  }
  return (
    <ExactPayloadInspector
      ariaLabel={ariaLabel}
      exactValue={
        error ? "Invalid input · no argv payload" : JSON.stringify(argv)
      }
      exactValueLabel="Exact argv JSON value"
      footer={footer}
      help={help}
      items={items}
      summary={error ? `Parser error · ${error}` : (summary ?? countSummary)}
      summaryIsError={Boolean(error)}
      title={title}
    />
  );
}
