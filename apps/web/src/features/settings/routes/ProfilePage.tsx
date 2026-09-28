import { Skeleton } from "antd";
import { ErrorAlert } from "../../../shared/errors";
import { useProfile } from "../api/queries";
import { ProfileForm } from "../components/ProfileForm";

export function ProfilePage() {
  const profile = useProfile();
  if (profile.data !== undefined) {
    return <ProfileForm key={profile.data.id} profile={profile.data} />;
  }
  return profile.isError ? <ErrorAlert error={profile.error} /> : <Skeleton active />;
}
