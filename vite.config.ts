import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async ({ mode }) => ({
  plugins: [tailwindcss(), solid()],

  // Pin browser conditions so the client build resolves correctly under
  // `deno task dev` (Deno injects a `deno` condition that can otherwise
  // win over `browser`/`solid` and load SSR bundles on the client).
  resolve: {
    conditions: ["browser", "development", "import"],
  },

  // Pre-bundle solid-js so cold start skip dep re-discovery.
  optimizeDeps: {
    include: [
      "solid-js",
      "solid-js/web",
      "solid-js/store",
    ],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,

  // Native minifiers: esbuild (Go) for JS, lightningcss (Rust) for CSS.
  // Drop console/debugger in production for smaller, cleaner output.
  esbuild: {
    drop: mode === "production" ? ["console", "debugger"] : [],
    legalComments: "none",
  },

  // Build: target esnext skip ES2020 downlevel
  build: {
    target: "esnext",
    cssMinify: "lightningcss",
    reportCompressedSize: false,
  },
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
