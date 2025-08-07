"use client";

import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, Search, X } from "lucide-react";
import { useTranslations } from "next-intl";
import * as AiIcons from "react-icons/ai";
import * as BiIcons from "react-icons/bi";
import * as BsIcons from "react-icons/bs";
import * as CgIcons from "react-icons/cg";
import * as CiIcons from "react-icons/ci";
import * as DiIcons from "react-icons/di";
import * as FaIcons from "react-icons/fa";
import * as Fa6Icons from "react-icons/fa6";
import * as FcIcons from "react-icons/fc";
import * as FiIcons from "react-icons/fi";
import * as GiIcons from "react-icons/gi";
import * as GoIcons from "react-icons/go";
import * as GrIcons from "react-icons/gr";
import * as HiIcons from "react-icons/hi";
import * as Hi2Icons from "react-icons/hi2";
import * as ImIcons from "react-icons/im";
import * as IoIcons from "react-icons/io";
import * as Io5Icons from "react-icons/io5";
import * as LiaIcons from "react-icons/lia";
import * as LuIcons from "react-icons/lu";
import * as MdIcons from "react-icons/md";
import * as PiIcons from "react-icons/pi";
import * as RiIcons from "react-icons/ri";
import * as RxIcons from "react-icons/rx";
import * as SiIcons from "react-icons/si";
import * as SlIcons from "react-icons/sl";
import * as TbIcons from "react-icons/tb";
import * as TfiIcons from "react-icons/tfi";
import * as TiIcons from "react-icons/ti";
import * as VscIcons from "react-icons/vsc";
import * as WiIcons from "react-icons/wi";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

const customStyles = `
  .grid-cols-16 {
    grid-template-columns: repeat(16, minmax(0, 1fr));
  }
  .grid-cols-20 {
    grid-template-columns: repeat(20, minmax(0, 1fr));
  }
`;

if (typeof document !== "undefined") {
  const styleElement = document.createElement("iconPicker.style");
  styleElement.textContent = customStyles;
  if (!document.head.querySelector("style[data-icon-picker]")) {
    styleElement.setAttribute("data-icon-picker", "true");
    document.head.appendChild(styleElement);
  }
}

interface IconData {
  name: string;
  component: React.ComponentType<{ className?: string }>;
  category: string;
}

interface IconPickerProps {
  value?: string;
  onChange: (iconName: string) => void;
  placeholder?: string;
}

const ICONS_PER_BATCH = 100;
const SCROLL_THRESHOLD = 200;

interface VirtualizedIconGridProps {
  icons: IconData[];
  onIconSelect: (iconName: string) => void;
  renderIcon: (icon: IconData, className?: string, size?: number) => React.ReactNode;
  showCategories?: boolean;
}

function VirtualizedIconGrid({ icons, onIconSelect, renderIcon, showCategories = false }: VirtualizedIconGridProps) {
  const t = useTranslations();
  const [visibleCount, setVisibleCount] = useState(ICONS_PER_BATCH);
  const [isLoading, setIsLoading] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const sentinelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel) return;

    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[0];
        if (entry.isIntersecting && visibleCount < icons.length && !isLoading) {
          setIsLoading(true);
          setTimeout(() => {
            setVisibleCount((prev) => Math.min(prev + ICONS_PER_BATCH, icons.length));
            setIsLoading(false);
          }, 100);
        }
      },
      {
        root: scrollRef.current,
        rootMargin: `${SCROLL_THRESHOLD}px`,
        threshold: 0.1,
      }
    );

    observer.observe(sentinel);

    return () => {
      observer.unobserve(sentinel);
    };
  }, [visibleCount, icons.length, isLoading]);

  useEffect(() => {
    setVisibleCount(ICONS_PER_BATCH);
  }, [icons]);

  useEffect(() => {
    setVisibleCount(ICONS_PER_BATCH);
  }, [showCategories]);

  const visibleIcons = useMemo(() => icons.slice(0, visibleCount), [icons, visibleCount]);

  const iconsByCategory = useMemo(() => {
    if (!showCategories) return [];
    const grouped = new Map<string, IconData[]>();
    visibleIcons.forEach((icon) => {
      if (!grouped.has(icon.category)) {
        grouped.set(icon.category, []);
      }
      grouped.get(icon.category)!.push(icon);
    });
    return Array.from(grouped.entries()).sort(([a], [b]) => a.localeCompare(b));
  }, [visibleIcons, showCategories]);

  if (showCategories) {
    return (
      <div ref={scrollRef} className="max-h-[600px] overflow-y-auto overflow-x-hidden pr-2">
        <div className="space-y-6">
          {iconsByCategory.map(([category, categoryIcons]) => (
            <div key={category}>
              <Badge variant="secondary" className="text-xs mb-3">
                {t("iconPicker.categoryBadge", {
                  category,
                  count: icons.filter((icon) => icon.category === category).length,
                })}
              </Badge>
              <div className="grid grid-cols-8 sm:grid-cols-12 lg:grid-cols-16 xl:grid-cols-20 gap-2 sm:gap-3">
                {categoryIcons.map((icon) => (
                  <button
                    key={`${icon.category}-${icon.name}`}
                    className="h-12 w-12 sm:h-14 sm:w-14 p-0 hover:bg-muted transition-colors flex-shrink-0 rounded-md flex items-center justify-center cursor-pointer"
                    onClick={() => onIconSelect(icon.name)}
                    title={icon.name}
                  >
                    {renderIcon(icon, "", 32)}
                  </button>
                ))}
              </div>
            </div>
          ))}

          {/* Loading indicator and sentinel */}
          <div ref={sentinelRef} className="flex justify-center py-4">
            {isLoading && <div className="text-sm text-muted-foreground">{t("iconPicker.loadingMore")}</div>}
            {visibleCount >= icons.length && icons.length > 0 && (
              <div className="text-sm text-muted-foreground">
                {t("iconPicker.allIconsLoaded", { count: icons.length.toLocaleString() })}
              </div>
            )}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div ref={scrollRef} className="max-h-[600px] overflow-y-auto overflow-x-hidden pr-2">
      <div className="grid grid-cols-8 sm:grid-cols-12 lg:grid-cols-16 xl:grid-cols-20 gap-2 sm:gap-3">
        {visibleIcons.map((icon) => (
          <button
            key={`${icon.category}-${icon.name}`}
            className="h-12 w-12 sm:h-14 sm:w-14 p-0 hover:bg-muted transition-colors flex-shrink-0 rounded-md flex items-center justify-center cursor-pointer"
            onClick={() => onIconSelect(icon.name)}
            title={`${icon.name} (${icon.category})`}
          >
            {renderIcon(icon, "", 32)}
          </button>
        ))}
      </div>

      <div ref={sentinelRef} className="flex justify-center py-4">
        {isLoading && <div className="text-sm text-muted-foreground">{t("iconPicker.loadingMore")}</div>}
        {visibleCount >= icons.length && icons.length > 0 && (
          <div className="text-sm text-muted-foreground">
            {t("iconPicker.allIconsLoaded", { count: icons.length.toLocaleString() })}
          </div>
        )}
      </div>
    </div>
  );
}

const POPULAR_ICONS = [
  "FaGoogle",
  "FaGithub",
  "FaMicrosoft",
  "FaApple",
  "FaAmazon",
  "FaFacebook",
  "FaTwitter",
  "FaLinkedin",
  "FaDiscord",
  "FaSlack",
  "FaLock",
  "FaShield",
  "FaKey",
  "FaUser",
  "FaUsers",
  "FaCloud",
  "FaDatabase",
  "FaServer",
  "FaCog",
  "FaTools",
  "SiGoogle",
  "SiGithub",
  "SiMicrosoft",
  "SiAuth0",
  "SiOkta",
  "BsShield",
  "HiUser",
  "MdSecurity",
  "RiLockLine",
  "TbKey",
];

const AUTH_PROVIDER_ICONS = [
  "SiGoogle",
  "SiGithub",
  "SiMicrosoft",
  "SiAuth0",
  "SiOkta",
  "SiKeycloak",
  "SiAzuredevops",
  "FaShieldAlt",
  "FaLock",
  "FaKey",
  "FaUserShield",
  "FaIdCard",
  "FaFingerprint",
  "FaUserCheck",
  "BsShieldCheck",
  "HiShieldCheck",
  "MdSecurity",
  "RiLockPasswordLine",
  "TbShieldCheck",
  "AiOutlineSecurityScan",
];

export function IconPicker({ value, onChange, placeholder }: IconPickerProps) {
  const t = useTranslations();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");

  const displayPlaceholder = placeholder || t("iconPicker.placeholder");

  const allIcons = useMemo(() => {
    const iconSets = [
      { icons: AiIcons, prefix: "Ai", category: "Ant Design Icons" },
      { icons: BiIcons, prefix: "Bi", category: "BoxIcons" },
      { icons: BsIcons, prefix: "Bs", category: "Bootstrap Icons" },
      { icons: CgIcons, prefix: "Cg", category: "css.gg" },
      { icons: CiIcons, prefix: "Ci", category: "Circum Icons" },
      { icons: DiIcons, prefix: "Di", category: "Devicons" },
      { icons: FaIcons, prefix: "Fa", category: "Font Awesome 5" },
      { icons: Fa6Icons, prefix: "Fa", category: "Font Awesome 6" },
      { icons: FcIcons, prefix: "Fc", category: "Flat Color Icons" },
      { icons: FiIcons, prefix: "Fi", category: "Feather" },
      { icons: GiIcons, prefix: "Gi", category: "Game Icons" },
      { icons: GoIcons, prefix: "Go", category: "Github Octicons" },
      { icons: GrIcons, prefix: "Gr", category: "Grommet Icons" },
      { icons: HiIcons, prefix: "Hi", category: "Heroicons" },
      { icons: Hi2Icons, prefix: "Hi", category: "Heroicons 2" },
      { icons: ImIcons, prefix: "Im", category: "IcoMoon Free" },
      { icons: IoIcons, prefix: "Io", category: "Ionicons 4" },
      { icons: Io5Icons, prefix: "Io", category: "Ionicons 5" },
      { icons: LiaIcons, prefix: "Lia", category: "Icons8 Line Awesome" },
      { icons: LuIcons, prefix: "Lu", category: "Lucide" },
      { icons: MdIcons, prefix: "Md", category: "Material Design" },
      { icons: PiIcons, prefix: "Pi", category: "Phosphor Icons" },
      { icons: RiIcons, prefix: "Ri", category: "Remix Icon" },
      { icons: RxIcons, prefix: "Rx", category: "Radix Icons" },
      { icons: SiIcons, prefix: "Si", category: "Simple Icons" },
      { icons: SlIcons, prefix: "Sl", category: "Simple Line Icons" },
      { icons: TbIcons, prefix: "Tb", category: "Tabler Icons" },
      { icons: TfiIcons, prefix: "Tfi", category: "Themify Icons" },
      { icons: TiIcons, prefix: "Ti", category: "Typicons" },
      { icons: VscIcons, prefix: "Vsc", category: "VS Code Icons" },
      { icons: WiIcons, prefix: "Wi", category: "Weather Icons" },
    ];

    const icons: IconData[] = [];
    const seenNames = new Set<string>();

    iconSets.forEach(({ icons: iconSet, prefix, category }) => {
      Object.entries(iconSet).forEach(([name, component]) => {
        if (typeof component === "function" && name.startsWith(prefix)) {
          if (seenNames.has(name)) {
            return;
          }
          seenNames.add(name);

          icons.push({
            name,
            component: component as React.ComponentType<{ className?: string }>,
            category,
          });
        }
      });
    });

    return icons;
  }, []);

  const filteredIcons = useMemo(() => {
    if (!search) return allIcons;
    return allIcons.filter((icon) => icon.name.toLowerCase().includes(search.toLowerCase()));
  }, [allIcons, search]);

  const popularIcons = useMemo(() => {
    return POPULAR_ICONS.map((name) => allIcons.find((icon) => icon.name === name)).filter(Boolean) as IconData[];
  }, [allIcons]);

  const authProviderIcons = useMemo(() => {
    return AUTH_PROVIDER_ICONS.map((name) => allIcons.find((icon) => icon.name === name)).filter(Boolean) as IconData[];
  }, [allIcons]);

  const currentIcon = useMemo(() => {
    if (!value) return null;
    return allIcons.find((icon) => icon.name === value);
  }, [value, allIcons]);

  const handleIconSelect = useCallback(
    (iconName: string) => {
      onChange(iconName);
      setOpen(false);
      setSearch("");
    },
    [onChange]
  );

  const renderIcon = useCallback((icon: IconData, className = "w-7 h-7", size?: number) => {
    const IconComponent = icon.component as any;
    return <IconComponent className={className} size={size || 32} />;
  }, []);

  const categories = useMemo(() => {
    const uniqueCategories = Array.from(new Set(allIcons.map((icon) => icon.category)));
    return uniqueCategories.sort();
  }, [allIcons]);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" className="w-full justify-between">
          <div className="flex items-center gap-2">
            {currentIcon ? (
              <>
                {renderIcon(currentIcon, "", 18)}
                <span className="text-sm">{currentIcon.name}</span>
              </>
            ) : (
              <span className="text-muted-foreground">{displayPlaceholder}</span>
            )}
          </div>
          <ChevronDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-5xl xl:max-w-6xl max-h-[90vh] overflow-hidden">
        <div className="space-y-4 overflow-hidden">
          <div className="flex items-center justify-between">
            <DialogTitle>{t("iconPicker.title")}</DialogTitle>
            <div className="text-sm text-muted-foreground">
              {t("iconPicker.stats", {
                iconCount: allIcons.length.toLocaleString(),
                libraryCount: categories.length,
              })}
            </div>
          </div>

          {/* Search */}
          <div className="relative">
            <Search className="absolute left-2 top-2.5 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder={t("iconPicker.searchPlaceholder")}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="pl-8"
            />
            {search && (
              <Button
                variant="ghost"
                size="sm"
                className="absolute right-1 top-1 h-6 w-6 p-0"
                onClick={() => setSearch("")}
              >
                <X className="h-3 w-3" />
              </Button>
            )}
          </div>

          <Tabs defaultValue="all" className="w-full overflow-hidden">
            <TabsList className="grid w-full grid-cols-3">
              <TabsTrigger value="all">{t("iconPicker.tabs.all")}</TabsTrigger>
              <TabsTrigger value="popular">{t("iconPicker.tabs.popular")}</TabsTrigger>
              <TabsTrigger value="auth">{t("iconPicker.tabs.auth")}</TabsTrigger>
            </TabsList>

            {/* All Icons */}
            <TabsContent value="all" className="mt-4 overflow-hidden">
              {search ? (
                <VirtualizedIconGrid
                  key={`search-${search}`}
                  icons={filteredIcons}
                  onIconSelect={handleIconSelect}
                  renderIcon={renderIcon}
                  showCategories={false}
                />
              ) : (
                <VirtualizedIconGrid
                  key="categories"
                  icons={allIcons}
                  onIconSelect={handleIconSelect}
                  renderIcon={renderIcon}
                  showCategories={true}
                />
              )}
            </TabsContent>

            {/* Popular Icons */}
            <TabsContent value="popular" className="mt-4 overflow-hidden">
              <VirtualizedIconGrid
                key="popular"
                icons={popularIcons}
                onIconSelect={handleIconSelect}
                renderIcon={renderIcon}
                showCategories={false}
              />
            </TabsContent>

            {/* Auth Provider Icons */}
            <TabsContent value="auth" className="mt-4 overflow-hidden">
              <VirtualizedIconGrid
                key="auth"
                icons={authProviderIcons}
                onIconSelect={handleIconSelect}
                renderIcon={renderIcon}
                showCategories={false}
              />
            </TabsContent>
          </Tabs>

          {search && filteredIcons.length === 0 && (
            <div className="text-center py-8 text-muted-foreground">
              <Search className="mx-auto h-12 w-12 opacity-50 mb-2" />
              <p>{t("iconPicker.noIconsFound", { search })}</p>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

export function renderIconByName(iconName: string, className = "w-5 h-5") {
  const iconSets = [
    AiIcons,
    BiIcons,
    BsIcons,
    CgIcons,
    CiIcons,
    DiIcons,
    FaIcons,
    Fa6Icons,
    FcIcons,
    FiIcons,
    GiIcons,
    GoIcons,
    GrIcons,
    HiIcons,
    Hi2Icons,
    ImIcons,
    IoIcons,
    Io5Icons,
    LiaIcons,
    LuIcons,
    MdIcons,
    PiIcons,
    RiIcons,
    RxIcons,
    SiIcons,
    SlIcons,
    TbIcons,
    TfiIcons,
    TiIcons,
    VscIcons,
    WiIcons,
  ];

  for (const iconSet of iconSets) {
    const IconComponent = (iconSet as any)[iconName];
    if (IconComponent && typeof IconComponent === "function") {
      return React.createElement(IconComponent, { className });
    }
  }

  return React.createElement(FaIcons.FaCog, { className });
}
