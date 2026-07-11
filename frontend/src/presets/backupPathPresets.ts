import type { BackupMissingPathPolicy } from "../types";

export const DEFAULT_BACKUP_SELECTED_PATHS = "/etc/hostname";
export const DEFAULT_RESTORE_SELECTED_PATHS = "/etc/hostname";

export type BackupPathPreset = {
  description: string;
  label: string;
  missingPathPolicy: BackupMissingPathPolicy;
  paths: string[];
};

export const BACKUP_PATH_PRESETS: BackupPathPreset[] = [
  {
    description:
      "Strict snapshot of host identity files; every selected file must exist.",
    label: "Identity",
    missingPathPolicy: "fail",
    paths: ["/etc/hostname", "/etc/hosts"],
  },
  {
    description:
      "Regular files under common Linux identity, SSH, service, and network configuration roots. Unavailable roots are skipped.",
    label: "OS config",
    missingPathPolicy: "skip",
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
    description:
      "Regular files under common reverse-proxy configuration roots. Application data is intentionally excluded.",
    label: "Web config",
    missingPathPolicy: "skip",
    paths: ["/etc/nginx", "/etc/caddy"],
  },
  {
    description:
      "Regular files under Docker daemon configuration. Compose projects and volume data require a purpose-built backup workflow.",
    label: "Docker config",
    missingPathPolicy: "skip",
    paths: ["/etc/docker"],
  },
];

export const RESTORE_PATH_PRESETS: BackupPathPreset[] = [
  {
    description: "Restore only host identity files for a low-risk rehearsal.",
    label: "Identity",
    missingPathPolicy: "fail",
    paths: ["/etc/hostname", "/etc/hosts"],
  },
  {
    description:
      "Restore service and network configuration captured by OS config backups.",
    label: "OS config",
    missingPathPolicy: "skip",
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
    description: "Restore common reverse-proxy configuration paths.",
    label: "Web config",
    missingPathPolicy: "skip",
    paths: ["/etc/nginx", "/etc/caddy"],
  },
];

export const BACKUP_PATH_PLACEHOLDER =
  "/etc/hostname\n/etc/network/interfaces.d";
export const RESTORE_PATH_PLACEHOLDER =
  "/etc/hostname\n/etc/network/interfaces.d";

export function presetPathsText(paths: string[]): string {
  return paths.join("\n");
}
