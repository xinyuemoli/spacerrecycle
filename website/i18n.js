// SpaceRecycle website i18n (en / zh).
// No framework, no build step: a plain translation table + a tiny switcher.
(function () {
  "use strict";

  var I18N = {
    en: {
      lang_switch: "中文",
      page_title: "SpaceRecycle — a disk cleaner you can actually trust",
      page_desc: "A Windows disk-space reclamation tool that never deletes anything on its own. Open source, auditable, and safe by design.",
      nav_features: "Features",
      nav_trust: "Why trust it",
      nav_download: "Download",
      hero_title: "A disk cleaner you can <em>actually</em> trust.",
      hero_lede: "SpaceRecycle tells you where your disk space went, lets you pick what to clean, and asks you to confirm — <strong>it never deletes anything on its own.</strong>",
      cta_download: "Download for Windows",
      cta_features: "See what it finds",
      hero_hint: "Windows 10 1809+ &middot; ~10 MB &middot; open source (MIT) &middot; no ads, no bundleware, no background services",
      features_title: "Where your space actually went",
      feat1_title: "🧹 Temp junk &amp; caches",
      feat1_desc: "Temp folders, browser caches, crash dumps, and Windows Update leftovers — typically 1–20 GB, safe to reclaim.",
      feat2_title: "📦 Dev artifacts",
      feat2_desc: "<code>node_modules</code>, <code>target</code>, <code>.venv</code>, and package caches — usually the biggest win, 10–100+ GB.",
      feat3_title: "📁 Large files",
      feat3_desc: "Files over 500 MB, aggregated by directory so you see who is eating the space — then decide for yourself.",
      feat4_title: "🗑️ App leftovers",
      feat4_desc: "Directories left behind by uninstalled apps and stale installers, cross-checked against what's actually installed.",
      trust_title: "Built on trust, not tricks",
      trust1: "<strong>Nothing is preselected.</strong> Every action starts with you.",
      trust2: "<strong>Preview before anything happens.</strong> You see exactly what will be purged or quarantined, grouped by category.",
      trust3: "<strong>Quarantine, not deletion.</strong> Unsure items move to an NTFS-compressed, restorable store — they are only destroyed when you say so.",
      trust4: "<strong>Audit log.</strong> Every cleanup, restore, and purge is written to a traceable log.",
      trust5: "<strong>Open source.</strong> The source is public under MIT — read it, build it yourself, and verify there is no silent cleanup.",
      download_title: "Download",
      download_desc: "Get the latest release, or build it from source. No installer tricks, no bundled software.",
      download_releases: "GitHub Releases",
      download_source: "View source",
      download_hint: "Microsoft Store version coming soon.",
      footer_license: "MIT License"
    },
    zh: {
      lang_switch: "English",
      page_title: "SpaceRecycle — 一个真正值得信任的磁盘清理工具",
      page_desc: "一款从不擅自删除任何文件的 Windows 磁盘空间回收工具。开源、可审计、安全至上。",
      nav_features: "功能",
      nav_trust: "为何可信",
      nav_download: "下载",
      hero_title: "一个你<em>真正</em>能信任的磁盘清理工具。",
      hero_lede: "SpaceRecycle 告诉你磁盘空间去了哪里，让你选择清理什么，并请你确认——<strong>它从不擅自删除任何东西。</strong>",
      cta_download: "下载 Windows 版",
      cta_features: "看看它能发现什么",
      hero_hint: "Windows 10 1809+ &middot; 约 10 MB &middot; 开源 (MIT) &middot; 无广告、无捆绑、无后台服务",
      features_title: "你的磁盘空间到底去了哪里",
      feat1_title: "🧹 临时垃圾与缓存",
      feat1_desc: "临时文件夹、浏览器缓存、崩溃转储、Windows 更新残留——通常 1–20 GB，可安全回收。",
      feat2_title: "📦 开发产物",
      feat2_desc: "<code>node_modules</code>、<code>target</code>、<code>.venv</code> 以及各类包缓存——通常是最大头，10–100+ GB。",
      feat3_title: "📁 大文件",
      feat3_desc: "超过 500 MB 的文件，按目录聚合，让你看清到底是谁在吃空间——然后由你自己决定。",
      feat4_title: "🗑️ 应用残留",
      feat4_desc: "卸载应用后遗留的目录和陈旧安装包，并与实际已安装程序交叉比对。",
      trust_title: "建立在信任之上，而非套路",
      trust1: "<strong>绝不预选任何项。</strong>每个动作都由你发起。",
      trust2: "<strong>任何操作前先预览。</strong>你能清楚看到将被永久删除或移入隔离区的内容，按类别分组。",
      trust3: "<strong>隔离而非删除。</strong>不确定的项会移入 NTFS 压缩、可恢复的隔离区——只有你明确下令才会被销毁。",
      trust4: "<strong>审计日志。</strong>每次清理、恢复和永久删除都会写入可追溯的日志。",
      trust5: "<strong>开源。</strong>源码以 MIT 协议公开——你可以阅读、自行构建，验证不存在任何静默清理。",
      download_title: "下载",
      download_desc: "获取最新版本，或从源码自行构建。无安装器套路，无捆绑软件。",
      download_releases: "GitHub Releases",
      download_source: "查看源码",
      download_hint: "Microsoft Store 版本即将上线。",
      footer_license: "MIT 许可证"
    }
  };

  var STORAGE_KEY = "spacerrecycle-lang";

  function detectLang() {
    try {
      var saved = localStorage.getItem(STORAGE_KEY);
      if (saved === "en" || saved === "zh") return saved;
    } catch (_) {}
    var nav = (navigator.language || "en").toLowerCase();
    return nav.indexOf("zh") === 0 ? "zh" : "en";
  }

  function applyLang(lang) {
    var dict = I18N[lang] || I18N.en;
    document.documentElement.lang = lang;

    document.title = dict.page_title;
    var meta = document.querySelector('meta[name="description"]');
    if (meta) meta.setAttribute("content", dict.page_desc);

    var els = document.querySelectorAll("[data-i18n]");
    for (var i = 0; i < els.length; i++) {
      var key = els[i].getAttribute("data-i18n");
      if (dict[key] !== undefined) els[i].innerHTML = dict[key];
    }

    var btn = document.querySelector("[data-lang-switch]");
    if (btn) btn.textContent = dict.lang_switch;
  }

  function toggleLang() {
    var cur = document.documentElement.lang === "zh" ? "zh" : "en";
    var next = cur === "zh" ? "en" : "zh";
    try { localStorage.setItem(STORAGE_KEY, next); } catch (_) {}
    applyLang(next);
  }

  // First paint in the detected language, then wire the toggle.
  applyLang(detectLang());

  document.addEventListener("DOMContentLoaded", function () {
    var btn = document.querySelector("[data-lang-switch]");
    if (btn) btn.addEventListener("click", toggleLang);
  });
})();
