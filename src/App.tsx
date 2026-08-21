import { useEffect } from "react";
import { useStore } from "@/lib/store";
import { HomeView } from "@/components/HomeView";
import { Workspace } from "@/components/Workspace";
import { SettingsDialog } from "@/components/SettingsDialog";
import { CommandPalette } from "@/components/CommandPalette";
import { ImportOkfModal } from "@/components/ImportOkfModal";
import { ExternalAddModal } from "@/components/ExternalAddModal";
import { FileDrop } from "@/components/FileDrop";
import { MigrationOverlay } from "@/components/MigrationOverlay";
import { NoteWindow } from "@/components/NoteWindow";
import { PrintExportView } from "@/components/PrintExportView";
import { Onboarding } from "@/components/Onboarding";
import { Toaster } from "@/components/ui";
import { shortcutBlocked } from "@/lib/utils";

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
  const embedderDownload = useStore((s) => s.embedderDownload);
  const settingsTab = useStore((s) => s.settingsTab);
  const openSettings = useStore((s) => s.openSettings);
  const closeSettings = useStore((s) => s.closeSettings);

  useEffect(() => {
    void init();
  }, [init]);

  // Cmd/Ctrl+, opens Settings (standard desktop convention); Cmd/Ctrl+K
  // toggles the command menu — from anywhere, including inputs.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      if (e.key === ",") {
        // Don't stack Settings on top of an open dialog (confirms, palette).
        if (shortcutBlocked(e)) return;
        e.preventDefault();
        openSettings();
      } else if (e.key === "k") {
        // togglePalette handles open dialogs itself: it closes an open
        // palette and dismisses other dialogs before opening.
        e.preventDefault();
        useStore.getState().togglePalette();
      } else if (
        (e.key === "ArrowLeft" || e.key === "ArrowRight") &&
        !e.shiftKey &&
        !e.altKey
      ) {
        // Back/forward, Safari-style: handled here rather than as native
        // menu accelerators so Cmd+←/→ keep their line-start/line-end
        // meaning inside text fields (shortcutBlocked covers those).
        if (shortcutBlocked(e)) return;
        e.preventDefault();
        const s = useStore.getState();
        if (e.key === "ArrowLeft") s.navBack();
        else s.navForward();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openSettings]);

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
      <PrintExportView
        noteId={window.__ALCHEMY_NOTE__}
        pdfPath={window.__ALCHEMY_PRINT_EXPORT__}
      />
    );
  }

  // A note-reader window renders just the note — no panels, no palette.
  if (window.__ALCHEMY_NOTE__) {
    return (
      <>
        <NoteWindow noteId={window.__ALCHEMY_NOTE__} />
        <Toaster toasts={toasts} onDismiss={dismissToast} />
      </>
    );
  }

  return (
    <>
      {currentId ? (
        <Workspace onOpenSettings={() => openSettings()} />
      ) : (
        <HomeView onOpenSettings={() => openSettings()} />
      )}

      <SettingsDialog
        open={settingsOpen}
        onClose={closeSettings}
        initialTab={settingsTab}
      />
      <CommandPalette />
      <ImportOkfModal />
      {/* App-level, not inside Workspace: Home's "Add source…", the tray, and
          Services all raise this with no notebook open, and mounted under
          Workspace it simply never rendered there. */}
      <ExternalAddModal />
      {/* Always mounted: OKF-bundle drops import from the homepage too. */}
      <FileDrop />
      <MigrationOverlay />
      {needsSetup && !onboardingDismissed && !settingsOpen && (
        // Onboarding's buttons are model-setup affordances — take them to Models.
        <Onboarding onOpenSettings={() => openSettings("models")} />
      )}

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
    </>
  );
}

export default App;
