import { createSignal } from "solid-js";
import { getSetting, setSetting } from "./db.ts";

export type Theme = "light" | "dark" | "system";

const THEME_KEY = "ui_theme";

const [theme, setThemeSignal] = createSignal<Theme>("system");

function isDark(t: Theme): boolean {
  if (t === "dark") return true;
  if (t === "light") return false;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function apply(t: Theme) {
  document.documentElement.classList.toggle("dark", isDark(t));
}

export const currentTheme = theme;

export function setTheme(t: Theme) {
  setThemeSignal(t);
  void setSetting(THEME_KEY, t);
  apply(t);
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
  const saved = (await getSetting(THEME_KEY) ?? "system") as Theme;
  setThemeSignal(saved);
  apply(saved);

  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => {
      if (theme() === "system") apply("system");
    });
}
