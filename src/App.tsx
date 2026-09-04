import { lazy, Suspense, useEffect, useRef, useState } from "react";
import {
  NOTEBOOK_PANELS,
  navAtomic,
  toggleNotebookPanel,
  useStore,
} from "@/lib/store";
import { HOME_CARDS, toggleHomeCard } from "@/lib/homeCards";
import { HomeView } from "@/components/HomeView";
import { FileDrop } from "@/components/FileDrop";
import { Toaster } from "@/components/ui";
import { FatalOverlay } from "@/components/ErrorBoundary";
import { shortcutBlocked } from "@/lib/utils";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api } from "@/lib/api";
import { report } from "@/lib/diagnostics";
import { afterStartupPaint } from "@/lib/startup";
import type { HomeSection } from "@/lib/storeTypes";
import { THEME_LIST, SYSTEM_THEME } from "@/lib/themes";
import { ARTIFACTS, AUDIO_OVERVIEW } from "@/components/studioArtifacts";

// Home is the first frame while init resolves the last saved view. The full
// notebook workspace cannot render until then, so load its panels in parallel
// instead of making WebKit parse them before it can paint Home.
const Workspace = lazy(() =>
  import("@/components/Workspace").then((m) => ({ default: m.Workspace })),
);

// Pop-out notes and print/export windows are separate WebViews selected by
// their boot globals. The main window can never render either route, so keep
// their Markdown, diagram, slide, audio, and print stacks out of its entry
// chunk. Their dedicated windows pay the import only when they exist.
const NoteWindow = lazy(() =>
  import("@/components/NoteWindow").then((m) => ({ default: m.NoteWindow })),
);
const PrintExportView = lazy(() =>
  import("@/components/PrintExportView").then((m) => ({
    default: m.PrintExportView,
  })),
);

// These surfaces cannot be visible on the first committed frame. Keeping
// them out of the startup graph also keeps Settings-only integrations and
// the command palette's corpus-search UI out of the code WebKit must parse
// before it can paint the view the user actually opened.
const SettingsDialog = lazy(() =>
  import("@/components/SettingsDialog").then((m) => ({
    default: m.SettingsDialog,
  })),
);
const CommandPalette = lazy(() =>
  import("@/components/CommandPalette").then((m) => ({
    default: m.CommandPalette,
  })),
);
const ImportOkfModal = lazy(() =>
  import("@/components/ImportOkfModal").then((m) => ({
    default: m.ImportOkfModal,
  })),
);
const ExternalAddModal = lazy(() =>
  import("@/components/ExternalAddModal").then((m) => ({
    default: m.ExternalAddModal,
  })),
);
const MigrationOverlay = lazy(() =>
  import("@/components/MigrationOverlay").then((m) => ({
    default: m.MigrationOverlay,
  })),
);
const Onboarding = lazy(() =>
  import("@/components/Onboarding").then((m) => ({
    default: m.Onboarding,
  })),
);

/** Mounted inside the restored main view's Suspense boundary, so a lazy
 * workspace cannot claim readiness while its fallback is still showing. */
function StartupReady() {
  useEffect(() => {
    if (!isTauri() || getCurrentWebview().label !== "main") return;
    return afterStartupPaint(() => {
      void api.reportStartupInteractive().catch(() => {
        /* Older backends have no beacon; the window still works. */
      });
    });
  }, []);
  return null;
}

function App() {
  const init = useStore((s) => s.init);
  const currentId = useStore((s) => s.currentId);
  const error = useStore((s) => s.error);
  const setError = useStore((s) => s.setError);
  const toasts = useStore((s) => s.toasts);
  const pushToast = useStore((s) => s.pushToast);
  const dismissToast = useStore((s) => s.dismissToast);
  const health = useStore((s) => s.modelHealth);
  const onboardingDismissed = useStore((s) => s.onboardingDismissed);
  const needsSetup =
    !!health && (!health.chat.working || !health.embed.working);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const paletteOpen = useStore((s) => s.paletteOpen);
  const importOkfOpen = useStore((s) => s.importOkfOpen);
  const pendingExternalAdd = useStore((s) => s.pendingExternalAdd);
  const migration = useStore((s) => s.migration);
  const embedderDownload = useStore((s) => s.embedderDownload);
  const settingsTab = useStore((s) => s.settingsTab);
  const openSettings = useStore((s) => s.openSettings);
  const closeSettings = useStore((s) => s.closeSettings);

  const [initialized, setInitialized] = useState(false);
  const notebookLoading = useStore((s) => s.notebookLoading);
  useEffect(() => {
    let active = true;
    void init().then(
      () => {
        if (active) setInitialized(true);
      },
      (error: unknown) => {
        if (active)
          report(
            "fatal",
            "startup",
            "Could not initialize the window",
            String(error),
          );
      },
    );
    return () => {
      active = false;
    };
  }, [init]);

  // The native menu's Theme and Generate submenus render TypeScript-owned
  // lists (themes.ts, studioArtifacts.tsx) — push them over IPC at startup,
  // and again when the theme changes so the selection dot tracks.
  const theme = useStore((s) => s.theme);
  useEffect(() => {
    if (!isTauri()) return;
    const themes: [string, string][] = [
      [SYSTEM_THEME, "System"],
      ...THEME_LIST.map((t): [string, string] => [t.id, t.label]),
    ];
    const generators: [string, string][] = [AUDIO_OVERVIEW, ...ARTIFACTS].map(
      (a): [string, string] => [a.kind, a.label],
    );
    void api.fillMenuLists(themes, generators, theme).catch(() => {
      /* older backend without the command — menu just keeps its placeholder */
    });
  }, [theme]);

  // The View menu carries two groups of sidebar toggles — Home's four cards
  // and a notebook's four panels — and only one view can act on either. Tell
  // the menu which view is on screen so the other group greys out instead of
  // offering a click that does nothing. The app menu is global to the
  // process, so this follows the focused window: a background window's
  // notebook must not grey out the menu over the Home window in front.
  // (A note pop-out has neither set of sidebars and stays out of it.)
  const inNotebook = !!currentId;
  // Detection cost (docs/RFC-events.md §4): the backend watches the folders
  // of notebooks some window has open and sweeps the rest slowly, so each
  // window reports its notebook as it changes — null on the way to Home.
  // Note pop-outs have no notebook of their own and stay out of it.
  useEffect(() => {
    if (!isTauri() || window.__ALCHEMY_NOTE__) return;
    void api.setOpenNotebook(currentId).catch(() => {
      /* older backend without the command — the minute sweep still runs */
    });
  }, [currentId]);
  const reportedContext = useRef<boolean | null>(null);
  useEffect(() => {
    if (!isTauri() || window.__ALCHEMY_NOTE__) return;
    const report = (force: boolean) => {
      // Notebook to notebook is the same context; only the crossing matters.
      if (!force && reportedContext.current === inNotebook) return;
      reportedContext.current = inNotebook;
      void api.setMenuContext(inNotebook).catch(() => {
        /* older backend without the command — items just stay enabled */
      });
    };
    // Unconditional first report: an app restored straight into a notebook
    // may not have focus yet, and waiting for a focus event left the menu
    // stuck on its built-in Home default — notebook items greyed out inside
    // a notebook. Idempotent, and the focus re-assert below still
    // arbitrates between windows.
    report(false);
    // Taking focus back means re-asserting this window's context over
    // whatever the window that had it last reported.
    const onFocus = () => report(true);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [inNotebook]);

  // Cmd/Ctrl+, opens Settings (standard desktop convention); Cmd/Ctrl+K
  // toggles the command menu — from anywhere, including inputs.
  // In the app both keys belong to the native menu accelerators (menu.rs),
  // which consume the keystroke before the webview sees it — these branches
  // exist for the browser dev build, where no menu exists. Handling them in
  // both places would double-fire togglePalette (open, then instantly close)
  // whenever a keydown does reach JS.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      if (e.key === ",") {
        if (isTauri()) return;
        // Don't stack Settings on top of an open dialog (confirms, palette).
        if (shortcutBlocked(e)) return;
        e.preventDefault();
        openSettings();
      } else if (e.key === "k") {
        if (isTauri()) return;
        // togglePalette handles open dialogs itself: it closes an open
        // palette and dismisses other dialogs before opening.
        e.preventDefault();
        useStore.getState().togglePalette();
      } else if (
        (e.key === "ArrowLeft" ||
          e.key === "ArrowRight" ||
          e.key === "[" ||
          e.key === "]") &&
        !e.shiftKey &&
        !e.altKey
      ) {
        // Back/forward, Safari-style: handled here rather than as native
        // menu accelerators so Cmd+←/→ keep their line-start/line-end
        // meaning inside text fields (shortcutBlocked covers those).
        // ⌘[ / ⌘] are the canonical pair and mean nothing to a text field,
        // but they ride the same guard so a shortcut can't fire under an
        // open dialog.
        if (shortcutBlocked(e)) return;
        e.preventDefault();
        const s = useStore.getState();
        if (e.key === "ArrowLeft" || e.key === "[") s.navBack();
        else s.navForward();
      } else if (e.key >= "1" && e.key <= "5" && !e.shiftKey && !e.altKey) {
        // ⌘1–5 run down whichever set of sidebars is on screen: a notebook's
        // Sources/Studio/Gallery/Grow/Ledger, or Home's Chats/Staff/Brief/Latest
        // Reports — in the order the rails read, which is the View menu's
        // order too. Context-dependent, so it can't be a native menu key
        // equivalent (those are global to the process and would fire in the
        // wrong view); menu.rs keeps the items accelerator-less and documents
        // both meanings in Settings → Shortcuts.
        //
        // A note pop-out renders neither set of sidebars, so it leaves the
        // keystroke alone — as it does the View menu's two groups.
        if (window.__ALCHEMY_NOTE__ || shortcutBlocked(e)) return;
        const i = Number(e.key) - 1;
        if (useStore.getState().currentId) {
          if (i >= NOTEBOOK_PANELS.length) return;
          e.preventDefault();
          toggleNotebookPanel(NOTEBOOK_PANELS[i]);
        } else if (toggleHomeCard(HOME_CARDS[i])) {
          // Home registers its four toggles while it is mounted; nothing
          // registered means nothing to show or hide.
          e.preventDefault();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openSettings]);

  // Home's chat surface, from the native menu (menu.rs). The store owns the
  // rest of `menu://action`; these ride the same broadcast — it reaches every
  // window, and the payload's target label is what keeps only the addressed
  // one acting on it. Each hop leaves a notebook if one is open, so they are
  // one back-stack entry apiece (navAtomic), not two.
  //
  // A note pop-out mounts no Home at all; the store's listener has already
  // handed the action to the main window, so this shell stays out of it.
  useEffect(() => {
    if (!isTauri() || window.__ALCHEMY_NOTE__) return;
    const label = getCurrentWebview().label;
    const goHome = (go: () => Promise<void> | void) =>
      void navAtomic(async () => {
        const s = useStore.getState();
        if (s.currentId) s.closeNotebook();
        await go();
      });
    const section = (id: HomeSection) =>
      goHome(() => useStore.setState({ homeSection: id, openCardId: null }));
    const un = listen<{ target: string; id: string }>("menu://action", (e) => {
      if (e.payload.target !== label) return;
      if (e.payload.id === "menu-new-chat") {
        goHome(() => useStore.getState().openHomeThread(null));
      } else if (e.payload.id === "menu-home-notebooks") {
        section("notebooks");
      } else if (e.payload.id === "menu-home-registry") {
        section("registry");
      } else if (e.payload.id === "menu-home-chat") {
        // The conversation that was last on screen, minting one only if there
        // has never been one — what Home's own Chat tab does.
        goHome(() => {
          const s = useStore.getState();
          return s.openHomeThread(s.homeChat.threadId);
        });
      }
    });
    return () => {
      void un.then((off) => off());
    };
  }, []);

  // Bridge the legacy `error` field into the toast stack so every error path
  // (many still `set({ error })` directly) surfaces consistently and dismisses.
  // An error whose fix lives in Settings → Models (the backend classifier's
  // literal grammar, RFC-self-resolve) is clickable and jumps straight there.
  useEffect(() => {
    if (error) {
      pushToast(
        "error",
        error,
        error.includes("Settings → Models")
          ? () => openSettings("models")
          : undefined,
      );
      setError(null);
    }
  }, [error, pushToast, setError, openSettings]);

  // An export window renders only the note's print sheet, prints itself
  // to the boot-named temp PDF, and is closed by the backend (export.rs).
  if (window.__ALCHEMY_PRINT_EXPORT__ && window.__ALCHEMY_NOTE__) {
    return (
      <Suspense fallback={null}>
        <PrintExportView
          noteId={window.__ALCHEMY_NOTE__}
          pdfPath={window.__ALCHEMY_PRINT_EXPORT__}
        />
      </Suspense>
    );
  }

  // A note-reader window renders just the note — no panels, no palette.
  if (window.__ALCHEMY_NOTE__) {
    return (
      <>
        <Suspense fallback={null}>
          <NoteWindow noteId={window.__ALCHEMY_NOTE__} />
        </Suspense>
        <Toaster toasts={toasts} onDismiss={dismissToast} />
      </>
    );
  }

  return (
    <>
      {currentId ? (
        <Suspense fallback={null}>
          <Workspace onOpenSettings={() => openSettings()} />
          {initialized && !notebookLoading && <StartupReady />}
        </Suspense>
      ) : (
        <>
          <HomeView onOpenSettings={() => openSettings()} />
          {initialized && <StartupReady />}
        </>
      )}

      <Suspense fallback={null}>
        {settingsOpen && (
          <SettingsDialog
            open
            onClose={closeSettings}
            initialTab={settingsTab}
          />
        )}
        {paletteOpen && <CommandPalette />}
        {importOkfOpen && <ImportOkfModal />}
        {/* App-level, not inside Workspace: Home's "Add source…", the tray,
            and Services all raise this with no notebook open. */}
        {pendingExternalAdd && <ExternalAddModal />}
        {migration && <MigrationOverlay />}
        {needsSetup && !onboardingDismissed && !settingsOpen && (
          // Onboarding's buttons are model-setup affordances — take them to Models.
          <Onboarding onOpenSettings={() => openSettings("models")} />
        )}
      </Suspense>
      {/* Always mounted: OKF-bundle drops import from the homepage too. */}
      <FileDrop />

      {embedderDownload && (
        <div className="fixed bottom-4 right-4 z-[70] flex items-center gap-2.5 rounded-lg border border-border-strong bg-elevated px-3.5 py-2.5 shadow-lg">
          <span className="h-2 w-2 animate-pulse rounded-full bg-primary" />
          <div className="flex flex-col">
            <span className="text-caption font-medium text-foreground">
              {embedderDownload.title ?? "Setting up the built-in search model"}
            </span>
            <span className="text-micro text-muted-foreground">
              One-time download ·{" "}
              {embedderDownload.total > 0
                ? `${Math.round((embedderDownload.done / embedderDownload.total) * 100)}% of ${(embedderDownload.total / 1e6).toFixed(0)} MB`
                : `${(embedderDownload.done / 1e6).toFixed(1)} MB…`}
            </span>
          </div>
        </div>
      )}

      <Toaster toasts={toasts} onDismiss={dismissToast} />
      {/* Backend panics: last, and above everything, so the way out is
          visible even when the rest of the window is mid-failure. */}
      <FatalOverlay />
    </>
  );
}

export default App;
