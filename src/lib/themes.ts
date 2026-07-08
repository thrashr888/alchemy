// Color schemes. Each theme is a full set of design tokens applied to
// :root as CSS custom properties; Tailwind reads them via @theme inline.

export interface Theme {
  id: string;
  label: string;
  dark: boolean;
  vars: Record<string, string>;
}

export const DEFAULT_THEME = "midnight";

// Shared rgba helpers keep border/input alphas consistent within a mode.
const darkBorder = {
  border: "rgba(255,255,255,0.07)",
  "border-strong": "rgba(255,255,255,0.12)",
  input: "rgba(255,255,255,0.09)",
  scrollbar: "rgba(255,255,255,0.10)",
};
const lightBorder = {
  border: "rgba(0,0,0,0.09)",
  "border-strong": "rgba(0,0,0,0.16)",
  input: "rgba(0,0,0,0.12)",
  scrollbar: "rgba(0,0,0,0.18)",
};

export const THEMES: Record<string, Theme> = {
  midnight: {
    id: "midnight",
    label: "Midnight",
    dark: true,
    vars: {
      background: "#08090a", surface: "#0d0e10", "surface-2": "#141517", elevated: "#18191c",
      foreground: "#eceef1", muted: "#16171a", "muted-foreground": "#8a8f98",
      "subtle-foreground": "#62666d", ring: "#5e6ad2", primary: "#5e6ad2",
      "primary-hover": "#6c78e0", "primary-foreground": "#ffffff", accent: "#1c1d21",
      "accent-foreground": "#eceef1", destructive: "#eb5757", success: "#4cb782",
      citation: "#8b95f5", selection: "rgba(94,106,210,0.35)", ...darkBorder,
    },
  },
  light: {
    id: "light",
    label: "Light",
    dark: false,
    vars: {
      background: "#ffffff", surface: "#fbfbfa", "surface-2": "#f3f3f1", elevated: "#ffffff",
      foreground: "#1c1d20", muted: "#f3f3f1", "muted-foreground": "#6b7079",
      "subtle-foreground": "#9aa0a8", ring: "#5e6ad2", primary: "#5e6ad2",
      "primary-hover": "#4f5bc4", "primary-foreground": "#ffffff", accent: "#eceef1",
      "accent-foreground": "#1c1d20", destructive: "#d93636", success: "#2f9e6b",
      citation: "#5159c9", selection: "rgba(94,106,210,0.22)", ...lightBorder,
    },
  },
  slate: {
    id: "slate",
    label: "Slate",
    dark: true,
    vars: {
      background: "#0f172a", surface: "#141d33", "surface-2": "#1e293b", elevated: "#243044",
      foreground: "#e2e8f0", muted: "#1e293b", "muted-foreground": "#94a3b8",
      "subtle-foreground": "#64748b", ring: "#6366f1", primary: "#6366f1",
      "primary-hover": "#7c7ff5", "primary-foreground": "#ffffff", accent: "#1e293b",
      "accent-foreground": "#e2e8f0", destructive: "#f87171", success: "#34d399",
      citation: "#818cf8", selection: "rgba(99,102,241,0.32)", ...darkBorder,
    },
  },
  dracula: {
    id: "dracula",
    label: "Dracula",
    dark: true,
    vars: {
      background: "#21222c", surface: "#282a36", "surface-2": "#343746", elevated: "#3a3d4d",
      foreground: "#f8f8f2", muted: "#343746", "muted-foreground": "#9ba0c0",
      "subtle-foreground": "#6272a4", ring: "#bd93f9", primary: "#bd93f9",
      "primary-hover": "#cbaaff", "primary-foreground": "#21222c", accent: "#343746",
      "accent-foreground": "#f8f8f2", destructive: "#ff5555", success: "#50fa7b",
      citation: "#8be9fd", selection: "rgba(189,147,249,0.35)", ...darkBorder,
    },
  },
  monokai: {
    id: "monokai",
    label: "Monokai",
    dark: true,
    vars: {
      background: "#1e1f1c", surface: "#272822", "surface-2": "#33342c", elevated: "#3b3c33",
      foreground: "#f8f8f2", muted: "#33342c", "muted-foreground": "#a6a28c",
      "subtle-foreground": "#75715e", ring: "#a6e22e", primary: "#a6e22e",
      "primary-hover": "#b6ee48", "primary-foreground": "#1e1f1c", accent: "#33342c",
      "accent-foreground": "#f8f8f2", destructive: "#f92672", success: "#a6e22e",
      citation: "#66d9ef", selection: "rgba(166,226,46,0.25)", ...darkBorder,
    },
  },
  github: {
    id: "github",
    label: "GitHub",
    dark: true,
    vars: {
      background: "#0d1117", surface: "#11151c", "surface-2": "#161b22", elevated: "#1c2128",
      foreground: "#c9d1d9", muted: "#161b22", "muted-foreground": "#8b949e",
      "subtle-foreground": "#6e7681", ring: "#2f81f7", primary: "#2f81f7",
      "primary-hover": "#4c92f8", "primary-foreground": "#ffffff", accent: "#161b22",
      "accent-foreground": "#c9d1d9", destructive: "#f85149", success: "#3fb950",
      citation: "#58a6ff", selection: "rgba(47,129,247,0.30)", ...darkBorder,
    },
  },
  solarized: {
    id: "solarized",
    label: "Solarized",
    dark: true,
    vars: {
      background: "#002b36", surface: "#073642", "surface-2": "#0a4351", elevated: "#0e4b5a",
      foreground: "#93a1a1", muted: "#073642", "muted-foreground": "#839496",
      "subtle-foreground": "#586e75", ring: "#268bd2", primary: "#268bd2",
      "primary-hover": "#3a9bde", "primary-foreground": "#fdf6e3", accent: "#073642",
      "accent-foreground": "#eee8d5", destructive: "#dc322f", success: "#859900",
      citation: "#2aa198", selection: "rgba(38,139,210,0.30)", ...darkBorder,
    },
  },
  "tokyo-night": {
    id: "tokyo-night",
    label: "Tokyo Night",
    dark: true,
    vars: {
      background: "#1a1b26", surface: "#1f2335", "surface-2": "#24283b", elevated: "#2a2e42",
      foreground: "#c0caf5", muted: "#24283b", "muted-foreground": "#9aa5ce",
      "subtle-foreground": "#565f89", ring: "#7aa2f7", primary: "#7aa2f7",
      "primary-hover": "#8fb0f8", "primary-foreground": "#1a1b26", accent: "#24283b",
      "accent-foreground": "#c0caf5", destructive: "#f7768e", success: "#9ece6a",
      citation: "#7dcfff", selection: "rgba(122,162,247,0.32)", ...darkBorder,
    },
  },
  claude: {
    id: "claude",
    label: "Claude",
    dark: true,
    vars: {
      background: "#1f1e1b", surface: "#26241f", "surface-2": "#302d27", elevated: "#37332c",
      foreground: "#f0eee6", muted: "#302d27", "muted-foreground": "#b0a99b",
      "subtle-foreground": "#857d6e", ring: "#d97757", primary: "#d97757",
      "primary-hover": "#e08967", "primary-foreground": "#1f1e1b", accent: "#302d27",
      "accent-foreground": "#f0eee6", destructive: "#d9534f", success: "#7faa6e",
      citation: "#cc8a63", selection: "rgba(217,119,87,0.30)", ...darkBorder,
    },
  },
  openai: {
    id: "openai",
    label: "OpenAI",
    dark: true,
    vars: {
      background: "#0d0d0d", surface: "#141414", "surface-2": "#1d1d1d", elevated: "#242424",
      foreground: "#ececec", muted: "#1d1d1d", "muted-foreground": "#9b9b9b",
      "subtle-foreground": "#6e6e6e", ring: "#10a37f", primary: "#10a37f",
      "primary-hover": "#1bb78f", "primary-foreground": "#ffffff", accent: "#1d1d1d",
      "accent-foreground": "#ececec", destructive: "#ef4146", success: "#19c37d",
      citation: "#19c37d", selection: "rgba(16,163,127,0.28)", ...darkBorder,
    },
  },
  latte: {
    id: "latte",
    label: "Catppuccin Latte",
    dark: false,
    vars: {
      background: "#eff1f5", surface: "#e6e9ef", "surface-2": "#dce0e8", elevated: "#ffffff",
      foreground: "#4c4f69", muted: "#e6e9ef", "muted-foreground": "#6c6f85",
      "subtle-foreground": "#9ca0b0", ring: "#8839ef", primary: "#8839ef",
      "primary-hover": "#7a2fe0", "primary-foreground": "#ffffff", accent: "#dce0e8",
      "accent-foreground": "#4c4f69", destructive: "#d20f39", success: "#40a02b",
      citation: "#1e66f5", selection: "rgba(136,57,239,0.18)", ...lightBorder,
    },
  },
  sepia: {
    id: "sepia",
    label: "Sepia",
    dark: false,
    vars: {
      background: "#f4ecd8", surface: "#efe5cf", "surface-2": "#e7dbc0", elevated: "#f6efde",
      foreground: "#4a3b2a", muted: "#e7dbc0", "muted-foreground": "#7a6a52",
      "subtle-foreground": "#9c8c6f", ring: "#a0522d", primary: "#a0522d",
      "primary-hover": "#8f4624", "primary-foreground": "#f8f1e0", accent: "#e7dbc0",
      "accent-foreground": "#4a3b2a", destructive: "#b03a2e", success: "#6b8e23",
      citation: "#9a6a3c", selection: "rgba(160,82,45,0.22)", ...lightBorder,
    },
  },
};

export const THEME_LIST = Object.values(THEMES).sort((a, b) => a.label.localeCompare(b.label));

/** Pseudo-theme id: follow the OS appearance (Midnight when dark, Light when
 *  light), re-resolving live when the system setting changes. */
export const SYSTEM_THEME = "system";

let osListener: (() => void) | null = null;

export function applyTheme(name: string) {
  const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
  // (Un)subscribe the OS-appearance listener as we enter/leave system mode.
  if (osListener && mq) {
    mq.removeEventListener("change", osListener);
    osListener = null;
  }
  let resolved = name;
  if (name === SYSTEM_THEME) {
    resolved = mq && !mq.matches ? "light" : DEFAULT_THEME;
    if (mq) {
      osListener = () => applyTheme(SYSTEM_THEME);
      mq.addEventListener("change", osListener);
    }
  }
  const theme = THEMES[resolved] ?? THEMES[DEFAULT_THEME];
  const root = document.documentElement;
  for (const [key, value] of Object.entries(theme.vars)) {
    root.style.setProperty(`--${key}`, value);
  }
  root.dataset.theme = theme.id;
  root.style.colorScheme = theme.dark ? "dark" : "light";
}
