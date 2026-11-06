// Applies the saved theme and the installation's accent before the first paint (no flash of
// the wrong colors).
(function () {
  var theme = "system";
  try {
    theme = localStorage.getItem("slonbase.theme") || "system";
  } catch (e) {}
  var root = document.documentElement;
  var dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  root.dataset.theme = dark ? "dark" : "light";
  // a named accent (its colors are in app.css) or a color of its own; see lib/branding.js
  var branding = (window.SLONBASE_CONTEXT || {}).branding || {};
  var accent = branding.accent || "";
  if (/^#[0-9a-f]{6}$/i.test(accent) && ["#1055c9", "#4a5a72", "#5062a6", "#4f7a6c"].indexOf(accent.toLowerCase()) < 0) {
    root.dataset.accent = "custom";
    root.style.setProperty("--accent", accent);
  } else if (/^[a-z]+$/.test(accent)) {
    root.dataset.accent = accent;
  }
})();
