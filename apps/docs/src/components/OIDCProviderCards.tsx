import { Chrome, Egg, Github, Key, Lock, MessageSquare, Settings, Shield, Users } from "lucide-react";

import { Card, CardGrid } from "@/components/ui/card";

const providers = [
  {
    name: "Google",
    description: "Configure authentication using Google OAuth2 services",
    href: "/docs/3.1-beta/oidc-authentication/google",
    icon: <Chrome className="w-4 h-4" />,
  },
  {
    name: "Discord",
    description: "Set up Discord OAuth2 for community-based authentication",
    href: "/docs/3.1-beta/oidc-authentication/discord",
    icon: <MessageSquare className="w-4 h-4" />,
  },
  {
    name: "GitHub",
    description: "Enable GitHub OAuth for developer-friendly sign-in",
    href: "/docs/3.1-beta/oidc-authentication/github",
    icon: <Github className="w-4 h-4" />,
  },
  {
    name: "Zitadel",
    description: "Enterprise-grade identity and access management",
    href: "/docs/3.1-beta/oidc-authentication/zitadel",
    icon: <Shield className="w-4 h-4" />,
  },
  {
    name: "Auth0",
    description: "Flexible identity platform with extensive customization",
    href: "/docs/3.1-beta/oidc-authentication/auth0",
    icon: <Lock className="w-4 h-4" />,
  },
  {
    name: "Authentik",
    description: "Open-source identity provider with modern features",
    href: "/docs/3.1-beta/oidc-authentication/authentik",
    icon: <Key className="w-4 h-4" />,
  },
  {
    name: "Frontegg",
    description: "User management platform for B2B applications",
    href: "/docs/3.1-beta/oidc-authentication/frontegg",
    icon: <Egg className="w-4 h-4" />,
  },
  {
    name: "Kinde Auth",
    description: "Developer-first authentication and user management",
    href: "/docs/3.1-beta/oidc-authentication/kinde-auth",
    icon: <Users className="w-4 h-" />,
  },
  {
    name: "Pocket ID",
    description: "Open-source identity provider with OIDC support",
    href: "/docs/3.1-beta/oidc-authentication/pocket-id",
    icon: <Key className="w-4 h-4" />,
  },
  {
    name: "Other",
    description: "Configure any other OIDC-compliant identity provider",
    href: "/docs/3.1-beta/oidc-authentication/other",
    icon: <Settings className="w-4 h-4" />,
  },
];

export const OIDCProviderCards = () => {
  return (
    <CardGrid>
      {providers.map((provider) => (
        <Card
          key={provider.name}
          title={provider.name}
          description={provider.description}
          href={provider.href}
          icon={provider.icon}
        />
      ))}
    </CardGrid>
  );
};
