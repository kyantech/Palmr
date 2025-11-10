/**
 * Utility for detecting MIME types based on file extensions
 * Fallback to application/octet-stream if extension is unknown
 */

const mimeTypeMap: Record<string, string> = {
  // Images
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".png": "image/png",
  ".gif": "image/gif",
  ".bmp": "image/bmp",
  ".webp": "image/webp",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".tiff": "image/tiff",
  ".tif": "image/tiff",
  ".avif": "image/avif",
  ".heic": "image/heic",
  ".heif": "image/heif",
  ".jxl": "image/jxl",
  ".psd": "image/vnd.adobe.photoshop",
  ".raw": "image/x-canon-cr2",
  ".cr2": "image/x-canon-cr2",
  ".nef": "image/x-nikon-nef",
  ".arw": "image/x-sony-arw",
  ".dng": "image/x-adobe-dng",
  ".xcf": "image/x-xcf",
  ".pbm": "image/x-portable-bitmap",
  ".pgm": "image/x-portable-graymap",
  ".ppm": "image/x-portable-pixmap",
  ".pnm": "image/x-portable-anymap",

  // Documents
  ".pdf": "application/pdf",
  ".doc": "application/msword",
  ".docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  ".docm": "application/vnd.ms-word.document.macroEnabled.12",
  ".dotx": "application/vnd.openxmlformats-officedocument.wordprocessingml.template",
  ".dotm": "application/vnd.ms-word.template.macroEnabled.12",
  ".xls": "application/vnd.ms-excel",
  ".xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  ".xlsm": "application/vnd.ms-excel.sheet.macroEnabled.12",
  ".xltx": "application/vnd.openxmlformats-officedocument.spreadsheetml.template",
  ".xltm": "application/vnd.ms-excel.template.macroEnabled.12",
  ".xlsb": "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
  ".ppt": "application/vnd.ms-powerpoint",
  ".pptx": "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  ".pptm": "application/vnd.ms-powerpoint.presentation.macroEnabled.12",
  ".potx": "application/vnd.openxmlformats-officedocument.presentationml.template",
  ".potm": "application/vnd.ms-powerpoint.template.macroEnabled.12",
  ".ppsx": "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
  ".ppsm": "application/vnd.ms-powerpoint.slideshow.macroEnabled.12",
  ".odt": "application/vnd.oasis.opendocument.text",
  ".ods": "application/vnd.oasis.opendocument.spreadsheet",
  ".odp": "application/vnd.oasis.opendocument.presentation",
  ".odg": "application/vnd.oasis.opendocument.graphics",
  ".odf": "application/vnd.oasis.opendocument.formula",
  ".odb": "application/vnd.oasis.opendocument.database",
  ".odc": "application/vnd.oasis.opendocument.chart",
  ".odi": "application/vnd.oasis.opendocument.image",

  // Text and Code
  ".txt": "text/plain",
  ".html": "text/html",
  ".htm": "text/html",
  ".css": "text/css",
  ".js": "application/javascript",
  ".mjs": "application/javascript",
  ".ts": "text/typescript",
  ".tsx": "text/tsx",
  ".jsx": "text/jsx",
  ".json": "application/json",
  ".xml": "application/xml",
  ".csv": "text/csv",
  ".yaml": "text/yaml",
  ".yml": "text/yaml",
  ".toml": "text/plain",
  ".ini": "text/plain",
  ".cfg": "text/plain",
  ".conf": "text/plain",
  ".log": "text/plain",
  ".md": "text/markdown",
  ".markdown": "text/markdown",
  ".rst": "text/x-rst",
  ".tex": "text/x-tex",
  ".latex": "text/x-latex",
  ".rtf": "application/rtf",
  ".ps": "application/postscript",
  ".eps": "application/postscript",

  // Programming Languages
  ".c": "text/x-c",
  ".cc": "text/x-c",
  ".cpp": "text/x-c",
  ".cxx": "text/x-c",
  ".h": "text/x-c",
  ".hpp": "text/x-c",
  ".hxx": "text/x-c",
  ".java": "text/x-java-source",
  ".class": "application/java-vm",
  ".jar": "application/java-archive",
  ".war": "application/java-archive",
  ".py": "text/x-python",
  ".pyw": "text/x-python",
  ".rb": "text/x-ruby",
  ".php": "text/x-php",
  ".pl": "text/x-perl",
  ".pm": "text/x-perl",
  ".sh": "text/x-shellscript",
  ".bash": "text/x-shellscript",
  ".zsh": "text/x-shellscript",
  ".fish": "text/x-shellscript",
  ".bat": "text/x-msdos-batch",
  ".cmd": "text/x-msdos-batch",
  ".ps1": "text/plain",
  ".psm1": "text/plain",
  ".go": "text/x-go",
  ".rs": "text/x-rust",
  ".swift": "text/x-swift",
  ".kt": "text/x-kotlin",
  ".scala": "text/x-scala",
  ".clj": "text/x-clojure",
  ".hs": "text/x-haskell",
  ".elm": "text/x-elm",
  ".dart": "text/x-dart",
  ".r": "text/x-r",
  ".R": "text/x-r",
  ".sql": "text/x-sql",
  ".vb": "text/x-vb",
  ".cs": "text/x-csharp",
  ".fs": "text/x-fsharp",
  ".lua": "text/x-lua",
  ".m": "text/x-objc",
  ".mm": "text/x-objc",

  // Audio
  ".mp3": "audio/mpeg",
  ".mp2": "audio/mpeg",
  ".m4a": "audio/mp4",
  ".m4b": "audio/mp4",
  ".m4p": "audio/mp4",
  ".wav": "audio/wav",
  ".wave": "audio/wav",
  ".aiff": "audio/aiff",
  ".aif": "audio/aiff",
  ".aifc": "audio/aiff",
  ".flac": "audio/flac",
  ".ogg": "audio/ogg",
  ".oga": "audio/ogg",
  ".opus": "audio/opus",
  ".aac": "audio/aac",
  ".wma": "audio/x-ms-wma",
  ".ac3": "audio/ac3",
  ".amr": "audio/amr",
  ".au": "audio/basic",
  ".snd": "audio/basic",
  ".mid": "audio/midi",
  ".midi": "audio/midi",
  ".kar": "audio/midi",
  ".ra": "audio/x-realaudio",
  ".ram": "audio/x-realaudio",
  ".3gp": "audio/3gpp",
  ".3g2": "audio/3gpp2",
  ".spx": "audio/speex",
  ".wv": "audio/x-wavpack",
  ".ape": "audio/x-ape",
  ".mpc": "audio/x-musepack",

  // Video
  ".mp4": "video/mp4",
  ".m4v": "video/mp4",
  ".avi": "video/x-msvideo",
  ".mov": "video/quicktime",
  ".qt": "video/quicktime",
  ".wmv": "video/x-ms-wmv",
  ".asf": "video/x-ms-asf",
  ".flv": "video/x-flv",
  ".f4v": "video/x-f4v",
  ".webm": "video/webm",
  ".mkv": "video/x-matroska",
  ".mka": "audio/x-matroska",
  ".mks": "video/x-matroska",
  ".ogv": "video/ogg",
  ".ogm": "video/ogg",
  ".mxf": "application/mxf",
  ".m2ts": "video/mp2t",
  ".mts": "video/mp2t",
  ".vob": "video/dvd",
  ".mpg": "video/mpeg",
  ".mpeg": "video/mpeg",
  ".m1v": "video/mpeg",
  ".m2v": "video/mpeg",
  ".rm": "application/vnd.rn-realmedia",
  ".rmvb": "application/vnd.rn-realmedia-vbr",
  ".divx": "video/divx",
  ".xvid": "video/x-xvid",

  // Archives and Compression
  ".zip": "application/zip",
  ".rar": "application/vnd.rar",
  ".7z": "application/x-7z-compressed",
  ".tar": "application/x-tar",
  ".tar.gz": "application/gzip",
  ".tgz": "application/gzip",
  ".tar.bz2": "application/x-bzip2",
  ".tbz2": "application/x-bzip2",
  ".tar.xz": "application/x-xz",
  ".txz": "application/x-xz",
  ".tar.lz": "application/x-lzip",
  ".tar.Z": "application/x-compress",
  ".Z": "application/x-compress",
  ".gz": "application/gzip",
  ".bz2": "application/x-bzip2",
  ".xz": "application/x-xz",
  ".lz": "application/x-lzip",
  ".lzma": "application/x-lzma",
  ".lzo": "application/x-lzop",
  ".arj": "application/x-arj",
  ".ace": "application/x-ace-compressed",
  ".cab": "application/vnd.ms-cab-compressed",
  ".iso": "application/x-iso9660-image",
  ".dmg": "application/x-apple-diskimage",
  ".img": "application/x-img",
  ".bin": "application/octet-stream",
  ".cue": "application/x-cue",
  ".nrg": "application/x-nrg",
  ".mdf": "application/x-mdf",
  ".toast": "application/x-toast",

  // Executables and System
  ".exe": "application/vnd.microsoft.portable-executable",
  ".dll": "application/vnd.microsoft.portable-executable",
  ".msi": "application/x-msdownload",
  ".msp": "application/x-msdownload",
  ".deb": "application/vnd.debian.binary-package",
  ".rpm": "application/x-rpm",
  ".pkg": "application/x-newton-compatible-pkg",
  ".apk": "application/vnd.android.package-archive",
  ".ipa": "application/octet-stream",
  ".app": "application/octet-stream",
  ".snap": "application/x-snap",
  ".flatpak": "application/vnd.flatpak",
  ".appimage": "application/x-appimage",

  // Adobe and Design
  ".ai": "application/illustrator",
  ".indd": "application/x-indesign",
  ".idml": "application/vnd.adobe.indesign-idml-package",
  ".sketch": "application/x-sketch",
  ".fig": "application/x-figma",
  ".xd": "application/vnd.adobe.xd",

  // Fonts
  ".ttf": "font/ttf",
  ".otf": "font/otf",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".eot": "application/vnd.ms-fontobject",
  ".fon": "application/x-font-bdf",
  ".bdf": "application/x-font-bdf",
  ".pcf": "application/x-font-pcf",
  ".pfb": "application/x-font-type1",
  ".pfm": "application/x-font-type1",
  ".afm": "application/x-font-afm",

  // E-books
  ".epub": "application/epub+zip",
  ".mobi": "application/x-mobipocket-ebook",
  ".azw": "application/vnd.amazon.ebook",
  ".azw3": "application/vnd.amazon.ebook",
  ".fb2": "application/x-fictionbook+xml",
  ".lit": "application/x-ms-reader",
  ".pdb": "application/vnd.palm",
  ".prc": "application/vnd.palm",
  ".tcr": "application/x-psion3-s",

  // CAD and 3D
  ".dwg": "image/vnd.dwg",
  ".dxf": "image/vnd.dxf",
  ".step": "application/step",
  ".stp": "application/step",
  ".iges": "application/iges",
  ".igs": "application/iges",
  ".stl": "application/sla",
  ".obj": "application/x-tgif",
  ".3ds": "application/x-3ds",
  ".dae": "model/vnd.collada+xml",
  ".ply": "application/ply",
  ".x3d": "model/x3d+xml",

  // Database
  ".db": "application/x-sqlite3",
  ".sqlite": "application/x-sqlite3",
  ".sqlite3": "application/x-sqlite3",
  ".mdb": "application/x-msaccess",
  ".accdb": "application/x-msaccess",

  // Virtual Machine and Disk Images
  ".vmdk": "application/x-vmdk",
  ".vdi": "application/x-virtualbox-vdi",
  ".vhd": "application/x-virtualbox-vhd",
  ".vhdx": "application/x-virtualbox-vhdx",
  ".ova": "application/x-virtualbox-ova",
  ".ovf": "application/x-virtualbox-ovf",
  ".qcow2": "application/x-qemu-disk",

  // Scientific and Math
  ".mat": "application/x-matlab-data",
  ".nc": "application/x-netcdf",
  ".cdf": "application/x-netcdf",
  ".hdf": "application/x-hdf",
  ".h5": "application/x-hdf5",

  // Misc Application Formats
  ".torrent": "application/x-bittorrent",
  ".rss": "application/rss+xml",
  ".atom": "application/atom+xml",
  ".gpx": "application/gpx+xml",
  ".kml": "application/vnd.google-earth.kml+xml",
  ".kmz": "application/vnd.google-earth.kmz",
  ".ics": "text/calendar",
  ".vcs": "text/x-vcalendar",
  ".vcf": "text/x-vcard",
  ".p7s": "application/pkcs7-signature",
  ".p7m": "application/pkcs7-mime",
  ".p12": "application/x-pkcs12",
  ".pfx": "application/x-pkcs12",
  ".cer": "application/x-x509-ca-cert",
  ".crt": "application/x-x509-ca-cert",
  ".pem": "application/x-pem-file",
  ".key": "application/x-pem-file",
};

/**
 * Get MIME type from file extension
 * @param filename - The filename or extension (with or without leading dot)
 * @returns MIME type string, defaults to 'application/octet-stream' if unknown
 */
export function getMimeType(filename: string): string {
  if (!filename) {
    return "application/octet-stream";
  }

  let extension: string;

  if (filename.startsWith(".")) {
    extension = filename.toLowerCase();
  } else if (!filename.includes(".")) {
    extension = "." + filename.toLowerCase();
  } else {
    const lastDotIndex = filename.lastIndexOf(".");
    if (lastDotIndex === -1) {
      return "application/octet-stream";
    }
    extension = filename.substring(lastDotIndex).toLowerCase();
  }

  return mimeTypeMap[extension] || "application/octet-stream";
}

/**
 * Check if a MIME type represents an image
 * @param mimeType - The MIME type to check
 * @returns true if the MIME type is an image type
 */
export function isImageMimeType(mimeType: string): boolean {
  return mimeType.startsWith("image/");
}

/**
 * Get appropriate Content-Type header value for a file
 * @param filename - The filename to detect type for
 * @returns Content-Type header value
 */
export function getContentType(filename: string): string {
  return getMimeType(filename);
}
