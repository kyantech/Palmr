import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const REACT_VENDOR = /[\\/]node_modules[\\/](?:react|react-dom|react-router|scheduler)[\\/]/;
const ANTD_VENDOR = /[\\/]node_modules[\\/](?:antd|@ant-design|@rc-component)[\\/]/;
const LOCALE_DATA = /[\\/]locale[\\/]/;

const DEV_API_TARGET = process.env.PALMR_DEV_API ?? "http://127.0.0.1:5487";

export default defineConfig({
  base: "./",
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: { "/api": DEV_API_TARGET },
  },
  build: {
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [
            { name: "vendor-react", test: REACT_VENDOR, priority: 20 },
            {
              name: "vendor-antd",
              test: (id) => ANTD_VENDOR.test(id) && !LOCALE_DATA.test(id),
              priority: 10,
            },
          ],
        },
      },
    },
  },
});
