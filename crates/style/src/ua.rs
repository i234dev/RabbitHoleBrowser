//! The user-agent stylesheet, modeled on the HTML standard's "Rendering"
//! section. Scripting is disabled in RabbitHole, so `noscript` renders.

pub const UA_CSS: &str = r#"
/* ---- hidden elements ---- */
[hidden], area, base, basefont, datalist, head, link, meta, noembed, noframes,
param, rp, script, style, template, title, dialog:not([open]) { display: none; }
slot { display: contents; }
input[type=hidden i] { display: none; }

/* ---- the page ---- */
html { display: block; }
body { display: block; margin: 8px; }

/* ---- flow content ---- */
address, blockquote, center, div, figure, figcaption, footer, form, header, hr,
legend, listing, main, p, plaintext, pre, search, xmp, article, aside, h1, h2,
h3, h4, h5, h6, hgroup, nav, section, dir, dd, dl, dt, menu, ol, ul, details,
summary, fieldset, optgroup, option, marquee, frameset, frame { display: block; }

li { display: list-item; }
summary { cursor: pointer; }

blockquote, figure, dl, listing, plaintext, pre, xmp, ol, ul, menu, dir,
h4, p { margin: 1em 0; }
blockquote, figure { margin: 1em 40px; }
address { font-style: italic; }
listing, plaintext, pre, xmp { font-family: monospace; white-space: pre; tab-size: 8; }
dd { margin-left: 40px; }
ol, ul, menu, dir { padding-left: 40px; }
ol { list-style-type: decimal; }
ul, menu, dir { list-style-type: disc; }
ul ul, ol ul, menu ul, dir ul { list-style-type: circle; }
ul ul ul, ol ul ul, ul ol ul, ol ol ul { list-style-type: square; }
ol ol, ul ol { list-style-type: decimal; }
ul ul, ol ul, ul ol, ol ol, menu menu { margin-top: 0; margin-bottom: 0; }

h1 { font-size: 2em; font-weight: bold; margin: 0.67em 0; }
h2 { font-size: 1.5em; font-weight: bold; margin: 0.83em 0; }
h3 { font-size: 1.17em; font-weight: bold; margin: 1em 0; }
h4 { font-size: 1em; font-weight: bold; margin: 1.33em 0; }
h5 { font-size: 0.83em; font-weight: bold; margin: 1.67em 0; }
h6 { font-size: 0.67em; font-weight: bold; margin: 2.33em 0; }
article h1, aside h1, nav h1, section h1 { font-size: 1.5em; margin: 0.83em 0; }

hr { color: gray; border-style: solid; border-width: 1px; margin: 0.5em auto; overflow: hidden; }
fieldset { margin: 0 2px; padding: 0.35em 0.75em 0.625em; border: 2px solid #c0c0c0; }
legend { padding: 0 2px; }
center { text-align: center; }

/* ---- phrasing content ---- */
cite, dfn, em, i, var { font-style: italic; }
b, strong { font-weight: bold; }
code, kbd, samp, tt { font-family: monospace; }
big { font-size: 1.17em; }
small { font-size: 0.83em; }
sub { vertical-align: sub; font-size: 0.83em; }
sup { vertical-align: super; font-size: 0.83em; }
sub, sup { line-height: normal; }
ins, u { text-decoration: underline; }
abbr[title] { text-decoration: underline; }
del, s, strike { text-decoration: line-through; }
mark { background-color: yellow; color: black; }
q, ruby, rt, rb, span, font, bdi, bdo, wbr, abbr, time, data, output, label { display: inline; }
nobr { white-space: nowrap; }
br { display: inline; }

a:any-link { color: #0000ee; text-decoration: underline; cursor: pointer; }
a:any-link img { border-color: #0000ee; }
q::before { content: open-quote; }
q::after { content: close-quote; }
abbr[title] { text-decoration: underline dotted; }
blink { text-decoration: none; }
[dir=rtl i] { direction: rtl; }
[dir=ltr i] { direction: ltr; }
bdo[dir=rtl i] { direction: rtl; }

/* ---- tables ---- */
table { display: table; box-sizing: border-box; text-indent: 0; border-spacing: 2px; border-collapse: separate; }
caption { display: table-caption; text-align: center; }
colgroup, colgroup[hidden] { display: table-column-group; }
col, col[hidden] { display: table-column; }
thead, thead[hidden] { display: table-header-group; }
tbody, tbody[hidden] { display: table-row-group; }
tfoot, tfoot[hidden] { display: table-footer-group; }
tr, tr[hidden] { display: table-row; }
td, th { display: table-cell; padding: 1px; vertical-align: middle; }
th { font-weight: bold; text-align: center; }
table[border]:not([border="0"]) td, table[border]:not([border="0"]) th { border: 1px solid #a0a0a0; }
table[border]:not([border="0"]) { border: 1px solid #808080; }

/* ---- form controls ----
   Geometry and colors of Blink's (refreshed) controls. Blink lays text
   fields and buttons out with a 2px border of which it paints 1px; here
   the border is 1px and the padding 1px larger, which gives the same
   boxes (a 13.333px text field is 21px tall). Checkboxes, radios, sliders,
   progress bars and meters are painted natively (layout's widgets.rs). */
input, select, button, textarea {
  display: inline-block; margin: 0; font-family: sans-serif; font-size: 13.333px; font-style: normal;
  font-weight: normal; line-height: normal; letter-spacing: normal; word-spacing: normal;
  text-transform: none; text-indent: 0; text-align: start; text-decoration: none; text-shadow: none;
  color: black; appearance: auto; text-overflow: clip;
}
input, select, textarea { white-space: nowrap; overflow: hidden; }
input { border: 1px solid #767676; padding: 2px 3px; background-color: white; width: 11em; cursor: text; }
input[type=search i] { box-sizing: border-box; width: calc(11em + 8px); }
input[type=hidden i] { display: none; }
input[type=checkbox i], input[type=radio i] {
  box-sizing: border-box; width: 13px; height: 13px; padding: 0; border: none;
  background-color: transparent; margin: 3px 3px 3px 4px; cursor: default; overflow: visible;
}
input[type=radio i] { margin: 3px 3px 0 5px; }
input[type=range i] {
  box-sizing: border-box; width: 129px; height: 16px; margin: 2px; padding: 0; border: none;
  background-color: transparent; color: #101010; cursor: default;
}
input[type=color i] {
  box-sizing: border-box; width: 50px; height: 27px; padding: 1px 2px; border: 1px solid #767676;
  background-color: #efefef; cursor: default;
}
input[type=file i] { width: auto; padding: 0; border: none; background-color: transparent; color: inherit; cursor: default; }
input[type=file i]::file-selector-button {
  border: 1px solid #767676; border-radius: 2px; background-color: #efefef; color: black; padding: 1px 6px;
  margin: 0 4px 0 0; white-space: pre; text-align: center; font: inherit; cursor: default;
}
input[type=image i] { border: none; padding: 0; background-color: transparent; width: auto; height: auto; cursor: pointer; }
input[type=submit i], input[type=button i], input[type=reset i], button {
  box-sizing: border-box; border: 1px solid #767676; background-color: #efefef; padding: 2px 7px;
  width: auto; height: auto; text-align: center; cursor: pointer; user-select: none;
}
input[type=submit i], input[type=button i], input[type=reset i] { white-space: pre; }
button:hover, input[type=submit i]:hover, input[type=button i]:hover, input[type=reset i]:hover, input[type=color i]:hover { background-color: #e5e5e5; border-color: #4f4f4f; }
button:active, input[type=submit i]:active, input[type=button i]:active, input[type=reset i]:active, input[type=color i]:active { background-color: #f5f5f5; border-color: #8d8d8d; }
select {
  box-sizing: border-box; border: 1px solid #767676; background-color: white;
  padding: 1px 0 1px 4px; white-space: pre; cursor: default;
}
select[multiple], select[size]:not([size="1"]):not([size="0"]):not([size=""]) { padding: 0; }
textarea {
  font-family: monospace; border: 1px solid #767676; padding: 2px; background-color: white;
  width: calc(20ch + 15px); white-space: pre-wrap; overflow-wrap: break-word; resize: both; overflow: auto; cursor: text;
}
input::placeholder, textarea::placeholder { color: #757575; }
input:hover, textarea:hover, select:hover { border-color: #4f4f4f; }
input:disabled, textarea:disabled, select:disabled { color: #6d6d6d; background-color: rgba(239, 239, 239, 0.3); border-color: rgba(118, 118, 118, 0.3); cursor: default; }
button:disabled, input[type=submit i]:disabled, input[type=button i]:disabled, input[type=reset i]:disabled {
  color: rgba(16, 16, 16, 0.3); background-color: rgba(239, 239, 239, 0.3); border-color: rgba(118, 118, 118, 0.3);
}
input[type=checkbox i]:disabled, input[type=radio i]:disabled, input[type=range i]:disabled, input[type=file i]:disabled { background-color: transparent; }
:focus-visible { outline: 2px solid #101010; outline-offset: 1px; }
input:focus-visible, textarea:focus-visible, select:focus-visible { outline-offset: 0; }
label { cursor: default; }
optgroup, option { display: block; }
select option { display: none; }
progress, meter { display: inline-block; box-sizing: border-box; height: 1em; vertical-align: -0.2em; appearance: auto; }
progress { width: 10em; }
meter { width: 5em; }

/* ---- embedded content ---- */
img, video, audio, canvas, embed, iframe, svg, math, picture, source { display: inline-block; }
picture, source { display: inline; }
img { vertical-align: baseline; }
iframe { border: 2px solid #c0c0c0; width: 300px; height: 150px; }
video { object-fit: contain; }
audio { display: none; }
audio[controls] { display: inline-block; width: 300px; height: 54px; background-color: #f1f3f4; }

/* ---- misc ---- */
details > summary:first-of-type { display: list-item; list-style-type: none; }
details:not([open]) > :not(summary) { display: none; }
marquee { display: block; overflow: hidden; white-space: nowrap; }
dialog[open] { display: block; position: absolute; left: 0; right: 0; width: fit-content; height: fit-content; margin: auto; border: solid; padding: 1em; background-color: white; color: black; }
details > summary::marker { content: "\25B8 "; }
details[open] > summary::marker { content: "\25BE "; }
::marker { unicode-bidi: isolate; font-variant-numeric: tabular-nums; white-space: pre; }
hr[color], hr[noshade] { border-style: solid; }
textarea[cols] { width: auto; }
textarea[rows] { height: auto; }
ruby { display: inline; }
rt { display: inline; font-size: 50%; }
noscript { display: inline; }
@media (scripting: enabled) { noscript { display: none !important; } }
"#;
