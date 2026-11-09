//! The user-agent stylesheet, modeled on the HTML standard's "Rendering"
//! section. Scripting is disabled in RabbitHole, so `noscript` renders.

pub const UA_CSS: &str = r#"
/* ---- hidden elements ---- */
[hidden], area, base, basefont, datalist, head, link, meta, noembed, noframes,
param, rp, script, style, template, title, dialog:not([open]) { display: none; }
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
listing, plaintext, pre, xmp { font-family: monospace; white-space: pre; }
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

/* ---- tables ---- */
table { display: table; box-sizing: border-box; text-indent: 0; }
caption { display: table-caption; text-align: center; }
colgroup, colgroup[hidden] { display: table-column-group; }
col, col[hidden] { display: table-column; }
thead, thead[hidden] { display: table-header-group; }
tbody, tbody[hidden] { display: table-row-group; }
tfoot, tfoot[hidden] { display: table-footer-group; }
tr, tr[hidden] { display: table-row; }
td, th { display: table-cell; padding: 1px; vertical-align: middle; }
th { font-weight: bold; text-align: center; }
table[border] td, table[border] th { border: 1px solid #a0a0a0; }
table[border] { border: 1px solid #808080; }

/* ---- form controls ---- */
input, select, button, textarea, meter, progress {
  display: inline-block; font-family: sans-serif; font-size: 13.333px; font-style: normal;
  font-weight: normal; line-height: normal; letter-spacing: normal; word-spacing: normal;
  text-transform: none; text-indent: 0; text-align: left; text-decoration: none; color: initial;
  white-space: nowrap; box-sizing: border-box; vertical-align: middle;
}
input, textarea, select { border: 1px solid #767676; padding: 1px 2px; background-color: white; }
input { width: 20ch; height: 1.6em; }
input[type=checkbox i], input[type=radio i] { width: 13px; height: 13px; padding: 0; margin: 3px 3px 3px 4px; }
input[type=submit i], input[type=button i], input[type=reset i], button {
  border: 1px solid #767676; background-color: #efefef; padding: 1px 6px; width: auto; height: auto;
  text-align: center; cursor: pointer; border-radius: 2px;
}
input[type=range i], input[type=color i] { width: 129px; height: 1.6em; }
select { padding: 1px 4px; width: auto; min-width: 8ch; border-radius: 2px; }
textarea { width: 20ch; height: 2.6em; white-space: pre-wrap; }
input::placeholder, textarea::placeholder { color: #757575; }
label { cursor: default; }
optgroup, option { display: block; }
select option { display: none; }

/* ---- embedded content ---- */
img, video, audio, canvas, embed, object, iframe, svg, math, picture, source { display: inline-block; }
picture, source { display: inline; }
img { vertical-align: baseline; }
iframe { border: 2px solid #c0c0c0; width: 300px; height: 150px; }
video { width: 300px; height: 150px; }
audio { display: none; }
audio[controls] { display: inline-block; width: 300px; height: 54px; background-color: #f1f3f4; }

/* ---- misc ---- */
details > summary:first-of-type { display: list-item; list-style-type: none; }
details:not([open]) > :not(summary) { display: none; }
progress, meter { width: 10em; height: 1em; border: 1px solid #767676; background-color: #e6e6e6; }
marquee { display: block; overflow: hidden; }
ruby { display: inline; }
rt { display: inline; font-size: 50%; }
noscript { display: inline; }
"#;
