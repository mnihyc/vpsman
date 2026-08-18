// Compatibility exports for call sites moving to the unified alert model.
// New FleetAlertRecord consumers should import from alertPresentation directly.
export {
  alertLifecycleLabel as policyAlertLifecycleLabel,
  alertLifecycleTone as policyAlertLifecycleTone,
  isActiveFleetAlert,
  isActivePolicyAlert,
  isCurrentPolicyAlert,
  isResolvedPolicyAlert,
} from "./alertPresentation";

import { presentFleetAlert } from "./alertPresentation";
import type { FleetAlertRecord } from "./types";

export function fleetAlertLifecycleState(
  alert: FleetAlertRecord,
): string | null {
  return presentFleetAlert(alert).lifecycleState;
}
