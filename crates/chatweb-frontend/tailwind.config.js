/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./index.html",
    "../chatweb-app/src/**/*.rs",
  ],
  darkMode: ['selector', '[data-theme="dark"]'],
  theme: {
    extend: {},
  },
  plugins: [],
}
