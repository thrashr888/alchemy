/// <reference types="vite/client" />

// Boot targets injected by the new_window command's init script.
interface Window {
  __ALCHEMY_NOTEBOOK__?: string;
  __ALCHEMY_FRESH__?: boolean;
}
