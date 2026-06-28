import { Component, type ErrorInfo, type ReactNode } from "react";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { isTransientModuleLoadError } from "../lazyImport";

type WorkspaceErrorBoundaryProps = {
  children: ReactNode;
  resetKey: string;
  subpageLabel: string;
  viewLabel: string;
};

type WorkspaceErrorBoundaryState = {
  error: Error | null;
};

export class WorkspaceErrorBoundary extends Component<
  WorkspaceErrorBoundaryProps,
  WorkspaceErrorBoundaryState
> {
  state: WorkspaceErrorBoundaryState = {
    error: null,
  };

  static getDerivedStateFromError(
    error: unknown,
  ): WorkspaceErrorBoundaryState {
    return {
      error: error instanceof Error ? error : new Error(String(error)),
    };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error("Workspace route failed to render", error, errorInfo);
  }

  componentDidUpdate(previousProps: WorkspaceErrorBoundaryProps) {
    if (
      previousProps.resetKey !== this.props.resetKey &&
      this.state.error !== null
    ) {
      this.setState({ error: null });
    }
  }

  private reloadWorkspace = () => {
    window.location.reload();
  };

  render() {
    if (this.state.error === null) {
      return this.props.children;
    }

    const transient = isTransientModuleLoadError(this.state.error);
    const detail = transient
      ? "A workspace code chunk could not be fetched. Navigation and session controls are still available; reload the console to request the route files again."
      : "The workspace failed before it could render. Navigation and session controls are still available; reload the console after checking the browser console or API state.";

    return (
      <section className="workspace singleColumn workspaceRouteError">
        <div
          aria-labelledby="workspace-route-error-title"
          className="workspaceErrorPanel"
          role="alert"
        >
          <div className="workspaceErrorIcon" aria-hidden="true">
            <AlertTriangle size={20} />
          </div>
          <div className="workspaceErrorCopy">
            <h2 id="workspace-route-error-title">Workspace did not load</h2>
            <p>
              {this.props.viewLabel} / {this.props.subpageLabel}
            </p>
            <span>{detail}</span>
            <code>{this.state.error.message}</code>
          </div>
          <div className="emptyStateActions workspaceErrorActions">
            <button
              className="primaryAction"
              onClick={this.reloadWorkspace}
              title="Reload the console and request the current workspace files again."
              type="button"
            >
              <RefreshCw size={16} />
              Reload console
            </button>
          </div>
        </div>
      </section>
    );
  }
}
