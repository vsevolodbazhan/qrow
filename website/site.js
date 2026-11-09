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
})();
