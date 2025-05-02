"use client";

import { zodResolver } from "@hookform/resolvers/zod";
import { useTranslations } from "next-intl";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import { z } from "zod";

import { Button } from "@/components/ui/button";
import { Form, FormControl, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { useAppInfo } from "@/contexts/app-info-context";
import { registerUser, updateConfig } from "@/http/endpoints";
import { PasswordVisibilityToggle } from "./password-visibility-toggle";

interface RegisterFormProps {
  isVisible: boolean;
  onToggleVisibility: () => void;
}

export function RegisterForm({ isVisible, onToggleVisibility }: RegisterFormProps) {
  const t = useTranslations();
  const { refreshAppInfo } = useAppInfo();

  const registerSchema = z.object({
    firstName: z.string().min(1, t("register.validation.firstNameRequired")),
    lastName: z.string().min(1, t("register.validation.lastNameRequired")),
    username: z.string().min(3, t("register.validation.usernameMinLength")),
    email: z.string().email(t("register.validation.invalidEmail")),
    password: z.string().min(6, t("register.validation.passwordMinLength")),
  });

  const form = useForm<z.infer<typeof registerSchema>>({
    resolver: zodResolver(registerSchema),
    defaultValues: {
      firstName: "",
      lastName: "",
      username: "",
      email: "",
      password: "",
    },
  });

  const onSubmit = async (data: z.infer<typeof registerSchema>) => {
    try {
      await registerUser({
        ...data,
      });

      await updateConfig("firstUserAccess", {
        value: "false",
      });

      await refreshAppInfo();
      toast.success("Usuário administrador criado com sucesso!");
    } catch (error) {
      console.error(error);
      toast.error("Erro ao criar usuário administrador");
    }
  };

  return (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
        <FormField
          control={form.control}
          name="firstName"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.firstName")}</FormLabel>
              <FormControl>
                <Input {...field} className="bg-transparent backdrop-blur-md" />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="lastName"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.lastName")}</FormLabel>
              <FormControl>
                <Input {...field} className="bg-transparent backdrop-blur-md" />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="username"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.username")}</FormLabel>
              <FormControl>
                <Input {...field} className="bg-transparent backdrop-blur-md" />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="email"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.email")}</FormLabel>
              <FormControl>
                <Input {...field} type="email" className="bg-transparent backdrop-blur-md" />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="password"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.password")}</FormLabel>
              <FormControl>
                <div className="relative">
                  <Input
                    {...field}
                    type={isVisible ? "text" : "password"}
                    className="bg-transparent backdrop-blur-md pr-10"
                  />
                  <PasswordVisibilityToggle isVisible={isVisible} onToggle={onToggleVisibility} />
                </div>
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <Button className="w-full mt-4" type="submit" disabled={form.formState.isSubmitting}>
          {form.formState.isSubmitting ? t("register.buttons.creating") : t("register.buttons.createAdmin")}
        </Button>
      </form>
    </Form>
  );
}
