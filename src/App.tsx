import { useEffect } from "react";
import { useStore } from "@/lib/store";
import { HomeView } from "@/components/HomeView";
import { Workspace } from "@/components/Workspace";
import { SettingsDialog } from "@/components/SettingsDialog";
import { FileDrop } from "@/components/FileDrop";
import { MigrationOverlay } from "@/components/MigrationOverlay";
import { Onboarding } from "@/components/Onboarding";
import { AlertTriangle, X } from "lucide-react";

function App() {
  const init = useStore((s) => s.init);
  const currentId = useStore((s) => s.currentId);
  const error = useStore((s) => s.error);
  const setError = useStore((s) => s.setError);
  const health = useStore((s) => s.modelHealth);
  const onboardingDismissed = useStore((s) => s.onboardingDismissed);
  const needsSetup = !!health && (!health.chat.working || !health.embed.working);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const settingsTab = useStore((s) => s.settingsTab);
  const openSettings = useStore((s) => s.openSettings);
  const closeSettings = useStore((s) => s.closeSettings);

  useEffect(() => {
    void init();
  }, [init]);

  return (
    <>
      {currentId ? (
        <Workspace onOpenSettings={() => openSettings()} />
      ) : (
        <HomeView onOpenSettings={() => openSettings()} />
      )}

      <SettingsDialog open={settingsOpen} onClose={closeSettings} initialTab={settingsTab} />
      {/* Drag-drop only routes into a notebook when one is open. */}
      {currentId && <FileDrop />}
      <MigrationOverlay />
      {needsSetup && !onboardingDismissed && !settingsOpen && (
        <Onboarding onOpenSettings={() => openSettings()} />
      )}

      {error && (
        <div className="fixed bottom-4 left-1/2 z-[70] flex max-w-[520px] -translate-x-1/2 items-start gap-2.5 rounded-lg border border-destructive/40 bg-elevated px-3.5 py-2.5 shadow-lg">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" />
          <div className="text-[12px] text-foreground/90 selectable">{error}</div>
          <button
            className="ml-1 rounded p-0.5 text-muted-foreground hover:text-foreground"
            onClick={() => setError(null)}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}
    </>
  );
}

export default App;
