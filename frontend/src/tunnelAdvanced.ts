import type {
  RuntimeTunnelCommand,
  RuntimeTunnelControl,
  RuntimeTunnelLifecycleHooks,
  TunnelEndpointSide,
  TunnelKind,
} from "./types";

export const TUNNEL_HOOK_PHASES = [
  { key: "pre_start", label: "Pre-start" },
  { key: "post_start", label: "Post-start" },
  { key: "pre_shutdown", label: "Pre-shutdown" },
  { key: "post_shutdown", label: "Post-shutdown" },
] as const;

export type TunnelHookDraft = {
  argv: string;
  timeout: string;
  output: string;
};

export type TunnelEndpointAdvancedDraft = {
  configOverride: string;
  hooks: Record<keyof RuntimeTunnelLifecycleHooks, TunnelHookDraft>;
};

export type TunnelAdvancedDraft = Record<
  TunnelEndpointSide,
  TunnelEndpointAdvancedDraft
>;

export function tunnelAdvancedFromRuntime(
  runtime?: RuntimeTunnelControl,
): TunnelAdvancedDraft {
  const endpoint = (side: TunnelEndpointSide): TunnelEndpointAdvancedDraft => ({
    configOverride: runtime?.openvpn?.[`${side}_config_override`] ?? "",
    hooks: Object.fromEntries(
      TUNNEL_HOOK_PHASES.map(({ key }) => {
        const command = runtime?.hooks?.[side]?.[key];
        return [
          key,
          {
            argv: command?.argv.join("\n") ?? "",
            timeout: command?.max_timeout_secs == null
              ? ""
              : String(command.max_timeout_secs),
            output: command?.max_output_bytes == null
              ? ""
              : String(command.max_output_bytes),
          },
        ];
      }),
    ) as TunnelEndpointAdvancedDraft["hooks"],
  });
  return { left: endpoint("left"), right: endpoint("right") };
}

function commandFromDraft(
  draft: TunnelHookDraft,
): RuntimeTunnelCommand | undefined {
  if (!draft.argv.trim()) return undefined;
  return {
    // Parse only on submission: skip blank lines without trimming actual arguments.
    argv: draft.argv
      .replace(/\r\n/g, "\n")
      .split("\n")
      .filter((line) => line.trim().length > 0),
    ...(draft.timeout.trim() ? { max_timeout_secs: Number(draft.timeout) } : {}),
    ...(draft.output.trim() ? { max_output_bytes: Number(draft.output) } : {}),
  };
}

export function validateTunnelAdvanced(
  draft: TunnelAdvancedDraft,
): string | null {
  for (const side of ["left", "right"] as const) {
    for (const { key, label } of TUNNEL_HOOK_PHASES) {
      const command = draft[side].hooks[key];
      if (!command.argv.trim()) continue;
      const argv = commandFromDraft(command)!.argv;
      if (!argv[0]?.startsWith("/")) {
        return `${side === "left" ? "Left" : "Right"} ${label}: the first argument must be an absolute executable path`;
      }
      if (
        argv.length > 32 ||
        argv.some((argument) =>
          argument.includes("\0") ||
          new TextEncoder().encode(argument).byteLength > 4096,
        )
      ) {
        return `${side === "left" ? "Left" : "Right"} ${label}: use at most 32 arguments, each at most 4096 UTF-8 bytes and without NUL characters`;
      }
      for (const [name, value, minimum, maximum] of [
        ["timeout seconds", command.timeout, 1, 120],
        ["maximum output bytes", command.output, 1024, 65536],
      ] as const) {
        if (
          value.trim() &&
          (!Number.isSafeInteger(Number(value)) ||
            Number(value) < minimum ||
            Number(value) > maximum)
        ) {
          return `${side === "left" ? "Left" : "Right"} ${label}: ${name} must be a whole number from ${minimum} to ${maximum}`;
        }
      }
    }
  }
  return null;
}

export function withTunnelAdvanced(
  runtime: RuntimeTunnelControl,
  kind: TunnelKind,
  draft: TunnelAdvancedDraft,
): RuntimeTunnelControl {
  if (runtime.manager !== "agent_builtin") return runtime;
  const hooks = Object.fromEntries(
    (["left", "right"] as const).flatMap((side) => {
      const commands = Object.fromEntries(
        TUNNEL_HOOK_PHASES.flatMap(({ key }) => {
          const command = commandFromDraft(draft[side].hooks[key]);
          return command ? [[key, command]] : [];
        }),
      );
      return Object.keys(commands).length ? [[side, commands]] : [];
    }),
  );
  const openvpn = kind === "openvpn" && runtime.openvpn
    ? {
        openvpn: {
          ...runtime.openvpn,
          ...(draft.left.configOverride.trim()
            ? { left_config_override: draft.left.configOverride }
            : {}),
          ...(draft.right.configOverride.trim()
            ? { right_config_override: draft.right.configOverride }
            : {}),
        },
      }
    : {};
  return { ...runtime, ...openvpn, ...(Object.keys(hooks).length ? { hooks } : {}) };
}
