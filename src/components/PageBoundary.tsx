import { Component, type ReactNode } from "react";
import { Button, EmptyState } from "./ui";

interface State {
  error: Error | null;
}

/**
 * Keeps a page that fails to draw from taking the whole window with it. Projects are saved by the core, not by the
 * page, so "Try again" draws the page again from what is saved.
 */
export class PageBoundary extends Component<{ children: ReactNode; name: string }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="flex flex-1 items-center justify-center p-8">
        <EmptyState
          icon={null}
          title={`${this.props.name} couldn't be shown`}
          action={
            <Button onClick={() => this.setState({ error: null })} variant="primary">
              Try again
            </Button>
          }
        >
          Something went wrong drawing this page. Your projects are saved and unchanged. {this.state.error.message}
        </EmptyState>
      </div>
    );
  }
}
