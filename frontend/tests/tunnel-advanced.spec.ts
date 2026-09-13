import { expect, test } from "@playwright/test";
import {
  tunnelAdvancedFromRuntime,
  validateTunnelAdvanced,
  withTunnelAdvanced,
} from "../src/tunnelAdvanced";
import type { RuntimeTunnelControl } from "../src/types";

const runtime: RuntimeTunnelControl = {
  manager: "agent_builtin",
  openvpn: { transport: "udp", listener_side: "left", port: 1194 },
};

test("empty Advanced retains the existing runtime declaration without optional fields", () => {
  expect(withTunnelAdvanced(runtime, "openvpn", tunnelAdvancedFromRuntime())).toEqual(runtime);
});

test("endpoint hooks round trip direct argv, literal quotes and argument spacing", () => {
  const configured: RuntimeTunnelControl = {
    ...runtime,
    hooks: {
      left: { pre_start: { argv: ["/usr/bin/printf", "  {interface}  ", '"literal"'] } },
      right: { post_shutdown: { argv: ["/usr/bin/true"], max_timeout_secs: 45, max_output_bytes: 32768 } },
    },
  };
  const draft = tunnelAdvancedFromRuntime(configured);
  expect(validateTunnelAdvanced(draft)).toBeNull();
  expect(withTunnelAdvanced(runtime, "openvpn", draft)).toEqual(configured);
  draft.left.hooks.pre_start.argv += "\nanother argument";
  expect(withTunnelAdvanced(runtime, "openvpn", draft).hooks?.right).toEqual(configured.hooks?.right);
});

test("blank hook lines are removed at submission without altering the editable draft", () => {
  const draft = tunnelAdvancedFromRuntime();
  const typed = "\n/usr/bin/printf\n\n  {interface}  \n ";
  draft.left.hooks.pre_start.argv = typed;
  expect(validateTunnelAdvanced(draft)).toBeNull();
  expect(withTunnelAdvanced(runtime, "openvpn", draft).hooks?.left?.pre_start?.argv).toEqual(["/usr/bin/printf", "  {interface}  "]);
  expect(draft.left.hooks.pre_start.argv).toBe(typed);
});

test("clearing a hook removes its command and limits rather than retaining a hidden command", () => {
  const draft = tunnelAdvancedFromRuntime({
    ...runtime,
    hooks: { left: { pre_start: { argv: ["/bin/true"], max_timeout_secs: 50 } } },
  });
  draft.left.hooks.pre_start.argv = "";
  expect(withTunnelAdvanced(runtime, "openvpn", draft).hooks).toBeUndefined();
});

test("OpenVPN overrides retain exact native text per side without frontend tuning checks", () => {
  const draft = tunnelAdvancedFromRuntime();
  draft.left.configOverride = "# My tuning\nping 15\nunknown-native-option abc\n";
  draft.right.configOverride = "verb 4";
  expect(validateTunnelAdvanced(draft)).toBeNull();
  const configured = withTunnelAdvanced(runtime, "openvpn", draft);
  expect(configured.openvpn?.left_config_override).toBe(draft.left.configOverride);
  expect(tunnelAdvancedFromRuntime(configured)).toEqual(draft);
  expect(withTunnelAdvanced({ manager: "agent_builtin" }, "gre", draft).openvpn).toBeUndefined();
});

test("Advanced drafts cannot leak into non-builtin runtime ownership", () => {
  const draft = tunnelAdvancedFromRuntime();
  draft.left.configOverride = "ping 15";
  draft.left.hooks.pre_start.argv = "/bin/true";
  for (const manager of ["custom_adapter", "external_observed"] as const) {
    const declaration = { manager };
    expect(withTunnelAdvanced(declaration, "openvpn", draft)).toEqual(declaration);
  }
});

test("hook validation reuses executable and command budget requirements", () => {
  const draft = tunnelAdvancedFromRuntime();
  draft.left.hooks.pre_start.argv = "printf\n{interface}";
  expect(validateTunnelAdvanced(draft)).toContain("absolute executable path");
  draft.left.hooks.pre_start.argv = "/usr/bin/printf\n{interface}";
  draft.left.hooks.pre_start.timeout = "121";
  expect(validateTunnelAdvanced(draft)).toContain("1 to 120");
  draft.left.hooks.pre_start.timeout = "120";
  draft.left.hooks.pre_start.output = "65536";
  expect(validateTunnelAdvanced(draft)).toBeNull();
});
