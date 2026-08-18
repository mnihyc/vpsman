import { useEffect, useState } from "react";
import { apiGet } from "../api";
import { selectorExpressionForClientIds } from "../searchExpression";
import type {
  MonitoringCardView,
  MonitoringCardsPageView,
  SystemInformationView,
} from "../types";

type ProjectedMonitoringMetadata = {
  loading: boolean;
  productName: string | null;
  systemInformation: SystemInformationView | null;
};

export function useProjectedMonitoringMetadata(
  apiToken: string,
  clientId: string | null | undefined,
  fallbackProductName: string | null = null,
): ProjectedMonitoringMetadata {
  const [projection, setProjection] = useState<{
    apiToken: string;
    card: MonitoringCardView | null;
    clientId: string;
    failed: boolean;
  } | null>(null);

  useEffect(() => {
    if (!apiToken || !clientId) {
      setProjection(null);
      return;
    }
    setProjection((current) =>
      current?.apiToken === apiToken && current.clientId === clientId
        ? current
        : null,
    );
    let active = true;
    const params = new URLSearchParams({
      include_history: "false",
      limit: "1",
      offset: "0",
      selector_expression: selectorExpressionForClientIds([clientId]),
    });
    void apiGet<MonitoringCardsPageView>(
      `/api/v1/monitoring/cards?${params.toString()}`,
      apiToken,
    )
      .then((page) => {
        if (!active) return;
        setProjection({
          apiToken,
          card: page.items.find((item) => item.client.id === clientId) ?? null,
          clientId,
          failed: false,
        });
      })
      .catch(() => {
        if (!active) return;
        // Display metadata is optional. Retain the caller's authorized
        // product fallback and leave system information unavailable.
        setProjection({ apiToken, card: null, clientId, failed: true });
      });
    return () => {
      active = false;
    };
  }, [apiToken, clientId]);

  const current =
    projection &&
    projection.apiToken === apiToken &&
    projection.clientId === clientId
      ? projection
      : null;
  return {
    loading: Boolean(apiToken && clientId && !current),
    productName:
      current && !current.failed
        ? (current.card?.product_name ?? null)
        : fallbackProductName,
    systemInformation: current?.card?.system_information ?? null,
  };
}
