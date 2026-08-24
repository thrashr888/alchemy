// Text-undo delegation (docs/RFC-professional-grade.md Pillar 5).
//
// Giving the app a real ⌘Z means claiming the keystroke: native menu
// accelerators consume it before the webview ever sees a keydown (the same
// reason ⌘←/⌘→ live in App.tsx rather than menu.rs). So the Edit menu's
// Undo becomes app-routed — and the app then owes every text field the
// undo it just took away.
//
// A focused rich-text editor registers its own history here. Plain inputs
// and textareas need no registration: WebKit still honours execCommand for
// them. Everything else falls through to the session history stack.

export interface TextUndo {
  undo: () => void;
  redo: () => void;
}

let active: TextUndo | null = null;

/** Called by RichEditor on focus (with its TipTap commands) and on blur
 *  (with null). Last focus wins — only one editor can hold the caret. */
export function registerTextUndo(handler: TextUndo | null): void {
  active = handler;
}

/** Stand down, but only if still the holder. An editor unmounting after
 *  focus already moved on must not clear the registration belonging to the
 *  editor that now has the caret. */
export function releaseTextUndo(handler: TextUndo): void {
  if (active === handler) active = null;
}


/** True when a text-editing context claimed the keystroke, meaning the
 *  session history must NOT also fire. */
export function claimTextUndo(redo: boolean): boolean {
  if (active) {
    if (redo) active.redo();
    else active.undo();
    return true;
  }
  const el = document.activeElement;
  const editable =
    el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
  if (!editable) return false;
  // Deprecated, but it is still the only route to a native input's own undo
  // stack in WKWebView — and that stack is the one the user has been
  // building by typing.
  document.execCommand(redo ? "redo" : "undo");
  return true;
}
