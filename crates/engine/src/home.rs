//! The built-in `about:home` page.

pub const HOME_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>RabbitHole</title>
<style>
  body { margin: 0; background: #faf7f2; color: #2b2b2b; font-family: sans-serif; }
  .hero { background: #1f1b2e; color: #f4efe6; padding: 48px 32px 40px 32px; border-bottom: 6px solid #e0a84a; }
  .hero h1 { font-size: 44px; margin: 0 0 8px 0; letter-spacing: 1px; }
  .hero p { margin: 0; font-size: 18px; color: #d7cfc1; }
  .hero .tag { display: inline-block; background: #e0a84a; color: #1f1b2e; font-weight: bold; padding: 3px 10px; border-radius: 12px; font-size: 13px; margin-top: 16px; }
  main { max-width: 820px; margin: 0 auto; padding: 24px 32px; }
  h2 { font-size: 22px; border-bottom: 2px solid #e0a84a; padding-bottom: 4px; margin-top: 32px; }
  .cards { display: flex; }
  .card { background: #ffffff; border: 1px solid #e2dbd0; border-radius: 10px; padding: 14px 16px; margin: 8px; width: 33%; }
  .card h3 { margin: 0 0 6px 0; font-size: 16px; }
  .card p { margin: 0; font-size: 14px; color: #5a5650; }
  a { color: #8a4b08; }
  ul.links li { margin: 6px 0; }
  code { background: #efe9df; padding: 1px 5px; border-radius: 4px; font-family: monospace; }
  .footer { margin-top: 40px; font-size: 13px; color: #8a847a; text-align: center; }
  table { border-collapse: collapse; margin: 12px 0; }
  td, th { border: 1px solid #d8d0c4; padding: 6px 12px; text-align: left; }
  th { background: #efe9df; }
</style>
</head>
<body>
<div class="hero">
  <h1>RabbitHole</h1>
  <p>A web browser written from scratch. No Chromium, no WebKit, no Gecko, no webview.</p>
  <span class="tag">v0.1 &middot; own HTML parser &middot; own CSS engine &middot; own layout &middot; own rasterizer</span>
</div>
<main>
  <h2>Try a few pages</h2>
  <ul class="links">
    <li><a href="https://example.com/">example.com</a> &ndash; the classic</li>
    <li><a href="https://en.wikipedia.org/wiki/Web_browser">Wikipedia: Web browser</a></li>
    <li><a href="https://news.ycombinator.com/">Hacker News</a></li>
    <li><a href="https://lite.duckduckgo.com/lite/">DuckDuckGo Lite</a> &ndash; search without JavaScript</li>
    <li><a href="https://www.gnu.org/">gnu.org</a></li>
    <li><a href="https://github.com/i234dev/RabbitHoleBrowser">RabbitHole on GitHub</a></li>
  </ul>

  <h2>What is rendering this page</h2>
  <div class="cards">
    <div class="card"><h3>HTML</h3><p>A WHATWG-style tokenizer and a forgiving tree builder with implied tags and scope handling.</p></div>
    <div class="card"><h3>CSS</h3><p>Tokenizer, selectors with specificity, media queries, cascade and inheritance.</p></div>
    <div class="card"><h3>Layout &amp; paint</h3><p>Block, inline, float, flex and table layout, painted by a software rasterizer with anti-aliased text.</p></div>
  </div>

  <h2>Keyboard</h2>
  <table>
    <tr><th>Keys</th><th>Action</th></tr>
    <tr><td><code>Ctrl+L</code></td><td>Focus the address bar</td></tr>
    <tr><td><code>Enter</code></td><td>Go (plain words search DuckDuckGo)</td></tr>
    <tr><td><code>Alt+&larr;</code> / <code>Alt+&rarr;</code></td><td>Back / forward</td></tr>
    <tr><td><code>F5</code></td><td>Reload</td></tr>
    <tr><td><code>Space</code>, <code>PageDown</code>, arrows</td><td>Scroll</td></tr>
  </table>

  <p>There is also a command line: <code>rabbithole text &lt;url&gt;</code>, <code>rabbithole screenshot &lt;url&gt;</code>, and an MCP server with <code>rabbithole mcp</code>.</p>
  <p class="footer">RabbitHole is free software (MIT). Everything you see was fetched over pure-Rust TLS and painted pixel by pixel.</p>
</main>
</body>
</html>
"#;
