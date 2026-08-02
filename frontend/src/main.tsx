import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import "@xterm/xterm/css/xterm.css";
import { App } from "./App";
import { PublicMonitoringSharePage } from "./PublicMonitoringSharePage";
import { parsePublicShareRouteHash } from "./publicShareRoute";
import "./styles.css";

function RootRouter() {
  const [shareRoute, setShareRoute] = useState(() =>
    parsePublicShareRouteHash(window.location.hash),
  );
  useEffect(() => {
    const applyLocation = () =>
      setShareRoute(parsePublicShareRouteHash(window.location.hash));
    window.addEventListener("hashchange", applyLocation);
    window.addEventListener("popstate", applyLocation);
    return () => {
      window.removeEventListener("hashchange", applyLocation);
      window.removeEventListener("popstate", applyLocation);
    };
  }, []);
  return shareRoute ? (
    <PublicMonitoringSharePage
      initialClientKey={shareRoute.clientKey}
      key={`${shareRoute.shareId}:${shareRoute.secret}`}
      secret={shareRoute.secret}
      shareId={shareRoute.shareId}
    />
  ) : <App />;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <RootRouter />
  </React.StrictMode>,
);
