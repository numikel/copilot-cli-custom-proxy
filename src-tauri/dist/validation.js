// Copilot Proxy — pure URL/address helpers shared by the settings webview and
// the Node unit tests (`node --test src-tauri/webview-tests/`). Loaded as a
// classic script *before* app.js (top-level declarations become globals), so
// keep it dependency-free and side-effect-free.

// API mode suffixes — the endpoint URL must end in one of these.
const API_SUFFIX = { chat: "/chat/completions", responses: "/responses" };

// The wire API implied by a URL's suffix ("chat" | "responses" | null).
function detectApi(url) {
  const u = String(url || "").trim().replace(/\/+$/, "");
  if (u.endsWith(API_SUFFIX.chat)) return "chat";
  if (u.endsWith(API_SUFFIX.responses)) return "responses";
  return null;
}

// Rewrites a URL's API suffix (swapping chat ⟷ responses, or appending when the
// URL currently stops at the base, e.g. ".../v1").
function rewriteSuffix(url, suffix) {
  let u = String(url || "").trim().replace(/\/+$/, "");
  if (u.endsWith(API_SUFFIX.chat)) u = u.slice(0, -API_SUFFIX.chat.length);
  else if (u.endsWith(API_SUFFIX.responses)) u = u.slice(0, -API_SUFFIX.responses.length);
  u = u.replace(/\/+$/, "");
  return u + suffix;
}

// Client-side endpoint validation mirroring the backend rule. Returns an error
// string, or null when valid.
function endpointError(url) {
  const u = String(url || "").trim();
  if (!/^https?:\/\//i.test(u)) return "URL must start with http:// or https://";
  if (!detectApi(u)) return "URL must end in /chat/completions or /responses (not just /v1).";
  return null;
}

// Host portion of a host:port listen address (handles bracketed IPv6).
function listenHost(addr) {
  const a = String(addr || "").trim();
  if (a.startsWith("[")) {
    const i = a.indexOf("]");
    return i > 0 ? a.slice(1, i) : "";
  }
  const i = a.lastIndexOf(":");
  return i >= 0 ? a.slice(0, i) : a;
}

// Whether a listen address binds to loopback only (mirrors the backend rule, so
// the UI can warn before the backend rejects a non-loopback bind).
function isLoopbackListenAddr(addr) {
  const h = listenHost(addr).toLowerCase();
  return h === "localhost" || h === "::1" || /^127\./.test(h);
}

// Client-side listen-address validation mirroring proxy-core's
// `validate_listen_addr` — keep the two in sync. This is pre-flight UX only
// (friendlier messages, no IPC round-trip); the backend re-validates and stays
// the source of truth (including the host-character whitelist). Returns an
// error string, or null when valid.
function listenAddrError(addr) {
  const a = String(addr || "").trim();
  let port = null;
  if (a.startsWith("[")) {
    // Bracketed IPv6: the colons inside the address must not be mistaken for
    // the port separator.
    const end = a.indexOf("]");
    if (end > 1 && a[end + 1] === ":") port = a.slice(end + 2);
  } else {
    const i = a.lastIndexOf(":");
    if (i > 0) port = a.slice(i + 1); // i > 0 also guarantees a non-empty host
  }
  if (port === null) {
    return "Listen address must be in host:port form (e.g. 127.0.0.1:8080).";
  }
  if (!/^\d{1,5}$/.test(port) || Number(port) < 1 || Number(port) > 65535) {
    return "Listen address port must be a number between 1 and 65535.";
  }
  return null;
}

// Validates an optional positive-integer token limit (Copilot max prompt /
// output). Empty = "no override" (valid → falls back to the model's advertised
// limit). Pre-flight UX only; the backend re-parses to u32 and stays the source
// of truth. Returns an error string, or null when valid.
function tokenLimitError(value) {
  const v = String(value == null ? "" : value).trim();
  if (v === "") return null;
  if (!/^\d+$/.test(v)) return "Token limits must be whole numbers.";
  const n = Number(v);
  if (n < 1 || n > 4000000) return "Token limits must be between 1 and 4000000.";
  return null;
}

// Known non-chat model kinds — keep in sync with `ModelKind` in
// proxy-core/src/models.rs and the `cp-kindtag--*` classes in styles.css.
const MODEL_KINDS = ["embed", "image", "audio", "rerank", "moderation"];

// CSS classes for a model-kind tag; an unknown kind degrades to the bare
// base tag instead of minting a nonexistent cp-kindtag--… modifier.
function kindTagClass(kind) {
  return MODEL_KINDS.includes(kind) ? `cp-kindtag cp-kindtag--${kind}` : "cp-kindtag";
}

// Node test hook — a no-op in the webview (classic scripts have no `module`).
if (typeof module !== "undefined") {
  module.exports = {
    API_SUFFIX,
    detectApi,
    rewriteSuffix,
    endpointError,
    listenHost,
    isLoopbackListenAddr,
    listenAddrError,
    tokenLimitError,
    MODEL_KINDS,
    kindTagClass,
  };
}
