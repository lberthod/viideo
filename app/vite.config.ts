import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri attend un port fixe et ne doit pas masquer les erreurs Rust dans la console.
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "safari16", outDir: "dist" },
});
