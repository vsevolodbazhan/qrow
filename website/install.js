// Install section: select the tab of the visitor's system, link the download
// buttons to the files of the latest stable release, and copy commands.
// Without this script, or if GitHub does not answer, the page shows all
// systems and the buttons open the latest release page.
(() => {
  "use strict";

  const LATEST = "https://api.github.com/repos/vsevolodbazhan/qrow/releases/latest";
  const RELEASE_PAGE = "https://github.com/vsevolodbazhan/qrow/releases/latest";
  const SYSTEMS =["macos", "windows", "linux"];
  const NAMES = { macos: "macOS", windows: "Windows", linux: "Linux" };
  // The package that the hero button downloads, by system and architecture.
  const PRIMARY = {
    macos: { arm: "arm64.dmg", x86: "x86_64.dmg" },
    windows: { arm: "windows-aarch64-setup.exe", x86: "windows-x86_64-setup.exe" },
    linux: { arm: "linux-aarch64.tar.gz", x86: "linux-x86_64.tar.gz" },
  };
  const ARCH_NAMES = {
    macos: { arm: "Apple Silicon", x86: "Intel" },
    windows: { arm: "ARM64", x86: "Intel/AMD" },
    linux: { arm: "ARM64", x86: "x86-64" },
  };

  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

  function detectSystem() {
    const ua = navigator.userAgent;
    const platform = (navigator.userAgentData && navigator.userAgentData.platform) || "";
    // iPadOS reports a Mac user agent, but has touch input.
    if (/Android|iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1)) {
      return null;
    }
    if (/Windows/.test(platform) || /Windows/.test(ua)) return "windows";
    if (/macOS/.test(platform) || /Macintosh|Mac OS X/.test(ua)) return "macos";
    if (/Linux|X11/.test(platform) || /Linux|X11/.test(ua)) return "linux";
    return null;
  }

  // Returns "arm" or "x86". Browsers hide the architecture more and more, so
  // this is a best guess. The default is the most common build of the system.
  async function detectArch(system) {
    try {
      const data = navigator.userAgentData;
      if (data && data.getHighEntropyValues) {
        const { architecture } = await data.getHighEntropyValues(["architecture"]);
        if (architecture === "arm") return "arm";
        if (architecture === "x86") return system === "macos" ? macGpuArch() : "x86";
      }
    } catch (_) { /* Not available: use the guesses below. */ }
    const ua = navigator.userAgent;
    if (system === "macos") return macGpuArch();
    return /aarch64|arm64|armv8/i.test(ua) ? "arm" : "x86";
  }

  // All Mac browsers report "Intel" in the user agent. Chrome on Apple
  // Silicon can also report x86. The GPU name separates the two.
  function macGpuArch() {
    try {
      const gl = document.createElement("canvas").getContext("webgl");
      const info = gl && gl.getExtension("WEBGL_debug_renderer_info");
      const renderer = info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : "";
      if (/Intel|AMD|Radeon|NVIDIA/i.test(renderer)) return "x86";
    } catch (_) { /* No WebGL: assume Apple Silicon. */ }
    return "arm";
  }

  function setupTabs(initial) {
    const tablist = $(".tabs");
    const tabs = SYSTEMS.map((system) => $(`#tab-${system}`));
    const select = (system, focus) => {
      SYSTEMS.forEach((name, index) => {
        const active = name === system;
        tabs[index].setAttribute("aria-selected", String(active));
        tabs[index].tabIndex = active ? 0 : -1;
        $(`#panel-${name}`).hidden = !active;
      });
      if (focus) tabs[SYSTEMS.indexOf(system)].focus();
    };
    tabs.forEach((tab, index) => {
      tab.addEventListener("click", () => select(SYSTEMS[index], false));
      tab.addEventListener("keydown", (event) => {
        const moves = { ArrowLeft: -1, ArrowRight: 1 };
        let next = null;
        if (event.key in moves) next = (index + moves[event.key] + SYSTEMS.length) % SYSTEMS.length;
        if (event.key === "Home") next = 0;
        if (event.key === "End") next = SYSTEMS.length - 1;
        if (next === null) return;
        event.preventDefault();
        select(SYSTEMS[next], true);
      });
    });
    document.body.classList.add("has-tabs");
    tablist.hidden = false;
    select(initial, false);
  }

  function setupCopy() {
    if (!navigator.clipboard) return;
    $$(".command").forEach((command) => {
      const button = $(".copy", command);
      let timer = 0;
      button.hidden = false;
      button.addEventListener("click", async () => {
        try {
          await navigator.clipboard.writeText($("code", command).textContent);
          button.textContent = "Copied";
        } catch (_) {
          button.textContent = "Copy failed";
        }
        clearTimeout(timer);
        timer = setTimeout(() => { button.textContent = "Copy"; }, 1500);
      });
    });
  }

  async function latestRelease() {
    const response = await fetch(LATEST, { headers: { Accept: "application/vnd.github+json" } });
    if (!response.ok) throw new Error(`GitHub answered ${response.status}`);
    const release = await response.json();
    const version = String(release.tag_name || "").replace(/^v/, "");
    const assets = new Map((release.assets || []).map((asset) => [asset.name, asset.browser_download_url]));
    return { version, assets };
  }

  // The release files are named Qrow-VERSION-SUFFIX, for example
  // Qrow-0.4.1-arm64.dmg. The data-asset attribute holds the suffix.
  function linkAssets({ version, assets }) {
    $$("[data-asset]").forEach((link) => {
      const name = `Qrow-${version}-${link.dataset.asset}`;
      const url = assets.get(name);
      if (url) {
        link.href = url;
        link.title = name;
      }
    });
  }

  async function main() {
    const system = detectSystem();
    setupTabs(system || "macos");
    setupCopy();

    const button = $("#download");
    const label = $("#download-label");
    const note = $("#download-note");
    const arch = system ? await detectArch(system) : null;
    if (system) label.textContent = `Download for ${NAMES[system]}`;

    let release = null;
    try {
      release = await latestRelease();
      linkAssets(release);
    } catch (_) { /* Keep the links to the release page. */ }

    const archive = $("#linux-archive");
    if (release && archive) {
      archive.textContent = `Qrow-${release.version}-${PRIMARY.linux[system === "linux" ? arch : "x86"]}`;
    }

    if (!system) return;
    const suffix = PRIMARY[system][arch];
    const url = release && release.assets.get(`Qrow-${release.version}-${suffix}`);
    button.href = url || RELEASE_PAGE;
    const parts = [ARCH_NAMES[system][arch]];
    if (release && release.version) parts.push(`Version ${release.version}`);
    note.textContent = `${parts.join(" · ")} · `;
    const other = document.createElement("a");
    other.href = "#install";
    other.textContent = "Other downloads";
    note.append(other);
  }

  main();
})();
