// Applies the saved theme before the first paint (no flash of the wrong theme).
(function () {
  var theme = "system";
  try {
    theme = localStorage.getItem("slonbase.theme") || "system";
  } catch (e) {}
  var dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
})();
