/* The Tree of IRIX 7 — camera, panel, search, filters */
(() => {
  "use strict";

  const data = JSON.parse(document.getElementById("roadmap-data").textContent);
  const nodes = data.nodes;
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const statuses = data.statuses;
  const branches = data.branches;
  const W = 3000;
  const H = 2300;
  const SUN = { x: 1500, y: 380 };
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  const stage = document.getElementById("stage");
  const svg = document.getElementById("tree");
  const panel = document.getElementById("panel");
  const panelEyebrow = document.getElementById("panel-eyebrow");
  const panelStatus = document.getElementById("panel-status");
  const panelTitle = document.getElementById("panel-title");
  const panelSummary = document.getElementById("panel-summary");
  const panelBody = document.getElementById("panel-body");
  const panelDoc = document.getElementById("panel-doc");
  const intro = document.getElementById("intro");
  const searchLayer = document.getElementById("search-layer");
  const searchInput = document.getElementById("search-input");
  const searchResults = document.getElementById("search-results");
  const about = document.getElementById("about");
  const rail = document.querySelector(".rail");
  const minimap = document.getElementById("minimap");
  const miniView = document.getElementById("mini-view");
  const chrome = [
    stage,
    document.querySelector(".topbar"),
    document.querySelector(".hud-left"),
    document.querySelector(".hud-right"),
    rail,
    document.getElementById("now-growing"),
    panel,
  ].filter(Boolean);

  let vw = window.innerWidth;
  let vh = window.innerHeight;
  let minScale = 0.2;
  let maxScale = 3.2;
  let cam = { x: 1500, y: 1500, scale: 0.8 };
  let camRaf = 0;
  let moved = false;
  let selectedId = null;
  let activeMilestone = null;
  let lastFocused = null;
  let searchIndex = 0;
  let searchMatches = [];
  let searchReturnFocus = null;

  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const lerp = (a, b, t) => a + (b - a) * t;
  const easeInOut = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
  const esc = (s) =>
    String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const fmt = (s) =>
    esc(s).replace(/`([^`]+)`/g, "<code>$1</code>");

  const statusColour = (s) => (statuses[s] ? statuses[s].colour : "#8fe3c0");
  const statusLabel = (s) => (statuses[s] ? statuses[s].label : s);
  const branchName = (b) => (branches[b] ? branches[b].name : b);

  /* ------------------------------------------------------------- camera */

  function measure() {
    vw = window.innerWidth;
    vh = window.innerHeight;
    const reserve = vw <= 820 ? 184 : 170;
    minScale = Math.max(0.06, Math.min(vw / W, (vh - reserve) / H));
    maxScale = Math.max(2.8, minScale * 3);
  }

  function clampCamera() {
    cam.scale = clamp(cam.scale, minScale, maxScale);
    const halfW = vw / (2 * cam.scale);
    const halfH = vh / (2 * cam.scale);
    if (halfW * 2 < W) cam.x = clamp(cam.x, halfW - 140, W - halfW + 140);
    else cam.x = W / 2;
    if (halfH * 2 < H) cam.y = clamp(cam.y, halfH - 140, H - halfH + 140);
    else cam.y = H / 2;
  }

  function applyCamera() {
    clampCamera();
    const w = vw / cam.scale;
    const h = vh / cam.scale;
    svg.setAttribute("viewBox", `${(cam.x - w / 2).toFixed(2)} ${(cam.y - h / 2).toFixed(2)} ${w.toFixed(2)} ${h.toFixed(2)}`);
    updateMini();
  }

  function updateMini() {
    if (!miniView) return;
    const w = vw / cam.scale;
    const h = vh / cam.scale;
    miniView.setAttribute("x", clamp(cam.x - w / 2, 0, W));
    miniView.setAttribute("y", clamp(cam.y - h / 2, 0, H));
    miniView.setAttribute("width", Math.min(w, W));
    miniView.setAttribute("height", Math.min(h, H));
  }

  function animateCamera(target, duration = 1200) {
    cancelAnimationFrame(camRaf);
    if (reduced) {
      cam = { ...cam, ...target };
      applyCamera();
      return Promise.resolve();
    }
    const from = { ...cam };
    const t0 = performance.now();
    stage.classList.add("is-flying");
    return new Promise((resolve) => {
      const step = (now) => {
        const k = Math.min(1, (now - t0) / duration);
        const e = easeInOut(k);
        cam.x = lerp(from.x, target.x, e);
        cam.y = lerp(from.y, target.y, e);
        cam.scale = lerp(from.scale, target.scale, e);
        applyCamera();
        if (k < 1) camRaf = requestAnimationFrame(step);
        else {
          stage.classList.remove("is-flying");
          resolve();
        }
      };
      camRaf = requestAnimationFrame(step);
    });
  }

  function startCamera() {
    if (vh < 520) {
      return { x: 1500, y: 1850, scale: Math.max(minScale, 0.46) };
    }
    const scale =
      vw < 560
        ? Math.max(minScale, 0.68)
        : clamp(vw / 2000, Math.max(minScale, 0.64), 0.78);
    return {
      x: 1500,
      y: vw < 820 ? 1800 : 1700,
      scale,
    };
  }

  function fitCamera() {
    const top = vw <= 820 ? 88 : 96;
    const bottom = vw <= 820 ? 96 : 74;
    const scale = Math.min(vw / W, (vh - top - bottom) / H);
    const bandCentre = (top + vh - bottom) / 2;
    return {
      x: W / 2,
      y: H / 2 + (vh / 2 - bandCentre) / scale,
      scale: Math.max(scale, 0.06),
    };
  }

  function introCamera() {
    if (vh < 520) return { x: 1500, y: 1500, scale: 0.4 };
    if (vw < 820) return { x: 1500, y: 1450, scale: 0.45 };
    return { x: 1500, y: 1180, scale: Math.max(minScale, 0.42) };
  }

  function zoomAt(clientX, clientY, factor) {
    const rect = stage.getBoundingClientRect();
    const sx = (clientX - rect.left) / rect.width;
    const sy = (clientY - rect.top) / rect.height;
    const newScale = clamp(cam.scale * factor, minScale, maxScale);
    if (newScale === cam.scale) return;
    const wx = cam.x + (sx - 0.5) * (vw / cam.scale);
    const wy = cam.y + (sy - 0.5) * (vh / cam.scale);
    cam.scale = newScale;
    cam.x = wx - (sx - 0.5) * (vw / cam.scale);
    cam.y = wy - (sy - 0.5) * (vh / cam.scale);
    applyCamera();
  }

  function targetForNode(node, scale = 1.02) {
    const x = node._x ?? (node.anchor ? node.anchor[0] : 1500);
    const y = node._y ?? (node.anchor ? node.anchor[1] : 1500);
    const s = clamp(Math.max(scale, Math.min(cam.scale, 1.3)), minScale, maxScale);
    let tx = x;
    let ty = y;
    if (vw <= 820) {
      tx = x;
      ty = y + (0.5 - 0.105) * (vh / s);
    } else {
      tx = x + (0.5 - (vw - 442) / (2 * vw)) * (vw / s);
      ty = y;
    }
    return { x: tx, y: ty, scale: s };
  }

  /* --------------------------------------------------------------- pan/zoom */

  const pointers = new Map();
  let dragStart = null;
  let pinchStart = null;
  let dragCaptured = false;

  stage.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    if (e.target.closest(".node")) {
      // still allow a drag that starts on a node
    }
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    moved = false;
    dragCaptured = false;
    dragStart = { x: e.clientX, y: e.clientY, camX: cam.x, camY: cam.y };
    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      pinchStart = {
        dist: Math.hypot(a.x - b.x, a.y - b.y),
        mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 },
        scale: cam.scale,
        camX: cam.x,
        camY: cam.y,
      };
    }
    stage.classList.add("is-dragging");
  });

  stage.addEventListener("pointermove", (e) => {
    if (!pointers.has(e.pointerId)) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });

    if (pointers.size === 2 && pinchStart) {
      const [a, b] = [...pointers.values()];
      const dist = Math.hypot(a.x - b.x, a.y - b.y);
      const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      if (pinchStart.dist > 0) {
        const rect = stage.getBoundingClientRect();
        const startSx = (pinchStart.mid.x - rect.left) / rect.width;
        const startSy = (pinchStart.mid.y - rect.top) / rect.height;
        const wx = pinchStart.camX + (startSx - 0.5) * (vw / pinchStart.scale);
        const wy = pinchStart.camY + (startSy - 0.5) * (vh / pinchStart.scale);
        const s = clamp(pinchStart.scale * (dist / pinchStart.dist), minScale, maxScale);
        const sx = (mid.x - rect.left) / rect.width;
        const sy = (mid.y - rect.top) / rect.height;
        cam.scale = s;
        cam.x = wx - (sx - 0.5) * (vw / s);
        cam.y = wy - (sy - 0.5) * (vh / s);
        applyCamera();
        moved = true;
      }
      return;
    }

    if (!dragStart) return;
    const dx = e.clientX - dragStart.x;
    const dy = e.clientY - dragStart.y;
    if (Math.abs(dx) > 4 || Math.abs(dy) > 4) moved = true;
    if (!moved) return;
    if (!dragCaptured) {
      try {
        stage.setPointerCapture(e.pointerId);
        dragCaptured = true;
      } catch (_) {}
    }
    cam.x = dragStart.camX - dx / cam.scale;
    cam.y = dragStart.camY - dy / cam.scale;
    applyCamera();
  });

  function endPointer(e) {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinchStart = null;
    if (pointers.size === 0) {
      stage.classList.remove("is-dragging");
      dragStart = null;
      dragCaptured = false;
      window.setTimeout(() => {
        moved = false;
      }, 0);
    }
  }
  stage.addEventListener("pointerup", endPointer);
  stage.addEventListener("pointercancel", endPointer);

  stage.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const factor = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0016));
      zoomAt(e.clientX, e.clientY, factor);
    },
    { passive: false }
  );

  stage.addEventListener("dblclick", (e) => {
    e.preventDefault();
    zoomAt(e.clientX, e.clientY, e.shiftKey ? 0.6 : 1.6);
  });

  /* --------------------------------------------------------------- nodes */

  function ancestorsOf(id) {
    const out = new Set();
    let n = byId.get(id);
    while (n && n.parent) {
      out.add(n.parent);
      n = byId.get(n.parent);
    }
    return out;
  }

  function descendantsOf(id) {
    const out = new Set();
    const walk = (nid) => {
      for (const child of nodes) {
        if (child.parent === nid && !out.has(child.id)) {
          out.add(child.id);
          walk(child.id);
        }
      }
    };
    walk(id);
    return out;
  }

  function clearHot() {
    stage.classList.remove("has-hot");
    svg.querySelectorAll(".node.is-hot,.node.is-ancestor").forEach((n) => {
      n.classList.remove("is-hot", "is-ancestor");
    });
  }

  function setHot(id) {
    clearHot();
    const node = byId.get(id);
    if (!node) return;
    const hot = svg.querySelector(`.node[data-id="${id}"]`);
    if (hot) hot.classList.add("is-hot");
    for (const a of ancestorsOf(id)) {
      const el = svg.querySelector(`.node[data-id="${a}"]`);
      if (el) el.classList.add("is-ancestor");
    }
    for (const d of descendantsOf(id)) {
      const el = svg.querySelector(`.node[data-id="${d}"]`);
      if (el) el.classList.add("is-ancestor");
    }
    stage.classList.add("has-hot");
  }

  svg.querySelectorAll(".node").forEach((el) => {
    const id = el.getAttribute("data-id");
    el.addEventListener("pointerenter", () => {
      if (!moved && !stage.classList.contains("is-dragging")) setHot(id);
    });
    el.addEventListener("pointerleave", clearHot);
    el.addEventListener("click", (e) => {
      if (moved) return;
      e.preventDefault();
      e.stopPropagation();
      openNode(id, { focus: true });
    });
    el.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openNode(id, { focus: true });
      }
    });
    el.addEventListener("focus", () => setHot(id));
    el.addEventListener("blur", clearHot);
  });

  stage.addEventListener("click", (e) => {
    if (moved) return;
    if (e.target.closest(".node")) return;
    closePanel();
  });

  /* --------------------------------------------------------------- panel */

  function renderPanel(node) {
    const colour = statusColour(node.status);
    panel.style.setProperty("--s", colour);
    panelStatus.style.setProperty("--s", colour);
    panelEyebrow.textContent = [node.code || node.version || "", branchName(node.branch)]
      .filter(Boolean)
      .join(" · ");
    panelStatus.textContent = statusLabel(node.status);
    panelTitle.textContent = node.fullTitle || node.title;
    panelSummary.textContent = node.summary || "";

    let html = "";
    const parent = node.parent ? byId.get(node.parent) : null;

    if (parent) {
      html += `<button class="panel-parent" data-node="${esc(parent.id)}" style="--s:${statusColour(
        parent.status
      )}"><i></i>${esc(parent.code || parent.version || "")} · ${esc(parent.title)}</button>`;
    }

    if (node.demo) {
      html += `<h3>The demo</h3><blockquote class="panel-demo">${fmt(node.demo)}</blockquote>`;
    }
    if (node.note) {
      html += `<p class="panel-note">${fmt(node.note)}</p>`;
    }
    if (node.criteria && node.criteria.length) {
      html += `<h3>What must be true</h3><ul class="panel-list">${node.criteria
        .map((c) => `<li>${fmt(c)}</li>`)
        .join("")}</ul>`;
    }
    if (node.includes && node.includes.length) {
      html += `<h3>In the release</h3><ul class="panel-list tags">${node.includes
        .map((i) => `<li>${esc(i)}</li>`)
        .join("")}</ul>`;
    }
    if (node.items && node.items.length) {
      html += `<h3>The workstream</h3><ul class="panel-list tags">${node.items
        .map((i) => `<li>${esc(i)}</li>`)
        .join("")}</ul>`;
    }
    if (node.reason) {
      html += `<h3>Why it was pruned</h3><p class="panel-demo">${fmt(node.reason)}</p>`;
    }
    if (node.kind === "feature" && parent) {
      html += `<h3>Why it matters</h3><p class="panel-note">This is one of the things that has to be true for ${esc(
        parent.code || ""
      )} ${esc(parent.title)} to be finished. The full acceptance criteria live on the milestone.</p>`;
    }

    const meta = [];
    if (node.deps) meta.push(["Depends on", node.deps]);
    if (node.estimate) meta.push(["Estimate", node.estimate]);
    if (node.risks) meta.push(["Risks", node.risks]);
    if (node.audience) meta.push(["Audience", node.audience]);
    if (node.status) meta.push(["State", statuses[node.status].note]);
    if (meta.length) {
      html += `<h3>Notes</h3><dl class="panel-meta">${meta
        .map(([k, v]) => `<div><dt>${esc(k)}</dt><dd>${fmt(v)}</dd></div>`)
        .join("")}</dl>`;
    }

    panelBody.innerHTML = html;

    let doc = node.docs;
    if (!doc && node.kind === "release") doc = "../plan/milestones.html#2-release-map";
    if (!doc && node.kind === "pruned") doc = "../plan/milestones.html#5-scope-cuts";
    if (!doc && node.kind === "workstream") doc = "../plan/index.html";
    if (!doc && node.kind === "seed") doc = "../index.html";
    if (!doc && parent && parent.docs) doc = parent.docs;
    if (doc) {
      panelDoc.href = doc;
      panelDoc.hidden = false;
      const labels = {
        pruned: "Read the scope cuts",
        workstream: "Read the plan",
        release: "Read the release map",
        seed: "Read the documentation",
      };
      panelDoc.querySelector("span").textContent = labels[node.kind] || "Read the full milestone";
    } else {
      panelDoc.hidden = true;
    }
    panel.querySelector(".panel-scroll").scrollTop = 0;
  }

  function openNode(id, opts = {}) {
    const node = byId.get(id);
    if (!node) return;
    if (!introDismissed) dismissIntro(false);
    selectedId = id;
    lastFocused = document.activeElement;
    renderPanel(node);
    panel.classList.add("is-open");
    panel.setAttribute("aria-hidden", "false");
    document.body.classList.add("panel-open");
    svg.querySelectorAll(".node.is-selected").forEach((n) => n.classList.remove("is-selected"));
    const el = svg.querySelector(`.node[data-id="${id}"]`);
    if (el) el.classList.add("is-selected");
    setActiveFor(node);
    if (opts.focus !== false) {
      history.replaceState(null, "", `#${id}`);
      animateCamera(targetForNode(node), 1000);
    }
    if (!opts.keepHot) setHot(id);
  }

  function closePanel() {
    if (!panel.classList.contains("is-open")) return;
    panel.classList.remove("is-open");
    panel.setAttribute("aria-hidden", "true");
    document.body.classList.remove("panel-open");
    selectedId = null;
    clearHot();
    svg.querySelectorAll(".node.is-selected").forEach((n) => n.classList.remove("is-selected"));
    history.replaceState(null, "", location.pathname + location.search);
    if (lastFocused && lastFocused.focus) {
      try {
        lastFocused.focus({ preventScroll: true });
      } catch (_) {}
    }
  }

  panel.querySelector("[data-close-panel]").addEventListener("click", closePanel);
  panel.addEventListener("click", (e) => {
    const link = e.target.closest("[data-node]");
    if (link) openNode(link.getAttribute("data-node"), { focus: true });
  });

  /* ----------------------------------------------------------- rail / state */

  function setActiveFor(node) {
    let m = node;
    while (m && m.kind !== "milestone") m = m.parent ? byId.get(m.parent) : null;
    if (m) setActiveMilestone(m.id);
  }

  function setActiveMilestone(id) {
    if (activeMilestone === id) return;
    activeMilestone = id;
    rail.querySelectorAll(".rail-item").forEach((el) => {
      el.classList.toggle("is-active", el.getAttribute("data-node") === id);
    });
  }

  rail.querySelectorAll(".rail-item").forEach((el) => {
    el.addEventListener("click", () => {
      const id = el.getAttribute("data-node");
      openNode(id, { focus: true });
    });
  });

  document.querySelectorAll("[data-node]").forEach((el) => {
    if (el.closest(".rail") || el.closest(".panel")) return;
    el.addEventListener("click", () => {
      if (intro && !intro.classList.contains("is-leaving")) dismissIntro(false);
      openNode(el.getAttribute("data-node"), { focus: true });
    });
  });

  /* --------------------------------------------------------------- filters */

  const offStatuses = new Set();
  document.querySelectorAll(".lg-chip").forEach((chip) => {
    chip.setAttribute("aria-pressed", "true");
    chip.addEventListener("click", () => {
      const s = chip.getAttribute("data-filter");
      if (offStatuses.has(s)) {
        offStatuses.delete(s);
        chip.classList.remove("is-off");
        chip.setAttribute("aria-pressed", "true");
      } else {
        offStatuses.add(s);
        chip.classList.add("is-off");
        chip.setAttribute("aria-pressed", "false");
      }
      for (const key of Object.keys(statuses)) {
        stage.classList.toggle(`off-${key}`, offStatuses.has(key));
      }
    });
  });

  /* --------------------------------------------------------------- search */

  const searchable = nodes.filter((n) => n.kind !== "seed");
  const isMac = /Mac|iPhone|iPad|iPod/.test(navigator.platform || "");
  const kbd = document.querySelector(".search-trigger kbd");
  if (kbd) kbd.textContent = isMac ? "⌘K" : "Ctrl K";
  const normalise = (s) => String(s || "").toLowerCase();
  const searchHay = new Map(
    searchable.map((n) => [
      n.id,
      normalise(
        [
          n.title,
          n.fullTitle,
          n.code,
          n.version,
          n.summary,
          statusLabel(n.status),
          branchName(n.branch),
        ].join(" ")
      ),
    ])
  );

  function scoreNode(n, q) {
    const title = normalise(n.title);
    const full = normalise(n.fullTitle);
    const code = normalise(n.code || n.version);
    let score = 0;
    if (code === q) score += 100;
    else if (code.startsWith(q)) score += 60;
    if (title.startsWith(q)) score += 50;
    else if (title.includes(q)) score += 30;
    if (full.includes(q)) score += 12;
    if (normalise(n.summary).includes(q)) score += 8;
    if (searchHay.get(n.id).includes(q)) score += 2;
    if (n.kind === "milestone") score += 6;
    return score;
  }

  function openSearch() {
    if (!searchLayer.hidden) return;
    searchReturnFocus = document.activeElement;
    searchLayer.hidden = false;
    searchInput.value = "";
    searchInput.focus();
    renderSearch("");
  }

  function closeSearch() {
    searchLayer.hidden = true;
    searchMatches = [];
    searchIndex = 0;
    if (searchReturnFocus && searchReturnFocus.focus) {
      try {
        searchReturnFocus.focus({ preventScroll: true });
      } catch (_) {}
    }
    searchReturnFocus = null;
  }

  function renderSearch(q) {
    q = normalise(q.trim());
    const labelFor = (n) => {
      if (n.code) return n.code;
      if (n.version) return n.version;
      if (n.kind === "feature") {
        const p = byId.get(n.parent);
        return p && p.code ? p.code : "FEA";
      }
      if (n.kind === "workstream") return "WS";
      if (n.kind === "pruned") return "×";
      return n.kind.slice(0, 3).toUpperCase();
    };
    searchMatches = q
      ? searchable
          .map((n) => ({ n, s: scoreNode(n, q) }))
          .filter((r) => r.s > 0)
          .sort((a, b) => b.s - a.s || a.n.title.localeCompare(b.n.title))
          .slice(0, 10)
          .map((r) => r.n)
      : ["m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m0", "m1", "m10"]
          .map((id) => byId.get(id))
          .filter(Boolean);
    searchIndex = 0;
    if (!searchMatches.length) {
      searchResults.innerHTML = `<li class="search-empty">No branch matches “${esc(q)}”.</li>`;
      return;
    }
    searchResults.innerHTML = searchMatches
      .map((n, i) => {
        const label = labelFor(n);
        return `<li><button class="search-result ${i === 0 ? "is-active" : ""}" data-id="${esc(
          n.id
        )}" role="option" aria-selected="${i === 0}" style="--s:${statusColour(n.status)}">
          <span class="sr-code">${esc(label)}</span>
          <span><span class="sr-title">${esc(n.fullTitle || n.title)}</span></span>
          <span class="sr-branch"><span class="sr-dot"></span>${esc(statusLabel(n.status))}</span>
        </button></li>`;
      })
      .join("");
  }

  function selectSearch(i) {
    searchIndex = (i + searchMatches.length) % searchMatches.length;
    searchResults.querySelectorAll(".search-result").forEach((el, j) => {
      el.classList.toggle("is-active", j === searchIndex);
      el.setAttribute("aria-selected", String(j === searchIndex));
      if (j === searchIndex) el.scrollIntoView({ block: "nearest" });
    });
  }

  searchInput.addEventListener("input", () => renderSearch(searchInput.value));
  searchInput.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectSearch(searchIndex + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      selectSearch(searchIndex - 1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const n = searchMatches[searchIndex];
      if (n) {
        closeSearch();
        openNode(n.id, { focus: true });
      }
    }
  });
  searchResults.addEventListener("click", (e) => {
    const btn = e.target.closest(".search-result");
    if (!btn) return;
    closeSearch();
    openNode(btn.getAttribute("data-id"), { focus: true });
  });
  searchLayer.addEventListener("click", (e) => {
    if (e.target === searchLayer) closeSearch();
  });
  document.querySelector("[data-open-search]").addEventListener("click", openSearch);

  /* ---------------------------------------------------------------- intro */

  let introDismissed = false;

  function setChromeInert(on) {
    for (const el of chrome) {
      if (on) el.setAttribute("inert", "");
      else el.removeAttribute("inert");
    }
  }

  function dismissIntro(goToTree = true) {
    if (introDismissed) return;
    introDismissed = true;
    setChromeInert(false);
    intro.classList.add("is-leaving");
    document.body.classList.remove("intro-open");
    window.setTimeout(() => {
      intro.style.display = "none";
    }, 750);
    if (goToTree) {
      animateCamera(startCamera(), 1650);
      window.setTimeout(() => {
        // reveal the selected "now growing" node after the swoop
        if (!selectedId) setActiveMilestone("m2");
        if (!panel.classList.contains("is-open")) stage.focus({ preventScroll: true });
      }, 1200);
    }
  }

  document.querySelector("[data-enter]").addEventListener("click", () => dismissIntro(true));
  intro.addEventListener("click", (e) => {
    if (e.target.closest("button, a")) return;
    dismissIntro(true);
  });

  /* ----------------------------------------------------------------- about */

  function buildAboutStatuses() {
    const el = document.getElementById("about-statuses");
    el.innerHTML = Object.entries(statuses)
      .map(
        ([key, s]) =>
          `<span class="about-status" style="--s:${s.colour}" title="${esc(s.note)}"><i></i>${esc(
            s.label
          )} — ${esc(s.note)}</span>`
      )
      .join("");
  }
  buildAboutStatuses();

  function openAbout() {
    if (typeof about.showModal === "function") about.showModal();
    else about.setAttribute("open", "");
  }
  function closeAbout() {
    if (typeof about.close === "function") about.close();
    else about.removeAttribute("open");
  }
  document.querySelector("[data-open-about]").addEventListener("click", openAbout);
  document.querySelector("[data-close-about]").addEventListener("click", closeAbout);
  about.addEventListener("click", (e) => {
    if (e.target === about) closeAbout();
  });

  /* -------------------------------------------------------------- keyboard */

  document.addEventListener("keydown", (e) => {
    const typing = /input|textarea|select/i.test(document.activeElement.tagName);
    if (e.key === "Escape") {
      if (!searchLayer.hidden) closeSearch();
      else if (about.open) closeAbout();
      else if (panel.classList.contains("is-open")) closePanel();
      else if (!introDismissed) dismissIntro(true);
      return;
    }
    if (about.open) return;
    if (typing) return;
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      openSearch();
      return;
    }
    if (e.key === "/") {
      e.preventDefault();
      openSearch();
      return;
    }
    if (e.key === "?") {
      e.preventDefault();
      openAbout();
      return;
    }
    if (e.key === "0") {
      animateCamera(fitCamera(), 900);
      return;
    }
    const step = e.shiftKey ? 220 : 80;
    if (e.key === "ArrowLeft") {
      e.preventDefault();
      cam.x -= step / cam.scale;
      applyCamera();
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      cam.x += step / cam.scale;
      applyCamera();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      cam.y -= step / cam.scale;
      applyCamera();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      cam.y += step / cam.scale;
      applyCamera();
    } else if (e.key === "+" || e.key === "=") {
      e.preventDefault();
      zoomAt(vw / 2, vh / 2, 1.25);
    } else if (e.key === "-" || e.key === "_") {
      e.preventDefault();
      zoomAt(vw / 2, vh / 2, 0.8);
    }
  });

  /* ------------------------------------------------------------- controls */

  document.querySelectorAll("[data-zoom]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const mode = btn.getAttribute("data-zoom");
      if (mode === "in") zoomAt(vw / 2, vh / 2, 1.3);
      else if (mode === "out") zoomAt(vw / 2, vh / 2, 0.77);
      else animateCamera(fitCamera(), 1000);
    });
  });

  /* -------------------------------------------------------------- minimap */

  let miniDrag = false;
  function miniMove(clientX, clientY) {
    const rect = minimap.getBoundingClientRect();
    const x = ((clientX - rect.left) / rect.width) * W;
    const y = ((clientY - rect.top) / rect.height) * H;
    cam.x = clamp(x, 0, W);
    cam.y = clamp(y, 0, H);
    applyCamera();
  }
  minimap.addEventListener("pointerdown", (e) => {
    miniDrag = true;
    minimap.setPointerCapture(e.pointerId);
    miniMove(e.clientX, e.clientY);
  });
  minimap.addEventListener("pointermove", (e) => {
    if (miniDrag) miniMove(e.clientX, e.clientY);
  });
  minimap.addEventListener("pointerup", () => {
    miniDrag = false;
  });
  minimap.addEventListener("pointercancel", () => {
    miniDrag = false;
  });

  /* ----------------------------------------------------------------- grow */

  function growTree() {
    if (reduced) return;
    const paths = svg.querySelectorAll(".trunk, .trunk-light, .bough, .bough-light, .bough-shadow");
    paths.forEach((p, i) => {
      let len = 0;
      try {
        len = p.getTotalLength();
      } catch (_) {
        return;
      }
      if (!len) return;
      p.style.strokeDasharray = `${len}`;
      p.style.strokeDashoffset = `${len}`;
      p.animate(
        [{ strokeDashoffset: len }, { strokeDashoffset: 0 }],
        {
          duration: 1500,
          delay: i * 7,
          easing: "cubic-bezier(.22,.61,.36,1)",
          fill: "forwards",
        }
      );
    });
  }

  /* --------------------------------------------------------------- resize */

  function onResize() {
    measure();
    applyCamera();
    if (panel.classList.contains("is-open") && selectedId) {
      const node = byId.get(selectedId);
      if (node) animateCamera(targetForNode(node), 300);
    }
  }
  window.addEventListener("resize", onResize);

  /* ----------------------------------------------------------------- boot */

  function boot() {
    measure();
    document.body.classList.add("intro-open");
    setChromeInert(true);
    cam = introCamera();
    applyCamera();
    setActiveMilestone("m2");
    growTree();

    const hash = location.hash.slice(1);
    if (hash && byId.has(hash)) {
      introDismissed = true;
      intro.style.display = "none";
      setChromeInert(false);
      document.body.classList.remove("intro-open");
      window.setTimeout(() => openNode(hash, { focus: true }), 250);
    }

    window.addEventListener("hashchange", () => {
      const id = location.hash.slice(1);
      if (id && byId.has(id)) openNode(id, { focus: true });
      else closePanel();
    });
  }

  boot();
})();
