import { Alert, Flex } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";
import { reportable } from "./feedback";

export interface ActionFeedback<Notice extends string> {
  failure: unknown;
  notice: Notice | null;
  clear: () => void;
  fail: (error: unknown) => void;
  succeed: (notice: Notice) => void;
}

export function useActionFeedback<Notice extends string>(): ActionFeedback<Notice> {
  const [failure, setFailure] = useState<unknown>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  return {
    failure,
    notice,
    clear: () => {
      setFailure(null);
      setNotice(null);
    },
    fail: (error) => {
      setNotice(null);
      setFailure(reportable(error));
    },
    succeed: (next) => {
      setFailure(null);
      setNotice(next);
    },
  };
}

interface FeedbackAlertsProps<Notice extends string> {
  feedback: ActionFeedback<Notice>;
  noticeKey: (notice: Notice) => string;
}

export function FeedbackAlerts<Notice extends string>({
  feedback,
  noticeKey,
}: FeedbackAlertsProps<Notice>) {
  const { t } = useTranslation("admin");
  if (feedback.failure === null && feedback.notice === null) {
    return null;
  }
  return (
    <Flex vertical gap={8}>
      {feedback.failure === null ? null : <ErrorAlert error={feedback.failure} />}
      {feedback.notice === null ? null : (
        <Alert type="success" showIcon role="status" title={t(noticeKey(feedback.notice))} />
      )}
    </Flex>
  );
}
