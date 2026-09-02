import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  // The bundle is served three ways — from Tauri's asset protocol, from the
  // Axum server, and from the Docker image — so every URL it emits must be
  // relative. An absolute /assets/... path breaks the moment the app is not at
  // the origin root.
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    // The Content-Security-Policy in both shells forbids inline scripts, so the
    // bundler must never inline one.
    assetsInlineLimit: 0,
  },
  server: { port: 5173, strictPort: true },
});
