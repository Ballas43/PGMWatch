(function () {
  try {
    var m = localStorage.getItem("pgmwatch_theme");
    var t = m === "light" || m === "dark" ? m : "dark";
    document.documentElement.setAttribute("data-theme", t);
    document.documentElement.style.colorScheme = t;
  } catch (_) {}
})();
