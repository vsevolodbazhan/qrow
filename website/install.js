// Install section: select the visitor's system and processor, link the
// download buttons to the files of the latest stable release, and copy
// commands. Without this script, or if GitHub does not answer, the page shows
// all systems and the buttons open the latest release page.
(() => {
  "use strict";

  const LATEST = "https://api.github.com/repos/vsevolodbazhan/qrow/releases/latest";
  const RELEASE_PAGE = "https://github.com/vsevolodbazhan/qrow/releases/latest";
  const SYSTEMS = ["macos", "windows", "linux"];
  const NAMES = { macos: "macOS", windows: "Windows", linux: "Linux" };
  // The processors of each system, the most common build first.
  const ARCHS = { macos: ["arm", "x86"], windows: ["x86", "arm"], linux: ["x86", "arm"] };
  // The package that the download button gives, by system and processor.
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
  // The processor of each system when the page cannot find it.
  const DEFAULT_ARCH = { macos: "arm", windows: "x86", linux: "x86" };

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
  // this is a best guess.
  async function detectArch(system) {
    try {
      const data = navigator.userAgentData;
      if (data && data.getHighEntropyValues) {
        const { architecture } = await data.getHighEntropyValues(["architecture"]);
        if (architecture === "arm") return "arm";
        if (architecture === "x86") return system === "macos" ? macGpuArch() : "x86";
      }
    } catch (_) { /* Not available: use the guesses below. */ }
    if (system === "macos") return macGpuArch();
    return /aarch64|arm64|armv8/i.test(navigator.userAgent) ? "arm" : "x86";
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

  // A row of segments with one selected item. Arrow keys, Home, and End move
  // the selection, as in a tab list.
  function segments(buttons, attribute, onSelect) {
    buttons.forEach((button, index) => {
      button.addEventListener("click", () => onSelect(index, false));
      button.addEventListener("keydown", (event) => {
        const moves = { ArrowLeft: -1, ArrowRight: 1 };
        let next = null;
        if (event.key in moves) next = (index + moves[event.key] + buttons.length) % buttons.length;
        if (event.key === "Home") next = 0;
        if (event.key === "End") next = buttons.length - 1;
        if (next === null) return;
        event.preventDefault();
        onSelect(next, true);
      });
    });
    return (selected, focus) => {
      buttons.forEach((button, index) => {
        button.setAttribute(attribute, String(index === selected));
        button.tabIndex = index === selected ? 0 : -1;
      });
      if (focus) buttons[selected].focus();
    };
  }

  function setupSelectors(state, render) {
    const tabs = SYSTEMS.map((system) => $(`#tab-${system}`));
    const archGroup = $("#arch");
    const archButtons = [0, 1].map(() => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "segment";
      button.setAttribute("role", "radio");
      archGroup.append(button);
      return button;
    });

    const markTabs = segments(tabs, "aria-selected", (index, focus) => {
      state.system = SYSTEMS[index];
      update(focus ? "tab" : null);
    });
    const markArch = segments(archButtons, "aria-checked", (index, focus) => {
      state.arch[state.system] = ARCHS[state.system][index];
      update(focus ? "arch" : null);
    });

    function update(focus) {
      const { system } = state;
      const arch = state.arch[system];
      markTabs(SYSTEMS.indexOf(system), focus === "tab");
      archButtons.forEach((button, index) => { button.textContent = ARCH_NAMES[system][ARCHS[system][index]]; });
      markArch(ARCHS[system].indexOf(arch), focus === "arch");
      SYSTEMS.forEach((name) => {
        const panel = $(`#panel-${name}`);
        panel.hidden = name !== system;
        $$("[data-arch]", panel).forEach((element) => { element.hidden = element.dataset.arch !== state.arch[name]; });
      });
      render();
    }

    // The processor selector names the build, so the buttons only say what
    // they do. The file name goes next to the button.
    $$(".downloads .btn").forEach((link) => {
      link.classList.replace("btn-secondary", "btn-primary");
      $("span", link).textContent = "Download";
      const file = document.createElement("span");
      file.className = "file-name";
      file.dataset.arch = link.dataset.arch;
      file.dataset.fileOf = link.dataset.asset;
      link.after(file);
    });
    document.body.classList.add("has-script");
    $(".selectors").hidden = false;
    update(null);
  }

  function setupCopy() {
    if (!navigator.clipboard) return;
    $$(".command").forEach((command) => {
      const button = $(".copy", command);
      let timer = 0;
      // The Linux command needs the file name from the API answer. Until then
      // it has a placeholder, so there is nothing useful to copy.
      button.hidden = Boolean($("#linux-archive", command));
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
  function linkAssets(release) {
    $$("[data-file-of]").forEach((file) => {
      file.textContent = `Qrow-${release.version}-${file.dataset.fileOf}`;
    });
    $$("[data-asset]").forEach((link) => {
      const name = `Qrow-${release.version}-${link.dataset.asset}`;
      const url = release.assets.get(name);
      if (!url) return;
      link.href = url;
      if (!link.classList.contains("btn")) link.textContent = name;
    });
  }

  function renderHero(system, arch, release) {
    const button = $("#download");
    const note = $("#download-note");
    if (!system) return;
    $("#download-label").textContent = `Download for ${NAMES[system]}`;
    const name = release && `Qrow-${release.version}-${PRIMARY[system][arch]}`;
    button.href = (name && release.assets.get(name)) || RELEASE_PAGE;
    const summary = document.createElement("span");
    summary.textContent = release && release.version
      ? `Version ${release.version} for ${ARCH_NAMES[system][arch]}`
      : `For ${ARCH_NAMES[system][arch]}`;
    const other = document.createElement("a");
    other.href = "#install";
    other.textContent = "Other downloads";
    note.replaceChildren(summary, other);
  }

  async function main() {
    const system = detectSystem();
    const arch = system ? await detectArch(system) : null;
    const state = { system: system || "macos", arch: { ...DEFAULT_ARCH } };
    if (system) state.arch[system] = arch;

    let release = null;
    const archive = $("#linux-archive");
    const render = () => {
      if (release && archive) {
        archive.textContent = `Qrow-${release.version}-${PRIMARY.linux[state.arch.linux]}`;
        $(".copy", archive.closest(".command")).hidden = !navigator.clipboard;
      }
    };

    setupSelectors(state, render);
    setupCopy();
    // Show the system at once. The version and the file links follow the API answer.
    renderHero(system, arch, null);
    try {
      release = await latestRelease();
      linkAssets(release);
    } catch (_) { /* Keep the links to the release page. */ }
    renderHero(system, arch, release);
    render();
  }

  main();
})();
