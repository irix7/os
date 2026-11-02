(() => {
  "use strict";

  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  const player = $("#player");
  const poster = $("#poster");
  const playBtn = $("[data-play]");
  const chapterIndex = $("#chapter-index");
  const chapterName = $("#chapter-name");
  const chapterMenu = $("#chapter-menu");
  const chapterGrid = $("#chapter-grid");
  const transcript = $("#transcript");
  const transcriptBody = $("#transcript-body");
  const scrub = $("#scrub");
  const scrubFill = $("#scrub-fill");
  const scrubHead = $("#scrub-head");
  const timeNow = $("#time-now");
  const timeTotal = $("#time-total");
  const runtime = $("#runtime");

  const state = {
    ready: false,
    started: false,
    playing: false,
    time: 0,
    duration: 358,
    chapter: 0,
    pending: null,
    pendingSeek: null,
    scrubbing: false,
    wasPlaying: false,
    hashHandled: false,
  };

  let chapters = [];
  let captions = [];

  const fallbackChapters = [
    ["A frame on a desk", 24],
    ["The machine has a border", 32],
    ["One computer, many boxes", 30],
    ["Everything gets a name", 30],
    ["Who gets to say yes", 30],
    ["Where things live", 30],
    ["A call across the room", 30],
    ["Pulling the plug", 32],
    ["Old programs, new machine", 32],
    ["The 1994 machine", 32],
    ["What actually runs", 30],
    ["The network is the computer", 26],
  ].map(([title, duration]) => ({ title, duration, start: 0, captions: [] }));

  let cursor = 0;
  for (const chapter of fallbackChapters) {
    chapter.start = cursor;
    cursor += chapter.duration;
  }
  const fallbackDuration = cursor;

  const format = (seconds) => {
    const s = Math.max(0, Math.floor(seconds || 0));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };
  const formatLong = (seconds) => {
    const m = Math.floor(seconds / 60);
    const s = Math.round(seconds % 60);
    return `${m} min ${String(s).padStart(2, "0")} sec`;
  };

  const currentChapter = (time) => {
    if (!chapters.length) return 0;
    for (let i = chapters.length - 1; i >= 0; i -= 1) {
      if (time >= chapters[i].start) return i;
    }
    return 0;
  };

  const updateCaption = () => {
    const text = $("#mobile-caption-text");
    const kicker = $("#mobile-caption-kicker");
    if (!text || !captions.length) return;
    let current = captions[0];
    for (const caption of captions) {
      if (caption.start <= state.time) current = caption;
      else break;
    }
    if (current && text.textContent !== current.text) {
      text.textContent = current.text;
      const index = currentChapter(state.time);
      if (kicker && chapters[index]) {
        kicker.textContent = `${String(index + 1).padStart(2, "0")} · ${chapters[index].title}`;
      }
    }
  };

  const updateProgress = () => {
    const pct = state.duration ? (state.time / state.duration) * 100 : 0;
    const p = clamp(pct, 0, 100);
    scrubFill.style.width = `${p}%`;
    scrubHead.style.left = `${p}%`;
    scrub.setAttribute("aria-valuenow", p.toFixed(1));
    timeNow.textContent = format(state.time);
    timeTotal.textContent = format(state.duration);
    updateCaption();
    const index = currentChapter(state.time);
    if (index !== state.chapter) {
      state.chapter = index;
      const chapter = chapters[index];
      if (chapter) {
        chapterIndex.textContent = String(index + 1).padStart(2, "0");
        chapterName.textContent = chapter.title;
        $$(".chapter-card", chapterGrid).forEach((card, i) => {
          card.classList.toggle("is-current", i === index);
        });
        try {
          history.replaceState(null, "", `#chapter-${index + 1}`);
        } catch (_) {}
      }
    }
  };

  const setPlaying = (playing) => {
    state.playing = playing;
    document.body.classList.toggle("is-playing", playing);
    playBtn.setAttribute("aria-label", playing ? "Pause" : "Play");
    if (!playing && state.time >= state.duration - 0.05) {
      playBtn.setAttribute("aria-label", "Replay");
    }
  };

  const hidePoster = () => {
    poster.classList.add("is-hidden");
    poster.setAttribute("aria-hidden", "true");
    state.started = true;
    document.body.classList.add("started");
  };

  const play = () => {
    if (!state.ready) {
      state.pending = "play";
      return;
    }
    hidePoster();
    if (state.time >= state.duration - 0.05) {
      state.time = 0;
      player.currentTime = 0;
      updateProgress();
    }
    const result = player.play();
    if (result && typeof result.catch === "function") {
      result.catch(() => setPlaying(false));
    }
    setPlaying(true);
  };

  const pause = () => {
    if (!state.ready) return;
    player.pause();
    setPlaying(false);
  };

  const seek = (time) => {
    const next = clamp(time, 0, state.duration);
    state.time = next;
    if (state.ready) player.currentTime = next;
    else state.pendingSeek = next;
    updateProgress();
  };

  const handleHash = () => {
    if (state.hashHandled) return;
    const hash = location.hash.match(/^#chapter-(\d+)$/);
    if (!hash) return;
    if (!chapters.length) return;
    state.hashHandled = true;
    const i = clamp(Number(hash[1]) - 1, 0, chapters.length - 1);
    hidePoster();
    seek(chapters[i].start);
    pause();
  };

  const startAtChapter = (index, autoplay = true) => {
    if (!chapters.length) return;
    const i = clamp(index, 0, chapters.length - 1);
    hidePoster();
    seek(chapters[i].start);
    if (autoplay) play();
    else pause();
    closeChapters();
  };

  const onReady = () => {
    if (state.ready) return;
    state.ready = true;
    state.duration = Number(player.duration) || fallbackDuration;
    if (state.pendingSeek != null) {
      player.currentTime = state.pendingSeek;
      state.pendingSeek = null;
    }
    runtime.textContent = formatLong(state.duration);
    updateProgress();
    if (state.pending === "play") {
      state.pending = null;
      play();
    }
    handleHash();
  };

  const buildUI = () => {
    if (!chapters.length) chapters = fallbackChapters;
    chapterGrid.innerHTML = "";
    transcriptBody.innerHTML = "";

    chapters.forEach((chapter, i) => {
      const card = document.createElement("button");
      card.className = "chapter-card";
      card.type = "button";
      card.dataset.index = String(i);
      card.innerHTML = `<span class="ch-no">${String(i + 1).padStart(2, "0")}</span><span class="ch-title"></span><span class="ch-time">${format(chapter.duration)}</span>`;
      $(".ch-title", card).textContent = chapter.title;
      card.addEventListener("click", () => startAtChapter(i, true));
      chapterGrid.append(card);

      const article = document.createElement("article");
      article.className = "tr-chapter";
      const h3 = document.createElement("h3");
      h3.textContent = `${String(i + 1).padStart(2, "0")} · ${format(chapter.duration)}`;
      const h4 = document.createElement("h4");
      h4.textContent = chapter.title;
      article.append(h3, h4);
      for (const caption of chapter.captions) {
        const p = document.createElement("p");
        p.textContent = caption;
        article.append(p);
      }
      transcriptBody.append(article);
    });

    const initial = currentChapter(state.time);
    state.chapter = initial;
    chapterIndex.textContent = String(initial + 1).padStart(2, "0");
    chapterName.textContent = chapters[initial]?.title || "";
    $$(".chapter-card", chapterGrid).forEach((card, i) => {
      card.classList.toggle("is-current", i === initial);
    });
  };

  const loadData = async () => {
    try {
      const res = await fetch("hyperframes/index.html", { cache: "no-cache" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const text = await res.text();
      const doc = new DOMParser().parseFromString(text, "text/html");
      const hosts = [...doc.querySelectorAll("[data-composition-id^='scene-']")];
      const parsed = hosts.map((host, i) => ({
        title: host.dataset.title || fallbackChapters[i]?.title || `Chapter ${i + 1}`,
        start: Number(host.dataset.start) || 0,
        duration: Number(host.dataset.duration) || 0,
        captions: [],
      }));
      if (!parsed.length) throw new Error("No chapters");
      const parsedCaptions = [...doc.querySelectorAll(".caption")].map((el) => {
        const clone = el.cloneNode(true);
        clone.querySelectorAll(".ch").forEach((ch) => ch.remove());
        return {
          start: Number(el.dataset.start) || 0,
          text: clone.textContent.replace(/\s+/g, " ").trim(),
        };
      }).filter((c) => c.text);
      for (const caption of parsedCaptions) {
        let index = 0;
        for (let i = parsed.length - 1; i >= 0; i -= 1) {
          if (caption.start >= parsed[i].start) {
            index = i;
            break;
          }
        }
        parsed[index].captions.push(caption.text);
      }
      chapters = parsed;
      const total = parsed.reduce((sum, c) => sum + c.duration, 0);
      state.duration = total || fallbackDuration;
      captions = parsedCaptions;
    } catch (_) {
      chapters = fallbackChapters;
      state.duration = fallbackDuration;
      captions = [];
    }
    runtime.textContent = formatLong(state.duration);
    timeTotal.textContent = format(state.duration);
    buildUI();
    updateProgress();
    handleHash();
  };

  /* ------------------------------------------------------------------ player */
  if (player) {
    player.addEventListener("ready", onReady);
    player.addEventListener("timeupdate", (event) => {
      const time = event.detail?.currentTime;
      if (typeof time === "number" && !state.scrubbing) {
        state.time = time;
        updateProgress();
      }
    });
    player.addEventListener("play", () => setPlaying(true));
    player.addEventListener("pause", () => setPlaying(false));
    player.addEventListener("ended", () => {
      setPlaying(false);
      state.time = state.duration;
      updateProgress();
      playBtn.setAttribute("aria-label", "Replay");
    });
    if (player.ready) onReady();
  }

  /* ----------------------------------------------------------------- controls */
  playBtn.addEventListener("click", () => {
    if (!state.started) {
      play();
    } else if (state.playing) {
      pause();
    } else {
      play();
    }
  });

  $("[data-start]")?.addEventListener("click", play);

  $("[data-prev]").addEventListener("click", () => {
    const i = currentChapter(state.time);
    const target = state.time - chapters[i].start > 3 ? i : Math.max(0, i - 1);
    startAtChapter(target, state.playing);
  });
  $("[data-next]").addEventListener("click", () => {
    const i = currentChapter(state.time);
    startAtChapter(Math.min(chapters.length - 1, i + 1), state.playing);
  });

  const seekFromEvent = (event) => {
    const rect = scrub.getBoundingClientRect();
    const x = "clientX" in event ? event.clientX : event.touches?.[0]?.clientX;
    if (x == null) return;
    const p = clamp((x - rect.left) / rect.width, 0, 1);
    seek(p * state.duration);
  };

  scrub.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    state.scrubbing = true;
    state.wasPlaying = state.playing;
    if (state.playing) pause();
    scrub.setPointerCapture?.(event.pointerId);
    seekFromEvent(event);
  });
  scrub.addEventListener("pointermove", (event) => {
    if (state.scrubbing) seekFromEvent(event);
  });
  const endScrub = () => {
    if (!state.scrubbing) return;
    state.scrubbing = false;
    if (state.wasPlaying) play();
    state.wasPlaying = false;
  };
  scrub.addEventListener("pointerup", endScrub);
  scrub.addEventListener("pointercancel", endScrub);
  scrub.addEventListener("click", (event) => {
    if (!state.scrubbing) seekFromEvent(event);
  });
  scrub.addEventListener("keydown", (event) => {
    if (event.key === "ArrowRight") {
      event.preventDefault();
      seek(state.time + 10);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      seek(state.time - 10);
    } else if (event.key === "Home") {
      event.preventDefault();
      seek(0);
    } else if (event.key === "End") {
      event.preventDefault();
      seek(state.duration);
    }
  });

  /* ------------------------------------------------------------------ chapters */
  const openChapters = () => {
    if (state.playing) pause();
    chapterMenu.hidden = false;
    $("[data-open-chapters]")?.setAttribute("aria-expanded", "true");
    const current = $(".chapter-card.is-current", chapterGrid);
    current?.focus({ preventScroll: true });
  };
  const closeChapters = () => {
    chapterMenu.hidden = true;
    $("[data-open-chapters]")?.setAttribute("aria-expanded", "false");
  };
  $("[data-open-chapters]").addEventListener("click", openChapters);
  $$("[data-close-chapters]").forEach((b) => b.addEventListener("click", closeChapters));
  chapterMenu.addEventListener("click", (event) => {
    if (event.target === chapterMenu) closeChapters();
  });

  /* ---------------------------------------------------------------- transcript */
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
    if (transcriptReturn?.focus) {
      try { transcriptReturn.focus({ preventScroll: true }); } catch (_) {}
    }
    transcriptReturn = null;
  };
  $$("[data-open-transcript]").forEach((b) => b.addEventListener("click", openTranscript));
  $$("[data-close-transcript]").forEach((b) => b.addEventListener("click", closeTranscript));

  /* ---------------------------------------------------------------- fullscreen */
  $("[data-fullscreen]").addEventListener("click", async () => {
    try {
      if (!document.fullscreenElement) await document.documentElement.requestFullscreen();
      else await document.exitFullscreen();
    } catch (_) {}
  });

  /* ----------------------------------------------------------------- keyboard */
  document.addEventListener("keydown", (event) => {
    const typing = /input|textarea|select/i.test(document.activeElement?.tagName || "");
    if (typing) return;
    if (event.key === "Escape") {
      if (!chapterMenu.hidden) { closeChapters(); return; }
      if (transcript.classList.contains("is-open")) { closeTranscript(); return; }
      if (state.started) pause();
      return;
    }
    if (event.key === " " || event.key.toLowerCase() === "k") {
      if (/button|a/i.test(document.activeElement?.tagName || "") && event.key === " ") return;
      event.preventDefault();
      playBtn.click();
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      seek(state.time + 10);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      seek(state.time - 10);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      $("[data-next]").click();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      $("[data-prev]").click();
    } else if (event.key.toLowerCase() === "c") {
      if (chapterMenu.hidden) openChapters(); else closeChapters();
    } else if (event.key.toLowerCase() === "t") {
      if (transcript.classList.contains("is-open")) closeTranscript(); else openTranscript();
    } else if (event.key.toLowerCase() === "f") {
      $("[data-fullscreen]").click();
    } else if (event.key === "Home" && state.started) {
      seek(0);
    }
  });

  window.addEventListener("resize", updateProgress);
  window.addEventListener("hashchange", () => {
    state.hashHandled = false;
    handleHash();
  });

  /* ------------------------------------------------------------------- start */
  if (reduced) {
    const note = document.createElement("p");
    note.className = "poster-meta";
    note.textContent = "The film uses motion throughout. Pause any time.";
    $(".poster-actions").after(note);
  }

  updateProgress();
  loadData();
})();
