import { useEffect, useState } from "react";
import { apiGet } from "../api";
import { selectorExpressionForClientIds } from "../searchExpression";
import type { MonitoringCardsPageView } from "../types";

export function useProjectedProductName(
  apiToken: string,
  clientId: string | null | undefined,
  fallback: string | null = null,
): string | null {
  const [projection, setProjection] = useState<{
    apiToken: string;
    clientId: string;
    value: string | null;
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
        const card = page.items.find((item) => item.client.id === clientId);
        setProjection({
          apiToken,
          clientId,
          value: card?.product_name ?? null,
        });
      })
      .catch(() => {
        // Product metadata is optional. Retain the caller's authorized
        // fallback if the narrow fleet-read projection is unavailable.
      });
    return () => {
      active = false;
    };
  }, [apiToken, clientId]);

  return projection &&
    projection.apiToken === apiToken &&
    projection.clientId === clientId
    ? projection.value
    : fallback;
}
