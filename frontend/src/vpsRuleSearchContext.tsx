import { createContext, useContext, type ReactNode } from "react";
import type { VpsRuleValueRecord } from "./types";
import { indexVpsRulesByClient } from "./vpsRules";

export type VpsRuleSearchContextValue = {
  available: boolean;
  rules: readonly VpsRuleValueRecord[];
  rulesByClient: ReadonlyMap<string, readonly VpsRuleValueRecord[]>;
};

const fallbackValue: VpsRuleSearchContextValue = {
  available: false,
  rules: [],
  rulesByClient: new Map(),
};

const VpsRuleSearchContext =
  createContext<VpsRuleSearchContextValue>(fallbackValue);

export function createVpsRuleSearchContextValue(
  rules: readonly VpsRuleValueRecord[],
  available: boolean,
): VpsRuleSearchContextValue {
  return {
    available,
    rules,
    rulesByClient: indexVpsRulesByClient(rules),
  };
}

export function VpsRuleSearchProvider({
  children,
  value,
}: {
  children: ReactNode;
  value: VpsRuleSearchContextValue;
}) {
  return (
    <VpsRuleSearchContext.Provider value={value}>
      {children}
    </VpsRuleSearchContext.Provider>
  );
}

export function useVpsRuleSearchContext(): VpsRuleSearchContextValue {
  return useContext(VpsRuleSearchContext);
}
