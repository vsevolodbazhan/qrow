(function () {
  "use strict";

  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Copy buttons: the closest [data-copy] holds the text.
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

  // Scroll spy: mark the visible section in the title bar and the feature tree.
  var links = Array.prototype.slice.call(document.querySelectorAll('.nav-tabs a[href^="#"], a.tree-row[href^="#"]'));
  var targets = {};
  links.forEach(function (link) {
    var id = link.getAttribute("href").slice(1);
    var el = document.getElementById(id);
    if (el) (targets[id] = targets[id] || { el: el, links: [] }).links.push(link);
  });
  var ids = Object.keys(targets);
  function updateSpy() {
    var line = window.innerHeight * 0.35;
    var activeNav = null;
    var activeTree = null;
    ids.forEach(function (id) {
      var rect = targets[id].el.getBoundingClientRect();
      if (rect.top <= line && rect.bottom > line) {
        targets[id].links.forEach(function (link) {
          if (link.classList.contains("tree-row")) activeTree = link; else activeNav = link;
        });
      }
    });
    links.forEach(function (link) {
      var on = link === activeNav || link === activeTree;
      if (on) link.setAttribute("aria-current", "true"); else link.removeAttribute("aria-current");
    });
  }
  var spyQueued = false;
  window.addEventListener("scroll", function () {
    if (spyQueued) return;
    spyQueued = true;
    requestAnimationFrame(function () { spyQueued = false; updateSpy(); });
  }, { passive: true });
  window.addEventListener("resize", updateSpy);
  updateSpy();

  // Install tabs (WAI-ARIA tabs with arrow keys).
  var tablist = document.querySelector('.install-panel [role="tablist"]');
  if (tablist) {
    var tabs = Array.prototype.slice.call(tablist.querySelectorAll('[role="tab"]'));
    var select = function (tab, focus) {
      tabs.forEach(function (t) {
        var on = t === tab;
        t.setAttribute("aria-selected", on ? "true" : "false");
        t.tabIndex = on ? 0 : -1;
        t.classList.toggle("qtab-active", on);
        document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
      });
      if (focus) tab.focus();
      tab.scrollIntoView({ block: "nearest", inline: "nearest" });
    };
    tabs.forEach(function (tab, i) {
      tab.addEventListener("click", function () { select(tab, false); });
      tab.addEventListener("keydown", function (e) {
        var next = null;
        if (e.key === "ArrowRight") next = tabs[(i + 1) % tabs.length];
        if (e.key === "ArrowLeft") next = tabs[(i - 1 + tabs.length) % tabs.length];
        if (e.key === "Home") next = tabs[0];
        if (e.key === "End") next = tabs[tabs.length - 1];
        if (next) { e.preventDefault(); select(next, true); }
      });
    });
  }

  // Run demo: tint the statement, then show rows after the measured startup time.
  var panel = document.querySelector("[data-run-demo]");
  if (panel) {
    var runButton = panel.querySelector("[data-run]");
    var dot = panel.querySelector(".qtab .dot");
    var elapsed = panel.querySelector("[data-elapsed]");
    var running = false;
    var run = function () {
      if (running) return;
      running = true;
      panel.classList.remove("ran");
      panel.classList.add("running");
      dot.classList.remove("dot-ok");
      dot.classList.add("dot-busy");
      elapsed.textContent = "Running…";
      var start = performance.now();
      setTimeout(function () {
        var seconds = (performance.now() - start) / 1000;
        elapsed.textContent = seconds.toFixed(2) + " s";
        panel.classList.remove("running");
        panel.classList.add("ran");
        dot.classList.remove("dot-busy");
        dot.classList.add("dot-ok");
        var rows = panel.querySelectorAll(".grid tbody tr");
        rows.forEach(function (row, i) {
          row.style.transitionDelay = (i * 70) + "ms";
        });
        setTimeout(function () {
          panel.classList.remove("ran");
          rows.forEach(function (row) { row.style.transitionDelay = ""; });
          running = false;
        }, 1400);
      }, 520);
    };
    runButton.addEventListener("click", run);
    if (!reduceMotion && "IntersectionObserver" in window) {
      var once = new IntersectionObserver(function (entries) {
        if (entries[0].isIntersecting) { once.disconnect(); run(); }
      }, { threshold: 0.6 });
      once.observe(panel);
    }
  }

  // Videos: load and play only while visible. With reduced motion, show controls instead.
  var videos = document.querySelectorAll("video");
  if (reduceMotion || !("IntersectionObserver" in window)) {
    videos.forEach(function (v) { v.controls = true; v.preload = "metadata"; });
  } else {
    var watcher = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        var v = entry.target;
        if (entry.isIntersecting) {
          var p = v.play();
          if (p && p.catch) p.catch(function () { v.controls = true; });
        } else {
          v.pause();
        }
      });
    }, { threshold: 0.35 });
    videos.forEach(function (v) { watcher.observe(v); });
  }
})();
