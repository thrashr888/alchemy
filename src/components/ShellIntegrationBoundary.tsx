import { Component, type ErrorInfo, type ReactNode } from "react";
import { useStore } from "@/lib/store";

/**
 * Keeps optional desktop-shell integrations from taking down the research UI.
 * These components have no visual fallback; the toast explains that only the
 * affected OS integration is unavailable.
 */
export class ShellIntegrationBoundary extends Component<
  { children: ReactNode; name: string },
  { failed: boolean }
> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`${this.props.name} integration failed`, error, info);
    useStore
      .getState()
      .pushToast("error", `${this.props.name} is unavailable. Restart Alchemy to retry.`);
  }

  render() {
    return this.state.failed ? null : this.props.children;
  }
}
