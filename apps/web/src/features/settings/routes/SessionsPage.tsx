import { SessionsList } from "../components/SessionsList";

export interface SessionsPageProps {
  onCurrentSessionEnded: () => Promise<void>;
}

export function SessionsPage({ onCurrentSessionEnded }: SessionsPageProps) {
  return <SessionsList onCurrentSessionEnded={onCurrentSessionEnded} />;
}
