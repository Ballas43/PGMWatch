import React from "react";
import { BrandLogo } from "./BrandLogo";

interface ErrorBoundaryState {
  hasError: boolean;
  message: string;
}

export class ErrorBoundary extends React.Component<React.PropsWithChildren, ErrorBoundaryState> {
  state: ErrorBoundaryState = { hasError: false, message: "" };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, message: error.message || "Unexpected rendering error" };
  }

  componentDidCatch(error: Error) {
    console.error("PGMWatch UI boundary caught error:", error);
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="flex min-h-screen items-center justify-center bg-[var(--bg-page)] p-6 text-[var(--text-primary)]">
          <div className="w-full max-w-md rounded-xl border border-[var(--border-strong)] bg-[var(--bg-card)] p-6">
            <div className="flex items-center gap-3">
              <BrandLogo size={36} variant="badge" />
              <h1 className="text-lg font-medium">PGMWatch Console Recovery</h1>
            </div>
            <p className="mt-3 text-sm text-[var(--text-secondary)]">{this.state.message}</p>
            <button
              type="button"
              onClick={() => this.setState({ hasError: false, message: "" })}
              className="mt-4 inline-flex h-9 items-center rounded-lg bg-[var(--text-primary)] px-4 text-sm font-medium text-[var(--bg-rail)]"
            >
              Reload Console
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
