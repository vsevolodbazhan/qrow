(function () {
  "use strict";

  // Copy button: the closest [data-copy] holds the text.
  document.querySelectorAll(".copy").forEach(function (button) {
    button.addEventListener("click", function () {
      var host = button.closest("[data-copy]");
      if (!host || !navigator.clipboard) return;
      navigator.clipboard.writeText(host.getAttribute("data-copy")).then(function () {
        var use = button.querySelector("use");
        use.setAttribute("href", "#i-check");
        button.setAttribute("data-tip", "Copied");
        setTimeout(function () {
          use.setAttribute("href", "#i-copy");
          button.setAttribute("data-tip", "Copy");
        }, 1600);
      });
    });
  });

  // Theme toggle. The page follows the system theme until the visitor picks one.
  var toggle = document.querySelector(".theme-toggle");
  if (toggle) {
    toggle.addEventListener("click", function () {
      var root = document.documentElement;
      var current = root.dataset.theme ||
        (window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark");
      var next = current === "light" ? "dark" : "light";
      root.dataset.theme = next;
      try { localStorage.setItem("qrow-site-theme", next); } catch (e) {}
    });
  }
})();
