/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,js,html}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        // SpaceMolt brand accent (purple-blue) for connection indicators
        spacemolt: {
          DEFAULT: "#7c3aed",
          online: "#10b981",
          offline: "#6b7280",
          connecting: "#f59e0b",
          error: "#ef4444",
        },
      },
    },
  },
  plugins: [],
};