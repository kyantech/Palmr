import { Component, type ReactNode } from "react";
import { isStaleChunkError, type StaleChunkOutcome, type StaleChunkRecovery } from "./staleChunk";

interface ErrorRecoveryProps {
  error: unknown;
  recovery: StaleChunkRecovery;
  fallback: ReactNode;
}

interface ErrorRecoveryState {
  outcome: StaleChunkOutcome | "pending";
}

export class ErrorRecovery extends Component<ErrorRecoveryProps, ErrorRecoveryState> {
  override state: ErrorRecoveryState = { outcome: pendingOutcome(this.props.error) };

  override componentDidMount() {
    this.recover();
  }

  override componentDidUpdate(previous: ErrorRecoveryProps) {
    if (previous.error !== this.props.error) {
      this.recover();
    }
  }

  private recover() {
    const outcome = this.props.recovery.recover(this.props.error);
    if (outcome !== this.state.outcome) {
      this.setState({ outcome });
    }
  }

  override render() {
    const { outcome } = this.state;
    return outcome === "pending" || outcome === "reloading" ? null : this.props.fallback;
  }
}

function pendingOutcome(error: unknown): ErrorRecoveryState["outcome"] {
  return isStaleChunkError(error) ? "pending" : "notStale";
}
