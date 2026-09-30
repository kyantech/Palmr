const SINK_URL = process.env.PALMR_E2E_SINK_URL ?? "http://127.0.0.1:8025";

interface SinkSummary {
  ID: string;
  Subject: string;
  To: { Address: string }[];
}

interface SinkMessage extends SinkSummary {
  Text: string;
  HTML: string;
}

async function sink<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${SINK_URL}${path}`, init);
  if (!response.ok) {
    throw new Error(
      `the SMTP sink answered ${String(response.status)} for ${path}`,
    );
  }
  return (await response.json()) as T;
}

export async function clearSink() {
  const response = await fetch(`${SINK_URL}/api/v1/messages`, {
    method: "DELETE",
  });
  if (!response.ok) {
    throw new Error(
      `the SMTP sink refused to clear: ${String(response.status)}`,
    );
  }
}

export async function messagesTo(address: string): Promise<SinkMessage[]> {
  const found = await sink<{ messages: SinkSummary[] }>(
    `/api/v1/search?query=${encodeURIComponent(`to:${address}`)}`,
  );
  return Promise.all(
    found.messages.map((message) =>
      sink<SinkMessage>(`/api/v1/message/${message.ID}`),
    ),
  );
}

export async function sinkMessageCount(): Promise<number> {
  const all = await sink<{ messages_count: number }>("/api/v1/messages");
  return all.messages_count;
}
