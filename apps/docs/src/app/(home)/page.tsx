"use client";

import type { ReactNode } from "react";
import Link from "next/link";
import {
  BatteryChargingIcon,
  BookOpenText,
  CloudIcon,
  DatabaseIcon,
  GithubIcon,
  KeyboardIcon,
  LayoutIcon,
  LockIcon,
  MousePointer,
  RocketIcon,
  SearchIcon,
  TimerIcon,
  UploadIcon,
  type LucideIcon,
} from "lucide-react";

import { AnimatedGridPattern } from "@/components/magicui/animated-grid-pattern";
import { PulsatingButton } from "@/components/magicui/pulsating-button";
import { RippleButton } from "@/components/magicui/ripple-button";
import { TypingAnimation } from "@/components/magicui/typing-animation";
import { WordRotate } from "@/components/magicui/word-rotate";
import { ThreeDMarquee } from "@/components/ui/3d-marquee";
import { TextHoverEffect } from "@/components/ui/text-hover-effect";

const images = [
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./dash_wt_kqtzxi.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./login_xtlnif.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./poase_wt_plhgwc.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./profile_wt_fnj3rz.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./settigngs_open_hjkomr.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./Screenshot_j0csjm.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./dash_cndhwr.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./profile_mizwvg.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./profile_wt_fnj3rz.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./poase_wt_plhgwc.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./dash_wt_kqtzxi.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./dash_cndhwr.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./login_xtlnif.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./poase_wt_plhgwc.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./settigngs_open_hjkomr.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./Screenshot_j0csjm.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./dash_cndhwr.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./login_xtlnif.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./profile_mizwvg.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./dash_wt_kqtzxi.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./profile_wt_fnj3rz.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./profile_mizwvg.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./dash_wt_kqtzxi.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./login_xtlnif.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./poase_wt_plhgwc.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./settigngs_open_hjkomr.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./Screenshot_j0csjm.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546004/Palmr./dash_cndhwr.png",
  "https://res.cloudinary.com/technical-intelligence/image/upload/v1745546005/Palmr./profile_mizwvg.png",
];

const docsLink = "/docs/v3-beta";

function Hero() {
  return (
    <section className="relative z-[2] flex flex-col border-x border-t  px-6 pt-12 pb-10 md:px-12 md:pt-16 max-md:text-center">
      <h1 className="mb-8 text-6xl font-bold">
        Palmr. <span className="text-[13px] font-light text-muted-foreground/50 font-mono">v3-beta</span>
      </h1>
      <h1 className="hidden text-4xl font-medium max-w-[600px] md:block mb-4">Modern & efficient file sharing</h1>
      <p className="mb-8 text-fd-muted-foreground md:max-w-[80%] md:text-xl">
        Palmr is a fast and secure platform for sharing files, built with performance and privacy in mind.
      </p>
      <div className="hidden h-[10rem] lg:flex items-center justify-center absolute right-0 top-10">
        <TextHoverEffect text="Palmr." />
      </div>
      <div className="inline-flex items-center gap-6 max-md:mx-auto mb-4">
        <PulsatingButton>
          <div className="flex gap-2 items-center">
            <BookOpenText size={18} />
            <Link href={docsLink}>Documentation</Link>
          </div>
        </PulsatingButton>
        <RippleButton>
          <a
            href="https://github.com/kyantech/Palmr"
            target="_blank"
            rel="noreferrer noopener"
            className="flex gap-2 items-center"
          >
            <GithubIcon size={18} />
            GitHub
          </a>
        </RippleButton>
      </div>
    </section>
  );
}

function LogoShowcase() {
  return (
    <div className="z-[2] border-x bg-background">
      <ThreeDMarquee images={images} className="rounded-none" />
    </div>
  );
}

function Feedback() {
  return (
    <section className="relative flex flex-col items-center overflow-hidden border-x border-t px-6 py-8 md:py-16">
      <p className="text-xl font-medium flex items-center justify-center gap-2">
        A modern way to share files
        <WordRotate
          duration={4000}
          words={["efficiently", "securely", "privately", "reliably", "seamlessly"]}
          className="min-w-[100px] inline-block"
        />
      </p>
    </section>
  );
}

function Highlight({ icon: Icon, heading, children }: { icon: LucideIcon; heading: ReactNode; children: ReactNode }) {
  return (
    <div className="border-l border-t px-6 py-12">
      <div className="mb-4 flex items-center gap-2 text-fd-muted-foreground">
        <Icon className="size-6" />
        <h2 className="text-sm font-medium">{heading}</h2>
      </div>
      <span className="font-medium">{children}</span>
    </div>
  );
}

function Features() {
  return (
    <>
      {/* Core Features */}
      <section className="grid grid-cols-1 border-t border-x md:grid-cols-2">
        <div className="flex flex-col gap-4 border-r p-8 md:p-12">
          <div className="flex gap-4 items-center">
            <div className="flex items-center gap-3 text-muted-foreground border border-foreground w-fit p-3 rounded-lg">
              <UploadIcon className="size-6 text-foreground" />
            </div>
            <h3 className="text-2xl font-semibold">Upload & Share</h3>
          </div>
          <p className="text-muted-foreground">
            Send your files quickly and safely. Share easily with anyone through secure links.
          </p>
        </div>
        <div className="flex flex-col gap-4 border-r p-8 md:p-12">
          <div className="flex gap-4 items-center">
            <div className="flex items-center gap-3 text-muted-foreground border border-foreground w-fit p-3 rounded-lg">
              <LockIcon className="size-6 text-foreground" />
            </div>
            <h3 className="text-2xl font-semibold">Secure & Private</h3>
          </div>
          <p className="text-muted-foreground">Files are encrypted and protected. You control your data completely.</p>
        </div>
      </section>

      {/* Hero Section with Animation */}
      <section
        className="relative overflow-hidden border-x border-t px-8 py-16 sm:py-24"
        style={{
          backgroundImage:
            "radial-gradient(circle at center, var(--color-fd-secondary), var(--color-fd-background) 40%)",
        }}
      >
        <div className="text-center">
          <p className="mb-4 w-fit bg-fd-primary px-3 py-1 text-sm font-bold font-mono text-fd-primary-foreground mx-auto">
            Open Source & Self-Hosted
          </p>
          <h2 className="text-center text-2xl font-semibold sm:text-3xl mb-4">Complete File Sharing Solution</h2>
          <TypingAnimation className="text-center text-xl text-muted-foreground">
            Built with Next.js, Fastify, and SQLite
          </TypingAnimation>
        </div>
        <AnimatedGridPattern className="opacity-5" />
      </section>

      {/* Technical Features Grid */}
      <section className="grid grid-cols-1 border-r md:grid-cols-2 lg:grid-cols-3">
        <div className="col-span-full flex items-start justify-center border-l border-t p-8 pb-2 text-center">
          <h2 className="bg-fd-primary px-1 text-2xl font-semibold text-fd-primary-foreground">Key Features</h2>
          <MousePointer className="-ml-1 mt-8" />
        </div>

        <Highlight icon={TimerIcon} heading="Lightning Fast">
          Optimized upload/download speeds with modern architecture
        </Highlight>

        <Highlight icon={CloudIcon} heading="Flexible Storage">
          Local filesystem or S3-compatible storage options
        </Highlight>

        <Highlight icon={KeyboardIcon} heading="Developer API">
          Full REST API with webhooks for seamless integration
        </Highlight>

        <Highlight icon={SearchIcon} heading="Smart Search">
          Find and manage your shared files effortlessly
        </Highlight>

        <Highlight icon={LayoutIcon} heading="Modern UI">
          Clean, intuitive interface built with best practices
        </Highlight>

        <Highlight icon={DatabaseIcon} heading="SQLite Powered">
          Lightweight, reliable database for efficient data handling
        </Highlight>
      </section>
    </>
  );
}

function GetStarted() {
  return (
    <section className="flex w-full flex-1">
      <div className="w-full flex flex-col gap-8 overflow-hidden border px-8 py-14">
        <div className="text-center mb-6">
          <h2 className="text-4xl font-extrabold font-mono uppercase mb-3">Get Started Today</h2>
          <p className="text-lg text-muted-foreground max-w-2xl mx-auto">
            Deploy your own secure file sharing platform in minutes. Take control of your data with our self-hosted
            solution.
          </p>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-8 mt-8">
          <div className="text-center space-y-4">
            <div className="flex items-center justify-center">
              <div className="flex items-center gap-3 text-muted-foreground border border-foreground w-fit p-4 rounded-full">
                <TimerIcon className="size-8 text-foreground" />
              </div>
            </div>
            <h3 className="text-xl font-semibold">Quick Setup</h3>
            <p className="text-muted-foreground">
              Docker deployment or direct installation - get running in under 5 minutes
            </p>
          </div>

          <div className="text-center space-y-4">
            <div className="flex items-center justify-center">
              <div className="flex items-center gap-3 text-muted-foreground border border-foreground w-fit p-4 rounded-full">
                <BatteryChargingIcon className="size-8 text-foreground" />
              </div>
            </div>
            <h3 className="text-xl font-semibold">Full Control</h3>
            <p className="text-muted-foreground">
              Self-hosted means you own your data and control every aspect of the platform
            </p>
          </div>

          <div className="text-center space-y-4">
            <div className="flex items-center justify-center">
              <div className="flex items-center gap-3 text-muted-foreground border border-foreground w-fit p-4 rounded-full">
                <RocketIcon className="size-8 text-foreground" />
              </div>
            </div>
            <h3 className="text-xl font-semibold">Production Ready</h3>
            <p className="text-muted-foreground">Latest technologies optimized for performance and security</p>
          </div>
        </div>

        <div className="border-t pt-8 mt-12">
          <div className="flex flex-col sm:flex-row items-center justify-center gap-4">
            <PulsatingButton>
              <div className="flex gap-2 items-center">
                <BookOpenText size={18} />
                <Link href={docsLink}>Read documentation</Link>
              </div>
            </PulsatingButton>
            <RippleButton>
              <a
                href="https://github.com/kyantech/Palmr"
                target="_blank"
                rel="noreferrer noopener"
                className="flex gap-2 items-center"
              >
                <GithubIcon size={18} />
                View on GitHub
              </a>
            </RippleButton>
          </div>
        </div>
      </div>
    </section>
  );
}

function FullWidthFooter() {
  return (
    <footer className="w-full flex items-center justify-center p-6 border-t font-light container max-w-7xl">
      <div className="flex items-center gap-1 text-sm max-w-7xl">
        <span>Powered by</span>
        <Link
          href="https://github.com/kyantech"
          rel="noopener noreferrer"
          target="_blank"
          className="flex items-center hover:text-green-700 text-green-500 transition-colors font-light"
        >
          Kyantech Solutions ©
        </Link>
      </div>
    </footer>
  );
}

export default function HomePage() {
  return (
    <>
      <main className="relative z-[2] w-full px-4 py-6 sm:px-6 lg:px-8">
        <div className="relative mx-auto max-w-screen-xl bg-background">
          <Hero />
          <LogoShowcase />
          <Feedback />
          <Features />
          <GetStarted />
        </div>
      </main>
      <FullWidthFooter />
    </>
  );
}
