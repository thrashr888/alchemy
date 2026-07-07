/// <reference types="vite/client" />

// Boot targets injected by the new_window command's init script.
interface Window {
  __ALCHEMY_NOTEBOOK__?: string;
  /** Render this window as a single-note reader (notebook id also set). */
  __ALCHEMY_NOTE__?: string;
  __ALCHEMY_FRESH__?: boolean;
}
