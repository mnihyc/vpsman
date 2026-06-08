export const DEFAULT_BACKUP_SELECTED_PATHS = "/etc/hostname";
export const DEFAULT_RESTORE_SELECTED_PATHS = "/etc/hostname";

export type BackupPathPreset = {
  description: string;
  label: string;
  paths: string[];
};

export const BACKUP_PATH_PRESETS: BackupPathPreset[] = [
  {
    description:
      "Small identity snapshot for connectivity and inventory checks.",
    label: "Identity",
    paths: ["/etc/hostname", "/etc/hosts"],
  },
  {
    description:
      "Common Linux service, SSH, network, and package configuration.",
    label: "System config",
    paths: [
      "/etc/hostname",
      "/etc/hosts",
      "/etc/ssh",
      "/etc/systemd/system",
      "/etc/network",
      "/etc/netplan",
    ],
  },
  {
    description: "Typical web app roots and reverse-proxy configuration.",
    label: "Web stack",
    paths: ["/etc/nginx", "/etc/caddy", "/srv", "/var/www"],
  },
  {
    description: "Container compose files and persistent container state.",
    label: "Docker data",
    paths: ["/etc/docker", "/opt", "/srv", "/var/lib/docker/volumes"],
  },
];

export const RESTORE_PATH_PRESETS: BackupPathPreset[] = [
  {
    description: "Restore only host identity files for a low-risk rehearsal.",
    label: "Identity",
    paths: ["/etc/hostname", "/etc/hosts"],
  },
  {
    description:
      "Restore service and network configuration captured by system config backups.",
    label: "System config",
    paths: [
      "/etc/hostname",
      "/etc/hosts",
      "/etc/ssh",
      "/etc/systemd/system",
      "/etc/network",
      "/etc/netplan",
    ],
  },
  {
    description: "Restore common web application and proxy paths.",
    label: "Web stack",
    paths: ["/etc/nginx", "/etc/caddy", "/srv", "/var/www"],
  },
];

export const BACKUP_PATH_PLACEHOLDER =
  "/etc/hostname\n/etc/network/interfaces.d";
export const RESTORE_PATH_PLACEHOLDER =
  "/etc/hostname\n/etc/network/interfaces.d";

export function presetPathsText(paths: string[]): string {
  return paths.join("\n");
}
