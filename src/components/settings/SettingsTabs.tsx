import { useEffect, useState, type ReactNode } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { checkForUpdates } from "@/lib/updates";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { SYSTEM_THEME, THEME_LIST, THEMES, resolveThemeId } from "@/lib/themes";
import { SLASH_COMMANDS } from "@/lib/slashCommands";
import type { BuildInfo, ChatConfig, ReleaseNote } from "@/lib/types";
import { cn } from "@/lib/utils";
import { AlchemySymbol } from "../AlchemyHero";
import { Markdown } from "../Markdown";
import { Input, Textarea } from "../ui";
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
  { id: "buddy", label: "Buddy", icon: Smile, hint: "A sharp friend who did the reading. Matches your register, still cited." },
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

const CHAT_FONTS = [
  { id: "sans", label: "Sans", className: "font-sans" },
  { id: "serif", label: "Serif", className: "font-serif" },
  { id: "mono", label: "Mono", className: "font-mono" },
  { id: "system", label: "System", className: "chat-system" },
] as const;

const CHAT_SIZES = [
  { id: "small", label: "Small" },
  { id: "medium", label: "Medium" },
  { id: "large", label: "Large" },
] as const;

const CHAT_ALIGNS = [
  { id: "natural", label: "Natural" },
  { id: "justified", label: "Justified" },
] as const;

export function ChatTab() {
  const chatConfig = useStore((state) => state.chatConfig);
  const setChatConfig = useStore((state) => state.setChatConfig);
  const currentId = useStore((state) => state.currentId);
  const notebook = useStore((state) =>
    state.notebooks.find((candidate) => candidate.id === state.currentId),
  );
  const apply = (patch: Partial<ChatConfig>) =>
    setChatConfig({ ...chatConfig, ...patch });
  const styleHint = CHAT_STYLES.find((style) => style.id === chatConfig.style)?.hint;
  const lengthHint = CHAT_LENGTHS.find((length) => length.id === chatConfig.length)?.hint;

  return (
    <div className="flex flex-col gap-4">
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

      <Field label="Conversational goal, style, or role">
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
        {styleHint && <span className="text-micro text-subtle-foreground">{styleHint}</span>}
        {chatConfig.style === "custom" && (
          <Textarea
            rows={4}
            className="mt-1"
            aria-label="Custom conversational style"
            placeholder="Act as a skeptical peer reviewer; challenge claims and ask for evidence…"
            value={chatConfig.customPrompt}
            onChange={(event) => apply({ customPrompt: event.target.value })}
          />
        )}
      </Field>

      <Field label="Response length">
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
        {lengthHint && <span className="text-micro text-subtle-foreground">{lengthHint}</span>}
      </Field>
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
    assistantName: "",
  });

  useEffect(() => {
    if (aiConfig?.profile) setDraft({ ...aiConfig.profile, assistantName: aiConfig.profile.assistantName ?? "" });
    // Load once so a blur-save round trip cannot clobber in-progress typing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const saveOnBlur = () => {
    if (!aiConfig) return;
    const profile = aiConfig.profile ?? {
      name: "",
      profession: "",
      instructions: "",
      assistantName: "",
    };
    if (
      draft.name !== profile.name ||
      draft.profession !== profile.profession ||
      draft.instructions !== profile.instructions ||
      draft.assistantName !== (profile.assistantName ?? "")
    ) {
      void save({ ...aiConfig, profile: { ...draft } });
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <p className="text-pretty text-body leading-relaxed text-muted-foreground">
        Personalization is added to chat and document prompts and is sent only to your configured model. Changes save automatically.
      </p>
      <Field label="What should the assistant call you?">
        <Input
          name="profile-name"
          autoComplete="name"
          aria-label="What should the assistant call you?"
          placeholder="Paul…"
          value={draft.name}
          onChange={(event) => setDraft({ ...draft, name: event.target.value })}
          onBlur={saveOnBlur}
        />
      </Field>
      <Field label="What do you call the assistant?">
        <Input
          name="profile-assistant-name"
          aria-label="What do you call the assistant?"
          placeholder="Pip…"
          value={draft.assistantName}
          onChange={(event) => setDraft({ ...draft, assistantName: event.target.value })}
          onBlur={saveOnBlur}
        />
      </Field>
      <Field label="What best describes your work?">
        <Input
          name="profile-profession"
          autoComplete="organization-title"
          aria-label="What best describes your work?"
          placeholder="Product management…"
          value={draft.profession}
          onChange={(event) => setDraft({ ...draft, profession: event.target.value })}
          onBlur={saveOnBlur}
        />
      </Field>
      <Field label="Instructions for the assistant">
        <Textarea
          rows={8}
          name="profile-instructions"
          aria-label="Instructions for the assistant"
          placeholder="Preferences to keep in mind across all notebooks…"
          value={draft.instructions}
          onChange={(event) => setDraft({ ...draft, instructions: event.target.value })}
          onBlur={saveOnBlur}
        />
      </Field>
    </div>
  );
}

export function AppearanceTab() {
  const reading = useStore((state) => state.reading);
  const setReading = useStore((state) => state.setReading);
  return (
    <div className="flex flex-col gap-4">
      <Field label="Theme">
        <ThemePicker />
      </Field>
      <div className="h-px bg-border" />
      <Field label="Chat font" hint="Display only; this does not change the model.">
        <div className="flex flex-wrap gap-1.5">
          {CHAT_FONTS.map((font) => (
            <Pill key={font.id} active={reading.font === font.id} onClick={() => setReading({ font: font.id })}>
              <span className={font.className}>{font.label}</span>
            </Pill>
          ))}
        </div>
      </Field>
      <Field label="Text size">
        <div className="flex flex-wrap gap-1.5">
          {CHAT_SIZES.map((size) => (
            <Pill key={size.id} active={reading.fontSize === size.id} onClick={() => setReading({ fontSize: size.id })}>
              {size.label}
            </Pill>
          ))}
        </div>
      </Field>
      <Field label="Alignment">
        <div className="flex flex-wrap gap-1.5">
          {CHAT_ALIGNS.map((alignment) => (
            <Pill key={alignment.id} active={reading.textAlign === alignment.id} onClick={() => setReading({ textAlign: alignment.id })}>
              {alignment.label}
            </Pill>
          ))}
        </div>
      </Field>
      <div className="h-px bg-border" />
      <Field
        label="Reader"
        hint="What the document reader shows around the text."
      >
        <div className="flex flex-wrap gap-1.5">
          <Pill
            active={reading.showToc}
            onClick={() => setReading({ showToc: !reading.showToc })}
          >
            Table of contents
          </Pill>
          <Pill
            active={reading.showRelated}
            onClick={() => setReading({ showRelated: !reading.showRelated })}
          >
            Related passages
          </Pill>
        </div>
      </Field>
      <Field
        label="Glass chrome"
        hint="Experimental: the desktop blurs through the chrome like native macOS apps. Tinted keeps more body; Clear lets more through."
      >
        <div className="flex flex-wrap gap-1.5">
          <Pill
            active={!reading.glass}
            onClick={() => setReading({ glass: false })}
          >
            Off
          </Pill>
          <Pill
            active={reading.glass && reading.glassStyle === "tinted"}
            onClick={() => setReading({ glass: true, glassStyle: "tinted" })}
          >
            Tinted
          </Pill>
          <Pill
            active={reading.glass && reading.glassStyle === "clear"}
            onClick={() => setReading({ glass: true, glassStyle: "clear" })}
          >
            Clear
          </Pill>
        </div>
      </Field>
    </div>
  );
}

export function ShortcutsTab() {
  // The rows come from the menu's command registry (menu.rs::CMD) — one
  // source of truth for the native menu and this tab, so a shortcut can no
  // longer be registered in one and missing from the other.
  const [shortcuts, setShortcuts] = useState<
    { keys: string; label: string; context: string }[]
  >([]);
  useEffect(() => {
    api
      .listShortcuts()
      .then(setShortcuts)
      .catch(() => setShortcuts([]));
  }, []);
  return (
    <div className="flex flex-col gap-1">
      {shortcuts.map((shortcut) => (
        <div key={`${shortcut.label}-${shortcut.context || "global"}`} className="flex items-center gap-3 rounded-md px-1 py-1.5">
          <div className="flex w-20 shrink-0 items-center gap-1">
            {shortcut.keys.split(" ").map((key) => <Kbd key={key}>{key}</Kbd>)}
          </div>
          <span className="text-body text-foreground/90">{shortcut.label}</span>
          {shortcut.context && <span className="ml-auto text-micro text-subtle-foreground">{shortcut.context}</span>}
        </div>
      ))}
      <p className="mt-2 text-micro leading-relaxed text-subtle-foreground">
        On Windows and Linux, use Ctrl in place of ⌘.
      </p>

      <div className="mt-5 mb-1 px-1 text-micro font-semibold uppercase tracking-wide text-subtle-foreground">
        Slash commands
      </div>
      <p className="mb-1.5 px-1 text-caption leading-relaxed text-muted-foreground">
        Type <code className="text-citation">/</code> at the start of the chat
        composer to open the command picker. Tab completes, Enter runs.
      </p>
      {SLASH_COMMANDS.map((c) => (
        <div key={c.name} className="flex items-center gap-3 rounded-md px-1 py-1.5">
          <code className="w-36 shrink-0 truncate text-caption text-citation">
            /{c.name}
            {c.argHint ? ` ${c.argHint}` : ""}
          </code>
          <span className="min-w-0 flex-1 text-body text-foreground/90">{c.description}</span>
          <span className="ml-auto shrink-0 text-micro text-subtle-foreground">{c.family}</span>
        </div>
      ))}
    </div>
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
  return <kbd className="inline-flex h-[22px] min-w-[22px] items-center justify-center rounded-md border border-border-strong bg-surface-2 px-1.5 font-sans text-caption text-foreground/85 shadow-[0_1px_0_var(--border)]">{children}</kbd>;
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

function Pill({ active, onClick, children }: { active: boolean; onClick: () => void; children: ReactNode }) {
  return <button type="button" aria-pressed={active} onClick={onClick} className={cn("rounded-md border px-3 py-1.5 text-caption transition-colors", active ? "border-primary/60 bg-primary/15 text-citation" : "border-border bg-surface-2 text-muted-foreground hover:text-foreground")}>{children}</button>;
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
