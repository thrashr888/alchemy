import { useEffect, useState, type ReactNode } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { checkForUpdates } from "@/lib/updates";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { SYSTEM_THEME, THEME_LIST, THEMES, resolveThemeId } from "@/lib/themes";
import { SLASH_COMMANDS } from "@/lib/slashCommands";
import type {
  BuildInfo,
  ChatConfig,
  ReadingPrefs,
  ReleaseNote,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import { AlchemySymbol } from "../AlchemyHero";
import { Markdown } from "../Markdown";
import {
  Button,
  EmptyState,
  FormGroup,
  FormRow,
  Input,
  Segmented,
  Spinner,
  Textarea,
  type SegmentedOption,
} from "../ui";
import {
  AlignLeft,
  Braces,
  Briefcase,
  Feather,
  FlaskConical,
  Globe,
  GraduationCap,
  Landmark,
  MessageCircle,
  PenLine,
  Scissors,
  ScrollText,
  Sparkles,
  Blocks,
  Smile,
  Wrench,
  Zap,
  type LucideIcon,
} from "lucide-react";

// These presets mirror rag::CHAT_STYLES in the backend. The specialist
// choices compress real writing standards (ASD-STE100, GOV.UK, US Federal
// Plain Language, and the i-have-adhd rules) to prompt size.
export const CHAT_STYLES = [
  { id: "default", label: "Default", icon: Sparkles, hint: "Balanced answers, cited to your sources." },
  { id: "friendly", label: "Friendly", icon: MessageCircle, hint: "Warm and direct. No cheerleading, no filler." },
  { id: "bffs", label: "BFFs", icon: Smile, hint: "{assistant}, your best friend who did the reading. Matches your register, still cited." },
  { id: "kids", label: "Kid-friendly", icon: Blocks, hint: "Simple words, patient, one idea at a time. Nothing scary." },
  { id: "professional", label: "Professional", icon: Briefcase, hint: "The takeaway first; evidence and caveats after, in workplace prose." },
  { id: "learning", label: "Learning Guide", icon: GraduationCap, hint: "Step-by-step explanations that define terms and build intuition." },
  { id: "scientific", label: "Scientific", icon: FlaskConical, hint: "Hedged to the evidence. Quantified, summary first." },
  { id: "adhd", label: "ADHD-friendly", icon: Zap, hint: "Answer first. Numbered steps, short lists, no preamble." },
  { id: "ste100", label: "Simplified Technical", icon: Wrench, hint: "Simplified Technical English (ASD-STE100): short sentences, one instruction each." },
  { id: "govuk", label: "GOV.UK", icon: Landmark, hint: "GOV.UK style. Everyday words, no metaphors, the point up front." },
  { id: "plain", label: "Plain Language", icon: Feather, hint: "US Federal plain-language rules: main point first, active voice." },
  { id: "gdev", label: "Google Developer", icon: Braces, hint: "Google's developer-docs voice: second person, present tense, no marketing." },
  { id: "custom", label: "Custom", icon: PenLine, hint: "Give your own goal, style, or role." },
] as const;

export const CHAT_LENGTHS = [
  { id: "shorter", label: "Concise", icon: Scissors, hint: "Direct answer in up to three short paragraphs or five bullets." },
  { id: "default", label: "Balanced", icon: AlignLeft, hint: "Matches the level of detail to the question." },
  { id: "longer", label: "Thorough", icon: ScrollText, hint: "Conclusion first, then evidence, reasoning, and examples." },
] as const;

// Display prefs, shared by every notebook. They live in Appearance → Text
// (they are display, and the model never sees them), not in a notebook's
// Chat tab.
const CHAT_FONTS: readonly SegmentedOption<ReadingPrefs["font"]>[] = [
  { value: "sans", label: "Sans" },
  { value: "serif", label: "Serif" },
  { value: "mono", label: "Mono" },
  { value: "system", label: "System" },
];

const CHAT_SIZES: readonly SegmentedOption<ReadingPrefs["fontSize"]>[] = [
  { value: "small", label: "Small" },
  { value: "medium", label: "Medium" },
  { value: "large", label: "Large" },
];

const CHAT_ALIGNS: readonly SegmentedOption<ReadingPrefs["textAlign"]>[] = [
  { value: "natural", label: "Natural" },
  { value: "justified", label: "Justified" },
];

const ACCENT_OPTIONS: readonly SegmentedOption<ReadingPrefs["accent"]>[] = [
  { value: "theme", label: "Theme" },
  { value: "system", label: "System accent" },
];

const GLASS_OPTIONS: readonly SegmentedOption<"off" | "tinted" | "clear">[] = [
  { value: "off", label: "Off" },
  { value: "tinted", label: "Tinted" },
  { value: "clear", label: "Clear" },
];

/** Mirrors ai::DEFAULT_ASSISTANT_NAME — the name a fresh install answers
 *  to. An empty field turns the persona off. */
const DEFAULT_ASSISTANT_NAME = "Alphonse";

/** A style's hint with the assistant's name filled in — BFFs is a friend,
 *  and a friend has a name. With the persona off, the hint reads without
 *  one. */
export function styleHint(styleId: string, assistantName: string | undefined): string | undefined {
  const raw = CHAT_STYLES.find((style) => style.id === styleId)?.hint;
  if (!raw) return raw;
  const name = (assistantName ?? DEFAULT_ASSISTANT_NAME).trim();
  return name
    ? raw.replace("{assistant}", name)
    : raw.replace("{assistant}, your", "Your");
}

export function ChatTab() {
  const assistantName = useStore((state) => state.aiConfig?.profile?.assistantName);
  const chatConfig = useStore((state) => state.chatConfig);
  const setChatConfig = useStore((state) => state.setChatConfig);
  const currentId = useStore((state) => state.currentId);
  const notebook = useStore((state) =>
    state.notebooks.find((candidate) => candidate.id === state.currentId),
  );
  const apply = (patch: Partial<ChatConfig>) =>
    setChatConfig({ ...chatConfig, ...patch });
  const hint = styleHint(chatConfig.style, assistantName);
  const lengthHint = CHAT_LENGTHS.find((length) => length.id === chatConfig.length)?.hint;

  return (
    <div className="flex flex-col gap-5">
      <p className="text-pretty text-body leading-relaxed text-muted-foreground">
        {currentId ? (
          <>
            Tune how the assistant responds in{" "}
            <span className="font-medium text-foreground">
              {notebook?.title ?? "this notebook"}
            </span>
            . Changes apply immediately.
          </>
        ) : (
          "Open a notebook to tune its chat — each notebook keeps its own style."
        )}
      </p>

      {/* The tile grids span the group's width: the glyph and its label ARE
          the option, so they keep their own shape (DESIGN.md §4 ledger). */}
      <FormGroup caption="Conversational goal, style, or role" footer={hint}>
        <div className="flex flex-col gap-3 px-3 py-2.5">
          <div className="grid grid-cols-3 gap-x-2 gap-y-3 sm:grid-cols-4 lg:grid-cols-5">
            {CHAT_STYLES.map((style) => (
              <OptionTile
                key={style.id}
                icon={style.icon}
                label={style.label}
                active={chatConfig.style === style.id}
                onClick={() => apply({ style: style.id })}
              />
            ))}
          </div>
          {chatConfig.style === "custom" && (
            <Textarea
              rows={4}
              aria-label="Custom conversational style"
              placeholder="Act as a skeptical peer reviewer; challenge claims and ask for evidence…"
              value={chatConfig.customPrompt}
              onChange={(event) => apply({ customPrompt: event.target.value })}
            />
          )}
        </div>
      </FormGroup>

      <FormGroup caption="Response length" footer={lengthHint}>
        <div className="px-3 py-2.5">
          <div className="grid grid-cols-3 gap-x-2 gap-y-3 sm:grid-cols-4 lg:grid-cols-5">
            {CHAT_LENGTHS.map((length) => (
              <OptionTile
                key={length.id}
                icon={length.icon}
                label={length.label}
                active={chatConfig.length === length.id}
                onClick={() => apply({ length: length.id })}
              />
            ))}
          </div>
        </div>
      </FormGroup>
    </div>
  );
}

export function PersonalizationTab() {
  const aiConfig = useStore((state) => state.aiConfig);
  const save = useStore((state) => state.saveAiConfig);
  const [draft, setDraft] = useState({
    name: "",
    profession: "",
    instructions: "",
    assistantName: DEFAULT_ASSISTANT_NAME,
  });

  useEffect(() => {
    if (aiConfig?.profile)
      setDraft({
        ...aiConfig.profile,
        assistantName: aiConfig.profile.assistantName ?? DEFAULT_ASSISTANT_NAME,
      });
    // Load once so a blur-save round trip cannot clobber in-progress typing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const saveOnBlur = () => {
    if (!aiConfig) return;
    const profile = aiConfig.profile ?? {
      name: "",
      profession: "",
      instructions: "",
      assistantName: DEFAULT_ASSISTANT_NAME,
    };
    if (
      draft.name !== profile.name ||
      draft.profession !== profile.profession ||
      draft.instructions !== profile.instructions ||
      draft.assistantName !== (profile.assistantName ?? DEFAULT_ASSISTANT_NAME)
    ) {
      void save({ ...aiConfig, profile: { ...draft } });
    }
  };

  return (
    <div className="flex flex-col gap-5">
      <p className="text-pretty text-body leading-relaxed text-muted-foreground">
        Personalization is added to chat and document prompts and is sent only to your configured model. Changes save automatically.
      </p>
      <FormGroup caption="You">
        <FormRow label="What should the assistant call you?">
          <Input
            name="profile-name"
            autoComplete="name"
            aria-label="What should the assistant call you?"
            placeholder="Paul…"
            className="w-44"
            value={draft.name}
            onChange={(event) => setDraft({ ...draft, name: event.target.value })}
            onBlur={saveOnBlur}
          />
        </FormRow>
        <FormRow label="What do you call the assistant?">
          <Input
            name="profile-assistant-name"
            aria-label="What do you call the assistant?"
            placeholder="Alphonse…"
            className="w-44"
            value={draft.assistantName}
            onChange={(event) => setDraft({ ...draft, assistantName: event.target.value })}
            onBlur={saveOnBlur}
          />
        </FormRow>
        <FormRow label="What best describes your work?">
          <Input
            name="profile-profession"
            autoComplete="organization-title"
            aria-label="What best describes your work?"
            placeholder="Product management…"
            className="w-44"
            value={draft.profession}
            onChange={(event) => setDraft({ ...draft, profession: event.target.value })}
            onBlur={saveOnBlur}
          />
        </FormRow>
      </FormGroup>
      <FormGroup caption="Instructions for the assistant">
        <div className="px-3 py-2.5">
          <Textarea
            rows={8}
            name="profile-instructions"
            aria-label="Instructions for the assistant"
            placeholder="Preferences to keep in mind across all notebooks…"
            value={draft.instructions}
            onChange={(event) => setDraft({ ...draft, instructions: event.target.value })}
            onBlur={saveOnBlur}
          />
        </div>
      </FormGroup>
    </div>
  );
}

export function AppearanceTab() {
  const reading = useStore((state) => state.reading);
  const setReading = useStore((state) => state.setReading);
  const theme = useStore((state) => state.theme);
  // Named from the same theme table the swatches come from, so System reads
  // as "System" rather than as whatever it resolves to this minute.
  const themeName =
    theme === SYSTEM_THEME ? "System" : THEMES[resolveThemeId(theme)].label;
  return (
    <div className="flex flex-col gap-5">
      <FormGroup
        caption="Theme"
        footer="37 themes, each with its own backdrop. System accent follows the color you chose in macOS settings."
      >
        <FormRow label="Theme">
          <span className="text-caption text-muted-foreground">{themeName}</span>
        </FormRow>
        {/* The swatch grid spans the row rather than sitting in the trailing
            slot: the strip IS the label (DESIGN.md §4 ledger). */}
        <div className="px-3 py-2.5">
          <ThemePicker />
        </div>
        <FormRow label="Selection color">
          <Segmented
            label="Selection color"
            options={ACCENT_OPTIONS}
            value={reading.accent}
            onChange={(accent) => setReading({ accent })}
          />
        </FormRow>
      </FormGroup>

      <FormGroup
        caption="Glass"
        footer="Tinted keeps the theme's colors over the desktop. Clear is the plain macOS material. Off is opaque."
      >
        <FormRow
          label="Window material"
          hint="Experimental: the desktop blurs through the chrome like native macOS apps."
        >
          <Segmented
            label="Window material"
            options={GLASS_OPTIONS}
            value={reading.glass ? reading.glassStyle : "off"}
            onChange={(style) =>
              style === "off"
                ? setReading({ glass: false })
                : setReading({ glass: true, glassStyle: style })
            }
          />
        </FormRow>
      </FormGroup>

      <FormGroup
        caption="Text"
        footer="Display only; this does not change the model. Every notebook shares these."
      >
        <FormRow label="Chat font">
          <Segmented
            label="Chat font"
            options={CHAT_FONTS}
            value={reading.font}
            onChange={(font) => setReading({ font })}
          />
        </FormRow>
        <FormRow label="Text size">
          <Segmented
            label="Text size"
            options={CHAT_SIZES}
            value={reading.fontSize}
            onChange={(fontSize) => setReading({ fontSize })}
          />
        </FormRow>
        <FormRow label="Alignment">
          <Segmented
            label="Alignment"
            options={CHAT_ALIGNS}
            value={reading.textAlign}
            onChange={(textAlign) => setReading({ textAlign })}
          />
        </FormRow>
      </FormGroup>
    </div>
  );
}

export function ShortcutsTab() {
  // The rows come from the menu's command registry (menu.rs::CMD) — one
  // source of truth for the native menu and this tab, so a shortcut can no
  // longer be registered in one and missing from the other.
  const [shortcuts, setShortcuts] = useState<ShortcutRow[] | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [loadAttempt, setLoadAttempt] = useState(0);

  useEffect(() => {
    setShortcuts(null);
    setLoadFailed(false);
    api
      .listShortcuts()
      .then(setShortcuts)
      .catch(() => {
        setShortcuts([]);
        setLoadFailed(true);
      });
  }, [loadAttempt]);

  if (shortcuts === null) {
    return (
      <div className="flex min-h-48 items-center justify-center rounded-lg border border-border">
        <div className="flex items-center gap-2 text-caption text-muted-foreground">
          <Spinner className="size-4" />
          Loading shortcuts…
        </div>
      </div>
    );
  }

  if (loadFailed) {
    return (
      <div className="rounded-lg border border-border">
        <EmptyState
          compact
          title="Couldn't load shortcuts"
          hint="Alchemy couldn't read the command registry."
        >
          <Button
            className="mt-2"
            size="sm"
            variant="secondary"
            onClick={() => setLoadAttempt((attempt) => attempt + 1)}
          >
            Try again
          </Button>
        </EmptyState>
      </div>
    );
  }

  const byContext = new Map<string, ShortcutRow[]>();
  for (const shortcut of shortcuts) {
    const context = shortcut.context || "App-wide";
    byContext.set(context, [...(byContext.get(context) ?? []), shortcut]);
  }

  const shortcutColumns = [
    ["Home", "Notebook"],
    ["App-wide", "Reader"],
  ].map((contexts) =>
    contexts
      .map((context) => ({ context, rows: byContext.get(context) ?? [] }))
      .filter((section) => section.rows.length > 0),
  );
  const knownContexts = new Set(
    shortcutColumns.flatMap((column) =>
      column.map((section) => section.context),
    ),
  );
  const extraShortcutSections = [...byContext.entries()]
    .filter(([context]) => !knownContexts.has(context))
    .map(([context, rows]) => ({ context, rows }));
  shortcutColumns[1].push(...extraShortcutSections);

  const slashColumns = [
    ["Generate", "Documents"],
    ["Actions", "Learning"],
  ].map((families) =>
    families.map((family) => ({
      family,
      commands: SLASH_COMMANDS.filter((command) => command.family === family),
    })),
  );

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-3">
        <p className="text-pretty text-caption leading-relaxed text-muted-foreground">
          Work quickly without leaving the keyboard. Shortcuts follow the
          part of Alchemy you are using.
        </p>

        <div className="grid grid-cols-2 items-start gap-2">
          {shortcutColumns.map((column, columnIndex) => (
            <div key={columnIndex} className="flex min-w-0 flex-col gap-2">
              {column.map((section) => (
                <ShortcutSection
                  key={section.context}
                  title={section.context}
                  rows={section.rows}
                />
              ))}
            </div>
          ))}
        </div>

        <p className="text-pretty px-1 text-micro leading-relaxed text-subtle-foreground">
          On Windows and Linux, use Ctrl in place of ⌘. In a chat, just start
          typing and the composer takes focus.
        </p>
      </div>

      <section
        aria-labelledby="slash-commands-heading"
        className="flex flex-col gap-3"
      >
        <div className="px-1">
          <h3
            id="slash-commands-heading"
            className="text-balance text-body font-semibold text-foreground"
          >
            Slash commands
          </h3>
          <p className="mt-1 text-pretty text-caption leading-relaxed text-muted-foreground">
            Type <code className="text-citation">/</code> at the start of the
            chat composer to open the command picker. Tab completes; Enter runs.
          </p>
        </div>

        <div className="grid grid-cols-2 items-start gap-2">
          {slashColumns.map((column, columnIndex) => (
            <div key={columnIndex} className="flex min-w-0 flex-col gap-2">
              {column.map((section) => (
                <SlashCommandSection
                  key={section.family}
                  title={section.family}
                  commands={section.commands}
                />
              ))}
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}

interface ShortcutRow {
  keys: string;
  label: string;
  context: string;
}

function ShortcutSection({
  title,
  rows,
}: {
  title: string;
  rows: ShortcutRow[];
}) {
  const headingId = `shortcut-section-${title.toLowerCase().replace(/\s+/g, "-")}`;
  return (
    <section
      aria-labelledby={headingId}
      className="overflow-hidden rounded-lg border border-border"
    >
      <h3
        id={headingId}
        className="text-balance border-b border-border px-3 py-2 text-body font-semibold text-foreground"
      >
        {title}
      </h3>
      <div className="divide-y divide-border">
        {rows.map((shortcut) => (
          <div
            key={`${shortcut.label}-${shortcut.keys}`}
            className="flex min-h-9 items-center gap-3 px-3 py-2"
          >
            <span className="min-w-0 flex-1 text-caption leading-snug text-foreground/90">
              {shortcut.label}
            </span>
            <ShortcutKeys keys={shortcut.keys} />
          </div>
        ))}
      </div>
    </section>
  );
}

function ShortcutKeys({ keys }: { keys: string }) {
  return (
    <span
      className="flex shrink-0 items-center justify-end gap-1"
      aria-label={keys}
    >
      {keys.split(" ").map((key, index) => (
        <Kbd key={`${key}-${index}`}>{displayKey(key)}</Kbd>
      ))}
    </span>
  );
}

function displayKey(key: string) {
  if (key === "esc") return "Esc";
  if (key === "space") return "Space";
  if (key === "click") return "Click";
  return key;
}

function SlashCommandSection({
  title,
  commands,
}: {
  title: string;
  commands: typeof SLASH_COMMANDS;
}) {
  const headingId = `slash-section-${title.toLowerCase()}`;
  return (
    <section
      aria-labelledby={headingId}
      className="overflow-hidden rounded-lg border border-border"
    >
      <h4
        id={headingId}
        className="text-balance border-b border-border px-3 py-2 text-body font-semibold text-foreground"
      >
        {title}
      </h4>
      <div className="divide-y divide-border">
        {commands.map((command) => (
          <div
            key={command.name}
            className="flex min-h-10 items-start gap-3 px-3 py-2"
          >
            <span className="min-w-0 flex-1 text-caption leading-snug text-foreground/90">
              {command.description}
            </span>
            <code
              className="max-w-36 shrink-0 truncate text-right text-micro leading-snug text-citation"
              title={`/${command.name}${command.argHint ? ` ${command.argHint}` : ""}`}
            >
              /{command.name}
              {command.argHint ? ` ${command.argHint}` : ""}
            </code>
          </div>
        ))}
      </div>
    </section>
  );
}

export function AboutTab() {
  const [version, setVersion] = useState("");
  const [build, setBuild] = useState<BuildInfo | null>(null);
  // Fresh look at the release feed every time About opens — "am I current?"
  // is the question this page exists to answer.
  const [latest, setLatest] = useState<"checking" | "current" | "offline" | string>("checking");
  // What's new: the hand-written notes each GitHub release carries, read
  // live from the feed rather than re-bundled into the app's artifacts.
  const [releases, setReleases] = useState<ReleaseNote[]>([]);
  const [showAllReleases, setShowAllReleases] = useState(false);
  const openSettings = useStore((s) => s.openSettings);
  useEffect(() => {
    getVersion().then(setVersion).catch(() => setVersion(""));
    api.buildInfo().then(setBuild).catch(() => setBuild(null));
    api.releaseHistory().then(setReleases).catch(() => setReleases([]));
    void checkForUpdates().then((flow) => {
      if (flow.status === "available") {
        useStore.setState({ updateAvailable: flow.version });
        setLatest(flow.version);
      } else setLatest(flow.status === "none" ? "current" : "offline");
    });
  }, []);
  const shownReleases = showAllReleases ? releases : releases.slice(0, 1);
  return (
    <div className="flex flex-col items-center gap-1 py-6 text-center">
      <AlchemySymbol className="h-16 w-16 text-citation/70" />
      <div className="mt-3 text-[1.0625rem] font-semibold">Alchemy</div>
      <div className="text-body text-muted-foreground">Local-first research notebooks</div>
      {version && (
        <div className="mt-2 text-caption text-subtle-foreground">
          Version {version}
          {build && <>{" · "}<span className="font-mono">{build.commit}</span>{build.profile === "dev" && <span className="ml-1.5 rounded bg-primary/15 px-1.5 py-0.5 font-medium text-citation">dev</span>}</>}
        </div>
      )}
      {latest === "current" ? (
        <div className="mt-1 text-caption text-subtle-foreground">You&rsquo;re on the latest version.</div>
      ) : latest !== "checking" && latest !== "offline" ? (
        <button
          type="button"
          className="mt-1 text-caption text-citation hover:underline"
          onClick={() => openSettings("general")}
        >
          Version {latest} is available — install from Settings → General
        </button>
      ) : null}
      <button type="button" className="mt-4 inline-flex items-center gap-1.5 text-caption text-citation hover:underline" onClick={() => void openUrl("https://github.com/thrashr888/alchemy")}>
        <Globe className="h-3.5 w-3.5" />
        github.com/thrashr888/alchemy
      </button>
      {shownReleases.length > 0 && (
        <div className="mt-6 w-full text-left">
          <div className="mb-2 text-micro font-semibold uppercase tracking-wide text-subtle-foreground">
            What&rsquo;s new
          </div>
          <div className="flex flex-col gap-4">
            {shownReleases.map((release) => (
              <div key={release.version} className="rounded-md border border-border p-3">
                <div className="mb-1.5 flex items-baseline gap-2">
                  <button
                    type="button"
                    className="text-body font-semibold hover:underline"
                    onClick={() => void openUrl(release.url)}
                  >
                    {release.name || `v${release.version}`}
                  </button>
                  {release.version === version && (
                    <span className="rounded bg-primary/15 px-1.5 py-0.5 text-micro font-medium text-citation">
                      installed
                    </span>
                  )}
                  {release.publishedAt && (
                    <span className="ml-auto text-micro text-subtle-foreground">
                      {new Date(release.publishedAt).toLocaleDateString()}
                    </span>
                  )}
                </div>
                <div className="text-caption leading-relaxed text-muted-foreground">
                  <Markdown>{release.body}</Markdown>
                </div>
              </div>
            ))}
          </div>
          {!showAllReleases && releases.length > 1 && (
            <button
              type="button"
              className="mt-3 text-caption text-citation hover:underline"
              onClick={() => setShowAllReleases(true)}
            >
              Show {releases.length - 1} earlier releases
            </button>
          )}
        </div>
      )}
      <div className="mt-4 text-caption text-subtle-foreground">© {new Date().getFullYear()} Paul Thrasher</div>
    </div>
  );
}

function ThemePicker() {
  const theme = useStore((state) => state.theme);
  const setTheme = useStore((state) => state.setTheme);
  return (
    <div className="grid grid-cols-2 gap-1.5">
      <ThemeButton
        label="System"
        selected={theme === SYSTEM_THEME}
        // The swatch shows what System resolves to right now, from the same
        // theme table as every other row — not a hand-copied triple.
        colors={(() => {
          const t = THEMES[resolveThemeId(SYSTEM_THEME)];
          return [t.vars.background, t.vars.surface, t.vars.primary];
        })()}
        onClick={() => setTheme(SYSTEM_THEME)}
      />
      {THEME_LIST.map((item) => {
        return (
          <ThemeButton
            key={item.id}
            label={item.label}
            selected={theme === item.id}
            colors={[item.vars.background, item.vars.surface, item.vars.primary]}
            onClick={() => setTheme(item.id)}
          />
        );
      })}
    </div>
  );
}

function ThemeButton({ label, selected, colors, onClick }: { label: string; selected: boolean; colors: string[]; onClick: () => void }) {
  return (
    <button type="button" aria-pressed={selected} onClick={onClick} className={cn("flex items-center gap-2 rounded-md border px-2.5 py-2 text-left text-caption transition-colors", selected ? "border-primary/60 bg-primary/10 text-foreground" : "border-border bg-surface-2 text-muted-foreground hover:text-foreground")}>
      <span className="flex overflow-hidden rounded border border-border">
        {colors.map((color) => <span key={color} className="h-4 w-3" style={{ backgroundColor: color }} />)}
      </span>
      {label}
    </button>
  );
}

function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-flex h-5 min-w-5 items-center justify-center rounded-md border border-border-strong bg-surface-2 px-1.5 font-sans text-micro text-foreground/85 shadow-sm">
      {children}
    </kbd>
  );
}

/** macOS System Settings-style option: an icon tile above its label, the
 *  selection carried by an accent ring on the tile (never color alone — the
 *  label bolds too). */
function OptionTile({
  icon: Icon,
  label,
  active,
  onClick,
}: {
  icon: LucideIcon;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className="group flex w-full flex-col items-center gap-1.5 outline-none"
    >
      <span
        className={cn(
          "flex h-11 w-16 items-center justify-center rounded-lg border transition-colors",
          "group-focus-visible:ring-2 group-focus-visible:ring-ring/60",
          active
            ? "border-primary bg-primary/15 text-citation ring-1 ring-primary"
            : "border-border-strong bg-surface-2 text-muted-foreground group-hover:bg-elevated group-hover:text-foreground",
        )}
      >
        <Icon className="h-[18px] w-[18px]" />
      </span>
      <span
        className={cn(
          "text-balance text-center text-caption leading-tight transition-colors",
          active ? "font-medium text-foreground" : "text-muted-foreground",
        )}
      >
        {label}
      </span>
    </button>
  );
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <div className="text-caption font-medium text-foreground">{label}</div>
      {children}
      {hint && <div className="text-pretty text-caption text-subtle-foreground">{hint}</div>}
    </section>
  );
}
