import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Task 3 shell config. Fixed dev port and strictPort match tauri.conf.json's
// devUrl; ignoring src-tauri keeps cargo build artifacts from retriggering
// the frontend dev server.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
