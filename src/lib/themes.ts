// Color schemes. Each theme is a full set of design tokens applied to
// :root as CSS custom properties; Tailwind reads them via @theme inline.

/** DitherBackground luminance-field variant. All variants share the Bayer
 *  dither + bg/primary tint + transmutation ring; only the field differs. */
export type ShaderVariant =
  | "mist"
  | "rain"
  | "horizon"
  | "grain"
  | "dial"
  | "slipstream"
  | "trellis"
  | "bars"
  | "network"
  | "snow"
  | "moon"
  | "glitch"
  | "orbit"
  | "contrib"
  | "corona"
  | "steam"
  | "phosphor";

export interface Theme {
  id: string;
  label: string;
  dark: boolean;
  /** Shader field matching the theme's design inspiration. Default: "mist". */
  shader?: ShaderVariant;
  /** Preferred transmutation-circle index in AlchemySymbol; random if unset. */
  sigil?: number;
  /** Mood phrase steering the generated epigraph. Default: alchemical. */
  mood?: string;
  /** Thinking-spinner verbs. Default: DEFAULT_VERBS. */
  verbs?: string[];
  vars: Record<string, string>;
}

/** Alchemical process verbs — the thinking-spinner default for every theme. */
export const DEFAULT_VERBS = [
  "Distilling",
  "Transmuting",
  "Calcining",
  "Sublimating",
  "Fermenting",
  "Coagulating",
  "Circulating",
  "Macerating",
];

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
      "subtle-foreground": "#7f848d", ring: "#5e6ad2", primary: "#5e6ad2",
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
      foreground: "#1c1d20", muted: "#f3f3f1", "muted-foreground": "#686d76",
      "subtle-foreground": "#656c75", ring: "#5e6ad2", primary: "#5e6ad2",
      "primary-hover": "#4f5bc4", "primary-foreground": "#ffffff", accent: "#eceef1",
      "accent-foreground": "#1c1d20", destructive: "#d42828", success: "#2f9e6b",
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
      "subtle-foreground": "#909caf", ring: "#6366f1", primary: "#5d60f0",
      "primary-hover": "#6063f3", "primary-foreground": "#ffffff", accent: "#1e293b",
      "accent-foreground": "#e2e8f0", destructive: "#f87171", success: "#34d399",
      citation: "#818cf8", selection: "rgba(99,102,241,0.32)", ...darkBorder,
    },
  },
  dracula: {
    id: "dracula",
    label: "Dracula",
    dark: true,
    shader: "moon", // a gibbous moon behind drifting fog
    sigil: 2, // pentagram — the old wards
    mood: "a candlelit Carpathian castle, velvet drapes, something at the window",
    verbs: [
      "Rising from the crypt",
      "Drawing the curtains",
      "Listening to the children of the night",
      "Crossing the Carpathians",
      "Decanting the vintage",
      "Awaiting an invitation",
    ],
    vars: {
      background: "#21222c", surface: "#282a36", "surface-2": "#343746", elevated: "#3a3d4d",
      foreground: "#f8f8f2", muted: "#343746", "muted-foreground": "#a6abc7",
      "subtle-foreground": "#a0aac8", ring: "#bd93f9", primary: "#bd93f9",
      "primary-hover": "#cbaaff", "primary-foreground": "#21222c", accent: "#343746",
      "accent-foreground": "#f8f8f2", destructive: "#ff7a7a", success: "#50fa7b",
      citation: "#8be9fd", selection: "rgba(189,147,249,0.35)", ...darkBorder,
    },
  },
  monokai: {
    id: "monokai",
    label: "Monokai",
    dark: true,
    vars: {
      background: "#1e1f1c", surface: "#272822", "surface-2": "#33342c", elevated: "#3b3c33",
      foreground: "#f8f8f2", muted: "#33342c", "muted-foreground": "#aba893",
      "subtle-foreground": "#aba896", ring: "#a6e22e", primary: "#a6e22e",
      "primary-hover": "#b6ee48", "primary-foreground": "#1e1f1c", accent: "#33342c",
      "accent-foreground": "#f8f8f2", destructive: "#fb689d", success: "#a6e22e",
      citation: "#66d9ef", selection: "rgba(166,226,46,0.25)", ...darkBorder,
    },
  },
  "one-dark": {
    id: "one-dark",
    label: "One Dark",
    dark: true,
    vars: {
      background: "#21252b", surface: "#282c34", "surface-2": "#2c313a", elevated: "#333842",
      foreground: "#abb2bf", muted: "#2c313a", "muted-foreground": "#9fa4ae",
      "subtle-foreground": "#9da3af", ring: "#61afef", primary: "#61afef",
      "primary-hover": "#74baf3", "primary-foreground": "#21252b", accent: "#2c313a",
      "accent-foreground": "#abb2bf", destructive: "#e37b83", success: "#98c379",
      citation: "#56b6c2", selection: "rgba(97,175,239,0.30)", ...darkBorder,
    },
  },
  nord: {
    id: "nord",
    label: "Nord",
    dark: true,
    shader: "snow", // three flake layers, parallax-deep
    sigil: 1, // hexagram — the snowflake's six arms
    mood: "an arctic night, snow falling on hushed fjords, aurora behind the clouds",
    verbs: [
      "Watching the aurora",
      "Breaking trail",
      "Reading the snowdrifts",
      "Waiting out the storm",
      "Stoking the stove",
      "Counting the flakes",
    ],
    vars: {
      background: "#2e3440", surface: "#3b4252", "surface-2": "#434c5e", elevated: "#4c566a",
      foreground: "#eceff4", muted: "#434c5e", "muted-foreground": "#c8cfdd",
      "subtle-foreground": "#cacfd9", ring: "#88c0d0", primary: "#88c0d0",
      "primary-hover": "#96cbd9", "primary-foreground": "#2e3440", accent: "#434c5e",
      "accent-foreground": "#eceff4", destructive: "#e1b4b8", success: "#a3be8c",
      citation: "#aac0d5", selection: "rgba(136,192,208,0.30)", ...darkBorder,
    },
  },
  gruvbox: {
    id: "gruvbox",
    label: "Gruvbox",
    dark: true,
    shader: "phosphor", // an amber CRT at rest
    sigil: 3, // transmutation array — the character grid
    mood: "an amber terminal humming in a dark room, phosphor and patience",
    verbs: [
      "Blinking the cursor",
      "Scrolling the buffer",
      "Compiling in the dark",
      "Reading the man pages",
      "Warming the phosphor",
      "Waiting on the modem",
    ],
    vars: {
      background: "#1d2021", surface: "#282828", "surface-2": "#3c3836", elevated: "#504945",
      foreground: "#ebdbb2", muted: "#3c3836", "muted-foreground": "#c4baac",
      "subtle-foreground": "#c4bcb5", ring: "#fe8019", primary: "#fe8019",
      "primary-hover": "#fe9539", "primary-foreground": "#1d2021", accent: "#3c3836",
      "accent-foreground": "#ebdbb2", destructive: "#fc7f70", success: "#b8bb26",
      citation: "#8eada1", selection: "rgba(254,128,25,0.28)", ...darkBorder,
    },
  },
  github: {
    id: "github",
    label: "GitHub",
    dark: true,
    shader: "contrib", // the contribution wall, density-driven
    sigil: 3, // transmutation array — the graph grid
    mood: "a year of little squares, midnight commits, the graph never sleeps",
    verbs: [
      "Committing",
      "Rebasing gently",
      "Opening the PR",
      "Squashing the history",
      "Greening the graph",
      "Merging at midnight",
    ],
    vars: {
      background: "#0d1117", surface: "#11151c", "surface-2": "#161b22", elevated: "#1c2128",
      foreground: "#c9d1d9", muted: "#161b22", "muted-foreground": "#8b949e",
      "subtle-foreground": "#848b96", ring: "#2f81f7", primary: "#126ff6",
      "primary-hover": "#116ef6", "primary-foreground": "#ffffff", accent: "#161b22",
      "accent-foreground": "#c9d1d9", destructive: "#f85149", success: "#3fb950",
      citation: "#58a6ff", selection: "rgba(47,129,247,0.30)", ...darkBorder,
    },
  },
  carbon: {
    id: "carbon",
    label: "IBM Carbon",
    dark: true,
    shader: "bars",
    sigil: 3, // transmutation array — the punched-card grid
    mood: "Big Blue: the eight-bar rebus, punched cards, THINK signs, mainframe hum",
    verbs: [
      "Thinking",
      "Punching the cards",
      "Spooling the tape",
      "Batching the jobs",
      "Warming the mainframe",
      "Waking Watson",
    ],
    vars: {
      // Carbon v11 Gray 100 tokens, verified against @carbon/themes:
      // layer-01/02 gray-90/80, text-secondary gray-30, helper gray-40,
      // button blue-60 (hover #0050e6), links blue-40, focus white.
      background: "#161616", surface: "#262626", "surface-2": "#393939", elevated: "#474747",
      foreground: "#f4f4f4", muted: "#393939", "muted-foreground": "#c6c6c6",
      "subtle-foreground": "#a8a8a8", ring: "#ffffff", primary: "#0f62fe",
      "primary-hover": "#0050e6", "primary-foreground": "#ffffff", accent: "#393939",
      "accent-foreground": "#f4f4f4", destructive: "#fa4d56", success: "#42be65",
      citation: "#78a9ff", selection: "rgba(69,137,255,0.35)", ...darkBorder,
    },
  },
  "carbon-light": {
    id: "carbon-light",
    label: "IBM Carbon Light",
    dark: false,
    shader: "bars",
    sigil: 3, // transmutation array — the punched-card grid
    mood: "Big Blue: the eight-bar rebus, punched cards, THINK signs, mainframe hum",
    verbs: [
      "Thinking",
      "Punching the cards",
      "Spooling the tape",
      "Batching the jobs",
      "Warming the mainframe",
      "Waking Watson",
    ],
    vars: {
      // Carbon v11 White tokens, verified against @carbon/themes:
      // layer-01 gray-10, accents gray-20, text-secondary gray-70, helper
      // gray-60, button + links blue-60 (hover #0050e6), success green-50.
      background: "#ffffff", surface: "#f4f4f4", "surface-2": "#e0e0e0", elevated: "#ffffff",
      foreground: "#161616", muted: "#e0e0e0", "muted-foreground": "#525252",
      "subtle-foreground": "#6f6f6f", ring: "#0f62fe", primary: "#0f62fe",
      "primary-hover": "#0050e6", "primary-foreground": "#ffffff", accent: "#e0e0e0",
      "accent-foreground": "#161616", destructive: "#da1e28", success: "#24a148",
      citation: "#0f62fe", selection: "rgba(15,98,254,0.20)", ...lightBorder,
    },
  },
  "github-light": {
    id: "github-light",
    label: "GitHub Light",
    dark: false,
    shader: "contrib", // the contribution wall, density-driven
    sigil: 3, // transmutation array — the graph grid
    mood: "a year of little squares, midnight commits, the graph never sleeps",
    verbs: [
      "Committing",
      "Rebasing gently",
      "Opening the PR",
      "Squashing the history",
      "Greening the graph",
      "Merging at midnight",
    ],
    vars: {
      background: "#ffffff", surface: "#f6f8fa", "surface-2": "#eaeef2", elevated: "#ffffff",
      foreground: "#1f2328", muted: "#eaeef2", "muted-foreground": "#57606a",
      "subtle-foreground": "#616a74", ring: "#0969da", primary: "#0969da",
      "primary-hover": "#0860ca", "primary-foreground": "#ffffff", accent: "#eaeef2",
      "accent-foreground": "#1f2328", destructive: "#cf222e", success: "#1a7f37",
      citation: "#0550ae", selection: "rgba(9,105,218,0.20)", ...lightBorder,
    },
  },
  solarized: {
    id: "solarized",
    label: "Solarized",
    dark: true,
    shader: "corona", // the sun itself, prominences breathing
    sigil: 4, // celestial descent — the measured sun
    mood: "low sun over still water, measured light, the lab notebook of color",
    verbs: [
      "Tracking the sun",
      "Balancing the contrast",
      "Reading the analemma",
      "Measuring the light",
      "Waiting for golden hour",
      "Charting the ecliptic",
    ],
    vars: {
      background: "#002b36", surface: "#073642", "surface-2": "#0a4351", elevated: "#0e4b5a",
      foreground: "#93a1a1", muted: "#073642", "muted-foreground": "#abb7b7",
      "subtle-foreground": "#a8b8be", ring: "#268bd2", primary: "#2076b3",
      "primary-hover": "#1e75b1", "primary-foreground": "#fdf6e3", accent: "#073642",
      "accent-foreground": "#eee8d5", destructive: "#ec8f8d", success: "#859900",
      citation: "#32beb3", selection: "rgba(38,139,210,0.30)", ...darkBorder,
    },
  },
  "solarized-light": {
    id: "solarized-light",
    label: "Solarized Light",
    dark: false,
    shader: "corona", // the sun itself, prominences breathing
    sigil: 4, // celestial descent — the measured sun
    mood: "low sun over still water, measured light, the lab notebook of color",
    verbs: [
      "Tracking the sun",
      "Balancing the contrast",
      "Reading the analemma",
      "Measuring the light",
      "Waiting for golden hour",
      "Charting the ecliptic",
    ],
    vars: {
      background: "#fdf6e3", surface: "#f5eeda", "surface-2": "#eee8d5", elevated: "#fffbf0",
      foreground: "#586e75", muted: "#eee8d5", "muted-foreground": "#55686e",
      "subtle-foreground": "#5c6a6a", ring: "#268bd2", primary: "#2076b3",
      "primary-hover": "#1c74b0", "primary-foreground": "#fdf6e3", accent: "#eee8d5",
      "accent-foreground": "#073642", destructive: "#ca2522", success: "#859900",
      citation: "#1d706a", selection: "rgba(38,139,210,0.22)", ...lightBorder,
    },
  },
  "tokyo-night": {
    id: "tokyo-night",
    label: "Tokyo Night",
    dark: true,
    shader: "network", // city lights as a drifting plexus
    sigil: 3, // transmutation array — the node grid
    mood: "a neon-lit Tokyo night, rain-slick streets, city lights networked to the horizon",
    verbs: [
      "Crossing at Shibuya",
      "Riding the Yamanote",
      "Reading the neon",
      "Waiting for the last train",
      "Tracing the network",
      "Chasing the vending-machine glow",
    ],
    vars: {
      background: "#1a1b26", surface: "#1f2335", "surface-2": "#24283b", elevated: "#2a2e42",
      foreground: "#c0caf5", muted: "#24283b", "muted-foreground": "#9aa5ce",
      "subtle-foreground": "#8f96b8", ring: "#7aa2f7", primary: "#7aa2f7",
      "primary-hover": "#8fb0f8", "primary-foreground": "#1a1b26", accent: "#24283b",
      "accent-foreground": "#c0caf5", destructive: "#f7768e", success: "#9ece6a",
      citation: "#7dcfff", selection: "rgba(122,162,247,0.32)", ...darkBorder,
    },
  },
  matrix: {
    id: "matrix",
    label: "Matrix",
    dark: true,
    shader: "rain",
    sigil: 3, // transmutation array — the grid
    mood: "a green-phosphor hacker terminal, digital rain, the desert of the real",
    verbs: [
      "Tracing the signal",
      "Decoding the stream",
      "Following the white rabbit",
      "Reading the code rain",
      "Bending the spoon",
      "Searching the construct",
    ],
    vars: {
      background: "#000a00", surface: "#001200", "surface-2": "#001a00", elevated: "#002200",
      foreground: "#00ff41", muted: "#001a00", "muted-foreground": "#22a344",
      "subtle-foreground": "#1d9943", ring: "#00ff41", primary: "#00ff41",
      "primary-hover": "#33ff67", "primary-foreground": "#000a00", accent: "#002000",
      "accent-foreground": "#00e038", destructive: "#ff003c", success: "#00ff41",
      citation: "#7dffab", selection: "rgba(0,255,65,0.25)",
      border: "rgba(0,255,65,0.12)", "border-strong": "rgba(0,255,65,0.20)",
      input: "rgba(0,255,65,0.15)", scrollbar: "rgba(0,255,65,0.16)",
    },
  },
  synthwave: {
    id: "synthwave",
    label: "Synthwave '84",
    dark: true,
    shader: "horizon",
    sigil: 4, // celestial descent — the setting sun
    mood: "neon retrowave dusk, chrome sunsets, an endless perspective grid",
    verbs: [
      "Riding the grid",
      "Chasing the horizon",
      "Rewinding the tape",
      "Tuning the synth",
      "Cruising the neon",
      "Waiting for the drop",
    ],
    vars: {
      background: "#1e1a29", surface: "#262335", "surface-2": "#2a2139", elevated: "#34294f",
      foreground: "#f4f2ff", muted: "#2a2139", "muted-foreground": "#9096c3",
      "subtle-foreground": "#9a98b7", ring: "#ff7edb", primary: "#ff7edb",
      "primary-hover": "#ff92df", "primary-foreground": "#262335", accent: "#2a2139",
      "accent-foreground": "#f4f2ff", destructive: "#fe4a56", success: "#72f1b8",
      citation: "#36f9f6", selection: "rgba(255,126,219,0.30)",
      border: "rgba(176,132,235,0.16)", "border-strong": "rgba(176,132,235,0.26)",
      input: "rgba(176,132,235,0.20)", scrollbar: "rgba(176,132,235,0.22)",
    },
  },
  claude: {
    id: "claude",
    label: "Claude",
    dark: true,
    vars: {
      background: "#1f1e1b", surface: "#26241f", "surface-2": "#302d27", elevated: "#37332c",
      foreground: "#f0eee6", muted: "#302d27", "muted-foreground": "#b0a99b",
      "subtle-foreground": "#a49e92", ring: "#d97757", primary: "#d97757",
      "primary-hover": "#e08967", "primary-foreground": "#1f1e1b", accent: "#302d27",
      "accent-foreground": "#f0eee6", destructive: "#e17572", success: "#7faa6e",
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
      "subtle-foreground": "#8c8c8c", ring: "#10a37f", primary: "#0d8265",
      "primary-hover": "#138265", "primary-foreground": "#ffffff", accent: "#1d1d1d",
      "accent-foreground": "#ececec", destructive: "#ef474b", success: "#19c37d",
      citation: "#19c37d", selection: "rgba(16,163,127,0.28)", ...darkBorder,
    },
  },
  latte: {
    id: "latte",
    label: "Catppuccin Latte",
    dark: false,
    shader: "steam", // two wisps off a fresh cup
    sigil: 0, // squared circle — the cup on its saucer
    mood: "morning light in a quiet café, crema and steam, an unhurried first sip",
    verbs: [
      "Pulling the shot",
      "Steaming the milk",
      "Pouring the rosetta",
      "Warming the cup",
      "Grinding the beans",
      "Savoring the crema",
    ],
    vars: {
      background: "#eff1f5", surface: "#e6e9ef", "surface-2": "#dce0e8", elevated: "#ffffff",
      foreground: "#4c4f69", muted: "#e6e9ef", "muted-foreground": "#5e6174",
      "subtle-foreground": "#5a5f71", ring: "#8839ef", primary: "#8839ef",
      "primary-hover": "#7a2fe0", "primary-foreground": "#ffffff", accent: "#dce0e8",
      "accent-foreground": "#4c4f69", destructive: "#c10e34", success: "#40a02b",
      citation: "#0a53e4", selection: "rgba(136,57,239,0.18)", ...lightBorder,
    },
  },
  "rose-pine-dawn": {
    id: "rose-pine-dawn",
    label: "Rosé Pine Dawn",
    dark: false,
    vars: {
      background: "#faf4ed", surface: "#fffaf3", "surface-2": "#f2e9e1", elevated: "#fffaf3",
      foreground: "#575279", muted: "#f2e9e1", "muted-foreground": "#67647f",
      "subtle-foreground": "#6a6478", ring: "#d7827e", primary: "#c44741",
      "primary-hover": "#bc524d", "primary-foreground": "#fffaf3", accent: "#f2e9e1",
      "accent-foreground": "#575279", destructive: "#a44f67", success: "#286983",
      citation: "#745d8f", selection: "rgba(215,130,126,0.25)", ...lightBorder,
    },
  },
  carrera: {
    id: "carrera",
    label: "Carrera",
    dark: false,
    shader: "dial", // the centre gauge of the five-dial cluster
    sigil: 3, // transmutation array — the four-spoke wheel
    mood: "a Guards Red 993 over Cashmere Beige hide, a brass crest, air-cooled and analog",
    verbs: [
      "Warming the flat-six",
      "Blipping the throttle",
      "Trimming the apex",
      "Reading the tarmac",
      "Heeling and toeing",
      "Settling the rear",
    ],
    vars: {
      // A "medium mode": true Cashmere Beige hide is a warm mid-tan, not
      // cream — darker canvas than the other light schemes, dark text.
      // Contrast-audited (WCAG AA on background/surface; muted holds 4.5+
      // even on surface-2): muted 5.4–4.7:1, subtle 5.0–4.5:1,
      // citation 5.3–4.8:1, success 5.3:1, destructive 5.2:1.
      background: "#c9b493", surface: "#c2ab88", "surface-2": "#b79e79", elevated: "#d6c4a6",
      foreground: "#221b12", muted: "#b79e79", "muted-foreground": "#40351f",
      "subtle-foreground": "#4d4129", ring: "#b30018", primary: "#b30018",
      "primary-hover": "#9a0015", "primary-foreground": "#ffffff", accent: "#b79e79",
      "accent-foreground": "#221b12", destructive: "#7d1810", success: "#2f440e",
      citation: "#4d3c0b", selection: "rgba(179,0,24,0.22)",
      border: "rgba(46,33,16,0.14)", "border-strong": "rgba(46,33,16,0.24)",
      input: "rgba(46,33,16,0.18)", scrollbar: "rgba(46,33,16,0.26)",
    },
  },
  italia: {
    id: "italia",
    label: "Italia",
    dark: true,
    shader: "slipstream", // the world smearing past at speed
    sigil: 2, // pentagram — the stellone, the star of Italy
    mood: "rosso corsa over carbon, a V12 at full song, heat haze off Maranello asphalt",
    verbs: [
      "Winding out the V12",
      "Sighting the apex",
      "Braking impossibly late",
      "Riding the redline",
      "Slotting the gate",
      "Chasing the tifosi",
    ],
    vars: {
      background: "#0b0708", surface: "#130c0d", "surface-2": "#1b1113", elevated: "#241719",
      foreground: "#f6f1ef", muted: "#1b1113", "muted-foreground": "#ab9d9c",
      "subtle-foreground": "#a29493", ring: "#f5d000", primary: "#ff2800",
      "primary-hover": "#ff4a2b", "primary-foreground": "#0b0708", accent: "#1b1113",
      "accent-foreground": "#f6f1ef", destructive: "#ff4d6d", success: "#17b866",
      citation: "#f5d000", selection: "rgba(255,40,0,0.30)",
      border: "rgba(255,86,64,0.11)", "border-strong": "rgba(255,86,64,0.20)",
      input: "rgba(255,86,64,0.15)", scrollbar: "rgba(255,86,64,0.18)",
    },
  },
  panigale: {
    id: "panigale",
    label: "Panigale",
    dark: true,
    shader: "trellis", // the triangulated frame under the tank
    sigil: 1, // hexagram — interlocked triangles, the lattice
    mood: "Ducati red over anthracite, a trellis frame and desmodromic valves at full chat",
    verbs: [
      "Winding the desmo",
      "Trail-braking in",
      "Hanging off",
      "Scraping a knee",
      "Snicking the quickshifter",
      "Holding the front wheel down",
    ],
    vars: {
      // Ducati's palette is red, white and black — so the accent is the white
      // of a race number board, not a hue. Body text sits a step down in
      // luminance to leave it room: citation reads brighter than prose.
      background: "#1a1c1e", surface: "#202325", "surface-2": "#282c2f", elevated: "#31363a",
      foreground: "#c6ced4", muted: "#282c2f", "muted-foreground": "#a3abb1",
      "subtle-foreground": "#99a1a8", ring: "#da291c", primary: "#da291c",
      "primary-hover": "#ea3a2c", "primary-foreground": "#ffffff", accent: "#282c2f",
      "accent-foreground": "#c6ced4", destructive: "#ff7d8a", success: "#3fb56b",
      citation: "#eef4f8", selection: "rgba(218,41,28,0.30)", ...darkBorder,
    },
  },
  "night-city": {
    id: "night-city",
    label: "Night City",
    dark: true,
    shader: "glitch", // the feed tears and re-locks
    sigil: 3, // transmutation array — the netrunner grid
    mood: "Night City at 3 a.m., chrome and brand yellow, a ghost in the net",
    verbs: [
      "Jacking in",
      "Breaching the ICE",
      "Scanning the net",
      "Flatlining the daemon",
      "Burning eddies",
      "Waking the samurai",
    ],
    vars: {
      // CP2077 brand: warning-label yellow on soot black, netrunner cyan
      // for citations. Yellow carries the UI; cyan only ever means a link.
      background: "#0b0b09", surface: "#12120e", "surface-2": "#1a1a13", elevated: "#211f16",
      foreground: "#f1efdc", muted: "#1a1a13", "muted-foreground": "#a6a486",
      "subtle-foreground": "#98977e", ring: "#fcee0a", primary: "#f3e600",
      "primary-hover": "#fff23d", "primary-foreground": "#0b0b09", accent: "#1a1a13",
      "accent-foreground": "#f1efdc", destructive: "#ff2e55", success: "#3ce6a3",
      citation: "#37ebf3", selection: "rgba(243,230,0,0.28)",
      border: "rgba(243,230,0,0.11)", "border-strong": "rgba(243,230,0,0.19)",
      input: "rgba(243,230,0,0.14)", scrollbar: "rgba(243,230,0,0.15)",
    },
  },
  durandal: {
    id: "durandal",
    label: "Durandal",
    dark: true,
    shader: "orbit", // satellites riding concentric paths
    sigil: 0, // squared circle — the orbital seal
    mood: "acid decals on hull grey, orbital relays, a rogue AI whispering in the static",
    verbs: [
      "Entering the stream",
      "Pinging the relay",
      "Tracing the orbit",
      "Compiling the runner",
      "Listening for Durandal",
      "Respawning",
    ],
    vars: {
      // Marathon's graphic-design brutalism: acid green over cool hull
      // greys, off-white text, everything vector-flat.
      background: "#0b0d0e", surface: "#111417", "surface-2": "#181c20", elevated: "#1f2429",
      foreground: "#e8edea", muted: "#181c20", "muted-foreground": "#93a09a",
      "subtle-foreground": "#89948e", ring: "#c8f542", primary: "#b6e32f",
      "primary-hover": "#c8f542", "primary-foreground": "#0b0d0e", accent: "#181c20",
      "accent-foreground": "#e8edea", destructive: "#ff5265", success: "#54d98c",
      citation: "#cdf76a", selection: "rgba(200,245,66,0.25)",
      border: "rgba(200,245,66,0.10)", "border-strong": "rgba(200,245,66,0.18)",
      input: "rgba(200,245,66,0.13)", scrollbar: "rgba(200,245,66,0.14)",
    },
  },
  sepia: {
    id: "sepia",
    label: "Sepia",
    dark: false,
    shader: "grain",
    sigil: 0, // squared circle — the philosopher's stone
    mood: "an aged manuscript, candlelit study, ink on vellum",
    verbs: [
      "Consulting the folios",
      "Deciphering marginalia",
      "Dipping the quill",
      "Turning brittle pages",
      "Sifting the archives",
      "Blotting the ink",
    ],
    vars: {
      background: "#f4ecd8", surface: "#efe5cf", "surface-2": "#e7dbc0", elevated: "#f6efde",
      foreground: "#4a3b2a", muted: "#e7dbc0", "muted-foreground": "#6b5d48",
      "subtle-foreground": "#685c47", ring: "#a0522d", primary: "#a0522d",
      "primary-hover": "#8f4624", "primary-foreground": "#f8f1e0", accent: "#e7dbc0",
      "accent-foreground": "#4a3b2a", destructive: "#ab382d", success: "#6b8e23",
      citation: "#7b5530", selection: "rgba(160,82,45,0.22)", ...lightBorder,
    },
  },
};

export const THEME_LIST = Object.values(THEMES).sort((a, b) => a.label.localeCompare(b.label));

/** Pseudo-theme id: follow the OS appearance (Midnight when dark, Light when
 *  light), re-resolving live when the system setting changes. */
export const SYSTEM_THEME = "system";

let osListener: (() => void) | null = null;

/** Resolve a stored theme name (possibly "system" or unknown) to a THEMES id. */
export function resolveThemeId(name?: string): string {
  if (!name || name === SYSTEM_THEME) {
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    return mq && !mq.matches ? "light" : DEFAULT_THEME;
  }
  return THEMES[name] ? name : DEFAULT_THEME;
}

/** Whether a stored theme name resolves to a dark palette right now. */
export function themeIsDark(name?: string): boolean {
  return THEMES[resolveThemeId(name)].dark;
}

export function applyTheme(name: string) {
  const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
  // (Un)subscribe the OS-appearance listener as we enter/leave system mode.
  if (osListener && mq) {
    mq.removeEventListener("change", osListener);
    osListener = null;
  }
  if (name === SYSTEM_THEME && mq) {
    // Re-check the persisted theme at fire time: the glass appearance pin
    // flips prefers-color-scheme, and (in dev) HMR can leak stale copies
    // of this listener — both must no-op unless System is still active.
    osListener = () => {
      if ((localStorage.getItem("theme") ?? SYSTEM_THEME) === SYSTEM_THEME)
        applyTheme(SYSTEM_THEME);
    };
    mq.addEventListener("change", osListener);
  }
  const theme = THEMES[resolveThemeId(name)];
  const root = document.documentElement;
  for (const [key, value] of Object.entries(theme.vars)) {
    root.style.setProperty(`--${key}`, value);
  }
  // Semantic warning amber. Themes may override with a `warning` var; the
  // default darkens in light schemes so amber text keeps contrast. Set
  // unconditionally — applyTheme only writes the vars a theme declares, so
  // a per-theme value would otherwise leak across switches.
  if (!theme.vars.warning)
    root.style.setProperty("--warning", theme.dark ? "#e8a33d" : "#9a6700");
  root.dataset.theme = theme.id;
  root.dataset.scheme = theme.dark ? "dark" : "light";
  root.style.colorScheme = theme.dark ? "dark" : "light";
}
