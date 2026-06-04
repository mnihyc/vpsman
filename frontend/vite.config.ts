import { defineConfig } from "vite";

const apiTarget = process.env.VPSMAN_API_PROXY ?? "http://127.0.0.1:18080";

export default defineConfig({
  server: {
    proxy: {
      "/api": apiTarget,
      "/health": apiTarget,
      "/ws": {
        target: apiTarget,
        ws: true,
      },
    },
  },
});
