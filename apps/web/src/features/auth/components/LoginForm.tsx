import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, Input, theme } from "antd";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ErrorAlert, presentError } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { type LoginRequest, useLogin } from "../api/mutations";

interface LoginFormProps {
  onSignedIn: () => Promise<void>;
}

export function LoginForm({ onSignedIn }: LoginFormProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const identifierId = `${idPrefix}-identifier`;
  const passwordId = `${idPrefix}-password`;
  const inFlight = useRef(false);
  const [failure, setFailure] = useState<unknown>(null);
  const [retryBlock, setRetryBlock] = useState<{ seconds: number } | null>(null);
  const login = useLogin();
  const schema = useMemo(
    () =>
      z.object({
        identifier: z.string().trim().min(1, t("login.identifierRequired")),
        password: z.string().min(1, t("login.passwordRequired")),
      }),
    [t],
  );
  const {
    control,
    handleSubmit,
    resetField,
    setFocus,
    formState: { errors, isSubmitting },
  } = useForm<LoginRequest>({
    resolver: zodResolver(schema),
    defaultValues: { identifier: "", password: "" },
  });

  useEffect(() => {
    setFocus("identifier");
  }, [setFocus]);

  useEffect(() => {
    if (retryBlock === null) {
      return;
    }
    const timer = window.setTimeout(() => {
      setRetryBlock(null);
    }, retryBlock.seconds * 1_000);
    return () => {
      window.clearTimeout(timer);
    };
  }, [retryBlock]);

  async function submit(values: LoginRequest) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      try {
        await login.mutateAsync(values);
      } catch (error) {
        const presented = presentError(error);
        if (presented.presentation.silent) {
          return;
        }
        setFailure(error);
        if (
          (presented.code === "RATE_LIMITED" || presented.code === "CLIENT_RATE_LIMITED") &&
          presented.retryAfterSeconds !== null &&
          presented.retryAfterSeconds > 0
        ) {
          setRetryBlock({ seconds: presented.retryAfterSeconds });
        }
        resetField("password");
        setFocus("password");
        return;
      }
      await onSignedIn();
    } catch (error) {
      setFailure(error);
    } finally {
      inFlight.current = false;
    }
  }

  return (
    <form
      noValidate
      aria-busy={isSubmitting}
      onSubmit={(event) => {
        if (retryBlock === null) {
          void handleSubmit(submit)(event);
        } else {
          event.preventDefault();
        }
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false}>
        {failure === null ? null : (
          <div style={{ marginBottom: token.marginLG }} data-testid="login-error">
            <ErrorAlert error={failure} />
          </div>
        )}
        <FormField
          id={identifierId}
          label={t("login.identifier")}
          error={errors.identifier?.message}
        >
          {(aria) => (
            <Controller
              name="identifier"
              control={control}
              render={({ field }) => (
                <Input
                  {...field}
                  {...aria}
                  size="large"
                  autoComplete="username"
                  autoCapitalize="none"
                  spellCheck={false}
                  readOnly={isSubmitting}
                />
              )}
            />
          )}
        </FormField>
        <FormField id={passwordId} label={t("login.password")} error={errors.password?.message}>
          {(aria) => (
            <Controller
              name="password"
              control={control}
              render={({ field }) => (
                <Input.Password
                  {...field}
                  {...aria}
                  size="large"
                  autoComplete="current-password"
                  readOnly={isSubmitting}
                />
              )}
            />
          )}
        </FormField>
        <Flex vertical style={{ marginTop: token.marginXS }}>
          <Button
            type="primary"
            htmlType="submit"
            size="large"
            block
            loading={isSubmitting}
            disabled={retryBlock !== null}
          >
            {t("login.submit")}
          </Button>
        </Flex>
      </Form>
    </form>
  );
}
