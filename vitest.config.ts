import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  resolve: {
    conditions: ["browser", "development", "import"],
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.tsx"],
    deps: {
      optimizer: { web: { include: ["solid-js", "solid-js/web", "solid-js/store"] } },
    },
  },
});
