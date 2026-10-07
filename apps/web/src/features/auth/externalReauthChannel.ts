export const EXTERNAL_REAUTH_CHANNEL_PREFIX = "palmr:external-reauth:";

const CHANNEL_ID = /^[A-Za-z0-9_-]{43}$/;

export function isExternalReauthChannelId(value: unknown): value is string {
  return typeof value === "string" && CHANNEL_ID.test(value);
}

export function externalReauthChannelName(channelId: string): string {
  return `${EXTERNAL_REAUTH_CHANNEL_PREFIX}${channelId}`;
}

export function openExternalReauthChannel(channelId: string): BroadcastChannel {
  return new BroadcastChannel(externalReauthChannelName(channelId));
}
