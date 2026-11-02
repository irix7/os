(() => {
  "use strict";

  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const finePointer = window.matchMedia("(pointer: fine)").matches;

  const sceneEls = $$(".scene");
  const poster = $("#poster");
  const film = $("#film");
  const playBtn = $("[data-play]");
  const timeNow = $("#time-now");
  const timeTotal = $("#time-total");
  const runtime = $("#runtime");
  const scrub = $("#scrub");
  const scrubSegments = $("#scrub-segments");
  const scrubFill = $("#scrub-fill");
  const scrubHead = $("#scrub-head");
  const captionBar = $("#caption-bar");
  const captionKicker = $("#caption-kicker");
  const captionText = $("#caption-text");
  const chapterIndex = $("#chapter-index");
  const chapterName = $("#chapter-name");
  const chapterMenu = $("#chapter-menu");
  const chapterGrid = $("#chapter-grid");
  const transcript = $("#transcript");
  const transcriptBody = $("#transcript-body");
  const topbar = $(".topbar");

  const scenes = sceneEls.map((el, i) => {
    const duration = Number(el.dataset.duration) || 20;
    return {
      el,
      index: i,
      number: el.dataset.scene || String(i + 1),
      chapter: el.dataset.chapter || String(i + 1).padStart(2, "0"),
      title: el.dataset.title || "",
      duration,
      start: 0,
      cues: $$(".cue", el).map((cue) => ({
        at: Number(cue.dataset.at) || 0,
        text: cue.textContent.trim(),
      })),
    };
  });
  let cursor = 0;
  for (const scene of scenes) {
    scene.start = cursor;
    cursor += scene.duration;
  }
  const total = cursor;

  sceneEls.forEach((el) => el.setAttribute("aria-hidden", "true"));
  film.setAttribute("aria-hidden", "true");

  const state = {
    elapsed: 0,
    playing: false,
    started: false,
    scene: -1,
    last: 0,
    raf: 0,
    caption: "",
    seeking: false,
  };

  const format = (seconds) => {
    const s = Math.max(0, Math.floor(seconds));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };
  const runtimeLabel = () => {
    const m = Math.floor(total / 60);
    const s = total % 60;
    return `${m} min ${String(s).padStart(2, "0")} sec`;
  };

  timeTotal.textContent = format(total);
  runtime.textContent = runtimeLabel();
  if (reduced) {
    const note = document.createElement("p");
    note.className = "poster-note";
    note.textContent = "Reduced motion is on — the film will run as a captioned slideshow.";
    $(".poster-actions").after(note);
  }

  /* ---------------------------------------------------------------- backdrop */
  const starField = $(".stars");
  for (let i = 0; i < 110; i += 1) {
    const star = document.createElement("i");
    star.className = "star";
    star.style.left = `${Math.random() * 100}%`;
    star.style.top = `${Math.random() * 100}%`;
    star.style.setProperty("--t", `${(4 + Math.random() * 8).toFixed(1)}s`);
    star.style.setProperty("--d", `${(-Math.random() * 8).toFixed(1)}s`);
    star.style.opacity = (0.15 + Math.random() * 0.6).toFixed(2);
    starField.append(star);
  }
  const warp = $(".warp");
  for (let i = 0; i < 30; i += 1) {
    const streak = document.createElement("i");
    streak.className = "warp-streak";
    streak.style.setProperty("--a", `${(i / 30) * 360 + Math.random() * 8}deg`);
    streak.style.setProperty("--l", `${35 + Math.random() * 35}%`);
    streak.style.setProperty("--w", `${0.5 + Math.random() * 1.4}px`);
    streak.style.setProperty("--d", `${(1.1 + Math.random() * 1.4).toFixed(2)}s`);
    streak.style.setProperty("--delay", `${(-Math.random() * 2).toFixed(2)}s`);
    warp.append(streak);
  }

  /* ------------------------------------------------------------------- build */
  scenes.forEach((scene, i) => {
    const seg = document.createElement("div");
    seg.className = "scrub-seg";
    seg.style.flex = `${scene.duration} 1 0`;
    seg.dataset.index = String(i);
    seg.title = `${scene.chapter} · ${scene.title} · ${format(scene.duration)}`;
    scrubSegments.append(seg);

    const card = document.createElement("button");
    card.className = "chapter-card";
    card.type = "button";
    card.dataset.index = String(i);
    card.innerHTML = `
      <span class="ch-art" aria-hidden="true"></span>
      <span class="ch-no">${scene.chapter}</span>
      <span class="ch-title">${scene.title}</span>
      <span class="ch-time">${format(scene.duration)}</span>`;
    card.addEventListener("click", () => {
      closeChapters();
      startAtChapter(i);
    });
    chapterGrid.append(card);

    const article = document.createElement("article");
    article.className = "tr-chapter";
    article.innerHTML = `<h3>${scene.chapter} · ${format(scene.duration)}</h3><h4>${scene.title}</h4>`;
    for (const cue of scene.cues) {
      const p = document.createElement("p");
      p.textContent = cue.text;
      article.append(p);
    }
    transcriptBody.append(article);
  });

  /* ------------------------------------------------------------------ helpers */
  const currentScene = (elapsed) => {
    for (let i = scenes.length - 1; i >= 0; i -= 1) {
      if (elapsed >= scenes[i].start) return i;
    }
    return 0;
  };

  const activateScene = (index) => {
    if (state.scene === index) return;
    state.scene = index;
    scenes.forEach((scene, i) => {
      const active = i === index;
      if (active) {
        scene.el.classList.remove("is-active");
        void scene.el.offsetWidth;
        scene.el.classList.add("is-active");
        scene.el.setAttribute("aria-hidden", "false");
      } else {
        scene.el.classList.remove("is-active");
        scene.el.setAttribute("aria-hidden", "true");
      }
    });
    const scene = scenes[index];
    chapterIndex.textContent = scene.chapter;
    chapterName.textContent = scene.title;
    $$(".chapter-card", chapterGrid).forEach((card, i) => {
      card.classList.toggle("is-current", i === index);
    });
    state.caption = "";
    animateCounters(scene.el);
    updateCaption(scene, 0);
    try { history.replaceState(null, "", `${location.pathname}${location.search}#chapter-${index + 1}`); } catch (_) {}
  };

  const animateCounters = (root) => {
    $$("[data-count]", root).forEach((el) => {
      const target = Number(el.dataset.count) || 0;
      const comma = el.dataset.format === "comma";
      const formatNumber = (v) => (comma ? Math.round(v).toLocaleString("en-AU") : String(Math.round(v)));
      if (el._countRaf) cancelAnimationFrame(el._countRaf);
      if (reduced) {
        el.textContent = formatNumber(target);
        return;
      }
      el.textContent = "0";
      const start = performance.now();
      const duration = target > 1000 ? 1700 : 1300;
      const tick = (now) => {
        const k = clamp((now - start) / duration, 0, 1);
        const eased = 1 - Math.pow(1 - k, 3);
        el.textContent = formatNumber(target * eased);
        if (k < 1) el._countRaf = requestAnimationFrame(tick);
      };
      el._countRaf = requestAnimationFrame(tick);
    });
  };

  const updateCaption = (scene, sceneTime) => {
    let cue = scene.cues[0];
    for (const c of scene.cues) if (sceneTime >= c.at) cue = c;
    if (!cue) return;
    if (cue.text !== state.caption) {
      state.caption = cue.text;
      captionText.textContent = cue.text;
      captionKicker.textContent = `${scene.chapter} · ${scene.title}`;
      captionBar.classList.remove("is-changing");
      void captionBar.offsetWidth;
      captionBar.classList.add("is-changing");
    }
  };

  const updateUI = () => {
    const index = currentScene(state.elapsed);
    const scene = scenes[index];
    if (index !== state.scene) activateScene(index);
    const sceneTime = state.elapsed - scene.start;
    updateCaption(scene, sceneTime);
    const pct = total ? (state.elapsed / total) * 100 : 0;
    scrubFill.style.width = `${pct}%`;
    scrubHead.style.left = `${pct}%`;
    scrub.setAttribute("aria-valuenow", pct.toFixed(1));
    timeNow.textContent = format(state.elapsed);
    $$(".scrub-seg", scrubSegments).forEach((seg, i) => {
      seg.classList.toggle("seen", state.elapsed >= scenes[i].start + scenes[i].duration);
    });
  };

  const tick = (now) => {
    if (!state.playing) return;
    const delta = (now - state.last) / 1000;
    state.last = now;
    state.elapsed += delta;
    if (state.elapsed >= total) {
      state.elapsed = total;
      updateUI();
      pause(true);
      return;
    }
    updateUI();
    state.raf = requestAnimationFrame(tick);
  };

  const play = () => {
    if (!state.started) {
      startFilm(true);
      return;
    }
    if (state.elapsed >= total) {
      state.elapsed = 0;
      state.scene = -1;
      updateUI();
    }
    state.playing = true;
    state.last = performance.now();
    document.body.classList.add("is-playing");
    document.body.classList.remove("is-ended");
    playBtn.setAttribute("aria-label", "Pause");
    cancelAnimationFrame(state.raf);
    state.raf = requestAnimationFrame(tick);
  };

  const pause = (ended = false) => {
    state.playing = false;
    cancelAnimationFrame(state.raf);
    document.body.classList.remove("is-playing");
    playBtn.setAttribute("aria-label", ended ? "Replay" : "Play");
    if (ended) document.body.classList.add("is-ended");
  };

  const seek = (seconds, keepPlaying = state.playing) => {
    state.elapsed = clamp(seconds, 0, total);
    if (state.elapsed < total) document.body.classList.remove("is-ended");
    state.scene = -1;
    updateUI();
    if (keepPlaying) {
      state.playing = true;
      state.last = performance.now();
      cancelAnimationFrame(state.raf);
      state.raf = requestAnimationFrame(tick);
    }
  };

  const startAtChapter = (index, autoplay = true) => {
    if (!state.started) startFilm(false);
    seek(scenes[index].start, false);
    if (autoplay) play();
    else pause();
  };

  const startFilm = (autoplay = true) => {
    if (state.started) {
      if (autoplay) play();
      return;
    }
    state.started = true;
    document.body.classList.add("started");
    poster.classList.add("is-hidden");
    poster.setAttribute("aria-hidden", "true");
    film.removeAttribute("aria-hidden");
    film.focus({ preventScroll: true });
    seek(0, false);
    if (autoplay) play();
    else pause();
  };

  /* ----------------------------------------------------------------- controls */
  playBtn.addEventListener("click", () => {
    if (!state.started) startFilm(true);
    else if (state.playing) pause();
    else play();
  });

  $("[data-prev]").addEventListener("click", () => {
    const i = currentScene(state.elapsed);
    const target = state.elapsed - scenes[i].start > 3 ? i : Math.max(0, i - 1);
    startAtChapter(target, state.playing);
  });
  $("[data-next]").addEventListener("click", () => {
    const i = currentScene(state.elapsed);
    startAtChapter(Math.min(scenes.length - 1, i + 1), state.playing);
  });

  $("[data-start]").addEventListener("click", () => startFilm(true));

  scrub.addEventListener("click", (e) => {
    if (!state.started) startFilm(false);
    const rect = scrub.getBoundingClientRect();
    const p = clamp((e.clientX - rect.left) / rect.width, 0, 1);
    seek(p * total, state.playing);
  });
  scrub.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight") {
      e.preventDefault();
      seek(state.elapsed + 10, state.playing);
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      seek(state.elapsed - 10, state.playing);
    } else if (e.key === "Home") {
      e.preventDefault();
      seek(0, state.playing);
    } else if (e.key === "End") {
      e.preventDefault();
      seek(total, false);
    }
  });

  const openChapters = () => {
    if (state.playing) pause();
    chapterMenu.hidden = false;
    const current = currentScene(state.elapsed);
    $$(".chapter-card", chapterGrid).forEach((card, i) => card.classList.toggle("is-current", i === current));
    $("[data-open-chapters]").setAttribute("aria-expanded", "true");
    const active = $(".chapter-card.is-current", chapterGrid);
    if (active) active.focus({ preventScroll: true });
  };
  const closeChapters = () => {
    chapterMenu.hidden = true;
    $("[data-open-chapters]").setAttribute("aria-expanded", "false");
  };
  $("[data-open-chapters]").addEventListener("click", openChapters);
  $$("[data-close-chapters]").forEach((b) => b.addEventListener("click", closeChapters));
  chapterMenu.addEventListener("click", (e) => {
    if (e.target === chapterMenu) closeChapters();
  });

  let transcriptReturn = null;
  const openTranscript = () => {
    if (state.playing) pause();
    transcriptReturn = document.activeElement;
    transcript.classList.add("is-open");
    transcript.setAttribute("aria-hidden", "false");
    transcriptBody.focus({ preventScroll: true });
  };
  const closeTranscript = () => {
    transcript.classList.remove("is-open");
    transcript.setAttribute("aria-hidden", "true");
    if (transcriptReturn && transcriptReturn.focus) {
      try { transcriptReturn.focus({ preventScroll: true }); } catch (_) {}
    }
    transcriptReturn = null;
  };
  $$("[data-open-transcript]").forEach((b) => b.addEventListener("click", openTranscript));
  $$("[data-close-transcript]").forEach((b) => b.addEventListener("click", closeTranscript));

  $("[data-fullscreen]").addEventListener("click", async () => {
    try {
      if (!document.fullscreenElement) await document.documentElement.requestFullscreen();
      else await document.exitFullscreen();
    } catch (_) {}
  });

  /* -------------------------------------------------------------------- tilt */
  if (finePointer && !reduced) {
    $$(".tilt").forEach((card) => {
      card.addEventListener("pointermove", (e) => {
        const rect = card.getBoundingClientRect();
        const x = (e.clientX - rect.left) / rect.width - 0.5;
        const y = (e.clientY - rect.top) / rect.height - 0.5;
        card.style.transform = `perspective(900px) rotateY(${x * 9}deg) rotateX(${-y * 9}deg)`;
      });
      card.addEventListener("pointerleave", () => {
        card.style.transform = "";
      });
    });
  }

  /* ----------------------------------------------------------------- keyboard */
  document.addEventListener("keydown", (e) => {
    const typing = /input|textarea|select/i.test(document.activeElement?.tagName || "");
    if (typing) return;
    if (e.key === "Escape") {
      if (!chapterMenu.hidden) { closeChapters(); return; }
      if (transcript.classList.contains("is-open")) { closeTranscript(); return; }
      if (!state.started) return;
      pause();
      return;
    }
    if (e.key === " " || e.key === "k") {
      if (/button|a/i.test(document.activeElement?.tagName || "") && e.key === " ") return;
      e.preventDefault();
      playBtn.click();
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      seek(state.elapsed + 10, state.playing);
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      seek(state.elapsed - 10, state.playing);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      $("[data-next]").click();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      $("[data-prev]").click();
    } else if (e.key.toLowerCase() === "c") {
      if (chapterMenu.hidden) openChapters(); else closeChapters();
    } else if (e.key.toLowerCase() === "t") {
      if (transcript.classList.contains("is-open")) closeTranscript(); else openTranscript();
    } else if (e.key.toLowerCase() === "f") {
      $("[data-fullscreen]").click();
    } else if (e.key === "Home") {
      if (state.started) seek(0, state.playing);
    }
  });

  window.addEventListener("resize", () => updateUI());
  document.addEventListener("visibilitychange", () => {
    if (document.hidden && state.playing) pause();
  });

  /* -------------------------------------------------------------------- boot */
  const hash = location.hash.match(/^#chapter-(\d+)$/);
  if (hash) {
    const i = clamp(Number(hash[1]) - 1, 0, scenes.length - 1);
    state.started = true;
    document.body.classList.add("started");
    poster.classList.add("is-hidden");
    poster.setAttribute("aria-hidden", "true");
    film.removeAttribute("aria-hidden");
    seek(scenes[i].start, false);
    pause();
  } else {
    setActivePreview();
  }

  function setActivePreview() {
    scenes[0].el.classList.add("is-active");
    scenes[0].el.setAttribute("aria-hidden", "false");
    state.scene = 0;
    chapterIndex.textContent = scenes[0].chapter;
    chapterName.textContent = scenes[0].title;
  }

  updateUI();
  $$(".chapter-card", chapterGrid)[0]?.classList.add("is-current");
})();
