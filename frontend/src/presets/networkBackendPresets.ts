import type { TunnelConfigBackend } from "../types";

export type NetworkBackendFilePreset = {
  blockKind: string;
  managedPath: string;
};

const NETWORK_BACKEND_PRESETS: Record<TunnelConfigBackend, NetworkBackendFilePreset[]> = {
  ifupdown: [
    {
      blockKind: "ifupdown",
      managedPath: "/etc/network/interfaces.d/vpsman-tunnels",
    },
  ],
  netplan: [
    {
      blockKind: "netplan",
      managedPath: "/etc/netplan/90-vpsman-tunnels.yaml",
    },
  ],
  systemd_networkd: [
    {
      blockKind: "systemd_networkd_netdev",
      managedPath: "/etc/systemd/network/90-vpsman-tunnels.netdev",
    },
    {
      blockKind: "systemd_networkd_network",
      managedPath: "/etc/systemd/network/90-vpsman-tunnels.network",
    },
  ],
};

export function networkBackendFilePresets(backend: TunnelConfigBackend): NetworkBackendFilePreset[] {
  return NETWORK_BACKEND_PRESETS[backend];
}

export function networkBackendPresetLabel(backend: TunnelConfigBackend): string {
  return networkBackendFilePresets(backend)
    .map((preset) => preset.managedPath)
    .join(", ");
}
