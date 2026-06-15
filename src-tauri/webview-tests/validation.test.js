// Unit tests for the settings webview's pure helpers (dist/validation.js).
// Run with: node --test src-tauri/webview-tests/
// Zero-dependency by design: Node's built-in test runner + assert, no npm.
// The listen-address cases mirror proxy-core's validate_listen_addr tests —
// keep the two suites in sync.

const test = require("node:test");
const assert = require("node:assert/strict");

const {
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
} = require("../dist/validation.js");

test("detectApi recognizes the wire API from the URL suffix", () => {
  assert.equal(detectApi("https://h/v1/chat/completions"), "chat");
  assert.equal(detectApi("https://h/v1/responses"), "responses");
  assert.equal(detectApi("https://h/v1/responses///"), "responses"); // trailing slashes
  assert.equal(detectApi("https://h/v1"), null);
  assert.equal(detectApi(""), null);
  assert.equal(detectApi(null), null);
});

test("rewriteSuffix swaps or appends the API suffix", () => {
  assert.equal(
    rewriteSuffix("https://h/v1/chat/completions", API_SUFFIX.responses),
    "https://h/v1/responses"
  );
  assert.equal(
    rewriteSuffix("https://h/v1/responses", API_SUFFIX.chat),
    "https://h/v1/chat/completions"
  );
  // A base URL (no recognized suffix) gets the suffix appended.
  assert.equal(rewriteSuffix("https://h/v1", API_SUFFIX.chat), "https://h/v1/chat/completions");
  assert.equal(rewriteSuffix("https://h/v1/", API_SUFFIX.responses), "https://h/v1/responses");
});

test("endpointError mirrors the backend endpoint rule", () => {
  assert.equal(endpointError("https://h/v1/chat/completions"), null);
  assert.equal(endpointError("http://h/v1/responses"), null);
  assert.match(endpointError("ftp://h/v1/responses"), /http/);
  assert.match(endpointError("https://h/v1"), /must end in/);
  assert.match(endpointError(""), /http/);
});

test("listenHost extracts the host, including bracketed IPv6", () => {
  assert.equal(listenHost("127.0.0.1:8080"), "127.0.0.1");
  assert.equal(listenHost("localhost:9000"), "localhost");
  assert.equal(listenHost("[::1]:8080"), "::1");
  assert.equal(listenHost("[2001:db8::1]:80"), "2001:db8::1");
});

test("isLoopbackListenAddr matches the backend's loopback gate", () => {
  assert.equal(isLoopbackListenAddr("127.0.0.1:8080"), true);
  assert.equal(isLoopbackListenAddr("localhost:9000"), true);
  assert.equal(isLoopbackListenAddr("[::1]:8080"), true);
  assert.equal(isLoopbackListenAddr("0.0.0.0:8080"), false);
  assert.equal(isLoopbackListenAddr("[2001:db8::1]:80"), false);
});

test("listenAddrError accepts valid host:port forms", () => {
  assert.equal(listenAddrError("127.0.0.1:8080"), null);
  assert.equal(listenAddrError("localhost:1"), null);
  assert.equal(listenAddrError("0.0.0.0:65535"), null);
  // Bracketed IPv6 was falsely rejected by the old regex — the backend accepts
  // it, so the pre-flight check must too.
  assert.equal(listenAddrError("[::1]:8080"), null);
  assert.equal(listenAddrError("[2001:db8::1]:80"), null);
  assert.equal(listenAddrError("  127.0.0.1:8080  "), null); // trimmed
});

test("listenAddrError rejects malformed addresses", () => {
  const formErr = /host:port form/;
  assert.match(listenAddrError("localhost"), formErr); // no port
  assert.match(listenAddrError(":8080"), formErr); // empty host
  assert.match(listenAddrError("[::1]"), formErr); // bracket without port
  assert.match(listenAddrError("[::1]8080"), formErr); // missing separator
  assert.match(listenAddrError(""), formErr);
});

test("listenAddrError rejects out-of-range ports (mirrors u16 + nonzero)", () => {
  const portErr = /between 1 and 65535/;
  assert.match(listenAddrError("127.0.0.1:0"), portErr);
  assert.match(listenAddrError("127.0.0.1:65536"), portErr);
  assert.match(listenAddrError("127.0.0.1:70000"), portErr);
  assert.match(listenAddrError("127.0.0.1:99999"), portErr);
  assert.match(listenAddrError("127.0.0.1:123456"), portErr); // >5 digits
  assert.match(listenAddrError("127.0.0.1:abc"), portErr);
  assert.match(listenAddrError("127.0.0.1:"), portErr); // empty port
});

test("kindTagClass maps known kinds to their modifier class", () => {
  for (const kind of MODEL_KINDS) {
    assert.equal(kindTagClass(kind), `cp-kindtag cp-kindtag--${kind}`);
  }
});

test("kindTagClass falls back to the bare tag for unknown kinds", () => {
  assert.equal(kindTagClass("video"), "cp-kindtag");
  assert.equal(kindTagClass(null), "cp-kindtag");
  assert.equal(kindTagClass(undefined), "cp-kindtag");
  assert.equal(kindTagClass(""), "cp-kindtag");
});

test("tokenLimitError accepts blank (no override) and positive integers", () => {
  // Blank / nullish → no override, which is valid.
  assert.equal(tokenLimitError(""), null);
  assert.equal(tokenLimitError("   "), null);
  assert.equal(tokenLimitError(null), null);
  assert.equal(tokenLimitError(undefined), null);
  // Valid positive integers (string or number).
  assert.equal(tokenLimitError("128000"), null);
  assert.equal(tokenLimitError(16384), null);
});

test("tokenLimitError rejects non-integers and out-of-range values", () => {
  assert.match(tokenLimitError("12.5"), /whole numbers/);
  assert.match(tokenLimitError("-5"), /whole numbers/); // the minus is non-digit
  assert.match(tokenLimitError("abc"), /whole numbers/);
  assert.match(tokenLimitError("0"), /between 1 and 4000000/);
  assert.match(tokenLimitError("5000000"), /between 1 and 4000000/);
});

test("MODEL_KINDS mirrors proxy-core's ModelKind", () => {
  // If the Rust enum ModelKind gains or loses a variant, update MODEL_KINDS
  // here, in validation.js, and add/remove the matching cp-kindtag--* rule
  // in styles.css.
  assert.deepStrictEqual(
    [...MODEL_KINDS].sort(),
    ["audio", "embed", "image", "moderation", "rerank"]
  );
});
