import { externalReauthChannelName } from "../features/auth/externalReauthChannel";

export const CHANNEL_ID = "AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_s";
export const OTHER_CHANNEL_ID = "BwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_s";

export interface ChannelListener {
  messages: unknown[];
  close: () => void;
}

export function listenOnChannel(channelId: string): ChannelListener {
  const channel = new BroadcastChannel(externalReauthChannelName(channelId));
  const messages: unknown[] = [];
  channel.addEventListener("message", (event: MessageEvent<unknown>) => {
    messages.push(event.data);
  });
  return {
    messages,
    close: () => {
      channel.close();
    },
  };
}

export function broadcastOnChannel(channelId: string, data: unknown) {
  const channel = new BroadcastChannel(externalReauthChannelName(channelId));
  channel.postMessage(data);
  channel.close();
}
