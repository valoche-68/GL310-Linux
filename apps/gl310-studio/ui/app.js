// GL310 Studio — Pure ES JS Frontend (Zero node_modules, Direct Tauri v2 IPC)
(() => {
  "use strict";

  const hasTauri = Boolean(window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke);
  const invoke = hasTauri
    ? (cmd, args) => window.__TAURI__.core.invoke(cmd, args)
    : async () => null;
  const listen = hasTauri && window.__TAURI__.event
    ? (ev, cb) => window.__TAURI__.event.listen(ev, cb)
    : async () => () => {};

  const $ = (id) => document.getElementById(id);

  let currentSnap = null;
  let settingsHydrated = false;
  let captureBusy = false;
  let previewBusy = false;
  let activePreviewKey = "";
  let previewRetryTimer = null;
  let logFilter = "all";
  let toastTimer = null;
  const customSelectUpdaters = [];

  function disconnectPreview() {
    if (activePreviewKey) {
      activePreviewKey = "";
      clearTimeout(previewRetryTimer);
      const img = $("preview-img");
      if (img) img.removeAttribute("src");
    }
  }

  // Zero-churn DOM helpers: skip writes when unchanged so WebKitGTK avoids needless repaints.
  function setText(el, text) {
    if (el && el.textContent !== text) el.textContent = text;
  }
  function setVal(el, val) {
    if (el && el.value !== val) el.value = val;
  }
  function setClass(el, cls) {
    if (el && el.className !== cls) el.className = cls;
  }
  function setHidden(el, hidden) {
    if (el && el.classList.contains("hidden") !== hidden) {
      el.classList.toggle("hidden", hidden);
    }
  }
  function setClassFlag(el, cls, active) {
    if (el && el.classList.contains(cls) !== active) {
      el.classList.toggle(cls, active);
    }
  }
  function setTransform(el, val) {
    if (el && el.style.transform !== val) el.style.transform = val;
  }

  // -------------------------------------------------------------------------
  // Custom In-DOM Dark Dropdowns (fixes WebKitGTK/Wayland white flash & unreadable native popups)
  // -------------------------------------------------------------------------
  function closeAllCustomSelects(exceptWrap = null) {
    document.querySelectorAll(".custom-select.is-open").forEach((wrap) => {
      if (wrap === exceptWrap) return;
      wrap.classList.remove("is-open", "drop-up");
      const trig = wrap.querySelector(".custom-select-trigger");
      const menu = wrap.querySelector(".custom-select-menu");
      if (trig) trig.setAttribute("aria-expanded", "false");
      if (menu) menu.classList.add("hidden");
    });
  }

  function syncCustomSelects() {
    customSelectUpdaters.forEach((fn) => fn());
  }

  function upgradeCustomSelects() {
    document.querySelectorAll("select.studio-select").forEach((sel) => {
      if (sel.dataset.upgraded === "1") return;
      sel.dataset.upgraded = "1";

      // Detach wrapping <label for="..."> so clicking inside never triggers native select focus
      const parentLabel = sel.closest("label.field-row");
      if (parentLabel && parentLabel.getAttribute("for") === sel.id) {
        parentLabel.removeAttribute("for");
      }

      sel.classList.add("native-select-hidden");
      sel.setAttribute("aria-hidden", "true");
      sel.tabIndex = -1;

      const wrap = document.createElement("div");
      wrap.className = "custom-select";

      const trigger = document.createElement("button");
      trigger.type = "button";
      trigger.className = "custom-select-trigger";
      trigger.setAttribute("aria-haspopup", "listbox");
      trigger.setAttribute("aria-expanded", "false");

      const valSpan = document.createElement("span");
      valSpan.className = "custom-select-value";

      const chev = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      chev.setAttribute("class", "custom-select-chevron");
      chev.setAttribute("width", "14");
      chev.setAttribute("height", "14");
      chev.setAttribute("viewBox", "0 0 24 24");
      chev.setAttribute("fill", "none");
      chev.setAttribute("stroke", "currentColor");
      chev.setAttribute("stroke-width", "2");
      chev.setAttribute("stroke-linecap", "round");
      chev.setAttribute("stroke-linejoin", "round");
      const chevPath = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
      chevPath.setAttribute("points", "6 9 12 15 18 9");
      chev.appendChild(chevPath);

      trigger.appendChild(valSpan);
      trigger.appendChild(chev);

      const menu = document.createElement("div");
      menu.className = "custom-select-menu hidden";
      menu.setAttribute("role", "listbox");

      const optionButtons = [];
      Array.from(sel.options).forEach((opt) => {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "custom-select-option";
        btn.setAttribute("role", "option");
        btn.dataset.value = opt.value;

        const lbl = document.createElement("span");
        lbl.textContent = opt.textContent;

        const chk = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        chk.setAttribute("class", "custom-select-check");
        chk.setAttribute("width", "13");
        chk.setAttribute("height", "13");
        chk.setAttribute("viewBox", "0 0 24 24");
        chk.setAttribute("fill", "none");
        chk.setAttribute("stroke", "currentColor");
        chk.setAttribute("stroke-width", "2.4");
        chk.setAttribute("stroke-linecap", "round");
        chk.setAttribute("stroke-linejoin", "round");
        const chkPath = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
        chkPath.setAttribute("points", "20 6 9 17 4 12");
        chk.appendChild(chkPath);

        btn.appendChild(lbl);
        btn.appendChild(chk);

        btn.addEventListener("click", (ev) => {
          ev.stopPropagation();
          const changed = sel.value !== opt.value;
          sel.value = opt.value;
          syncThis();
          closeAllCustomSelects();
          trigger.focus();
          if (changed) {
            sel.dispatchEvent(new Event("change", { bubbles: true }));
          }
        });

        menu.appendChild(btn);
        optionButtons.push(btn);
      });

      function syncThis() {
        const selectedOpt = sel.options[sel.selectedIndex] || sel.options[0];
        valSpan.textContent = selectedOpt ? selectedOpt.textContent : "";
        optionButtons.forEach((b) => {
          const isSel = b.dataset.value === sel.value;
          b.classList.toggle("selected", isSel);
          b.setAttribute("aria-selected", String(isSel));
        });
      }

      function toggleMenu(forceState = null) {
        const willOpen = forceState !== null ? forceState : !wrap.classList.contains("is-open");
        closeAllCustomSelects(willOpen ? wrap : null);
        if (willOpen) {
          const rect = trigger.getBoundingClientRect();
          const spaceBelow = window.innerHeight - rect.bottom;
          wrap.classList.toggle("drop-up", spaceBelow < 220 && rect.top > spaceBelow);
          wrap.classList.add("is-open");
          menu.classList.remove("hidden");
          trigger.setAttribute("aria-expanded", "true");
        } else {
          wrap.classList.remove("is-open", "drop-up");
          menu.classList.add("hidden");
          trigger.setAttribute("aria-expanded", "false");
        }
      }

      trigger.addEventListener("click", (ev) => {
        ev.stopPropagation();
        toggleMenu();
      });

      trigger.addEventListener("keydown", (ev) => {
        if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
          ev.preventDefault();
          const idx = sel.selectedIndex;
          const next = ev.key === "ArrowDown"
            ? Math.min(sel.options.length - 1, idx + 1)
            : Math.max(0, idx - 1);
          if (next !== idx) {
            sel.selectedIndex = next;
            syncThis();
            sel.dispatchEvent(new Event("change", { bubbles: true }));
          }
        } else if (ev.key === "Enter" || ev.key === " ") {
          ev.preventDefault();
          toggleMenu();
        } else if (ev.key === "Escape") {
          toggleMenu(false);
        }
      });

      sel.addEventListener("change", syncThis);
      sel.insertAdjacentElement("afterend", wrap);
      wrap.appendChild(trigger);
      wrap.appendChild(menu);

      syncThis();
      customSelectUpdaters.push(syncThis);
    });

    document.addEventListener("click", () => closeAllCustomSelects());
    document.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") closeAllCustomSelects();
    });
  }

  function showToast(message) {
    const el = $("toast");
    el.textContent = message;
    el.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.classList.add("hidden"), 3000);
  }

  function formatDuration(totalSecs) {
    const s = Math.max(0, Math.floor(totalSecs));
    const h = String(Math.floor(s / 3600)).padStart(2, "0");
    const m = String(Math.floor((s % 3600) / 60)).padStart(2, "0");
    const sec = String(s % 60).padStart(2, "0");
    return `${h}:${m}:${sec}`;
  }

  function formatSizeMb(bytes) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} Mo`;
  }

  // -------------------------------------------------------------------------
  // Tab Navigation
  // -------------------------------------------------------------------------
  document.querySelectorAll(".tab-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const target = btn.dataset.tab;
      document.querySelectorAll(".tab-btn").forEach((b) => {
        const active = b.dataset.tab === target;
        b.classList.toggle("active", active);
        b.setAttribute("aria-selected", String(active));
      });
      document.querySelectorAll(".tab-panel").forEach((p) => {
        p.classList.toggle("active", p.id === `panel-${target}`);
      });
      if (target === "device") {
        refreshLogs();
      }
    });
  });

  // -------------------------------------------------------------------------
  // Render Live Snapshot from gl310-core
  // -------------------------------------------------------------------------
  function applySnapshot(snap) {
    if (!snap) return;
    currentSnap = snap;

    // 1. Device Status Pill
    const pill = $("device-pill");
    const pillText = $("device-pill-text");
    const phaseMap = {
      absent: ["state-absent", "Absent (USB)"],
      permission_denied: ["state-warn", "udev requis"],
      locked_d835: ["state-error", "Verrouillé d835"],
      firmware_missing: ["state-warn", "Firmware absent"],
      booting: ["state-booting", "Chargement RAM…"],
      idle: ["state-idle", "Prêt (RAM QSOS)"],
      streaming: ["state-streaming", "Capture active"],
      error: ["state-error", "Erreur"],
    };
    const [cls, label] = phaseMap[snap.phase] || ["state-absent", snap.phase];
    setClass(pill, `status-pill ${cls}`);
    setText(pillText, label);

    // 2. HDMI Signal & Telemetry
    const sig = snap.signal;
    const resBadge = $("hud-res-badge");
    const warnEl = $("hud-warn-badge");
    if (sig) {
      const scan = sig.interlaced ? "i" : "p";
      const outPart = snap.out_fps > 0 ? ` → ${snap.out_height}p${snap.out_fps}` : "";
      setText($("signal-readout"), `${sig.width}×${sig.height}${scan}@${sig.refresh_hz}${outPart}`);
      setText(resBadge, `${sig.width}×${sig.height}${scan}@${sig.refresh_hz}`);
      setHidden(resBadge, false);

      if (sig.hdcp) {
        setText(warnEl, "HDCP DÉTECTÉ");
        setHidden(warnEl, false);
      } else if (sig.audio && sig.audio.compressed_non_pcm) {
        setText(warnEl, "AUDIO NON-PCM (PASSER EN LPCM)");
        setHidden(warnEl, false);
      } else {
        setHidden(warnEl, true);
      }
    } else {
      setText($("signal-readout"), "Aucun signal");
      setHidden(resBadge, true);
      setHidden(warnEl, true);
    }

    setText($("bitrate-readout"), `${(snap.bitrate_mbps || 0).toFixed(1)} Mbit/s`);
    setText($("ts-errors-readout"), String(snap.ts_stats ? snap.ts_stats.continuity_errors : 0));

    // 3. Mode Toggle Buttons & HUD
    const isMjpeg = snap.settings.mode === "mjpeg";
    setClassFlag($("mode-btn-h264"), "active", !isMjpeg);
    setClassFlag($("mode-btn-mjpeg"), "active", isMjpeg);
    setText($("hud-mode-badge"), isMjpeg ? "KVM · MJPEG HW" : "STUDIO · H.264 + AAC");

    // 4. Preview Monitor vs Standby / Live Diagnostic Overlay
    const isStreaming = snap.phase === "streaming";
    const previewImg = $("preview-img");
    const standby = $("monitor-standby");
    const previewEnabled = snap.settings.enable_preview !== false;
    const previewActive = snap.preview_active !== false;
    const hasVideoFrame = (snap.mjpeg_frames || 0) > 0;

    if (
      isStreaming &&
      snap.mjpeg_url &&
      previewEnabled &&
      previewActive &&
      hasVideoFrame &&
      !document.hidden
    ) {
      const localUrl = `http://127.0.0.1:${snap.settings.http_port}/mjpeg`;
      const previewKey = `${localUrl}#${snap.stream_generation || 0}`;
      if (activePreviewKey !== previewKey) {
        activePreviewKey = previewKey;
        clearTimeout(previewRetryTimer);
        previewImg.src = `${localUrl}?studio=1&g=${snap.stream_generation || 0}&t=${Date.now()}`;
      }
      setHidden(previewImg, false);
      setHidden(standby, true);
    } else {
      disconnectPreview();
      setHidden(previewImg, true);
      setHidden(standby, false);
      const tsPkts = snap.ts_stats ? snap.ts_stats.packets : 0;
      const standbyTitle = isStreaming
        ? !previewEnabled
          ? "Capture active — Aperçu désactivé (économie CPU)"
          : !snap.mjpeg_url
            ? "Capture active — aperçu HTTP désactivé"
            : !sig
              ? "Capture active — Aucun signal HDMI IN"
              : sig.hdcp
                ? "Signal bloqué par protection HDCP"
                : tsPkts === 0 && !isMjpeg
                  ? "Capture active — En attente du flux USB…"
                  : "Décodage de la première image…"
        : snap.phase === "idle"
          ? sig
            ? `Source prête : ${sig.width}×${sig.height}@${sig.refresh_hz} Hz`
            : "GL310 prêt — Aucune source HDMI branchée"
          : snap.phase === "permission_denied"
            ? "Permission USB requise (règle udev)"
            : snap.phase === "firmware_missing"
              ? "Firmwares AVerMedia requis"
              : snap.phase === "error"
                ? "Erreur de capture GL310"
                : "En attente du boîtier GL310";
      const standbySub = isStreaming
        ? !previewEnabled
          ? "OBS (/dev/video14, GL310 Audio, SRT/HTTP) et l’enregistrement .mp4 continuent normalement en arrière-plan."
          : !snap.mjpeg_url
            ? `Réactivez le serveur HTTP intégré (:${snap.settings.http_port}) dans l’onglet Sorties OBS pour afficher l’aperçu en direct.`
            : !sig
              ? "Vérifiez le câble HDMI IN et la source vidéo. L’aperçu et la caméra OBS (/dev/video14) s’activeront automatiquement dès le verrouillage HDMI."
              : sig.hdcp
                ? "Le boîtier GL310 ne peut pas capturer un flux chiffré HDCP. Désactivez HDCP sur la console ou la source."
                : `Mode ${isMjpeg ? "MJPEG matériel" : "H.264 + AAC"} · ${tsPkts} paquets TS reçus · Consultez « Boîtier & Logs » si aucun flux n’apparaît.`
        : snap.status_message;
      setText($("standby-title"), standbyTitle);
      setText($("standby-sub"), standbySub);
      const acts = $("standby-actions");
      const btnRefresh = $("btn-standby-refresh");
      const btnFw = $("btn-standby-fw");
      const btnEnablePrev = $("btn-standby-enable-preview");
      const showRefresh = !isStreaming && snap.phase !== "idle" && snap.phase !== "booting";
      const showEnablePrev = isStreaming && !previewEnabled;
      setHidden(btnRefresh, !showRefresh);
      setHidden(btnFw, !showRefresh || snap.phase !== "firmware_missing");
      setHidden(btnEnablePrev, !showEnablePrev);
      setHidden(acts, !showRefresh && !showEnablePrev);
    }

    // 5. Stereo VU-Meter (GPU-safe transform: scaleX, quantized to avoid idle churn)
    const vu = snap.vu || { left: 0, right: 0, left_db: -60, right_db: -60 };
    const vuL = Math.max(0, Math.min(1, vu.left)).toFixed(3);
    const vuR = Math.max(0, Math.min(1, vu.right)).toFixed(3);
    setTransform($("vu-bar-l"), `scaleX(${vuL})`);
    setTransform($("vu-bar-r"), `scaleX(${vuR})`);
    setText($("vu-db-readout"), `${vu.left_db.toFixed(1)} / ${vu.right_db.toFixed(1)} dBFS`);

    // 6. Transport Buttons & Recording Tally
    if (!captureBusy) {
      const capBtn = $("btn-toggle-capture");
      capBtn.disabled = false;
      setClassFlag(capBtn, "is-streaming", isStreaming);
      setText($("btn-capture-label"), isStreaming ? "Arrêter la capture" : "Démarrer la capture");
      setHidden($("icon-capture-play"), isStreaming);
      setHidden($("icon-capture-stop"), !isStreaming);
    }

    if (!previewBusy) {
      const prevBtn = $("btn-toggle-preview");
      if (prevBtn) {
        prevBtn.disabled = false;
        setClassFlag(prevBtn, "is-off", !previewEnabled);
        prevBtn.setAttribute("aria-pressed", String(previewEnabled));
        setText($("btn-preview-label"), previewEnabled ? "Aperçu" : "Aperçu coupé");
        setHidden($("icon-preview-on"), !previewEnabled);
        setHidden($("icon-preview-off"), previewEnabled);
      }
    }

    const recActive = Boolean(snap.recording && snap.recording.active);
    const recBtn = $("btn-toggle-record");
    recBtn.disabled = !isStreaming || isMjpeg;
    setClassFlag(recBtn, "recording", recActive);
    setText($("btn-record-label"), recActive ? "Arrêter REC" : "Enregistrer .mp4");
    const snapBtn = $("btn-snapshot");
    const snapDisabled = !isStreaming || (!isMjpeg && !previewEnabled);
    snapBtn.disabled = snapDisabled;
    snapBtn.title =
      !isMjpeg && !previewEnabled
        ? "Réactivez l’aperçu pour prendre une capture photo en mode H.264"
        : "Prendre une capture photo JPEG";

    const tally = $("hud-rec-tally");
    setHidden(tally, !recActive);
    if (recActive) {
      setText($("rec-timer"), formatDuration(snap.recording.duration_secs));
      setText($("rec-size"), formatSizeMb(snap.recording.bytes));
    }

    // 7. Device MCU, Firmware & URLs
    setText($("mcu-id-badge"), snap.device ? `MCU ${snap.device.mcu_identity}` : "MCU --");
    const fw = snap.firmware;
    if (fw) {
      setText($("fw-badge"), fw.valid ? "SHA-256 OK" : fw.found ? "INVALIDE" : "ABSENT");
      setText($("fw-detail"), fw.message);
    }

    const host =
      snap.display_host && snap.display_host !== "0.0.0.0"
        ? snap.display_host
        : "127.0.0.1";
    setVal(
      $("url-srt"),
      snap.srt_url ||
        `srt://${host}:${snap.settings.srt_port}?mode=caller&transtype=live&latency=120000`
    );
    setVal($("url-ts"), snap.http_url || `http://${host}:${snap.settings.http_port}/stream.ts`);
    setVal($("url-mjpeg"), snap.mjpeg_url || `http://${host}:${snap.settings.http_port}/mjpeg`);
    setText($("hdmi-out-status"), snap.hdmi_out_passthrough);

    if (!settingsHydrated) {
      hydrateControls(snap.settings);
      settingsHydrated = true;
    }
  }

  function hydrateControls(s) {
    $("sel-fps-policy").value = s.fps_policy;
    $("range-video-kbps").value = String(s.video_kbps);
    $("val-video-kbps").textContent = `${Math.round(s.video_kbps / 1000)} Mbit/s`;
    syncBitrateChips(s.video_kbps);
    $("sel-gop").value = s.gop;
    $("sel-v4l2-format").value = s.v4l2_format;
    $("sel-mjpeg-res").value = s.mjpeg_resolution;
    $("sel-mjpeg-fps").value = String(s.mjpeg_fps);
    $("range-mjpeg-q").value = String(s.mjpeg_quality);
    $("val-mjpeg-q").textContent = `${s.mjpeg_quality} / 15`;

    $("btn-ain-hdmi").classList.toggle("active", s.audio_input === "hdmi");
    $("btn-ain-jack").classList.toggle("active", s.audio_input === "jack");
    $("range-analog-vol").value = String(s.analog_volume);
    $("val-analog-vol").textContent = `${s.analog_volume} / 11`;
    $("range-enc-vol").value = String(s.encoder_volume);
    $("val-enc-vol").textContent = `${s.encoder_volume} / 15`;
    $("sel-audio-kbps").value = String(s.audio_kbps);
    $("chk-audio-out-jack").checked = s.audio_out_jack;
    $("chk-pc-monitor").checked = s.pc_monitor;
    $("range-pc-vol").value = String(s.pc_monitor_volume);
    $("val-pc-vol").textContent = `${s.pc_monitor_volume} %`;
    $("btn-pc-mute").classList.toggle("muted", s.pc_monitor_muted);
    $("btn-pc-mute").setAttribute("aria-pressed", String(s.pc_monitor_muted));

    $("chk-v4l2").checked = s.enable_v4l2;
    $("chk-pipewire").checked = s.enable_pipewire_audio;
    $("chk-srt").checked = s.enable_srt;
    $("chk-http").checked = s.enable_http;
    $("chk-lan").checked = s.expose_lan;

    $("sel-button-action").value = s.button_action;
    syncCustomSelects();
    updateLedUi(s.led_mode);
  }

  function collectSettings(patch = {}) {
    const base = currentSnap ? { ...currentSnap.settings } : {};
    return {
      ...base,
      fps_policy: $("sel-fps-policy").value,
      video_kbps: Number($("range-video-kbps").value),
      gop: $("sel-gop").value,
      v4l2_format: $("sel-v4l2-format").value,
      mjpeg_resolution: $("sel-mjpeg-res").value,
      mjpeg_fps: Number($("sel-mjpeg-fps").value),
      mjpeg_quality: Number($("range-mjpeg-q").value),
      audio_kbps: Number($("sel-audio-kbps").value),
      audio_out_jack: $("chk-audio-out-jack").checked,
      pc_monitor: $("chk-pc-monitor").checked,
      pc_monitor_volume: Number($("range-pc-vol").value),
      pc_monitor_muted: $("btn-pc-mute").classList.contains("muted"),
      enable_v4l2: $("chk-v4l2").checked,
      enable_pipewire_audio: $("chk-pipewire").checked,
      enable_srt: $("chk-srt").checked,
      enable_http: $("chk-http").checked,
      expose_lan: $("chk-lan").checked,
      button_action: $("sel-button-action").value,
      ...patch,
    };
  }

  function syncBitrateChips(kbps) {
    document.querySelectorAll('[data-preset="video_kbps"] .chip-btn').forEach((c) => {
      c.classList.toggle("active", Number(c.dataset.val) === Number(kbps));
    });
  }

  function updateLedUi(mode) {
    document.querySelectorAll("[data-led]").forEach((b) => {
      b.classList.toggle("active", b.dataset.led === mode);
    });
    const ring = $("led-preview-ring");
    ring.className = `led-ring led-${mode}`;
  }

  async function saveSettings(patch = {}, toastText = "Réglages appliqués") {
    try {
      const next = collectSettings(patch);
      await invoke("update_settings", { settings: next });
      showToast(toastText);
    } catch (err) {
      showToast(`Erreur : ${err}`);
    }
  }

  // Auto-reconnect MJPEG preview image if the local HTTP server restarts during reconfiguration
  $("preview-img").addEventListener("error", () => {
    if (
      !currentSnap ||
      currentSnap.phase !== "streaming" ||
      !currentSnap.mjpeg_url ||
      currentSnap.settings.enable_preview === false ||
      currentSnap.preview_active === false ||
      document.hidden
    ) {
      return;
    }
    clearTimeout(previewRetryTimer);
    previewRetryTimer = setTimeout(() => {
      if (
        !currentSnap ||
        currentSnap.phase !== "streaming" ||
        !currentSnap.mjpeg_url ||
        currentSnap.settings.enable_preview === false ||
        currentSnap.preview_active === false ||
        document.hidden
      ) {
        return;
      }
      const localUrl = `http://127.0.0.1:${currentSnap.settings.http_port}/mjpeg`;
      $("preview-img").src = `${localUrl}?studio=1&g=${currentSnap.stream_generation || 0}&retry=${Date.now()}`;
    }, 600);
  });

  // -------------------------------------------------------------------------
  // Event Wiring
  // -------------------------------------------------------------------------
  $("mode-btn-h264").addEventListener("click", () =>
    saveSettings({ mode: "h264_aac" }, "Mode Studio OBS (H.264 + AAC)")
  );
  $("mode-btn-mjpeg").addEventListener("click", () =>
    saveSettings({ mode: "mjpeg" }, "Mode KVM matériel (MJPEG)")
  );

  $("btn-toggle-capture").addEventListener("click", async () => {
    if (captureBusy) return;
    const capBtn = $("btn-toggle-capture");
    const stopping = Boolean(currentSnap && currentSnap.phase === "streaming");
    captureBusy = true;
    capBtn.disabled = true;
    setText($("btn-capture-label"), stopping ? "Arrêt…" : "Démarrage…");
    try {
      const nextSnap = await invoke(stopping ? "stop_capture" : "start_capture");
      captureBusy = false;
      if (nextSnap) {
        applySnapshot(nextSnap);
      } else {
        capBtn.disabled = false;
      }
      if (stopping) {
        showToast("Capture arrêtée");
      } else if (nextSnap && nextSnap.phase === "streaming") {
        showToast(`Capture démarrée (${nextSnap.out_width}×${nextSnap.out_height} @ ${nextSnap.out_fps} fps)`);
      }
    } catch (err) {
      captureBusy = false;
      capBtn.disabled = false;
      if (currentSnap) applySnapshot(currentSnap);
      showToast(`Erreur capture : ${err}`);
    }
  });

  async function setPreviewEnabled(nextEnabled) {
    if (previewBusy) return;
    previewBusy = true;
    const btn = $("btn-toggle-preview");
    if (btn) btn.disabled = true;
    try {
      const nextSnap = await invoke("set_preview_enabled", { enabled: nextEnabled });
      previewBusy = false;
      if (nextSnap) {
        applySnapshot(nextSnap);
      } else if (btn) {
        btn.disabled = false;
      }
      showToast(
        nextEnabled
          ? "Aperçu vidéo réactivé"
          : "Aperçu désactivé — OBS et l’enregistrement continuent (CPU réduit)"
      );
    } catch (err) {
      previewBusy = false;
      if (btn) btn.disabled = false;
      showToast(`Erreur aperçu : ${err}`);
    }
  }

  $("btn-toggle-preview").addEventListener("click", () => {
    const currentlyEnabled = !currentSnap || currentSnap.settings.enable_preview !== false;
    setPreviewEnabled(!currentlyEnabled);
  });

  $("btn-standby-enable-preview").addEventListener("click", () => setPreviewEnabled(true));

  $("btn-toggle-record").addEventListener("click", async () => {
    try {
      if (currentSnap && currentSnap.recording && currentSnap.recording.active) {
        const res = await invoke("stop_recording");
        showToast(res ? `Enregistré : ${res.final_path}` : "Enregistrement terminé");
      } else {
        await invoke("start_recording");
        showToast("Enregistrement démarré (.mp4)");
      }
    } catch (err) {
      showToast(`Erreur REC : ${err}`);
    }
  });

  $("btn-snapshot").addEventListener("click", async () => {
    try {
      const path = await invoke("take_snapshot");
      showToast(`Photo enregistrée : ${path}`);
    } catch (err) {
      showToast(`Erreur photo : ${err}`);
    }
  });

  $("btn-standby-refresh").addEventListener("click", async () => {
    await invoke("refresh_device");
    showToast("Nouvelle détection USB lancée");
  });

  $("btn-standby-fw").addEventListener("click", () => $("tab-device").click());

  $("range-video-kbps").addEventListener("input", (e) => {
    const val = Number(e.target.value);
    $("val-video-kbps").textContent = `${Math.round(val / 1000)} Mbit/s`;
    syncBitrateChips(val);
  });

  document.querySelectorAll('[data-preset="video_kbps"] .chip-btn').forEach((btn) => {
    btn.addEventListener("click", () => {
      const val = Number(btn.dataset.val);
      $("range-video-kbps").value = String(val);
      $("val-video-kbps").textContent = `${Math.round(val / 1000)} Mbit/s`;
      syncBitrateChips(val);
    });
  });

  $("range-mjpeg-q").addEventListener("input", (e) => {
    $("val-mjpeg-q").textContent = `${e.target.value} / 15`;
  });

  $("btn-apply-video").addEventListener("click", () => saveSettings({}, "Réglages vidéo appliqués"));
  $("btn-apply-outputs").addEventListener("click", () => saveSettings({}, "Sorties OBS & réseau mises à jour"));

  // Audio controls (immediate hardware updates)
  document.querySelectorAll("[data-ain]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const src = btn.dataset.ain;
      $("btn-ain-hdmi").classList.toggle("active", src === "hdmi");
      $("btn-ain-jack").classList.toggle("active", src === "jack");
      try {
        await invoke("set_audio_input", { input: src });
        showToast(src === "jack" ? "Entrée : Jack 3,5 mm AUDIO IN" : "Entrée : HDMI IN");
      } catch (err) {
        showToast(`Erreur audio : ${err}`);
      }
    });
  });

  $("range-analog-vol").addEventListener("input", (e) => {
    $("val-analog-vol").textContent = `${e.target.value} / 11`;
  });
  $("range-analog-vol").addEventListener("change", async (e) => {
    await invoke("set_analog_volume", { volume: Number(e.target.value) });
  });

  $("range-enc-vol").addEventListener("input", (e) => {
    $("val-enc-vol").textContent = `${e.target.value} / 15`;
  });
  $("range-enc-vol").addEventListener("change", async (e) => {
    await invoke("set_encoder_volume", { volume: Number(e.target.value) });
  });

  $("sel-audio-kbps").addEventListener("change", () => saveSettings({}, "Débit AAC mis à jour"));
  $("chk-audio-out-jack").addEventListener("change", () => saveSettings({}, "Jack AUDIO OUT mis à jour"));

  async function pushPcMonitor() {
    const enabled = $("chk-pc-monitor").checked;
    const muted = $("btn-pc-mute").classList.contains("muted");
    const volume = Number($("range-pc-vol").value);
    $("val-pc-vol").textContent = `${volume} %`;
    await invoke("set_pc_monitor", { enabled, muted, volume });
  }

  $("chk-pc-monitor").addEventListener("change", pushPcMonitor);
  $("range-pc-vol").addEventListener("input", pushPcMonitor);
  $("btn-pc-mute").addEventListener("click", () => {
    const btn = $("btn-pc-mute");
    const muted = !btn.classList.contains("muted");
    btn.classList.toggle("muted", muted);
    btn.setAttribute("aria-pressed", String(muted));
    pushPcMonitor();
  });

  // Copy URL buttons
  document.querySelectorAll("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const inp = $(btn.dataset.copy);
      try {
        await navigator.clipboard.writeText(inp.value);
        showToast("URL copiée dans le presse-papiers");
      } catch {
        inp.select();
        document.execCommand("copy");
        showToast("URL copiée");
      }
    });
  });

  // Device tab: Button action, LED ring, Firmware wizard, Logs
  $("sel-button-action").addEventListener("change", () => saveSettings({}, "Action du bouton enregistrée"));

  document.querySelectorAll("[data-led]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const mode = btn.dataset.led;
      updateLedUi(mode);
      try {
        await invoke("set_led", { mode });
        showToast(`Anneau LED : ${btn.textContent}`);
      } catch (err) {
        showToast(`Erreur LED : ${err}`);
      }
    });
  });

  $("btn-fw-fetch").addEventListener("click", async () => {
    const btn = $("btn-fw-fetch");
    btn.disabled = true;
    btn.textContent = "Téléchargement et audit…";
    try {
      const res = await invoke("firmware_fetch");
      showToast(res && res.ok ? "Firmwares audités et installés !" : "Échec téléchargement");
    } catch (err) {
      showToast(`Erreur firmware : ${err}`);
    } finally {
      btn.disabled = false;
      btn.textContent = "Télécharger (Audité Archive.org)";
      refreshLogs();
    }
  });

  $("btn-fw-import").addEventListener("click", async () => {
    const path = $("inp-fw-path").value.trim();
    if (!path) {
      showToast("Indiquez un chemin vers un fichier .zip ou dossier pilote Windows");
      return;
    }
    try {
      const res = await invoke("firmware_import", { path });
      showToast(res ? res.message : "Firmwares importés");
    } catch (err) {
      showToast(`Erreur import : ${err}`);
    } finally {
      refreshLogs();
    }
  });

  async function refreshLogs() {
    const entries = await invoke("get_logs");
    if (!Array.isArray(entries)) return;
    const box = $("log-console");
    box.innerHTML = "";
    entries
      .filter((e) => logFilter === "all" || e.level === logFilter)
      .forEach((e) => {
        const line = document.createElement("div");
        line.className = `log-line lvl-${e.level}`;
        line.textContent = `[${e.ts}] [${e.level.toUpperCase()}] ${e.message}`;
        box.appendChild(line);
      });
    box.scrollTop = box.scrollHeight;
  }

  document.querySelectorAll(".log-filter[data-level]").forEach((btn) => {
    btn.addEventListener("click", () => {
      logFilter = btn.dataset.level;
      document.querySelectorAll(".log-filter[data-level]").forEach((b) => {
        b.classList.toggle("active", b.dataset.level === logFilter);
      });
      refreshLogs();
    });
  });

  $("btn-clear-logs").addEventListener("click", async () => {
    await invoke("clear_logs");
    refreshLogs();
  });

  // -------------------------------------------------------------------------
  // Startup
  // -------------------------------------------------------------------------
  async function boot() {
    upgradeCustomSelects();
    if (!hasTauri) {
      $("standby-title").textContent = "Aperçu UI hors Tauri";
      $("standby-sub").textContent = "Lancez gl310-studio pour activer la communication USB directe avec gl310-core.";
      return;
    }
    const snap = await invoke("get_snapshot");
    applySnapshot(snap);
    await refreshLogs();
    await listen("gl310://snapshot", (ev) => applySnapshot(ev.payload));
    document.addEventListener("visibilitychange", async () => {
      if (document.hidden) {
        disconnectPreview();
      } else {
        const fresh = await invoke("get_snapshot");
        applySnapshot(fresh);
      }
    });
    setInterval(() => {
      if (!document.hidden && $("panel-device").classList.contains("active")) {
        refreshLogs();
      }
    }, 1500);
  }

  boot();
})();
