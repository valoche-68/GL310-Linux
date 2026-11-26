// GL310 Studio — Pure ES JS Frontend (Zero node_modules, Direct Tauri v2 IPC, Bilingual EN/FR)
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

  // -------------------------------------------------------------------------
  // Bilingual Dictionary (English default, live switchable to French)
  // -------------------------------------------------------------------------
  const I18N = {
    en: {
      aria_capture_mode: "Hardware capture mode",
      aria_language: "Interface language",
      aria_stage: "Video monitor and transport",
      aria_vu: "Stereo L and R VU meter",
      aria_console: "GL310 settings panel",
      aria_tabs: "Configuration sections",
      aria_pc_mute: "Mute or unmute PC monitoring",
      aria_pc_vol: "PC monitoring volume",
      aria_led_patterns: "LED ring patterns",
      usb_scanning: "Scanning USB…",
      telemetry_hdmi: "HDMI Signal",
      telemetry_bitrate: "Bitrate",
      telemetry_ts_errors: "TS Errors",
      no_signal: "No signal",
      unit_mbps: "Mbps",
      unit_mb: "MB",
      phase_absent: "Not connected (USB)",
      phase_permission_denied: "udev required",
      phase_locked_d835: "Locked d835",
      phase_firmware_missing: "Firmware missing",
      phase_booting: "Booting RAM…",
      phase_idle: "Ready (RAM QSOS)",
      phase_streaming: "Capturing",
      phase_error: "Error",
      warn_hdcp: "HDCP DETECTED",
      warn_non_pcm: "NON-PCM AUDIO (SWITCH TO LPCM)",
      hud_mode_mjpeg: "KVM · MJPEG HW",
      hud_mode_h264: "STUDIO · H.264 + AAC",
      standby_waiting_title: "Waiting for GL310 device",
      standby_waiting_sub: "Connect your AVerMedia LGP Lite over USB 2.0 (07ca:c835) and plug in an unencrypted HDMI source.",
      standby_preview_off_title: "Capture active — Preview disabled (CPU saving)",
      standby_preview_off_sub: "OBS (/dev/video14, GL310 Audio, SRT/HTTP) and .mp4 recording continue normally in the background.",
      standby_http_off_title: "Capture active — HTTP preview disabled",
      standby_http_off_sub: "Re-enable the built-in HTTP server (:{port}) in the OBS Outputs tab to display the live preview.",
      standby_no_hdmi_title: "Capture active — No HDMI IN signal",
      standby_no_hdmi_sub: "Check your HDMI IN cable and video source. Preview and OBS camera (/dev/video14) will start automatically once HDMI locks.",
      standby_hdcp_title: "Signal blocked by HDCP protection",
      standby_hdcp_sub: "The GL310 cannot capture HDCP-encrypted signals. Disable HDCP on your console or source device.",
      standby_wait_usb_title: "Capture active — Waiting for USB stream…",
      standby_decoding_title: "Decoding first video frame…",
      standby_decoding_sub: "Mode {mode} · {pkts} TS packets received · Check \"Device & Logs\" if video does not appear.",
      mode_name_mjpeg: "Hardware MJPEG",
      mode_name_h264: "H.264 + AAC",
      standby_ready_sig: "Source ready: {w}×{h}@{hz} Hz",
      standby_ready_nosig: "GL310 ready — No HDMI source connected",
      standby_ready_sub: "GL310 ready (verified firmware active in RAM).",
      standby_perm_title: "USB permission required (udev rule)",
      standby_fw_title: "AVerMedia firmwares required",
      standby_err_title: "GL310 capture error",
      standby_offline_title: "UI preview outside Tauri",
      standby_offline_sub: "Launch gl310-studio to enable direct USB communication with gl310-core.",
      btn_reenable_preview: "Re-enable Preview",
      btn_rescan_usb: "Rescan USB",
      btn_open_fw_wizard: "Open Firmware Assistant",
      btn_start_capture: "Start Capture",
      btn_stop_capture: "Stop Capture",
      btn_starting: "Starting…",
      btn_stopping: "Stopping…",
      btn_preview_on: "Preview",
      btn_preview_off: "Preview Off",
      title_preview_toggle: "Toggle live video preview (saves CPU without interrupting OBS or recording)",
      btn_record_start: "Record .mp4",
      btn_record_stop: "Stop REC",
      btn_snapshot: "Snapshot",
      title_snapshot: "Take a JPEG snapshot",
      title_snapshot_preview_off: "Re-enable preview to take a JPEG snapshot in H.264 mode",
      tab_video: "Video",
      tab_audio: "Audio & Jacks",
      tab_outputs: "OBS Outputs",
      tab_device: "Device & Logs",
      grp_h264: "H.264 Encoder (Studio OBS)",
      lbl_fps_policy: "Encoder Frame Rate",
      opt_fps_auto: "Auto (1080p30 / 720p60)",
      opt_fps_30: "Force 30 fps",
      opt_fps_25: "Force 25 fps (PAL)",
      opt_fps_24: "Force 24 fps",
      lbl_video_bitrate: "Video Bitrate",
      lbl_gop: "GOP Interval (Keyframes)",
      opt_gop_1s: "1 second (low latency)",
      opt_gop_2s: "2 seconds (OBS/YT standard)",
      lbl_v4l2_format: "V4L2 Camera Format (/dev/video14)",
      opt_v4l2_auto: "Auto (native resolution)",
      opt_v4l2_fixed: "Force fixed 1080p 30 fps",
      grp_kvm: "KVM Mode (Hardware MJPEG)",
      tag_zero_cpu: "0% CPU",
      lbl_mjpeg_res: "MJPEG Resolution",
      opt_mjpeg_360: "640 × 360 (ultra light)",
      opt_mjpeg_720: "1280 × 720 (recommended KVM)",
      lbl_mjpeg_fps: "KVM Frame Rate",
      opt_mjpeg_15: "15 fps (low bandwidth)",
      lbl_mjpeg_quality: "Hardware JPEG Quality",
      btn_apply_video: "Apply Video Settings",
      grp_audio_source: "Hardware Audio Source",
      btn_ain_jack: "3.5 mm Jack AUDIO IN",
      lbl_analog_gain: "Analog Line-In Gain Jack IN (MCU 0x41)",
      lbl_encoder_gain: "Encoder Digital Gain (0x6F8)",
      lbl_audio_bitrate: "AAC Audio Bitrate",
      opt_aac_128: "128 kbps",
      opt_aac_160: "160 kbps",
      opt_aac_192: "192 kbps",
      opt_aac_256: "256 kbps",
      grp_pc_monitor: "Headphone & PC Monitoring",
      lbl_audio_out_jack: "3.5 mm Jack AUDIO OUT (device)",
      hint_audio_out_jack: "Zero-latency analog passthrough (aou_pass_en)",
      lbl_pc_monitor: "Live PC Monitoring",
      hint_pc_monitor: "Play GL310 audio through your Linux speakers",
      banner_hdmi_out_title: "HDMI OUT Passthrough Status",
      banner_hdmi_out_body: "Direct hardware HDMI loop-through (always active when HDMI IN is locked — no software mute register on GL310).",
      grp_bridges: "Local & Network Bridges",
      lbl_v4l2_toggle: "V4L2 Camera <code class=\"mono\">/dev/video14</code>",
      hint_v4l2_toggle: "Created without sudo (\"AVerMedia GL310\")",
      lbl_pw_toggle: "PipeWire Virtual Mic \"GL310 Audio\"",
      hint_pw_toggle: "Dedicated OBS Studio audio source (48 kHz stereo)",
      lbl_srt_toggle: "SRT Stream <code class=\"mono\">:4850</code>",
      hint_srt_toggle: "Zero-transcode H.264+AAC copy for OBS",
      lbl_http_toggle: "Built-in HTTP Server <code class=\"mono\">:4848</code>",
      hint_http_toggle: "Exposes /stream.ts, /mjpeg, and /snapshot.jpg",
      lbl_lan_toggle: "Expose on LAN (<code class=\"mono\">0.0.0.0</code>)",
      hint_lan_toggle: "Disabled by default (127.0.0.1 only)",
      btn_apply_outputs: "Apply Outputs",
      grp_live_urls: "Live URLs",
      btn_copy: "Copy",
      banner_obs_title: "OBS Studio Quick Guide",
      banner_obs_m1: "<strong>Method 1 (Simple):</strong> Video Capture Device (V4L2) → <code>AVerMedia GL310 (/dev/video14)</code> + Audio Input Capture → <code>GL310 Audio</code>.",
      banner_obs_m2: "<strong>Method 2 (Zero Re-encoding):</strong> Media Source → uncheck <em>Local File</em> → paste the SRT or HTTP TS URL → FFmpeg Options: <code>probesize=500000 analyzeduration=500000</code>.",
      grp_led_btn: "LED Ring & Physical Button",
      lbl_btn_action: "Center Button Action",
      opt_btn_record: "Record / Stop .mp4",
      opt_btn_capture: "Start / Stop Capture",
      opt_btn_snapshot: "Take JPEG Snapshot",
      opt_btn_none: "No action",
      led_auto: "Auto",
      led_clockwise: "Spin",
      led_solid: "Solid",
      led_fade: "Breathe",
      led_fast: "Fast",
      grp_firmware: "AVerMedia Firmwares (SHA-256)",
      fw_checking: "Checking…",
      fw_ok: "SHA-256 OK",
      fw_invalid: "INVALID",
      fw_missing: "MISSING",
      fw_hint: "Not hosted in Git. Strict SHA-256 verification enforced before RAM upload.",
      btn_fw_fetch: "Download (Audited Archive.org)",
      btn_fw_fetching: "Downloading & auditing…",
      ph_fw_path: "/path/to/GL310_driver.zip or Windows folder",
      btn_fw_import: "Import",
      grp_logs: "System Console",
      log_all: "All",
      log_warn: "Warnings",
      log_error: "Errors",
      log_clear: "Clear",
      toast_settings_applied: "Settings applied",
      toast_error: "Error: {err}",
      toast_mode_h264: "Studio OBS mode (H.264 + AAC)",
      toast_mode_mjpeg: "Hardware KVM mode (MJPEG)",
      toast_capture_stopped: "Capture stopped",
      toast_capture_started: "Capture started ({w}×{h} @ {fps} fps)",
      toast_capture_error: "Capture error: {err}",
      toast_preview_on: "Video preview enabled",
      toast_preview_off: "Preview disabled — OBS and recording continue (reduced CPU)",
      toast_preview_error: "Preview error: {err}",
      toast_rec_saved: "Saved: {path}",
      toast_rec_stopped: "Recording stopped",
      toast_rec_started: "Recording started (.mp4)",
      toast_rec_error: "REC error: {err}",
      toast_snapshot_saved: "Snapshot saved: {path}",
      toast_snapshot_error: "Snapshot error: {err}",
      toast_usb_rescan: "USB rescan triggered",
      toast_video_applied: "Video settings applied",
      toast_outputs_applied: "OBS & network outputs updated",
      toast_audio_jack: "Input: 3.5 mm Jack AUDIO IN",
      toast_audio_hdmi: "Input: HDMI IN",
      toast_audio_error: "Audio error: {err}",
      toast_aac_updated: "AAC bitrate updated",
      toast_jack_out_updated: "AUDIO OUT jack updated",
      toast_url_copied: "URL copied to clipboard",
      toast_url_copied_short: "URL copied",
      toast_btn_action_saved: "Button action saved",
      toast_led_set: "LED ring: {label}",
      toast_led_error: "LED error: {err}",
      toast_fw_ok: "Firmwares audited and installed!",
      toast_fw_fail: "Download failed",
      toast_fw_error: "Firmware error: {err}",
      toast_fw_path_required: "Enter a path to a Windows driver .zip or folder",
      toast_fw_imported: "Firmwares imported",
      toast_fw_import_error: "Import error: {err}",
      toast_lang_en: "Language: English",
      toast_lang_fr: "Langue : Français",
    },
    fr: {
      aria_capture_mode: "Mode de capture matériel",
      aria_language: "Langue de l’interface",
      aria_stage: "Moniteur vidéo et transport",
      aria_vu: "VU-mètre stéréo L et R",
      aria_console: "Panneau de réglages GL310",
      aria_tabs: "Sections de configuration",
      aria_pc_mute: "Couper ou réactiver le monitoring PC",
      aria_pc_vol: "Volume monitoring PC",
      aria_led_patterns: "Motifs anneau LED",
      usb_scanning: "Recherche USB…",
      telemetry_hdmi: "Signal HDMI",
      telemetry_bitrate: "Débit",
      telemetry_ts_errors: "Erreurs TS",
      no_signal: "Aucun signal",
      unit_mbps: "Mbit/s",
      unit_mb: "Mo",
      phase_absent: "Absent (USB)",
      phase_permission_denied: "udev requis",
      phase_locked_d835: "Verrouillé d835",
      phase_firmware_missing: "Firmware absent",
      phase_booting: "Chargement RAM…",
      phase_idle: "Prêt (RAM QSOS)",
      phase_streaming: "Capture active",
      phase_error: "Erreur",
      warn_hdcp: "HDCP DÉTECTÉ",
      warn_non_pcm: "AUDIO NON-PCM (PASSER EN LPCM)",
      hud_mode_mjpeg: "KVM · MJPEG HW",
      hud_mode_h264: "STUDIO · H.264 + AAC",
      standby_waiting_title: "En attente du boîtier GL310",
      standby_waiting_sub: "Branchez votre AVerMedia LGP Lite en USB 2.0 (07ca:c835) et connectez une source HDMI sans HDCP.",
      standby_preview_off_title: "Capture active — Aperçu désactivé (économie CPU)",
      standby_preview_off_sub: "OBS (/dev/video14, GL310 Audio, SRT/HTTP) et l’enregistrement .mp4 continuent normalement en arrière-plan.",
      standby_http_off_title: "Capture active — aperçu HTTP désactivé",
      standby_http_off_sub: "Réactivez le serveur HTTP intégré (:{port}) dans l’onglet Sorties OBS pour afficher l’aperçu en direct.",
      standby_no_hdmi_title: "Capture active — Aucun signal HDMI IN",
      standby_no_hdmi_sub: "Vérifiez le câble HDMI IN et la source vidéo. L’aperçu et la caméra OBS (/dev/video14) s’activeront automatiquement dès le verrouillage HDMI.",
      standby_hdcp_title: "Signal bloqué par protection HDCP",
      standby_hdcp_sub: "Le boîtier GL310 ne peut pas capturer un flux chiffré HDCP. Désactivez HDCP sur la console ou la source.",
      standby_wait_usb_title: "Capture active — En attente du flux USB…",
      standby_decoding_title: "Décodage de la première image…",
      standby_decoding_sub: "Mode {mode} · {pkts} paquets TS reçus · Consultez « Boîtier & Logs » si aucun flux n’apparaît.",
      mode_name_mjpeg: "MJPEG matériel",
      mode_name_h264: "H.264 + AAC",
      standby_ready_sig: "Source prête : {w}×{h}@{hz} Hz",
      standby_ready_nosig: "GL310 prêt — Aucune source HDMI branchée",
      standby_ready_sub: "GL310 prêt (firmware vérifié actif en RAM).",
      standby_perm_title: "Permission USB requise (règle udev)",
      standby_fw_title: "Firmwares AVerMedia requis",
      standby_err_title: "Erreur de capture GL310",
      standby_offline_title: "Aperçu UI hors Tauri",
      standby_offline_sub: "Lancez gl310-studio pour activer la communication USB directe avec gl310-core.",
      btn_reenable_preview: "Réactiver l’aperçu",
      btn_rescan_usb: "Réanalyser l’USB",
      btn_open_fw_wizard: "Ouvrir l’assistant Firmware",
      btn_start_capture: "Démarrer la capture",
      btn_stop_capture: "Arrêter la capture",
      btn_starting: "Démarrage…",
      btn_stopping: "Arrêt…",
      btn_preview_on: "Aperçu",
      btn_preview_off: "Aperçu coupé",
      title_preview_toggle: "Activer ou désactiver l’aperçu vidéo (économise le CPU sans couper OBS ni l’enregistrement)",
      btn_record_start: "Enregistrer .mp4",
      btn_record_stop: "Arrêter REC",
      btn_snapshot: "Capture photo",
      title_snapshot: "Prendre une capture photo JPEG",
      title_snapshot_preview_off: "Réactivez l’aperçu pour prendre une capture photo en mode H.264",
      tab_video: "Vidéo",
      tab_audio: "Audio & Jacks",
      tab_outputs: "Sorties OBS",
      tab_device: "Boîtier & Logs",
      grp_h264: "Encodeur H.264 (Studio OBS)",
      lbl_fps_policy: "Cadence IPS encodeur",
      opt_fps_auto: "Auto (1080p30 / 720p60)",
      opt_fps_30: "Forcer 30 fps",
      opt_fps_25: "Forcer 25 fps (PAL)",
      opt_fps_24: "Forcer 24 fps",
      lbl_video_bitrate: "Débit vidéo",
      lbl_gop: "Intervalle GOP (images clés)",
      opt_gop_1s: "1 seconde (faible latence)",
      opt_gop_2s: "2 secondes (standard OBS/YT)",
      lbl_v4l2_format: "Format caméra V4L2 (/dev/video14)",
      opt_v4l2_auto: "Auto (résolution native)",
      opt_v4l2_fixed: "Forcer 1080p 30 fps fixe",
      grp_kvm: "Mode KVM (MJPEG matériel)",
      tag_zero_cpu: "0 % CPU",
      lbl_mjpeg_res: "Résolution MJPEG",
      opt_mjpeg_360: "640 × 360 (ultra léger)",
      opt_mjpeg_720: "1280 × 720 (KVM recommandé)",
      lbl_mjpeg_fps: "Cadence KVM",
      opt_mjpeg_15: "15 fps (faible bande passante)",
      lbl_mjpeg_quality: "Qualité JPEG matérielle",
      btn_apply_video: "Appliquer les réglages vidéo",
      grp_audio_source: "Source Audio Matérielle",
      btn_ain_jack: "Jack 3,5 mm AUDIO IN",
      lbl_analog_gain: "Gain ligne analogique Jack IN (MCU 0x41)",
      lbl_encoder_gain: "Gain numérique encodeur (0x6F8)",
      lbl_audio_bitrate: "Débit audio AAC",
      opt_aac_128: "128 kbit/s",
      opt_aac_160: "160 kbit/s",
      opt_aac_192: "192 kbit/s",
      opt_aac_256: "256 kbit/s",
      grp_pc_monitor: "Sorties Casque & Monitoring PC",
      lbl_audio_out_jack: "Jack 3,5 mm AUDIO OUT (boîtier)",
      hint_audio_out_jack: "Passthrough analogique zéro latence (aou_pass_en)",
      lbl_pc_monitor: "Monitoring PC en direct",
      hint_pc_monitor: "Écoute le son du GL310 sur vos haut-parleurs Linux",
      banner_hdmi_out_title: "Statut HDMI OUT Passthrough",
      banner_hdmi_out_body: "Boucle HDMI matérielle directe (toujours active quand HDMI IN est verrouillé — aucun registre de coupure logiciel sur GL310).",
      grp_bridges: "Ponts Locaux & Réseau",
      lbl_v4l2_toggle: "Caméra V4L2 <code class=\"mono\">/dev/video14</code>",
      hint_v4l2_toggle: "Créée sans sudo (« AVerMedia GL310 »)",
      lbl_pw_toggle: "Micro virtuel PipeWire « GL310 Audio »",
      hint_pw_toggle: "Source audio dédiée OBS Studio (48 kHz stéréo)",
      lbl_srt_toggle: "Flux SRT <code class=\"mono\">:4850</code>",
      hint_srt_toggle: "Copie H.264+AAC zéro transcodage pour OBS",
      lbl_http_toggle: "Serveur HTTP intégré <code class=\"mono\">:4848</code>",
      hint_http_toggle: "Expose /stream.ts, /mjpeg et /snapshot.jpg",
      lbl_lan_toggle: "Exposition réseau LAN (<code class=\"mono\">0.0.0.0</code>)",
      hint_lan_toggle: "Désactivé par défaut (127.0.0.1 uniquement)",
      btn_apply_outputs: "Appliquer les sorties",
      grp_live_urls: "URLs en direct",
      btn_copy: "Copier",
      banner_obs_title: "Guide rapide OBS Studio",
      banner_obs_m1: "<strong>Méthode 1 (Simple) :</strong> Périphérique de capture vidéo (V4L2) → <code>AVerMedia GL310 (/dev/video14)</code> + Capture d’entrée audio → <code>GL310 Audio</code>.",
      banner_obs_m2: "<strong>Méthode 2 (Zéro ré-encodage) :</strong> Source Média → décochez <em>Fichier local</em> → collez l’URL SRT ou HTTP TS → Options FFmpeg : <code>probesize=500000 analyzeduration=500000</code>.",
      grp_led_btn: "Anneau LED & Bouton physique",
      lbl_btn_action: "Action du bouton central",
      opt_btn_record: "Enregistrer / Arrêter .mp4",
      opt_btn_capture: "Démarrer / Arrêter capture",
      opt_btn_snapshot: "Prendre une capture photo",
      opt_btn_none: "Aucune action",
      led_auto: "Auto",
      led_clockwise: "Rotation",
      led_solid: "Fixe",
      led_fade: "Respiration",
      led_fast: "Rapide",
      grp_firmware: "Firmwares AVerMedia (SHA-256)",
      fw_checking: "Vérification…",
      fw_ok: "SHA-256 OK",
      fw_invalid: "INVALIDE",
      fw_missing: "ABSENT",
      fw_hint: "Non hébergés dans le dépôt Git. Vérification SHA-256 stricte avant chargement en RAM.",
      btn_fw_fetch: "Télécharger (Audité Archive.org)",
      btn_fw_fetching: "Téléchargement et audit…",
      ph_fw_path: "/chemin/vers/GL310_driver.zip ou dossier Windows",
      btn_fw_import: "Importer",
      grp_logs: "Console système",
      log_all: "Tous",
      log_warn: "Alertes",
      log_error: "Erreurs",
      log_clear: "Effacer",
      toast_settings_applied: "Réglages appliqués",
      toast_error: "Erreur : {err}",
      toast_mode_h264: "Mode Studio OBS (H.264 + AAC)",
      toast_mode_mjpeg: "Mode KVM matériel (MJPEG)",
      toast_capture_stopped: "Capture arrêtée",
      toast_capture_started: "Capture démarrée ({w}×{h} @ {fps} fps)",
      toast_capture_error: "Erreur capture : {err}",
      toast_preview_on: "Aperçu vidéo réactivé",
      toast_preview_off: "Aperçu désactivé — OBS et l’enregistrement continuent (CPU réduit)",
      toast_preview_error: "Erreur aperçu : {err}",
      toast_rec_saved: "Enregistré : {path}",
      toast_rec_stopped: "Enregistrement terminé",
      toast_rec_started: "Enregistrement démarré (.mp4)",
      toast_rec_error: "Erreur REC : {err}",
      toast_snapshot_saved: "Photo enregistrée : {path}",
      toast_snapshot_error: "Erreur photo : {err}",
      toast_usb_rescan: "Nouvelle détection USB lancée",
      toast_video_applied: "Réglages vidéo appliqués",
      toast_outputs_applied: "Sorties OBS & réseau mises à jour",
      toast_audio_jack: "Entrée : Jack 3,5 mm AUDIO IN",
      toast_audio_hdmi: "Entrée : HDMI IN",
      toast_audio_error: "Erreur audio : {err}",
      toast_aac_updated: "Débit AAC mis à jour",
      toast_jack_out_updated: "Jack AUDIO OUT mis à jour",
      toast_url_copied: "URL copiée dans le presse-papiers",
      toast_url_copied_short: "URL copiée",
      toast_btn_action_saved: "Action du bouton enregistrée",
      toast_led_set: "Anneau LED : {label}",
      toast_led_error: "Erreur LED : {err}",
      toast_fw_ok: "Firmwares audités et installés !",
      toast_fw_fail: "Échec téléchargement",
      toast_fw_error: "Erreur firmware : {err}",
      toast_fw_path_required: "Indiquez un chemin vers un fichier .zip ou dossier pilote Windows",
      toast_fw_imported: "Firmwares importés",
      toast_fw_import_error: "Erreur import : {err}",
      toast_lang_en: "Language: English",
      toast_lang_fr: "Langue : Français",
    },
  };

  let currentLang = "en";
  let currentSnap = null;
  let settingsHydrated = false;
  let captureBusy = false;
  let previewBusy = false;
  let fwFetching = false;
  let activePreviewKey = "";
  let previewRetryTimer = null;
  let logFilter = "all";
  let toastTimer = null;
  const customSelectUpdaters = [];

  function normalizeLang(lang) {
    return lang === "fr" ? "fr" : "en";
  }

  function t(key, vars = null) {
    const dict = I18N[currentLang] || I18N.en;
    let out = dict[key] !== undefined ? dict[key] : I18N.en[key] || key;
    if (vars) {
      for (const [k, v] of Object.entries(vars)) {
        out = out.replaceAll(`{${k}}`, String(v));
      }
    }
    return out;
  }

  function formatPercent(val) {
    return currentLang === "fr" ? `${val} %` : `${val}%`;
  }

  function formatMbps(kbps) {
    return `${Math.round(kbps / 1000)} ${t("unit_mbps")}`;
  }

  function applyLanguage(lang) {
    currentLang = normalizeLang(lang);
    document.documentElement.lang = currentLang;

    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.getAttribute("data-i18n");
      if (key) setText(el, t(key));
    });
    document.querySelectorAll("[data-i18n-html]").forEach((el) => {
      const key = el.getAttribute("data-i18n-html");
      if (key) el.innerHTML = t(key);
    });
    document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
      const key = el.getAttribute("data-i18n-placeholder");
      if (key) el.setAttribute("placeholder", t(key));
    });
    document.querySelectorAll("[data-i18n-title]").forEach((el) => {
      const key = el.getAttribute("data-i18n-title");
      if (key) el.setAttribute("title", t(key));
    });
    document.querySelectorAll("[data-i18n-aria]").forEach((el) => {
      const key = el.getAttribute("data-i18n-aria");
      if (key) el.setAttribute("aria-label", t(key));
    });

    document.querySelectorAll("[data-lang]").forEach((btn) => {
      const active = btn.dataset.lang === currentLang;
      setClassFlag(btn, "active", active);
      btn.setAttribute("aria-pressed", String(active));
    });

    // Refresh formatted readouts & custom dropdown labels
    const kbpsVal = Number($("range-video-kbps").value || 8000);
    setText($("val-video-kbps"), formatMbps(kbpsVal));
    setText($("val-pc-vol"), formatPercent(Number($("range-pc-vol").value || 80)));
    if (fwFetching) {
      setText($("btn-fw-fetch"), t("btn_fw_fetching"));
    }
    syncCustomSelects();

    if (currentSnap) {
      applySnapshot(currentSnap);
    } else if (!hasTauri) {
      setText($("standby-title"), t("standby_offline_title"));
      setText($("standby-sub"), t("standby_offline_sub"));
    }
  }

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

      const optionEntries = [];
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
        optionEntries.push({ btn, lbl, opt });
      });

      function syncThis() {
        const selectedOpt = sel.options[sel.selectedIndex] || sel.options[0];
        valSpan.textContent = selectedOpt ? selectedOpt.textContent : "";
        optionEntries.forEach(({ btn, lbl, opt }) => {
          if (lbl.textContent !== opt.textContent) {
            lbl.textContent = opt.textContent;
          }
          const isSel = btn.dataset.value === sel.value;
          btn.classList.toggle("selected", isSel);
          btn.setAttribute("aria-selected", String(isSel));
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
    return `${(bytes / (1024 * 1024)).toFixed(1)} ${t("unit_mb")}`;
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

    if (!settingsHydrated) {
      const savedLang = normalizeLang(snap.settings && snap.settings.ui_language);
      settingsHydrated = true;
      hydrateControls(snap.settings);
      if (savedLang !== currentLang) {
        applyLanguage(savedLang);
        return;
      }
    }

    // 1. Device Status Pill
    const pill = $("device-pill");
    const pillText = $("device-pill-text");
    const phaseMap = {
      absent: ["state-absent", t("phase_absent")],
      permission_denied: ["state-warn", t("phase_permission_denied")],
      locked_d835: ["state-error", t("phase_locked_d835")],
      firmware_missing: ["state-warn", t("phase_firmware_missing")],
      booting: ["state-booting", t("phase_booting")],
      idle: ["state-idle", t("phase_idle")],
      streaming: ["state-streaming", t("phase_streaming")],
      error: ["state-error", t("phase_error")],
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
        setText(warnEl, t("warn_hdcp"));
        setHidden(warnEl, false);
      } else if (sig.audio && sig.audio.compressed_non_pcm) {
        setText(warnEl, t("warn_non_pcm"));
        setHidden(warnEl, false);
      } else {
        setHidden(warnEl, true);
      }
    } else {
      setText($("signal-readout"), t("no_signal"));
      setHidden(resBadge, true);
      setHidden(warnEl, true);
    }

    setText($("bitrate-readout"), `${(snap.bitrate_mbps || 0).toFixed(1)} ${t("unit_mbps")}`);
    setText($("ts-errors-readout"), String(snap.ts_stats ? snap.ts_stats.continuity_errors : 0));

    // 3. Mode Toggle Buttons & HUD
    const isMjpeg = snap.settings.mode === "mjpeg";
    setClassFlag($("mode-btn-h264"), "active", !isMjpeg);
    setClassFlag($("mode-btn-mjpeg"), "active", isMjpeg);
    setText($("hud-mode-badge"), isMjpeg ? t("hud_mode_mjpeg") : t("hud_mode_h264"));

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
          ? t("standby_preview_off_title")
          : !snap.mjpeg_url
            ? t("standby_http_off_title")
            : !sig
              ? t("standby_no_hdmi_title")
              : sig.hdcp
                ? t("standby_hdcp_title")
                : tsPkts === 0 && !isMjpeg
                  ? t("standby_wait_usb_title")
                  : t("standby_decoding_title")
        : snap.phase === "idle"
          ? sig
            ? t("standby_ready_sig", { w: sig.width, h: sig.height, hz: sig.refresh_hz })
            : t("standby_ready_nosig")
          : snap.phase === "permission_denied"
            ? t("standby_perm_title")
            : snap.phase === "firmware_missing"
              ? t("standby_fw_title")
              : snap.phase === "error"
                ? t("standby_err_title")
                : t("standby_waiting_title");
      const standbySub = isStreaming
        ? !previewEnabled
          ? t("standby_preview_off_sub")
          : !snap.mjpeg_url
            ? t("standby_http_off_sub", { port: snap.settings.http_port })
            : !sig
              ? t("standby_no_hdmi_sub")
              : sig.hdcp
                ? t("standby_hdcp_sub")
                : t("standby_decoding_sub", {
                    mode: isMjpeg ? t("mode_name_mjpeg") : t("mode_name_h264"),
                    pkts: tsPkts,
                  })
        : snap.phase === "idle"
          ? t("standby_ready_sub")
          : snap.phase === "absent"
            ? t("standby_waiting_sub")
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
      setText($("btn-capture-label"), isStreaming ? t("btn_stop_capture") : t("btn_start_capture"));
      setHidden($("icon-capture-play"), isStreaming);
      setHidden($("icon-capture-stop"), !isStreaming);
    }

    if (!previewBusy) {
      const prevBtn = $("btn-toggle-preview");
      if (prevBtn) {
        prevBtn.disabled = false;
        setClassFlag(prevBtn, "is-off", !previewEnabled);
        prevBtn.setAttribute("aria-pressed", String(previewEnabled));
        setText($("btn-preview-label"), previewEnabled ? t("btn_preview_on") : t("btn_preview_off"));
        setHidden($("icon-preview-on"), !previewEnabled);
        setHidden($("icon-preview-off"), previewEnabled);
      }
    }

    const recActive = Boolean(snap.recording && snap.recording.active);
    const recBtn = $("btn-toggle-record");
    recBtn.disabled = !isStreaming || isMjpeg;
    setClassFlag(recBtn, "recording", recActive);
    setText($("btn-record-label"), recActive ? t("btn_record_stop") : t("btn_record_start"));
    const snapBtn = $("btn-snapshot");
    const snapDisabled = !isStreaming || (!isMjpeg && !previewEnabled);
    snapBtn.disabled = snapDisabled;
    snapBtn.title =
      !isMjpeg && !previewEnabled
        ? t("title_snapshot_preview_off")
        : t("title_snapshot");

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
      setText($("fw-badge"), fw.valid ? t("fw_ok") : fw.found ? t("fw_invalid") : t("fw_missing"));
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
    setText($("hdmi-out-status"), t("banner_hdmi_out_body"));
  }

  function hydrateControls(s) {
    $("sel-fps-policy").value = s.fps_policy;
    $("range-video-kbps").value = String(s.video_kbps);
    $("val-video-kbps").textContent = formatMbps(s.video_kbps);
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
    $("val-pc-vol").textContent = formatPercent(s.pc_monitor_volume);
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
      ui_language: currentLang,
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

  async function saveSettings(patch = {}, toastText = null) {
    try {
      const next = collectSettings(patch);
      await invoke("update_settings", { settings: next });
      showToast(toastText || t("toast_settings_applied"));
    } catch (err) {
      showToast(t("toast_error", { err }));
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
  document.querySelectorAll("[data-lang]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const nextLang = normalizeLang(btn.dataset.lang);
      if (nextLang === currentLang) return;
      applyLanguage(nextLang);
      if (hasTauri) {
        try {
          const nextSnap = await invoke("set_language", { language: nextLang });
          if (nextSnap) applySnapshot(nextSnap);
        } catch (err) {
          showToast(t("toast_error", { err }));
          return;
        }
      }
      showToast(nextLang === "fr" ? t("toast_lang_fr") : t("toast_lang_en"));
    });
  });

  $("mode-btn-h264").addEventListener("click", () =>
    saveSettings({ mode: "h264_aac" }, t("toast_mode_h264"))
  );
  $("mode-btn-mjpeg").addEventListener("click", () =>
    saveSettings({ mode: "mjpeg" }, t("toast_mode_mjpeg"))
  );

  $("btn-toggle-capture").addEventListener("click", async () => {
    if (captureBusy) return;
    const capBtn = $("btn-toggle-capture");
    const stopping = Boolean(currentSnap && currentSnap.phase === "streaming");
    captureBusy = true;
    capBtn.disabled = true;
    setText($("btn-capture-label"), stopping ? t("btn_stopping") : t("btn_starting"));
    try {
      const nextSnap = await invoke(stopping ? "stop_capture" : "start_capture");
      captureBusy = false;
      if (nextSnap) {
        applySnapshot(nextSnap);
      } else {
        capBtn.disabled = false;
      }
      if (stopping) {
        showToast(t("toast_capture_stopped"));
      } else if (nextSnap && nextSnap.phase === "streaming") {
        showToast(
          t("toast_capture_started", {
            w: nextSnap.out_width,
            h: nextSnap.out_height,
            fps: nextSnap.out_fps,
          })
        );
      }
    } catch (err) {
      captureBusy = false;
      capBtn.disabled = false;
      if (currentSnap) applySnapshot(currentSnap);
      showToast(t("toast_capture_error", { err }));
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
      showToast(nextEnabled ? t("toast_preview_on") : t("toast_preview_off"));
    } catch (err) {
      previewBusy = false;
      if (btn) btn.disabled = false;
      showToast(t("toast_preview_error", { err }));
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
        showToast(res ? t("toast_rec_saved", { path: res.final_path }) : t("toast_rec_stopped"));
      } else {
        await invoke("start_recording");
        showToast(t("toast_rec_started"));
      }
    } catch (err) {
      showToast(t("toast_rec_error", { err }));
    }
  });

  $("btn-snapshot").addEventListener("click", async () => {
    try {
      const path = await invoke("take_snapshot");
      showToast(t("toast_snapshot_saved", { path }));
    } catch (err) {
      showToast(t("toast_snapshot_error", { err }));
    }
  });

  $("btn-standby-refresh").addEventListener("click", async () => {
    await invoke("refresh_device");
    showToast(t("toast_usb_rescan"));
  });

  $("btn-standby-fw").addEventListener("click", () => $("tab-device").click());

  $("range-video-kbps").addEventListener("input", (e) => {
    const val = Number(e.target.value);
    $("val-video-kbps").textContent = formatMbps(val);
    syncBitrateChips(val);
  });

  document.querySelectorAll('[data-preset="video_kbps"] .chip-btn').forEach((btn) => {
    btn.addEventListener("click", () => {
      const val = Number(btn.dataset.val);
      $("range-video-kbps").value = String(val);
      $("val-video-kbps").textContent = formatMbps(val);
      syncBitrateChips(val);
    });
  });

  $("range-mjpeg-q").addEventListener("input", (e) => {
    $("val-mjpeg-q").textContent = `${e.target.value} / 15`;
  });

  $("btn-apply-video").addEventListener("click", () => saveSettings({}, t("toast_video_applied")));
  $("btn-apply-outputs").addEventListener("click", () => saveSettings({}, t("toast_outputs_applied")));

  // Audio controls (immediate hardware updates)
  document.querySelectorAll("[data-ain]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const src = btn.dataset.ain;
      $("btn-ain-hdmi").classList.toggle("active", src === "hdmi");
      $("btn-ain-jack").classList.toggle("active", src === "jack");
      try {
        await invoke("set_audio_input", { input: src });
        showToast(src === "jack" ? t("toast_audio_jack") : t("toast_audio_hdmi"));
      } catch (err) {
        showToast(t("toast_audio_error", { err }));
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

  $("sel-audio-kbps").addEventListener("change", () => saveSettings({}, t("toast_aac_updated")));
  $("chk-audio-out-jack").addEventListener("change", () => saveSettings({}, t("toast_jack_out_updated")));

  async function pushPcMonitor() {
    const enabled = $("chk-pc-monitor").checked;
    const muted = $("btn-pc-mute").classList.contains("muted");
    const volume = Number($("range-pc-vol").value);
    $("val-pc-vol").textContent = formatPercent(volume);
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
        showToast(t("toast_url_copied"));
      } catch {
        inp.select();
        document.execCommand("copy");
        showToast(t("toast_url_copied_short"));
      }
    });
  });

  // Device tab: Button action, LED ring, Firmware wizard, Logs
  $("sel-button-action").addEventListener("change", () => saveSettings({}, t("toast_btn_action_saved")));

  document.querySelectorAll("[data-led]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const mode = btn.dataset.led;
      updateLedUi(mode);
      try {
        await invoke("set_led", { mode });
        showToast(t("toast_led_set", { label: btn.textContent }));
      } catch (err) {
        showToast(t("toast_led_error", { err }));
      }
    });
  });

  $("btn-fw-fetch").addEventListener("click", async () => {
    const btn = $("btn-fw-fetch");
    fwFetching = true;
    btn.disabled = true;
    btn.textContent = t("btn_fw_fetching");
    try {
      const res = await invoke("firmware_fetch");
      showToast(res && res.ok ? t("toast_fw_ok") : t("toast_fw_fail"));
    } catch (err) {
      showToast(t("toast_fw_error", { err }));
    } finally {
      fwFetching = false;
      btn.disabled = false;
      btn.textContent = t("btn_fw_fetch");
      refreshLogs();
    }
  });

  $("btn-fw-import").addEventListener("click", async () => {
    const path = $("inp-fw-path").value.trim();
    if (!path) {
      showToast(t("toast_fw_path_required"));
      return;
    }
    try {
      const res = await invoke("firmware_import", { path });
      showToast(res ? res.message : t("toast_fw_imported"));
    } catch (err) {
      showToast(t("toast_fw_import_error", { err }));
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
    applyLanguage("en");
    if (!hasTauri) {
      setText($("standby-title"), t("standby_offline_title"));
      setText($("standby-sub"), t("standby_offline_sub"));
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
