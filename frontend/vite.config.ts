import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { copyFileSync, mkdirSync } from "node:fs";

// Reuse the audited local Plotly bundle. It is never loaded from a CDN.
export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    {
      name: "local-plotly",
      closeBundle() {
        mkdirSync("dist/assets", { recursive: true });
        copyFileSync(
          "../assets/plotly-2.35.2.min.js",
          "dist/assets/plotly-2.35.2.min.js",
        );
      },
    },
  ],
  server: {
    port: 5173,
    proxy: {
      "/api": "http://127.0.0.1:8080",
      "/assets/plotly-2.35.2.min.js": "http://127.0.0.1:8080",
    },
  },
});
