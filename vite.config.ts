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
  // In production, drop the `development` condition so solid-js (and
  // other condition-aware libs) resolve to their smaller prod builds
  // instead of the dev builds (which carry dev-only checks + warnings).
  resolve: {
    conditions: mode === "production"
      ? ["browser", "import"]
      : ["browser", "development", "import"],
    alias: {
      // @git-diff-view/lowlight bundles highlight.js `all` (~190 langs, ~1MB).
      // Redirect to a slim vendored build that registers only ~16 langs the
      // agent diff view actually renders. Same export surface, far smaller.
      "@git-diff-view/lowlight": new URL("./src/vendor/git-diff-lowlight.mjs", import.meta.url).pathname,
    },
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
    // Aggressive rolldown tree-shaking + oxc minify options.
    // Only in production — dev stays default for fast rebuilds + warnings.
    rollupOptions: mode === "production"
      ? {
        treeshake: {
          // Assume no module has side effects beyond its exports; lets
          // rolldown drop entire unused modules (e.g. ark-ui/zag helpers
          // that only re-export, micromark sub-packages pulled in by
          // barrel files). Safe because every dep here is pure ESM with
          // `sideEffects: false` in its package.json.
          moduleSideEffects: false,
          // Reading a property is assumed side-effect-free; drops getters
          // that only return constants (common in zag-js state machines).
          propertyReadSideEffects: false,
          // Don't treat unknown global accesses as side-effecting; removes
          // defensive guards around `globalThis.*` / `window.*` reads.
          unknownGlobalSideEffects: false,
          // Treat these dev-only helpers as pure so calls drop in prod.
          manualPureFunctions: [
            "console.log",
            "console.warn",
            "console.error",
            "console.debug",
            "console.info",
            "console.trace",
          ],
        },
        output: {
          // Mangle internal export names (non-exported identifiers) for
          // shorter generated code. Safe in a single-chunk bundle.
          minifyInternalExports: true,
        },
      }
      : undefined,
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
