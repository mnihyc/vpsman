import type { RuntimeTunnelControl, RuntimeTunnelTopologyIntent } from "./types";

export type PromoteTunnelPlanToCustomAdapterRequest = {
  plan_id: string;
  runtime_control: RuntimeTunnelControl;
  runtime_topology?: RuntimeTunnelTopologyIntent | null;
  name?: string | null;
  confirmed: boolean;
};
