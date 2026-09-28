import { RecentAuthModal } from "../../features/auth";
import { useBootState } from "../bootstrap/bootState";

export function RecentAuthHost() {
  const { me } = useBootState();
  return me === null ? null : <RecentAuthModal me={me} />;
}
