import { Alert, Flex, theme, Typography } from "antd";
import { type DragEvent, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { detailChecks, ErrorAlert, isApiErrorCode } from "../../../shared/errors";
import {
  useDeleteProvider,
  useReorderProviders,
  useTestProvider,
  useUpdateProvider,
} from "../api/mutations";
import type { Provider } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { ConfirmDialog } from "./ConfirmDialog";
import type { CheckLine } from "./ProviderChecks";
import { ProviderRow } from "./ProviderRow";

interface ProviderListProps {
  providers: readonly Provider[];
  onEdit: (provider: Provider) => void;
}

type Notice = "deleted";

const VISUALLY_HIDDEN = {
  position: "absolute",
  width: 1,
  height: 1,
  margin: -1,
  padding: 0,
  overflow: "hidden",
  clip: "rect(0 0 0 0)",
  whiteSpace: "nowrap",
  border: 0,
} as const;

function applyOrder(providers: readonly Provider[], order: readonly string[] | null) {
  if (order === null) {
    return [...providers];
  }
  const byId = new Map(providers.map((provider) => [provider.id, provider]));
  const ordered = order.flatMap((id) => {
    const provider = byId.get(id);
    return provider === undefined ? [] : [provider];
  });
  const placed = new Set(ordered.map((provider) => provider.id));
  return [...ordered, ...providers.filter((provider) => !placed.has(provider.id))];
}

function without<Value>(record: Record<string, Value>, id: string): Record<string, Value> {
  return Object.fromEntries(Object.entries(record).filter(([key]) => key !== id));
}

function moved(ids: readonly string[], from: number, to: number): string[] {
  const next = [...ids];
  const [item] = next.splice(from, 1);
  if (item !== undefined) {
    next.splice(to, 0, item);
  }
  return next;
}

export function ProviderList({ providers, onEdit }: ProviderListProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const feedback = useActionFeedback<Notice>();
  const update = useUpdateProvider();
  const remove = useDeleteProvider();
  const reorder = useReorderProviders();
  const test = useTestProvider();
  const [pendingOrder, setPendingOrder] = useState<string[] | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const [over, setOver] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const [deleting, setDeleting] = useState<Provider | null>(null);
  const [results, setResults] = useState<Record<string, readonly CheckLine[]>>({});
  const [testFailures, setTestFailures] = useState<Record<string, unknown>>({});

  const ordered = useMemo(() => applyOrder(providers, pendingOrder), [providers, pendingOrder]);
  const busyId = update.isPending ? update.variables.id : null;
  const anyBusy = reorder.isPending || busyId !== null;

  function commitOrder(next: string[], provider: Provider) {
    if (reorder.isPending) {
      return;
    }
    const position = next.indexOf(provider.id) + 1;
    feedback.clear();
    setPendingOrder(next);
    setAnnouncement(
      t("providers.reorder.moved", { name: provider.displayName, position, total: next.length }),
    );
    reorder.mutate(next, {
      onError: (error) => {
        feedback.fail(error);
      },
      onSettled: () => {
        setPendingOrder(null);
      },
    });
  }

  function move(provider: Provider, delta: -1 | 1) {
    const ids = ordered.map((item) => item.id);
    const from = ids.indexOf(provider.id);
    const to = from + delta;
    if (from === -1 || to < 0 || to >= ids.length) {
      return;
    }
    commitOrder(moved(ids, from, to), provider);
  }

  function dragProps(provider: Provider) {
    return {
      dragging: dragging === provider.id,
      over: over === provider.id && dragging !== provider.id,
      onDragStart: (event: DragEvent<HTMLElement>) => {
        if (anyBusy) {
          event.preventDefault();
          return;
        }
        event.dataTransfer.effectAllowed = "move";
        event.dataTransfer.setData("text/plain", provider.id);
        const row = event.currentTarget.closest("li");
        if (row !== null) {
          event.dataTransfer.setDragImage(row, 16, 16);
        }
        setDragging(provider.id);
      },
      onDragOver: (event: DragEvent<HTMLElement>) => {
        if (dragging !== null) {
          event.preventDefault();
          event.dataTransfer.dropEffect = "move";
          setOver(provider.id);
        }
      },
      onDrop: (event: DragEvent<HTMLElement>) => {
        event.preventDefault();
        const source = dragging;
        setDragging(null);
        setOver(null);
        if (source === null || source === provider.id) {
          return;
        }
        const ids = ordered.map((item) => item.id);
        const from = ids.indexOf(source);
        const to = ids.indexOf(provider.id);
        const dropped = ordered.find((item) => item.id === source);
        if (from !== -1 && to !== -1 && dropped !== undefined) {
          commitOrder(moved(ids, from, to), dropped);
        }
      },
      onDragEnd: () => {
        setDragging(null);
        setOver(null);
      },
    };
  }

  function toggle(provider: Provider, enabled: boolean) {
    feedback.clear();
    update.mutate(
      { id: provider.id, body: { enabled } },
      {
        onError: (error) => {
          feedback.fail(error);
        },
      },
    );
  }

  function runTest(provider: Provider) {
    feedback.clear();
    setResults((current) => without(current, provider.id));
    setTestFailures((current) => without(current, provider.id));
    test.mutate(provider.id, {
      onSuccess: (result) => {
        setResults((current) => ({ ...current, [provider.id]: result.checks }));
      },
      onError: (error) => {
        if (isApiErrorCode(error, "PROVIDER_VALIDATION_FAILED")) {
          setResults((current) => ({ ...current, [provider.id]: detailChecks(error) }));
        } else {
          setTestFailures((current) => ({ ...current, [provider.id]: error }));
        }
      },
    });
  }

  function confirmDelete() {
    if (deleting === null || remove.isPending) {
      return;
    }
    feedback.clear();
    remove.mutate(deleting.id, {
      onSuccess: () => {
        feedback.succeed("deleted");
      },
      onError: (error) => {
        feedback.fail(error);
      },
      onSettled: () => {
        setDeleting(null);
      },
    });
  }

  return (
    <Flex vertical gap={token.marginSM}>
      <FeedbackAlerts feedback={feedback} noticeKey={() => "providers.delete.done"} />
      {Object.entries(testFailures).map(([id, error]) => (
        <TestFailure key={id} error={error} provider={providers.find((item) => item.id === id)} />
      ))}
      <ul
        aria-label={t("providers.list.label")}
        data-testid="provider-list"
        style={{ margin: 0, padding: 0 }}
      >
        {ordered.map((provider, index) => (
          <ProviderRow
            key={provider.id}
            provider={provider}
            position={index + 1}
            total={ordered.length}
            busy={anyBusy}
            testing={test.isPending && test.variables === provider.id}
            checks={results[provider.id] ?? null}
            drag={dragProps(provider)}
            onEdit={onEdit}
            onTest={runTest}
            onDelete={(selected) => {
              remove.reset();
              setDeleting(selected);
            }}
            onToggle={toggle}
            onMove={move}
          />
        ))}
      </ul>
      <span role="status" aria-live="polite" style={VISUALLY_HIDDEN}>
        {announcement}
      </span>
      <ConfirmDialog
        open={
          deleting !== null &&
          !(remove.isError && !isApiErrorCode(remove.error, "AUTH_RECENT_AUTH_REQUIRED"))
        }
        title={t("providers.delete.title", { name: deleting?.displayName ?? "" })}
        description={
          <Flex vertical gap={token.marginXS}>
            <Typography.Text>{t("providers.delete.description")}</Typography.Text>
            {deleting !== null && deleting.linkedUserCount > 0 ? (
              <Alert
                type="warning"
                showIcon
                title={t("providers.delete.linked", { total: deleting.linkedUserCount })}
              />
            ) : null}
          </Flex>
        }
        confirmLabel={t("providers.delete.confirm")}
        danger
        loading={remove.isPending}
        onConfirm={confirmDelete}
        onCancel={() => {
          if (!remove.isPending) {
            setDeleting(null);
          }
        }}
      />
    </Flex>
  );
}

function TestFailure({ error, provider }: { error: unknown; provider: Provider | undefined }) {
  const { t } = useTranslation("admin");
  return (
    <Flex vertical gap={4} data-testid="provider-test-failure">
      {provider === undefined ? null : (
        <Typography.Text type="secondary">
          {t("providers.checks.title", { name: provider.displayName })}
        </Typography.Text>
      )}
      <ErrorAlert error={error} />
    </Flex>
  );
}
