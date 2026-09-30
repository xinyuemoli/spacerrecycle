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
      hero_hint: "Windows 10 1809+ &middot; ~6 MB &middot; open source (MIT) &middot; no ads, no bundleware, no background services",
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
      footer_license: "MIT License",
      footer_privacy: "Privacy",
      privacy_title: "Privacy Policy — SpaceRecycle",
      privacy_desc: "SpaceRecycle collects no data. No telemetry, no accounts, no tracking — in the app or on this website.",
      privacy_h1: "Privacy Policy",
      privacy_updated: "Last updated: September 30, 2026",
      privacy_intro: "SpaceRecycle is built around a simple promise: <strong>it never does anything on your device without you asking.</strong> This page explains exactly what that means for your data.",
      privacy_app_h2: "The desktop app collects nothing",
      privacy_app_1: "<strong>No telemetry.</strong> The app sends no usage data, analytics, or crash reports anywhere.",
      privacy_app_2: "<strong>No account.</strong> There is no sign-up, no login, and no cloud service.",
      privacy_app_3: "<strong>Everything stays on your machine.</strong> Scanning, cleanup, the quarantine store, and the audit log all happen locally. Nothing is uploaded.",
      privacy_app_4: "<strong>No network requests for your data.</strong> Your file names, paths, and usage are never transmitted to us or anyone else.",
      privacy_app_local: "The only files the app creates live on your own disk: the quarantine store (default <code>%ProgramData%\\SpaceRecycle\\Quarantine</code>), the audit log, and its config file. You can inspect, export, or delete all of them at any time.",
      privacy_site_h2: "This website",
      privacy_site_p: "The website uses <strong>Cloudflare Web Analytics</strong>, a cookie-less, privacy-first analytics service. It reports aggregate page views and visits only — no cookies, no fingerprinting, no cross-site tracking, and no personal data stored. The site is fully static: no accounts, no forms, no comments.",
      privacy_dl_h2: "Downloads",
      privacy_dl_p: "Installers are served by GitHub Releases. GitHub may record the download under <a href=\"https://docs.github.com/en/site-policy/privacy-policies/github-privacy-statement\">GitHub's Privacy Statement</a>. We only ever see an aggregate download count.",
      privacy_children_h2: "Children's privacy",
      privacy_children_p: "Neither the app nor the website collects personal data from anyone, including children.",
      privacy_changes_h2: "Changes",
      privacy_changes_p: "If this policy changes, the new version will be published on this page.",
      privacy_contact_h2: "Contact",
      privacy_contact_p: "Questions? Open an issue on <a href=\"https://github.com/xinyuemoli/spacerrecycle/issues\">GitHub</a>.",
      privacy_back: "← Back to home"
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
      hero_hint: "Windows 10 1809+ &middot; 约 6 MB &middot; 开源 (MIT) &middot; 无广告、无捆绑、无后台服务",
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
      footer_license: "MIT 许可证",
      footer_privacy: "隐私政策",
      privacy_title: "隐私政策 — SpaceRecycle",
      privacy_desc: "SpaceRecycle 不收集任何数据。无论应用还是官网，都没有遥测、没有账号、没有追踪。",
      privacy_h1: "隐私政策",
      privacy_updated: "最后更新：2026 年 9 月 30 日",
      privacy_intro: "SpaceRecycle 的核心承诺很简单：<strong>未经你的操作，绝不在你的设备上做任何事。</strong>本页说明这对你的数据意味着什么。",
      privacy_app_h2: "桌面应用不收集任何数据",
      privacy_app_1: "<strong>无遥测。</strong>应用不会向任何地方发送使用数据、统计信息或崩溃报告。",
      privacy_app_2: "<strong>无账号。</strong>没有注册、没有登录、没有云服务。",
      privacy_app_3: "<strong>一切都在你本机完成。</strong>扫描、清理、隔离区、审计日志全部在本机进行，不上传任何内容。",
      privacy_app_4: "<strong>不传输你的数据。</strong>你的文件名、路径和使用情况绝不会被发送给我们或任何第三方。",
      privacy_app_local: "应用只在你自己磁盘上创建文件：隔离区（默认 <code>%ProgramData%\\SpaceRecycle\\Quarantine</code>）、审计日志和配置文件。你随时可以查看、导出或删除它们。",
      privacy_site_h2: "关于官网",
      privacy_site_p: "官网使用 <strong>Cloudflare Web Analytics</strong>——一个无 cookie、隐私优先的统计服务。它只提供聚合的页面浏览量与访客数：不使用 cookie、不做指纹识别、不跨站追踪、不存储个人数据。官网是纯静态的：没有账号、表单或评论。",
      privacy_dl_h2: "下载",
      privacy_dl_p: "安装包由 GitHub Releases 提供。GitHub 可能依据 <a href=\"https://docs.github.com/en/site-policy/privacy-policies/github-privacy-statement\">GitHub 隐私声明</a> 记录下载。我们只能看到聚合的下载次数。",
      privacy_children_h2: "儿童隐私",
      privacy_children_p: "应用与官网均不收集任何人的个人数据，包括儿童。",
      privacy_changes_h2: "政策变更",
      privacy_changes_p: "若本政策发生变更，新版本将发布在本页面。",
      privacy_contact_h2: "联系方式",
      privacy_contact_p: "有问题？请在 <a href=\"https://github.com/xinyuemoli/spacerrecycle/issues\">GitHub</a> 提 issue。",
      privacy_back: "← 返回首页"
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

    // Title/description can be overridden per page through data-i18n-title /
    // data-i18n-desc on the <html> element; they default to the home page keys.
    var root = document.documentElement;
    var titleKey = root.getAttribute("data-i18n-title") || "page_title";
    var descKey = root.getAttribute("data-i18n-desc") || "page_desc";
    document.title = dict[titleKey] || dict.page_title;
    var meta = document.querySelector('meta[name="description"]');
    if (meta && dict[descKey]) meta.setAttribute("content", dict[descKey]);

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
