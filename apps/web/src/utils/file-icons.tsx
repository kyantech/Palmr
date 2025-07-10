import {
  Icon,
  IconApi,
  IconAtom,
  IconBook,
  IconBrandCss3,
  IconBrandDocker,
  IconBrandGit,
  IconBrandGolang,
  IconBrandHtml5,
  IconBrandJavascript,
  IconBrandKotlin,
  IconBrandNpm,
  IconBrandPhp,
  IconBrandPython,
  IconBrandReact,
  IconBrandRust,
  IconBrandSass,
  IconBrandSwift,
  IconBrandTypescript,
  IconBrandVue,
  IconBrandYarn,
  IconBug,
  IconCloud,
  IconCode,
  IconDatabase,
  IconDeviceDesktop,
  IconFile,
  IconFileCode,
  IconFileDescription,
  IconFileMusic,
  IconFileSpreadsheet,
  IconFileText,
  IconFileTypePdf,
  IconFileZip,
  IconKey,
  IconLock,
  IconMarkdown,
  IconMath,
  IconPalette,
  IconPhoto,
  IconPresentation,
  IconSettings,
  IconTerminal,
  IconTool,
  IconVideo,
} from "@tabler/icons-react";

interface FileIconMapping {
  extensions: string[];
  icon: Icon;
  color: string;
}

const fileIcons: FileIconMapping[] = [
  // Images
  {
    extensions: ["jpg", "jpeg", "png", "gif", "bmp", "webp", "svg", "tiff", "ico", "heic", "avif"],
    icon: IconPhoto,
    color: "text-blue-500",
  },

  // Documents
  {
    extensions: ["pdf"],
    icon: IconFileTypePdf,
    color: "text-red-500",
  },
  {
    extensions: ["doc", "docx", "odt", "rtf"],
    icon: IconFileText,
    color: "text-blue-600",
  },
  {
    extensions: ["xls", "xlsx", "ods", "csv"],
    icon: IconFileSpreadsheet,
    color: "text-green-600",
  },
  {
    extensions: ["ppt", "pptx", "odp"],
    icon: IconPresentation,
    color: "text-orange-500",
  },

  // Media
  {
    extensions: ["mp3", "wav", "ogg", "m4a", "aac", "flac", "wma", "opus"],
    icon: IconFileMusic,
    color: "text-purple-500",
  },
  {
    extensions: ["mp4", "avi", "mov", "wmv", "mkv", "webm", "flv", "m4v", "3gp"],
    icon: IconVideo,
    color: "text-pink-500",
  },

  // Archives
  {
    extensions: ["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "lz", "cab", "deb", "rpm"],
    icon: IconFileZip,
    color: "text-yellow-600",
  },

  // JavaScript/TypeScript
  {
    extensions: ["js", "mjs", "cjs"],
    icon: IconBrandJavascript,
    color: "text-yellow-500",
  },
  {
    extensions: ["ts", "tsx"],
    icon: IconBrandTypescript,
    color: "text-blue-600",
  },
  {
    extensions: ["jsx"],
    icon: IconBrandReact,
    color: "text-cyan-500",
  },
  {
    extensions: ["vue"],
    icon: IconBrandVue,
    color: "text-green-500",
  },

  // Web Technologies
  {
    extensions: ["html", "htm", "xhtml"],
    icon: IconBrandHtml5,
    color: "text-orange-600",
  },
  {
    extensions: ["css"],
    icon: IconBrandCss3,
    color: "text-blue-600",
  },
  {
    extensions: ["scss", "sass"],
    icon: IconBrandSass,
    color: "text-pink-600",
  },
  {
    extensions: ["less", "stylus"],
    icon: IconPalette,
    color: "text-purple-600",
  },

  // Programming Languages
  {
    extensions: ["py", "pyw", "pyc", "pyo", "pyd"],
    icon: IconBrandPython,
    color: "text-yellow-600",
  },
  {
    extensions: ["php", "phtml"],
    icon: IconBrandPhp,
    color: "text-purple-700",
  },
  {
    extensions: ["go"],
    icon: IconBrandGolang,
    color: "text-cyan-600",
  },
  {
    extensions: ["rs"],
    icon: IconBrandRust,
    color: "text-orange-700",
  },
  {
    extensions: ["swift"],
    icon: IconBrandSwift,
    color: "text-orange-500",
  },
  {
    extensions: ["kt", "kts"],
    icon: IconBrandKotlin,
    color: "text-purple-600",
  },
  {
    extensions: ["java", "class", "jar"],
    icon: IconCode,
    color: "text-red-600",
  },
  {
    extensions: ["c", "h"],
    icon: IconCode,
    color: "text-blue-700",
  },
  {
    extensions: ["cpp", "cxx", "cc", "hpp", "hxx"],
    icon: IconCode,
    color: "text-blue-800",
  },
  {
    extensions: ["cs"],
    icon: IconCode,
    color: "text-purple-700",
  },
  {
    extensions: ["rb", "rbw", "rake"],
    icon: IconCode,
    color: "text-red-500",
  },
  {
    extensions: ["scala", "sc"],
    icon: IconCode,
    color: "text-red-700",
  },
  {
    extensions: ["clj", "cljs", "cljc", "edn"],
    icon: IconCode,
    color: "text-green-700",
  },
  {
    extensions: ["hs", "lhs"],
    icon: IconCode,
    color: "text-purple-800",
  },
  {
    extensions: ["elm"],
    icon: IconCode,
    color: "text-blue-700",
  },
  {
    extensions: ["dart"],
    icon: IconCode,
    color: "text-blue-600",
  },
  {
    extensions: ["lua"],
    icon: IconCode,
    color: "text-blue-800",
  },
  {
    extensions: ["r", "rmd"],
    icon: IconMath,
    color: "text-blue-700",
  },
  {
    extensions: ["matlab", "m"],
    icon: IconMath,
    color: "text-orange-600",
  },
  {
    extensions: ["julia", "jl"],
    icon: IconMath,
    color: "text-purple-600",
  },

  // Shell Scripts
  {
    extensions: ["sh", "bash", "zsh", "fish"],
    icon: IconTerminal,
    color: "text-green-600",
  },
  {
    extensions: ["ps1", "psm1", "psd1"],
    icon: IconTerminal,
    color: "text-blue-700",
  },
  {
    extensions: ["bat", "cmd"],
    icon: IconTerminal,
    color: "text-gray-600",
  },

  // Database
  {
    extensions: ["sql", "mysql", "pgsql", "sqlite", "db"],
    icon: IconDatabase,
    color: "text-blue-700",
  },

  // Configuration Files
  {
    extensions: ["json", "json5"],
    icon: IconCode,
    color: "text-yellow-700",
  },
  {
    extensions: ["yaml", "yml"],
    icon: IconSettings,
    color: "text-purple-600",
  },
  {
    extensions: ["toml"],
    icon: IconSettings,
    color: "text-orange-600",
  },
  {
    extensions: ["xml", "xsd", "xsl", "xslt"],
    icon: IconCode,
    color: "text-orange-700",
  },
  {
    extensions: ["ini", "cfg", "conf", "config"],
    icon: IconSettings,
    color: "text-gray-600",
  },
  {
    extensions: ["env", "dotenv"],
    icon: IconKey,
    color: "text-green-700",
  },
  {
    extensions: ["properties"],
    icon: IconSettings,
    color: "text-blue-600",
  },

  // Docker & DevOps
  {
    extensions: ["dockerfile", "containerfile"],
    icon: IconBrandDocker,
    color: "text-blue-600",
  },
  {
    extensions: ["tf", "tfvars", "hcl"],
    icon: IconCloud,
    color: "text-purple-600",
  },
  {
    extensions: ["k8s", "kubernetes"],
    icon: IconCloud,
    color: "text-blue-700",
  },
  {
    extensions: ["ansible", "playbook"],
    icon: IconTool,
    color: "text-red-600",
  },

  // Package Managers
  {
    extensions: ["package"],
    icon: IconBrandNpm,
    color: "text-red-600",
  },
  {
    extensions: ["yarn"],
    icon: IconBrandYarn,
    color: "text-blue-600",
  },
  {
    extensions: ["cargo"],
    icon: IconBrandRust,
    color: "text-orange-700",
  },
  {
    extensions: ["gemfile"],
    icon: IconCode,
    color: "text-red-500",
  },
  {
    extensions: ["composer"],
    icon: IconBrandPhp,
    color: "text-purple-700",
  },
  {
    extensions: ["requirements", "pipfile", "poetry"],
    icon: IconBrandPython,
    color: "text-yellow-600",
  },
  {
    extensions: ["gradle", "build.gradle"],
    icon: IconTool,
    color: "text-green-700",
  },
  {
    extensions: ["pom"],
    icon: IconCode,
    color: "text-orange-600",
  },
  {
    extensions: ["makefile", "cmake"],
    icon: IconTool,
    color: "text-blue-700",
  },

  // Git
  {
    extensions: ["gitignore", "gitattributes", "gitmodules", "gitconfig"],
    icon: IconBrandGit,
    color: "text-orange-600",
  },

  // Documentation
  {
    extensions: ["md", "markdown"],
    icon: IconMarkdown,
    color: "text-emerald-500",
  },
  {
    extensions: ["rst", "txt"],
    icon: IconFileDescription,
    color: "text-gray-500",
  },
  {
    extensions: ["adoc", "asciidoc"],
    icon: IconBook,
    color: "text-blue-600",
  },
  {
    extensions: ["tex", "latex"],
    icon: IconMath,
    color: "text-green-700",
  },
  {
    extensions: ["log"],
    icon: IconBug,
    color: "text-yellow-600",
  },

  // Templates
  {
    extensions: ["hbs", "handlebars", "mustache"],
    icon: IconCode,
    color: "text-orange-600",
  },
  {
    extensions: ["twig"],
    icon: IconCode,
    color: "text-green-600",
  },
  {
    extensions: ["liquid"],
    icon: IconCode,
    color: "text-blue-600",
  },
  {
    extensions: ["ejs", "pug", "jade"],
    icon: IconCode,
    color: "text-brown-600",
  },

  // Data Formats
  {
    extensions: ["graphql", "gql"],
    icon: IconApi,
    color: "text-pink-600",
  },
  {
    extensions: ["proto", "protobuf"],
    icon: IconApi,
    color: "text-blue-700",
  },

  // Security & Certificates
  {
    extensions: ["pem", "crt", "cer", "key", "p12", "pfx"],
    icon: IconLock,
    color: "text-green-800",
  },

  // Web Assembly
  {
    extensions: ["wasm", "wat"],
    icon: IconAtom,
    color: "text-purple-700",
  },

  // Shaders
  {
    extensions: ["glsl", "hlsl", "vert", "frag", "geom"],
    icon: IconDeviceDesktop,
    color: "text-cyan-700",
  },

  // Specialized
  {
    extensions: ["vim", "vimrc"],
    icon: IconCode,
    color: "text-green-800",
  },
  {
    extensions: ["eslintrc", "prettierrc", "babelrc"],
    icon: IconSettings,
    color: "text-yellow-700",
  },
  {
    extensions: ["tsconfig", "jsconfig"],
    icon: IconSettings,
    color: "text-blue-700",
  },
  {
    extensions: ["webpack", "rollup", "vite"],
    icon: IconTool,
    color: "text-cyan-600",
  },
  {
    extensions: ["lock", "sum"],
    icon: IconLock,
    color: "text-gray-600",
  },

  // Fallback for general text/code files
  {
    extensions: ["svelte", "astro", "erb", "haml", "slim"],
    icon: IconFileCode,
    color: "text-gray-600",
  },
];

export function getFileIcon(filename: string): { icon: Icon; color: string } {
  const extension = filename.split(".").pop()?.toLowerCase() || "";

  const mapping = fileIcons.find((type) => type.extensions.includes(extension));

  return mapping || { icon: IconFile, color: "text-gray-400" };
}
