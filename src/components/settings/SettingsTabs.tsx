import { useEffect, useState, type ReactNode } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { SYSTEM_THEME, THEME_LIST } from "@/lib/themes";
import type { BuildInfo, ChatConfig } from "@/lib/types";
import { cn } from "@/lib/utils";
import { AlchemySymbol } from "../AlchemyHero";
import { Input, Textarea } from "../ui";
import { Globe } from "lucide-react";

const CHAT_STYLES = [
  { id: "default", label: "Default", hint: "Balanced, grounded answers for research and brainstorming." },
  { id: "learning", label: "Learning Guide", hint: "Explains step by step, defines terms, builds intuition." },
  { id: "custom", label: "Custom", hint: "Give your own goal, style, or role." },
] as const;

const CHAT_LENGTHS = [
  { id: "default", label: "Default" },
  { id: "longer", label: "Longer" },
  { id: "shorter", label: "Shorter" },
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

  return (
    <div className="flex flex-col gap-4">
      <p className="text-pretty text-[13px] leading-relaxed text-muted-foreground">
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
        <div className="flex flex-wrap gap-1.5">
          {CHAT_STYLES.map((style) => (
            <Pill
              key={style.id}
              active={chatConfig.style === style.id}
              onClick={() => apply({ style: style.id })}
            >
              {style.label}
            </Pill>
          ))}
        </div>
        {styleHint && <span className="text-[11px] text-subtle-foreground">{styleHint}</span>}
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
        <div className="flex flex-wrap gap-1.5">
          {CHAT_LENGTHS.map((length) => (
            <Pill
              key={length.id}
              active={chatConfig.length === length.id}
              onClick={() => apply({ length: length.id })}
            >
              {length.label}
            </Pill>
          ))}
        </div>
      </Field>
    </div>
  );
}

export function PersonalizationTab() {
  const aiConfig = useStore((state) => state.aiConfig);
  const save = useStore((state) => state.saveAiConfig);
  const [draft, setDraft] = useState({ name: "", profession: "", instructions: "" });

  useEffect(() => {
    if (aiConfig?.profile) setDraft(aiConfig.profile);
    // Load once so a blur-save round trip cannot clobber in-progress typing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const saveOnBlur = () => {
    if (!aiConfig) return;
    const profile = aiConfig.profile ?? { name: "", profession: "", instructions: "" };
    if (
      draft.name !== profile.name ||
      draft.profession !== profile.profession ||
      draft.instructions !== profile.instructions
    ) {
      void save({ ...aiConfig, profile: { ...draft } });
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <p className="text-pretty text-[13px] leading-relaxed text-muted-foreground">
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
      <Field label="Theme" hint="Applies immediately.">
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
        hint="Experimental: the desktop blurs through the sidebars and titlebar, like native macOS apps."
      >
        <div className="flex flex-wrap gap-1.5">
          <Pill
            active={reading.glass}
            onClick={() => setReading({ glass: !reading.glass })}
          >
            {reading.glass ? "On" : "Off"}
          </Pill>
        </div>
      </Field>
    </div>
  );
}

const SHORTCUTS: { keys: string[]; label: string; context?: string }[] = [
  { keys: ["⌘", "N"], label: "New notebook", context: "Home" },
  { keys: ["⌘", "N"], label: "New note", context: "Notebook" },
  { keys: ["⌘", "K"], label: "Open the command menu" },
  { keys: ["⌘", "F"], label: "Find in source", context: "Reader" },
  { keys: ["⌘", "1"], label: "Show or hide Sources", context: "Notebook" },
  { keys: ["⌘", "2"], label: "Show or hide Studio", context: "Notebook" },
  { keys: ["⌘", ","], label: "Open Settings" },
  { keys: ["↩"], label: "Send message · next find match" },
  { keys: ["⇧", "↩"], label: "New line in the composer" },
  { keys: ["esc"], label: "Close dialog or menu" },
];

export function ShortcutsTab() {
  return (
    <div className="flex flex-col gap-1">
      {SHORTCUTS.map((shortcut) => (
        <div key={`${shortcut.label}-${shortcut.context ?? "global"}`} className="flex items-center gap-3 rounded-md px-1 py-1.5">
          <div className="flex w-20 shrink-0 items-center gap-1">
            {shortcut.keys.map((key) => <Kbd key={key}>{key}</Kbd>)}
          </div>
          <span className="text-[13px] text-foreground/90">{shortcut.label}</span>
          {shortcut.context && <span className="ml-auto text-[11px] text-subtle-foreground">{shortcut.context}</span>}
        </div>
      ))}
      <p className="mt-2 text-[11px] leading-relaxed text-subtle-foreground">
        On Windows and Linux, use Ctrl in place of ⌘.
      </p>
    </div>
  );
}

export function AboutTab() {
  const [version, setVersion] = useState("");
  const [build, setBuild] = useState<BuildInfo | null>(null);
  useEffect(() => {
    getVersion().then(setVersion).catch(() => setVersion(""));
    api.buildInfo().then(setBuild).catch(() => setBuild(null));
  }, []);
  return (
    <div className="flex flex-col items-center gap-1 py-6 text-center">
      <AlchemySymbol className="h-16 w-16 text-citation/70" />
      <div className="mt-3 text-[17px] font-semibold">Alchemy</div>
      <div className="text-[13px] text-muted-foreground">Local-first research notebooks</div>
      {version && (
        <div className="mt-2 text-[12px] text-subtle-foreground">
          Version {version}
          {build && <>{" · "}<span className="font-mono">{build.commit}</span>{build.profile === "dev" && <span className="ml-1.5 rounded bg-primary/15 px-1.5 py-0.5 font-medium text-citation">dev</span>}</>}
        </div>
      )}
      <button type="button" className="mt-4 inline-flex items-center gap-1.5 text-[12px] text-citation hover:underline" onClick={() => void openUrl("https://github.com/thrashr888/alchemy")}>
        <Globe className="h-3.5 w-3.5" />
        github.com/thrashr888/alchemy
      </button>
      <div className="mt-4 text-[12px] text-subtle-foreground">© {new Date().getFullYear()} Paul Thrasher</div>
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
        colors={["#08090a", "#eceef1", "#5e6ad2"]}
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
    <button type="button" aria-pressed={selected} onClick={onClick} className={cn("flex items-center gap-2 rounded-md border px-2.5 py-2 text-left text-[12px] transition-colors", selected ? "border-primary/60 bg-primary/10 text-foreground" : "border-border bg-surface-2 text-muted-foreground hover:text-foreground")}>
      <span className="flex overflow-hidden rounded border border-border">
        {colors.map((color) => <span key={color} className="h-4 w-3" style={{ backgroundColor: color }} />)}
      </span>
      {label}
    </button>
  );
}

function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="inline-flex h-[22px] min-w-[22px] items-center justify-center rounded-md border border-border-strong bg-surface-2 px-1.5 font-sans text-[12px] text-foreground/85 shadow-[0_1px_0_var(--border)]">{children}</kbd>;
}

function Pill({ active, onClick, children }: { active: boolean; onClick: () => void; children: ReactNode }) {
  return <button type="button" aria-pressed={active} onClick={onClick} className={cn("rounded-md border px-3 py-1.5 text-[12px] transition-colors", active ? "border-primary/60 bg-primary/15 text-citation" : "border-border bg-surface-2 text-muted-foreground hover:text-foreground")}>{children}</button>;
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <div className="text-[12px] font-medium text-foreground">{label}</div>
      {children}
      {hint && <div className="text-pretty text-[11px] text-subtle-foreground">{hint}</div>}
    </section>
  );
}
