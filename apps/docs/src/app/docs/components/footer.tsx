import Link from "fumadocs-core/link";

export function Footer() {
  return (
    <footer className="flex items-center justify-center p-6 border-t font-light">
      <div className="flex items-center gap-1 text-sm ">
        <span>Powered by</span>
        <Link
          href="http://kyantech.com.br"
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
