import { createSignal } from "solid-js";
import { getSetting, setSetting } from "./db.ts";

export type Theme = "light" | "dark" | "system";

const THEME_KEY = "ui_theme";

const [theme, setThemeSignal] = createSignal<Theme>("system");
// False until the saved theme has been read from the DB and applied;
// the app shell waits on this so the UI never paints in the wrong theme.
const [themeReadySignal, setThemeReady] = createSignal(false);

function isDark(theme: Theme): boolean {
  if (theme === "dark") return true;
  if (theme === "light") return false;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function apply(theme: Theme) {
  document.documentElement.classList.toggle("dark", isDark(theme));
}

export const currentTheme = theme;
export const themeReady = themeReadySignal;

export function setTheme(theme: Theme) {
  setThemeSignal(theme);
  void setSetting(THEME_KEY, theme);
  apply(theme);
}

const nextTheme: Record<Theme, Theme> = {
  light: "dark",
  dark: "system",
  system: "light",
};

export function cycleTheme() {
  setTheme(nextTheme[theme()]);
}

export async function initTheme() {
  try {
    const saved = (await getSetting(THEME_KEY) ?? "system") as Theme;
    setThemeSignal(saved);
    apply(saved);
  } finally {
    setThemeReady(true);
  }

  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => {
      if (theme() === "system") apply("system");
    });
}
