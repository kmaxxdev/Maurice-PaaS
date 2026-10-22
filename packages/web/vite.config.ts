import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  server: {
    port: 5173,
    // The panel API runs on 7700; the SPA talks to it through the dev proxy.
    proxy: { "/maurice.v1.": "http://127.0.0.1:7700", "/healthz": "http://127.0.0.1:7700" },
  },
  build: { outDir: "dist", emptyOutDir: true },
});
