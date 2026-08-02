import { Component, useEffect, useState, type ErrorInfo, type ReactNode } from "react";

interface BoundaryProps {
  children: ReactNode;
  openGrid: () => void;
}

interface BoundaryState {
  failed: boolean;
}

export class ArenaBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { failed: false };

  static getDerivedStateFromError(): BoundaryState {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Rampage 3D Arena failed safely", error, info.componentStack);
  }

  render() {
    if (this.state.failed) {
      return (
        <div className="arena-unavailable" role="alert">
          <strong>The 3D Arena could not start.</strong>
          <span>Your fabric is still active; use the accessible grid while Rampage records this graphics failure.</span>
          <button type="button" onClick={this.props.openGrid}>Open Ops Grid</button>
        </div>
      );
    }
    return this.props.children;
  }
}

export function ArenaLoading({ openGrid }: { openGrid: () => void }) {
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    const timeout = window.setTimeout(() => setSlow(true), 4_000);
    return () => window.clearTimeout(timeout);
  }, []);
  return (
    <div className="arena-loading" role="status">
      <strong>Initializing spatial fabric…</strong>
      {slow && <>
        <span>The 3D bundle is taking longer than expected. Fabric services are unaffected.</span>
        <button type="button" onClick={openGrid}>Open Ops Grid</button>
      </>}
    </div>
  );
}
