//! HTML parsing per the WHATWG HTML Standard (§13.2): the tokenizer
//! state machine and the tree construction stage with all insertion
//! modes, the adoption agency algorithm, foster parenting, templates,
//! foreign (SVG/MathML) content and the fragment parsing algorithm.

pub mod entities;
mod entity_table;
pub mod tokenizer;
pub mod xml;

use rabbithole_dom::{DoctypeData, Document, Namespace, NodeData, NodeId, QuirksMode};
use tokenizer::{Doctype, State, Token, Tokenizer};

/// Split the text of a DOCTYPE token (after `<!DOCTYPE`) into name, public
/// and system identifiers. The flag is the tokenizer's "force-quirks" flag
/// (missing name, malformed identifiers).
pub fn parse_doctype(text: &str) -> (DoctypeData, bool) {
    let text = text.trim();
    let name_end = text.find(|c: char| c.is_ascii_whitespace()).unwrap_or(text.len());
    let name = text[..name_end].to_ascii_lowercase();
    let mut rest = text[name_end..].trim_start();
    let mut data = DoctypeData { name, ..Default::default() };
    let mut force_quirks = data.name.is_empty();
    // A quoted identifier at the start of `rest`.
    fn quoted(rest: &mut &str) -> Option<String> {
        let q = rest.chars().next().filter(|&c| c == '"' || c == '\'')?;
        let body = &rest[1..];
        let (value, after) = match body.find(q) {
            Some(e) => (&body[..e], &body[e + 1..]),
            None => (body, ""),
        };
        *rest = after.trim_start();
        Some(value.to_string())
    }
    let keyword = |rest: &str, kw: &str| rest.len() >= kw.len() && rest.is_char_boundary(kw.len()) && rest[..kw.len()].eq_ignore_ascii_case(kw);
    if keyword(rest, "public") {
        rest = rest[6..].trim_start();
        match quoted(&mut rest) {
            Some(p) => {
                data.public_id = p;
                if let Some(s) = quoted(&mut rest) {
                    data.system_id = s;
                }
            }
            None => force_quirks = true,
        }
    } else if keyword(rest, "system") {
        rest = rest[6..].trim_start();
        match quoted(&mut rest) {
            Some(s) => data.system_id = s,
            None => force_quirks = true,
        }
    } else if !rest.is_empty() {
        force_quirks = true;
    }
    (data, force_quirks)
}

/// The document's quirks mode for a DOCTYPE (HTML §13.2.6.4.1).
pub fn quirks_mode_for(d: &DoctypeData, force_quirks: bool) -> QuirksMode {
    const QUIRKY_PREFIXES: &[&str] = &[
        "+//silmaril//dtd html pro v0r11 19970101//", "-//as//dtd html 3.0 aswedit + extensions//",
        "-//advasoft ltd//dtd html 3.0 aswedit + extensions//", "-//ietf//dtd html 2.0 level 1//",
        "-//ietf//dtd html 2.0 level 2//", "-//ietf//dtd html 2.0 strict level 1//", "-//ietf//dtd html 2.0 strict level 2//",
        "-//ietf//dtd html 2.0 strict//", "-//ietf//dtd html 2.0//", "-//ietf//dtd html 2.1e//", "-//ietf//dtd html 3.0//",
        "-//ietf//dtd html 3.2 final//", "-//ietf//dtd html 3.2//", "-//ietf//dtd html 3//", "-//ietf//dtd html level 0//",
        "-//ietf//dtd html level 1//", "-//ietf//dtd html level 2//", "-//ietf//dtd html level 3//",
        "-//ietf//dtd html strict level 0//", "-//ietf//dtd html strict level 1//", "-//ietf//dtd html strict level 2//",
        "-//ietf//dtd html strict level 3//", "-//ietf//dtd html strict//", "-//ietf//dtd html//",
        "-//metrius//dtd metrius presentational//", "-//microsoft//dtd internet explorer 2.0 html strict//",
        "-//microsoft//dtd internet explorer 2.0 html//", "-//microsoft//dtd internet explorer 2.0 tables//",
        "-//microsoft//dtd internet explorer 3.0 html strict//", "-//microsoft//dtd internet explorer 3.0 html//",
        "-//microsoft//dtd internet explorer 3.0 tables//", "-//netscape comm. corp.//dtd html//",
        "-//netscape comm. corp.//dtd strict html//", "-//o'reilly and associates//dtd html 2.0//",
        "-//o'reilly and associates//dtd html extended 1.0//", "-//o'reilly and associates//dtd html extended relaxed 1.0//",
        "-//sq//dtd html 2.0 hotmetal + extensions//",
        "-//softquad software//dtd hotmetal pro 6.0::19990601::extensions to html 4.0//",
        "-//softquad//dtd hotmetal pro 4.0::19970916::extensions to html 4.0//", "-//spyglass//dtd html 2.0 extended//",
        "-//sun microsystems corp.//dtd hotjava html//", "-//sun microsystems corp.//dtd hotjava strict html//",
        "-//w3c//dtd html 3 1995-03-24//", "-//w3c//dtd html 3.2 draft//", "-//w3c//dtd html 3.2 final//",
        "-//w3c//dtd html 3.2//", "-//w3c//dtd html 3.2s draft//", "-//w3c//dtd html 4.0 frameset//",
        "-//w3c//dtd html 4.0 transitional//", "-//w3c//dtd html experimental 19960712//",
        "-//w3c//dtd html experimental 970421//", "-//w3c//dtd w3 html//", "-//w3o//dtd w3 html 3.0//",
        "-//webtechs//dtd mozilla html 2.0//", "-//webtechs//dtd mozilla html//",
    ];
    let public = d.public_id.to_ascii_lowercase();
    let system = d.system_id.to_ascii_lowercase();
    let html401 = public.starts_with("-//w3c//dtd html 4.01 frameset//") || public.starts_with("-//w3c//dtd html 4.01 transitional//");
    if force_quirks
        || d.name != "html"
        || matches!(public.as_str(), "-//w3o//dtd w3 html strict 3.0//en//" | "-/w3c/dtd html 4.0 transitional/en" | "html")
        || system == "http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd"
        || QUIRKY_PREFIXES.iter().any(|p| public.starts_with(p))
        || (d.system_id.is_empty() && html401)
    {
        return QuirksMode::Quirks;
    }
    if public.starts_with("-//w3c//dtd xhtml 1.0 frameset//") || public.starts_with("-//w3c//dtd xhtml 1.0 transitional//") || html401 {
        return QuirksMode::LimitedQuirks;
    }
    QuirksMode::NoQuirks
}

/// Parse a document with the scripting flag off (DOMParser,
/// `createHTMLDocument`, pages loaded without JavaScript): `<noscript>`
/// content is parsed as markup.
pub fn parse(input: &str) -> Document {
    parse_with_scripting(input, false)
}

/// Parse a document. With `scripting` on (a browsing context that runs
/// scripts), `<noscript>` content is raw text, as the HTML parsing
/// algorithm requires: its markup creates no elements, so its images,
/// iframes, styles and overlays never exist in the DOM.
pub fn parse_with_scripting(input: &str, scripting: bool) -> Document {
    let mut b = TreeBuilder::new(scripting);
    b.allow_dsd = true;
    b.run(input);
    b.doc
}

/// Parse an HTML fragment as the children of a `context` element (as
/// `innerHTML` does). The returned document's `body` holds the fragment
/// nodes, in order.
pub fn parse_fragment(input: &str, context: &str) -> Document {
    parse_fragment_with_scripting(input, context, false)
}

/// [`parse_fragment`] with the scripting flag of the context element's
/// document (`innerHTML` on a scripted page: `<noscript>` is raw text).
pub fn parse_fragment_with_scripting(input: &str, context: &str, scripting: bool) -> Document {
    let (mut doc, container) = parse_fragment_in(input, context, Namespace::Html, scripting, QuirksMode::NoQuirks);
    // Old contract: the nodes are the children of a `body` under `html`.
    let body = doc.create_element("body", vec![]);
    let kids: Vec<NodeId> = doc.children(container).collect();
    for k in kids {
        doc.detach(k);
        doc.append_child(body, k);
    }
    doc.append_child(container, body);
    doc
}

/// The HTML fragment parsing algorithm (§13.4) for a context element
/// with local name `context` in namespace `ns`. Returns the document and
/// the node whose children are the parsed nodes, in order.
pub fn parse_fragment_in(input: &str, context: &str, ns: Namespace, scripting: bool, quirks: QuirksMode) -> (Document, NodeId) {
    parse_fragment_with_options(input, context, ns, scripting, quirks, false)
}

/// [`parse_fragment_in`]; `declarative_shadow` lets `<template
/// shadowrootmode>` attach shadow roots (`setHTMLUnsafe`,
/// `parseHTMLUnsafe`), as the document's "allow declarative shadow roots".
pub fn parse_fragment_with_options(input: &str, context: &str, ns: Namespace, scripting: bool, quirks: QuirksMode, declarative_shadow: bool) -> (Document, NodeId) {
    let mut b = TreeBuilder::new(scripting);
    b.allow_dsd = declarative_shadow;
    b.doc.quirks_mode = quirks;
    let ctx = b.doc.create_element_ns(ns.clone(), None, context, vec![]);
    let html = b.doc.create_element("html", vec![]);
    b.doc.append_child(Document::ROOT, html);
    b.open.push(html);
    b.context = Some(ctx);
    b.fragment = true;
    let mut tk = Tokenizer::new(input);
    if ns == Namespace::Html {
        match context {
            "title" | "textarea" => tk.set_state(State::Rcdata),
            "style" | "xmp" | "iframe" | "noembed" | "noframes" => tk.set_state(State::Rawtext),
            "script" => tk.set_state(State::ScriptData),
            "noscript" if scripting => tk.set_state(State::Rawtext),
            "plaintext" => tk.set_state(State::Plaintext),
            _ => {}
        }
        if context == "template" {
            b.template_modes.push(Mode::InTemplate);
        }
    }
    b.reset_insertion_mode();
    b.run_tokens(&mut tk);
    (b.doc, html)
}

/// What an incremental [`Parser`] stopped for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseStatus {
    /// All available input (up to the insertion point) was consumed;
    /// more may come.
    NeedInput,
    /// A `<script>` end tag was processed: run the script element, then
    /// call [`Parser::run`] again.
    Script(NodeId),
    /// The end of the stream was reached: parsing is complete.
    Finished,
}

/// An incremental, re-entrant HTML document parser working on a live
/// [`Document`] (the "script-created parser" of `document.open()`, or a
/// network parser fed in chunks). Input is added with [`Parser::append`]
/// (network) or [`Parser::write`] (`document.write`, at the insertion
/// point); [`Parser::run`] tokenizes and builds the tree until the input
/// runs out, a script must run, or the stream ends.
pub struct Parser {
    tb: TreeBuilder,
    tk: Tokenizer,
    finished: bool,
}

impl Parser {
    /// A parser for a whole document; the document passed to
    /// [`Parser::run`] must start empty.
    pub fn new(scripting: bool) -> Parser {
        let mut tb = TreeBuilder::new(scripting);
        tb.allow_dsd = true;
        tb.pause_on_scripts = scripting;
        Parser { tb, tk: Tokenizer::streaming(), finished: false }
    }

    /// A streaming fragment parser (`streamHTML()` and friends): the HTML
    /// fragment parsing algorithm with `context` (an element of the
    /// document passed to [`Parser::run`], local name `context_tag` in
    /// namespace `context_ns`) whose nodes go straight into the live node
    /// `root`, before `before` (None: appended). `form` is the context's
    /// nearest form ancestor.
    #[allow(clippy::too_many_arguments)]
    pub fn new_fragment(context: NodeId, context_tag: &str, context_ns: Namespace, root: NodeId, before: Option<NodeId>, form: Option<NodeId>, scripting: bool, declarative_shadow: bool) -> Parser {
        let mut tb = TreeBuilder::new(scripting);
        tb.allow_dsd = declarative_shadow;
        tb.open.push(root);
        tb.context = Some(context);
        tb.fragment = true;
        tb.form = form;
        tb.root_before = before;
        let mut tk = Tokenizer::streaming();
        if context_ns == Namespace::Html {
            match context_tag {
                "title" | "textarea" => tk.set_state(State::Rcdata),
                "style" | "xmp" | "iframe" | "noembed" | "noframes" => tk.set_state(State::Rawtext),
                "script" => tk.set_state(State::ScriptData),
                "noscript" if scripting => tk.set_state(State::Rawtext),
                "plaintext" => tk.set_state(State::Plaintext),
                _ => {}
            }
            tk.set_last_start_tag(context_tag);
            if context_tag == "template" {
                tb.template_modes.push(Mode::InTemplate);
            }
        }
        tb.mode_reset_pending = true;
        Parser { tb, tk, finished: false }
    }

    /// Add input at the end of the stream.
    pub fn append(&mut self, text: &str) {
        self.tk.append(text);
    }

    /// `document.write`: insert input at the insertion point.
    pub fn write(&mut self, text: &str) {
        self.tk.insert(text);
    }

    /// Insert an explicit EOF at the end of the stream.
    pub fn close(&mut self) {
        self.tk.close();
    }

    pub fn is_closed(&self) -> bool {
        self.tk.is_closed()
    }

    /// Pause at `</script>` end tags ([`ParseStatus::Script`]).
    pub fn set_pause_on_scripts(&mut self, on: bool) {
        self.tb.pause_on_scripts = on;
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The insertion point, if defined.
    pub fn insertion_point(&self) -> Option<usize> {
        self.tk.insertion_point()
    }

    pub fn set_insertion_point_here(&mut self) {
        self.tk.set_insertion_point_here();
    }

    pub fn set_insertion_point_at_end(&mut self) {
        self.tk.set_insertion_point_at_end();
    }

    pub fn restore_insertion_point(&mut self, ip: Option<usize>) {
        self.tk.restore_insertion_point(ip);
    }

    /// Around a parser-inserted script: the insertion point is just after
    /// it while it runs, then back where it was (moved along by what the
    /// script wrote).
    pub fn push_insertion_point_here(&mut self) {
        self.tk.push_insertion_point_here();
    }

    pub fn pop_insertion_point(&mut self) {
        self.tk.pop_insertion_point();
    }

    /// Run the tokenizer and tree builder on `doc` until the available
    /// input is consumed, a script must run, or the stream ends.
    pub fn run(&mut self, doc: &mut Document) -> ParseStatus {
        if self.finished {
            return ParseStatus::Finished;
        }
        std::mem::swap(doc, &mut self.tb.doc);
        if std::mem::take(&mut self.tb.mode_reset_pending) {
            self.tb.reset_insertion_mode();
        }
        let status = self.tb.run_tokens(&mut self.tk);
        std::mem::swap(doc, &mut self.tb.doc);
        if status == ParseStatus::Finished {
            self.finished = true;
        }
        status
    }

    /// Stop parsing (`document.open` / abort): the parser is finished
    /// without running the end steps.
    pub fn abort(&mut self) {
        self.finished = true;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InTemplate,
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

/// A start tag token (for the list of active formatting elements and
/// re-created elements).
#[derive(Clone, Debug)]
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    self_closing: bool,
}

enum Afe {
    Marker,
    El(NodeId, Tag),
}

enum Tok {
    Doctype(Doctype),
    Start(Tag),
    End(String),
    Comment(String),
    /// A homogeneous run: all whitespace, all U+0000, or all other text.
    Chars(String),
    Eof,
}

fn is_ws_str(s: &str) -> bool {
    s.chars().all(|c| matches!(c, '\t' | '\n' | '\u{c}' | ' ' | '\r'))
}

const SPECIAL_HTML: &[&str] = &[
    "address", "applet", "area", "article", "aside", "base", "basefont", "bgsound", "blockquote", "body", "br", "button", "caption", "center",
    "col", "colgroup", "dd", "details", "dir", "div", "dl", "dt", "embed", "fieldset", "figcaption", "figure", "footer", "form", "frame",
    "frameset", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hgroup", "hr", "html", "iframe", "img", "input", "keygen", "li", "link",
    "listing", "main", "marquee", "menu", "meta", "nav", "noembed", "noframes", "noscript", "object", "ol", "p", "param", "plaintext", "pre",
    "script", "search", "section", "select", "source", "style", "summary", "table", "tbody", "td", "template", "textarea", "tfoot", "th",
    "thead", "title", "tr", "track", "ul", "wbr", "xmp",
];

const IMPLIED_END: &[&str] = &["dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc"];
const IMPLIED_END_THOROUGH: &[&str] = &["caption", "colgroup", "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc", "tbody", "td", "tfoot", "th", "thead", "tr"];

const SVG_TAGS: &[(&str, &str)] = &[
    ("altglyph", "altGlyph"), ("altglyphdef", "altGlyphDef"), ("altglyphitem", "altGlyphItem"), ("animatecolor", "animateColor"),
    ("animatemotion", "animateMotion"), ("animatetransform", "animateTransform"), ("clippath", "clipPath"), ("feblend", "feBlend"),
    ("fecolormatrix", "feColorMatrix"), ("fecomponenttransfer", "feComponentTransfer"), ("fecomposite", "feComposite"),
    ("feconvolvematrix", "feConvolveMatrix"), ("fediffuselighting", "feDiffuseLighting"), ("fedisplacementmap", "feDisplacementMap"),
    ("fedistantlight", "feDistantLight"), ("fedropshadow", "feDropShadow"), ("feflood", "feFlood"), ("fefunca", "feFuncA"),
    ("fefuncb", "feFuncB"), ("fefuncg", "feFuncG"), ("fefuncr", "feFuncR"), ("fegaussianblur", "feGaussianBlur"), ("feimage", "feImage"),
    ("femerge", "feMerge"), ("femergenode", "feMergeNode"), ("femorphology", "feMorphology"), ("feoffset", "feOffset"),
    ("fepointlight", "fePointLight"), ("fespecularlighting", "feSpecularLighting"), ("fespotlight", "feSpotLight"), ("fetile", "feTile"),
    ("feturbulence", "feTurbulence"), ("foreignobject", "foreignObject"), ("glyphref", "glyphRef"), ("lineargradient", "linearGradient"),
    ("radialgradient", "radialGradient"), ("textpath", "textPath"),
];

const SVG_ATTRS: &[(&str, &str)] = &[
    ("attributename", "attributeName"), ("attributetype", "attributeType"), ("basefrequency", "baseFrequency"), ("baseprofile", "baseProfile"),
    ("calcmode", "calcMode"), ("clippathunits", "clipPathUnits"), ("diffuseconstant", "diffuseConstant"), ("edgemode", "edgeMode"),
    ("filterunits", "filterUnits"), ("glyphref", "glyphRef"), ("gradienttransform", "gradientTransform"), ("gradientunits", "gradientUnits"),
    ("kernelmatrix", "kernelMatrix"), ("kernelunitlength", "kernelUnitLength"), ("keypoints", "keyPoints"), ("keysplines", "keySplines"),
    ("keytimes", "keyTimes"), ("lengthadjust", "lengthAdjust"), ("limitingconeangle", "limitingConeAngle"), ("markerheight", "markerHeight"),
    ("markerunits", "markerUnits"), ("markerwidth", "markerWidth"), ("maskcontentunits", "maskContentUnits"), ("maskunits", "maskUnits"),
    ("numoctaves", "numOctaves"), ("pathlength", "pathLength"), ("patterncontentunits", "patternContentUnits"), ("patterntransform", "patternTransform"),
    ("patternunits", "patternUnits"), ("pointsatx", "pointsAtX"), ("pointsaty", "pointsAtY"), ("pointsatz", "pointsAtZ"),
    ("preservealpha", "preserveAlpha"), ("preserveaspectratio", "preserveAspectRatio"), ("primitiveunits", "primitiveUnits"), ("refx", "refX"),
    ("refy", "refY"), ("repeatcount", "repeatCount"), ("repeatdur", "repeatDur"), ("requiredextensions", "requiredExtensions"),
    ("requiredfeatures", "requiredFeatures"), ("specularconstant", "specularConstant"), ("specularexponent", "specularExponent"),
    ("spreadmethod", "spreadMethod"), ("startoffset", "startOffset"), ("stddeviation", "stdDeviation"), ("stitchtiles", "stitchTiles"),
    ("surfacescale", "surfaceScale"), ("systemlanguage", "systemLanguage"), ("tablevalues", "tableValues"), ("targetx", "targetX"),
    ("targety", "targetY"), ("textlength", "textLength"), ("viewbox", "viewBox"), ("viewtarget", "viewTarget"), ("xchannelselector", "xChannelSelector"),
    ("ychannelselector", "yChannelSelector"), ("zoomandpan", "zoomAndPan"),
];

struct TreeBuilder {
    doc: Document,
    open: Vec<NodeId>,
    afe: Vec<Afe>,
    mode: Mode,
    original_mode: Mode,
    template_modes: Vec<Mode>,
    head: Option<NodeId>,
    form: Option<NodeId>,
    frameset_ok: bool,
    scripting: bool,
    foster: bool,
    pending_table_text: Vec<String>,
    /// Ignore a LF right after `<pre>`, `<listing>`, `<textarea>`.
    skip_lf: bool,
    fragment: bool,
    context: Option<NodeId>,
    /// Tokenizer state requested by the last start tag.
    switch_to: Option<State>,
    done: bool,
    /// Stop after a `</script>` end tag so the caller can run the script
    /// (incremental document parsing with scripting).
    pause_on_scripts: bool,
    pending_script: Option<NodeId>,
    /// The comment token being dispatched is a processing instruction
    /// with this target.
    pi_target: Option<String>,
    /// `<template shadowrootmode>` attaches a declarative shadow root.
    allow_dsd: bool,
    /// Templates whose contents are a declarative shadow root (they are
    /// on the stack but not in the tree).
    dsd_contents: std::collections::HashMap<NodeId, NodeId>,
    /// Streaming fragment parsing into a live node: insert the root's
    /// children before this node (None: append).
    root_before: Option<NodeId>,
    /// Reset the insertion mode on the first run (fragment parsers need
    /// the document to look at the context element).
    mode_reset_pending: bool,
}

impl TreeBuilder {
    fn new(scripting: bool) -> TreeBuilder {
        TreeBuilder {
            doc: Document::new(),
            open: Vec::new(),
            afe: Vec::new(),
            mode: Mode::Initial,
            original_mode: Mode::Initial,
            template_modes: Vec::new(),
            head: None,
            form: None,
            frameset_ok: true,
            scripting,
            foster: false,
            pending_table_text: Vec::new(),
            skip_lf: false,
            fragment: false,
            context: None,
            switch_to: None,
            done: false,
            pause_on_scripts: false,
            pending_script: None,
            pi_target: None,
            allow_dsd: false,
            dsd_contents: std::collections::HashMap::new(),
            root_before: None,
            mode_reset_pending: false,
        }
    }

    fn run(&mut self, input: &str) {
        let mut tk = Tokenizer::new(input);
        self.run_tokens(&mut tk);
    }

    fn run_tokens(&mut self, tk: &mut Tokenizer) -> ParseStatus {
        loop {
            tk.allow_cdata = self.adjusted_current().map(|n| !self.is_ns(n, Namespace::Html)).unwrap_or(false);
            let tok = tk.next_token();
            if tok == Token::NeedInput {
                return ParseStatus::NeedInput;
            }
            // "If the next token is a LF, ignore it" (after <pre>, ...).
            let skip_lf = std::mem::take(&mut self.skip_lf);
            match tok {
                Token::NeedInput => {}
                Token::Eof => {
                    self.dispatch(Tok::Eof);
                    break;
                }
                Token::Doctype(d) => self.dispatch(Tok::Doctype(d)),
                Token::Comment(c) => self.dispatch(Tok::Comment(c)),
                // Inserted wherever a comment would be.
                Token::ProcessingInstruction { target, data } => {
                    self.pi_target = Some(target);
                    self.dispatch(Tok::Comment(data));
                    self.pi_target = None;
                }
                Token::EndTag { name } => self.dispatch(Tok::End(name)),
                Token::StartTag { name, attrs, self_closing } => {
                    self.dispatch(Tok::Start(Tag { name, attrs, self_closing }));
                    if let Some(s) = self.switch_to.take() {
                        tk.set_state(s);
                    }
                }
                Token::Text(mut t) => {
                    if skip_lf && t.starts_with('\n') {
                        t.remove(0);
                    }
                    self.chars(&t);
                }
            }
            if self.done {
                break;
            }
            if let Some(s) = self.pending_script.take() {
                return ParseStatus::Script(s);
            }
        }
        self.mirror_selectedcontent();
        ParseStatus::Finished
    }

    /// A `<selectedcontent>` inside a `<select>` holds a copy of the
    /// selected option's contents (the last option with `selected`, else
    /// the first option).
    fn mirror_selectedcontent(&mut self) {
        let targets: Vec<NodeId> = self.doc.descendants(Document::ROOT).filter(|&n| self.is_html(n, "selectedcontent")).collect();
        for sc in targets {
            let Some(sel) = self.doc.ancestor_or_self(sc, |a| self.is_html(a, "select")) else { continue };
            let options: Vec<NodeId> = self.doc.descendants(sel).filter(|&n| self.is_html(n, "option")).collect();
            let chosen = options.iter().rev().find(|&&o| self.doc.attr(o, "selected").is_some()).or(options.first()).copied();
            let Some(option) = chosen else { continue };
            self.doc.clear_children(sc);
            let kids: Vec<NodeId> = self.doc.children(option).collect();
            for k in kids {
                let copy = self.doc.clone_subtree(k, true);
                self.doc.append_child(sc, copy);
            }
        }
    }

    /// Split text into homogeneous runs (whitespace / NUL / other).
    fn chars(&mut self, t: &str) {
        let mut start = 0;
        let mut kind = 3u8;
        for (i, c) in t.char_indices() {
            let k = match c {
                '\t' | '\n' | '\u{c}' | ' ' | '\r' => 0,
                '\0' => 1,
                _ => 2,
            };
            if k != kind {
                if i > start {
                    self.dispatch(Tok::Chars(t[start..i].to_string()));
                }
                start = i;
                kind = k;
            }
        }
        if start < t.len() {
            self.dispatch(Tok::Chars(t[start..].to_string()));
        }
    }

    // ------------------------------------------------------------ helpers

    fn current(&self) -> Option<NodeId> {
        self.open.last().copied()
    }

    fn adjusted_current(&self) -> Option<NodeId> {
        if self.fragment && self.open.len() == 1 {
            return self.context;
        }
        self.current()
    }

    fn el(&self, n: NodeId) -> (&str, &Namespace) {
        match self.doc.element(n) {
            Some(e) => (e.tag.as_str(), &e.ns),
            None => ("", &Namespace::None),
        }
    }

    fn is_ns(&self, n: NodeId, ns: Namespace) -> bool {
        *self.el(n).1 == ns
    }

    fn is_html(&self, n: NodeId, name: &str) -> bool {
        let (t, ns) = self.el(n);
        *ns == Namespace::Html && t == name
    }

    fn is_html_any(&self, n: NodeId, names: &[&str]) -> bool {
        let (t, ns) = self.el(n);
        *ns == Namespace::Html && names.contains(&t)
    }

    fn current_is(&self, name: &str) -> bool {
        self.current().map(|n| self.is_html(n, name)).unwrap_or(false)
    }

    fn current_is_any(&self, names: &[&str]) -> bool {
        self.current().map(|n| self.is_html_any(n, names)).unwrap_or(false)
    }

    fn is_special(&self, n: NodeId) -> bool {
        let (t, ns) = self.el(n);
        match ns {
            Namespace::Html => SPECIAL_HTML.contains(&t),
            Namespace::MathMl => matches!(t, "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"),
            Namespace::Svg => matches!(t, "foreignObject" | "desc" | "title"),
            _ => false,
        }
    }

    fn is_mathml_text_ip(&self, n: NodeId) -> bool {
        let (t, ns) = self.el(n);
        *ns == Namespace::MathMl && matches!(t, "mi" | "mo" | "mn" | "ms" | "mtext")
    }

    fn is_html_ip(&self, n: NodeId) -> bool {
        let (t, ns) = self.el(n);
        match ns {
            Namespace::MathMl if t == "annotation-xml" => self
                .doc
                .attr(n, "encoding")
                .map(|e| e.eq_ignore_ascii_case("text/html") || e.eq_ignore_ascii_case("application/xhtml+xml"))
                .unwrap_or(false),
            Namespace::Svg => matches!(t, "foreignObject" | "desc" | "title"),
            _ => false,
        }
    }

    fn scope_boundary(&self, n: NodeId, extra: &[&str]) -> bool {
        let (t, ns) = self.el(n);
        match ns {
            Namespace::Html => matches!(t, "applet" | "caption" | "html" | "table" | "td" | "th" | "marquee" | "object" | "template") || extra.contains(&t),
            Namespace::MathMl => matches!(t, "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"),
            Namespace::Svg => matches!(t, "foreignObject" | "desc" | "title"),
            _ => false,
        }
    }

    fn in_scope_with(&self, names: &[&str], extra: &[&str]) -> bool {
        for &n in self.open.iter().rev() {
            if self.is_html_any(n, names) {
                return true;
            }
            if self.scope_boundary(n, extra) {
                return false;
            }
        }
        false
    }

    fn in_scope(&self, name: &str) -> bool {
        self.in_scope_with(&[name], &[])
    }

    fn in_button_scope(&self, name: &str) -> bool {
        self.in_scope_with(&[name], &["button"])
    }

    fn in_list_scope(&self, name: &str) -> bool {
        self.in_scope_with(&[name], &["ol", "ul"])
    }

    fn in_table_scope(&self, names: &[&str]) -> bool {
        for &n in self.open.iter().rev() {
            if self.is_html_any(n, names) {
                return true;
            }
            if self.is_html_any(n, &["html", "table", "template"]) {
                return false;
            }
        }
        false
    }

    fn node_in_scope(&self, target: NodeId) -> bool {
        for &n in self.open.iter().rev() {
            if n == target {
                return true;
            }
            if self.scope_boundary(n, &[]) {
                return false;
            }
        }
        false
    }

    fn pop(&mut self) -> Option<NodeId> {
        self.open.pop()
    }

    fn pop_until(&mut self, name: &str) {
        while let Some(n) = self.pop() {
            if self.is_html(n, name) {
                break;
            }
        }
    }

    fn pop_until_any(&mut self, names: &[&str]) {
        while let Some(n) = self.pop() {
            if self.is_html_any(n, names) {
                break;
            }
        }
    }

    fn pop_until_node(&mut self, target: NodeId) {
        while let Some(n) = self.pop() {
            if n == target {
                break;
            }
        }
    }

    fn generate_implied_end_tags(&mut self, except: Option<&str>) {
        while let Some(c) = self.current() {
            let (t, ns) = self.el(c);
            if *ns == Namespace::Html && IMPLIED_END.contains(&t) && Some(t) != except {
                self.pop();
            } else {
                break;
            }
        }
    }

    fn generate_implied_end_tags_thoroughly(&mut self) {
        while self.current_is_any(IMPLIED_END_THOROUGH) {
            self.pop();
        }
    }

    fn close_p(&mut self) {
        self.generate_implied_end_tags(Some("p"));
        self.pop_until("p");
    }

    // ------------------------------------------------------------ insertion

    /// The appropriate place for inserting a node: (parent, before).
    fn insertion_place(&self, override_target: Option<NodeId>) -> (NodeId, Option<NodeId>) {
        let place = self.insertion_place_raw(override_target);
        // Streaming into a live parent: the root's children go before the
        // reference node.
        if place.1.is_none() && self.root_before.is_some() && self.open.first() == Some(&place.0) {
            return (place.0, self.root_before);
        }
        // Inside a template: its template contents (or declarative shadow root).
        if place.1.is_none() && self.is_html(place.0, "template") {
            if let Some(&c) = self.dsd_contents.get(&place.0).or_else(|| self.doc.template_contents.get(&place.0)) {
                return (c, None);
            }
        }
        place
    }

    fn insertion_place_raw(&self, override_target: Option<NodeId>) -> (NodeId, Option<NodeId>) {
        let target = override_target.or(self.current()).unwrap_or(Document::ROOT);
        if self.foster && self.is_html_any(target, &["table", "tbody", "tfoot", "thead", "tr"]) {
            let last_template = self.open.iter().rposition(|&n| self.is_html(n, "template"));
            let last_table = self.open.iter().rposition(|&n| self.is_html(n, "table"));
            if let Some(tp) = last_template {
                if last_table.map(|tb| tp > tb).unwrap_or(true) {
                    return (self.open[tp], None);
                }
            }
            let Some(tb) = last_table else { return (self.open[0], None) };
            let table = self.open[tb];
            if let Some(parent) = self.doc.parent(table) {
                return (parent, Some(table));
            }
            return (self.open[tb - 1], None);
        }
        (target, None)
    }

    fn insert_at(&mut self, place: (NodeId, Option<NodeId>), node: NodeId) {
        match place.1 {
            Some(before) => self.doc.insert_before(place.0, node, Some(before)),
            None => self.doc.append_child(place.0, node),
        }
    }

    fn insert_text(&mut self, s: &str) {
        let place = self.insertion_place(None);
        if place.0 == Document::ROOT {
            return;
        }
        let prev = match place.1 {
            Some(b) => self.doc.prev_sibling(b),
            None => self.doc.last_child(place.0),
        };
        if let Some(p) = prev {
            if let NodeData::Text(t) = &mut self.doc.node_mut(p).data {
                t.push_str(s);
                return;
            }
        }
        let t = self.doc.create_text(s);
        self.insert_at(place, t);
    }

    fn insert_comment(&mut self, c: String, place: Option<(NodeId, Option<NodeId>)>) {
        let place = place.unwrap_or_else(|| self.insertion_place(None));
        let n = match self.pi_target.take() {
            Some(t) => self.doc.create_pi(&t, &c),
            None => self.doc.create(NodeData::Comment(c)),
        };
        self.insert_at(place, n);
    }

    fn create_element(&mut self, tag: &Tag, ns: Namespace) -> NodeId {
        if ns == Namespace::Html {
            let el = self.doc.create_element(&tag.name, tag.attrs.clone());
            if tag.name == "template" {
                self.doc.template_content(el);
            }
            el
        } else {
            self.doc.create_element_ns(ns, None, &tag.name, tag.attrs.clone())
        }
    }

    fn insert_element(&mut self, tag: &Tag, ns: Namespace) -> NodeId {
        let place = self.insertion_place(None);
        let el = self.create_element(tag, ns);
        self.insert_at(place, el);
        self.open.push(el);
        el
    }

    fn insert_html(&mut self, tag: &Tag) -> NodeId {
        self.insert_element(tag, Namespace::Html)
    }

    fn insert_html_named(&mut self, name: &str) -> NodeId {
        self.insert_html(&Tag { name: name.to_string(), attrs: Vec::new(), self_closing: false })
    }

    fn insert_foreign(&mut self, mut tag: Tag, ns: Namespace) {
        match ns {
            Namespace::Svg => {
                if let Some((_, fixed)) = SVG_TAGS.iter().find(|(l, _)| *l == tag.name) {
                    tag.name = fixed.to_string();
                }
                for (n, _) in tag.attrs.iter_mut() {
                    if let Some((_, fixed)) = SVG_ATTRS.iter().find(|(l, _)| *l == n.as_str()) {
                        *n = fixed.to_string();
                    }
                }
            }
            Namespace::MathMl => {
                for (n, _) in tag.attrs.iter_mut() {
                    if n == "definitionurl" {
                        *n = "definitionURL".into();
                    }
                }
            }
            _ => {}
        }
        let self_closing = tag.self_closing;
        let is_script = tag.name == "script";
        let el = self.insert_element(&tag, ns);
        if is_script && (self.fragment || !self.scripting) {
            self.doc.script_started.insert(el);
        }
        // "Adjust foreign attributes": xlink:*, xml:* and xmlns get their
        // namespaces.
        for (n, _) in &tag.attrs {
            let ns = match n.as_str() {
                "xlink:actuate" | "xlink:arcrole" | "xlink:href" | "xlink:role" | "xlink:show" | "xlink:title" | "xlink:type" => "http://www.w3.org/1999/xlink",
                "xml:lang" | "xml:space" => "http://www.w3.org/XML/1998/namespace",
                "xmlns" | "xmlns:xlink" => "http://www.w3.org/2000/xmlns/",
                _ => continue,
            };
            self.doc.set_attr_namespace(el, n, Some(ns));
        }
        if self_closing {
            self.pop();
        }
    }

    // --------------------------------------------- active formatting elements

    fn push_afe(&mut self, el: NodeId, tag: &Tag) {
        // Noah's Ark: at most three identical entries after the last marker.
        let mut same = Vec::new();
        for (i, e) in self.afe.iter().enumerate().rev() {
            match e {
                Afe::Marker => break,
                Afe::El(_, t) => {
                    if t.name == tag.name && attrs_equal(&t.attrs, &tag.attrs) {
                        same.push(i);
                    }
                }
            }
        }
        if same.len() >= 3 {
            self.afe.remove(*same.last().unwrap());
        }
        self.afe.push(Afe::El(el, tag.clone()));
    }

    fn afe_index(&self, el: NodeId) -> Option<usize> {
        self.afe.iter().position(|e| matches!(e, Afe::El(n, _) if *n == el))
    }

    fn clear_afe_to_marker(&mut self) {
        while let Some(e) = self.afe.pop() {
            if matches!(e, Afe::Marker) {
                break;
            }
        }
    }

    fn reconstruct_afe(&mut self) {
        let Some(last) = self.afe.last() else { return };
        match last {
            Afe::Marker => return,
            Afe::El(n, _) if self.open.contains(n) => return,
            _ => {}
        }
        let mut i = self.afe.len() - 1;
        loop {
            if i == 0 {
                break;
            }
            i -= 1;
            let stop = match &self.afe[i] {
                Afe::Marker => true,
                Afe::El(n, _) => self.open.contains(n),
            };
            if stop {
                i += 1;
                break;
            }
        }
        for j in i..self.afe.len() {
            let tag = match &self.afe[j] {
                Afe::El(_, t) => t.clone(),
                Afe::Marker => continue,
            };
            let el = self.insert_html(&tag);
            self.afe[j] = Afe::El(el, tag);
        }
    }

    /// The adoption agency algorithm; false means "act as any other end tag".
    fn adoption_agency(&mut self, subject: &str) -> bool {
        if let Some(c) = self.current() {
            if self.is_html(c, subject) && self.afe_index(c).is_none() {
                self.pop();
                return true;
            }
        }
        for _ in 0..8 {
            // The formatting element: last entry named `subject` after the last marker.
            let mut fe = None;
            for e in self.afe.iter().rev() {
                match e {
                    Afe::Marker => break,
                    Afe::El(n, t) if t.name == subject => {
                        fe = Some(*n);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(fe) = fe else { return false };
            let Some(fe_stack) = self.open.iter().position(|&n| n == fe) else {
                if let Some(i) = self.afe_index(fe) {
                    self.afe.remove(i);
                }
                return true;
            };
            if !self.node_in_scope(fe) {
                return true;
            }
            let furthest = self.open[fe_stack + 1..].iter().position(|&n| self.is_special(n)).map(|i| fe_stack + 1 + i);
            let Some(fb_index) = furthest else {
                self.pop_until_node(fe);
                if let Some(i) = self.afe_index(fe) {
                    self.afe.remove(i);
                }
                return true;
            };
            let furthest_block = self.open[fb_index];
            let common_ancestor = self.open[fe_stack - 1];
            // Bookmark: None = the formatting element's position.
            let mut bookmark_after: Option<NodeId> = None;
            let mut node_index = fb_index;
            let mut last_node = furthest_block;
            let mut counter = 0;
            loop {
                counter += 1;
                node_index -= 1;
                let node = self.open[node_index];
                if node == fe {
                    break;
                }
                if counter > 3 {
                    if let Some(i) = self.afe_index(node) {
                        self.afe.remove(i);
                    }
                }
                let Some(ai) = self.afe_index(node) else {
                    self.open.remove(node_index);
                    continue;
                };
                let tag = match &self.afe[ai] {
                    Afe::El(_, t) => t.clone(),
                    Afe::Marker => unreachable!(),
                };
                let new_el = self.create_element(&tag, Namespace::Html);
                self.afe[ai] = Afe::El(new_el, tag);
                self.open[node_index] = new_el;
                if last_node == furthest_block {
                    bookmark_after = Some(new_el);
                }
                self.doc.detach(last_node);
                self.doc.append_child(new_el, last_node);
                last_node = new_el;
            }
            self.doc.detach(last_node);
            let place = self.insertion_place(Some(common_ancestor));
            self.insert_at(place, last_node);
            let fe_tag = match self.afe_index(fe).map(|i| &self.afe[i]) {
                Some(Afe::El(_, t)) => t.clone(),
                _ => Tag { name: subject.to_string(), attrs: Vec::new(), self_closing: false },
            };
            let new_el = self.create_element(&fe_tag, Namespace::Html);
            let kids: Vec<NodeId> = self.doc.children(furthest_block).collect();
            for k in kids {
                self.doc.detach(k);
                self.doc.append_child(new_el, k);
            }
            self.doc.append_child(furthest_block, new_el);
            match bookmark_after {
                None => {
                    if let Some(i) = self.afe_index(fe) {
                        self.afe[i] = Afe::El(new_el, fe_tag);
                    }
                }
                Some(after) => {
                    if let Some(i) = self.afe_index(fe) {
                        self.afe.remove(i);
                    }
                    let at = self.afe_index(after).map(|i| i + 1).unwrap_or(self.afe.len());
                    self.afe.insert(at, Afe::El(new_el, fe_tag));
                }
            }
            if let Some(i) = self.open.iter().position(|&n| n == fe) {
                self.open.remove(i);
            }
            let fbi = self.open.iter().position(|&n| n == furthest_block).unwrap_or(self.open.len() - 1);
            self.open.insert(fbi + 1, new_el);
        }
        true
    }

    // ------------------------------------------------------------ modes

    fn reset_insertion_mode(&mut self) {
        let mut i = self.open.len();
        while i > 0 {
            i -= 1;
            let last = i == 0;
            let node = if last && self.fragment { self.context.unwrap_or(self.open[0]) } else { self.open[i] };
            let (t, ns) = self.el(node);
            let t = t.to_string();
            if *ns != Namespace::Html {
                if last {
                    self.mode = Mode::InBody;
                    return;
                }
                continue;
            }
            match t.as_str() {
                "td" | "th" if !last => {
                    self.mode = Mode::InCell;
                    return;
                }
                "tr" => {
                    self.mode = Mode::InRow;
                    return;
                }
                "tbody" | "thead" | "tfoot" => {
                    self.mode = Mode::InTableBody;
                    return;
                }
                "caption" => {
                    self.mode = Mode::InCaption;
                    return;
                }
                "colgroup" => {
                    self.mode = Mode::InColumnGroup;
                    return;
                }
                "table" => {
                    self.mode = Mode::InTable;
                    return;
                }
                "template" => {
                    self.mode = *self.template_modes.last().unwrap_or(&Mode::InTemplate);
                    return;
                }
                "head" if !last => {
                    self.mode = Mode::InHead;
                    return;
                }
                "body" => {
                    self.mode = Mode::InBody;
                    return;
                }
                "frameset" => {
                    self.mode = Mode::InFrameset;
                    return;
                }
                "html" => {
                    self.mode = if self.head.is_none() { Mode::BeforeHead } else { Mode::AfterHead };
                    return;
                }
                _ => {}
            }
            if last {
                self.mode = Mode::InBody;
                return;
            }
        }
        self.mode = Mode::InBody;
    }

    /// Tree construction dispatcher: foreign content or the current mode.
    fn dispatch(&mut self, tok: Tok) {
        let Some(acn) = self.adjusted_current() else {
            return self.process(self.mode, tok);
        };
        let html = self.is_ns(acn, Namespace::Html);
        let use_mode = html
            || (self.is_mathml_text_ip(acn) && matches!(&tok, Tok::Start(t) if t.name != "mglyph" && t.name != "malignmark"))
            || (self.is_mathml_text_ip(acn) && matches!(tok, Tok::Chars(_)))
            || (self.el(acn) == ("annotation-xml", &Namespace::MathMl) && matches!(&tok, Tok::Start(t) if t.name == "svg"))
            || (self.is_html_ip(acn) && matches!(tok, Tok::Start(_) | Tok::Chars(_)))
            || matches!(tok, Tok::Eof);
        if use_mode {
            self.process(self.mode, tok)
        } else {
            self.foreign(tok)
        }
    }

    fn foreign(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(s) => {
                if s.starts_with('\0') {
                    let r: String = s.chars().map(|_| '\u{fffd}').collect();
                    self.insert_text(&r);
                } else {
                    if !is_ws_str(&s) {
                        self.frameset_ok = false;
                    }
                    self.insert_text(&s);
                }
            }
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(ref t)
                if matches!(
                    t.name.as_str(),
                    "b" | "big" | "blockquote" | "body" | "br" | "center" | "code" | "dd" | "div" | "dl" | "dt" | "em" | "embed" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "head" | "hr" | "i" | "img" | "li" | "listing" | "menu" | "meta" | "nobr" | "ol" | "p" | "pre" | "ruby" | "s" | "small" | "span" | "strong" | "strike" | "sub" | "sup" | "table" | "tt" | "u" | "ul" | "var"
                ) || (t.name == "font" && t.attrs.iter().any(|(n, _)| n == "color" || n == "face" || n == "size")) =>
            {
                self.pop_foreign_until_html();
                self.process(self.mode, tok);
            }
            Tok::End(ref n) if n == "br" || n == "p" => {
                self.pop_foreign_until_html();
                self.process(self.mode, tok);
            }
            Tok::Start(t) => {
                let acn = self.adjusted_current().unwrap();
                let ns = self.el(acn).1.clone();
                self.insert_foreign(t, ns);
            }
            Tok::End(name) => {
                let Some(mut idx) = self.open.len().checked_sub(1) else { return };
                loop {
                    let node = self.open[idx];
                    if idx == 0 {
                        return;
                    }
                    if self.el(node).0.to_ascii_lowercase() == name {
                        self.pop_until_node(node);
                        return;
                    }
                    idx -= 1;
                    if self.is_ns(self.open[idx], Namespace::Html) {
                        break;
                    }
                }
                self.process(self.mode, Tok::End(name));
            }
            Tok::Eof => self.process(self.mode, Tok::Eof),
        }
    }

    fn pop_foreign_until_html(&mut self) {
        while let Some(c) = self.current() {
            if self.is_mathml_text_ip(c) || self.is_html_ip(c) || self.is_ns(c, Namespace::Html) {
                break;
            }
            self.pop();
        }
    }

    fn process(&mut self, mode: Mode, tok: Tok) {
        match mode {
            Mode::Initial => self.initial(tok),
            Mode::BeforeHtml => self.before_html(tok),
            Mode::BeforeHead => self.before_head(tok),
            Mode::InHead => self.in_head(tok),
            Mode::InHeadNoscript => self.in_head_noscript(tok),
            Mode::AfterHead => self.after_head(tok),
            Mode::InBody => self.in_body(tok),
            Mode::Text => self.text_mode(tok),
            Mode::InTable => self.in_table(tok),
            Mode::InTableText => self.in_table_text(tok),
            Mode::InCaption => self.in_caption(tok),
            Mode::InColumnGroup => self.in_column_group(tok),
            Mode::InTableBody => self.in_table_body(tok),
            Mode::InRow => self.in_row(tok),
            Mode::InCell => self.in_cell(tok),
            Mode::InTemplate => self.in_template(tok),
            Mode::AfterBody => self.after_body(tok),
            Mode::InFrameset => self.in_frameset(tok),
            Mode::AfterFrameset => self.after_frameset(tok),
            Mode::AfterAfterBody => self.after_after_body(tok),
            Mode::AfterAfterFrameset => self.after_after_frameset(tok),
        }
    }

    fn reprocess(&mut self, tok: Tok) {
        self.process(self.mode, tok)
    }

    fn initial(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(ref s) if is_ws_str(s) => {}
            Tok::Comment(c) => self.insert_comment(c, Some((Document::ROOT, None))),
            Tok::Doctype(d) => {
                let data = DoctypeData { name: d.name.clone().unwrap_or_default(), public_id: d.public_id.clone().unwrap_or_default(), system_id: d.system_id.clone().unwrap_or_default() };
                let n = self.doc.create(NodeData::Doctype(data.clone()));
                self.doc.append_child(Document::ROOT, n);
                self.doc.quirks_mode = doctype_quirks(&d);
                self.mode = Mode::BeforeHtml;
            }
            _ => {
                self.doc.quirks_mode = QuirksMode::Quirks;
                self.mode = Mode::BeforeHtml;
                self.reprocess(tok);
            }
        }
    }

    fn before_html(&mut self, tok: Tok) {
        match tok {
            Tok::Doctype(_) => {}
            Tok::Comment(c) => self.insert_comment(c, Some((Document::ROOT, None))),
            Tok::Chars(ref s) if is_ws_str(s) => {}
            Tok::Start(ref t) if t.name == "html" => {
                let el = self.create_element(t, Namespace::Html);
                self.doc.append_child(Document::ROOT, el);
                self.open.push(el);
                self.mode = Mode::BeforeHead;
            }
            Tok::End(ref n) if !matches!(n.as_str(), "head" | "body" | "html" | "br") => {}
            _ => {
                let el = self.doc.create_element("html", vec![]);
                self.doc.append_child(Document::ROOT, el);
                self.open.push(el);
                self.mode = Mode::BeforeHead;
                self.reprocess(tok);
            }
        }
    }

    fn before_head(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(ref s) if is_ws_str(s) => {}
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::Start(ref t) if t.name == "head" => {
                let h = self.insert_html(t);
                self.head = Some(h);
                self.mode = Mode::InHead;
            }
            Tok::End(ref n) if !matches!(n.as_str(), "head" | "body" | "html" | "br") => {}
            _ => {
                let h = self.insert_html_named("head");
                self.head = Some(h);
                self.mode = Mode::InHead;
                self.reprocess(tok);
            }
        }
    }

    fn in_head(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(ref s) if is_ws_str(s) => self.insert_text(s),
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::Start(ref t) if matches!(t.name.as_str(), "base" | "basefont" | "bgsound" | "link" | "meta") => {
                self.insert_html(t);
                self.pop();
            }
            Tok::Start(ref t) if t.name == "title" => self.generic_raw(t, State::Rcdata),
            Tok::Start(ref t) if (t.name == "noscript" && self.scripting) || t.name == "noframes" || t.name == "style" => self.generic_raw(t, State::Rawtext),
            Tok::Start(ref t) if t.name == "noscript" => {
                self.insert_html(t);
                self.mode = Mode::InHeadNoscript;
            }
            Tok::Start(ref t) if t.name == "script" => {
                let place = self.insertion_place(None);
                let el = self.create_element(t, Namespace::Html);
                // Fragment parsers and parsers without scripting make
                // scripts that never run.
                if self.fragment || !self.scripting {
                    self.doc.script_started.insert(el);
                }
                self.insert_at(place, el);
                self.open.push(el);
                self.switch_to = Some(State::ScriptData);
                self.original_mode = self.mode;
                self.mode = Mode::Text;
            }
            Tok::End(ref n) if n == "head" => {
                self.pop();
                self.mode = Mode::AfterHead;
            }
            Tok::Start(ref t) if t.name == "template" => {
                self.afe.push(Afe::Marker);
                self.frameset_ok = false;
                self.mode = Mode::InTemplate;
                self.template_modes.push(Mode::InTemplate);
                self.insert_template(t);
            }
            Tok::End(ref n) if n == "template" => {
                if !self.has_template() {
                    return;
                }
                self.generate_implied_end_tags_thoroughly();
                self.pop_until("template");
                self.clear_afe_to_marker();
                self.template_modes.pop();
                self.reset_insertion_mode();
            }
            Tok::Start(ref t) if t.name == "head" => {}
            Tok::End(ref n) if !matches!(n.as_str(), "body" | "html" | "br") => {}
            _ => {
                self.pop();
                self.mode = Mode::AfterHead;
                self.reprocess(tok);
            }
        }
    }

    /// Insert a `<template>` start tag: an ordinary template, or a
    /// declarative shadow root on the adjusted current node.
    fn insert_template(&mut self, t: &Tag) {
        let mode = t.attrs.iter().find(|(n, _)| n == "shadowrootmode").map(|(_, v)| v.to_ascii_lowercase());
        let host = self.adjusted_current();
        let declarative = self.allow_dsd && matches!(mode.as_deref(), Some("open" | "closed")) && host.is_some() && host != self.open.first().copied();
        let host = match host {
            Some(h) if declarative && !self.doc.shadow_roots.contains_key(&h) && self.valid_shadow_host(h) => h,
            _ => {
                self.insert_html(t);
                return;
            }
        };
        let template = self.create_element(t, Namespace::Html);
        self.open.push(template);
        let root = self.doc.attach_shadow(host);
        self.doc.declarative_shadow.insert(root, mode.as_deref() == Some("open"));
        let flag = |n: &str| t.attrs.iter().any(|(a, _)| a == n);
        let mut bits = 0u8;
        if flag("shadowrootclonable") {
            bits |= 1;
        }
        if flag("shadowrootserializable") {
            bits |= 2;
        }
        if flag("shadowrootdelegatesfocus") {
            bits |= 4;
        }
        if bits != 0 {
            self.doc.declarative_shadow_flags.insert(root, bits);
        }
        self.dsd_contents.insert(template, root);
    }

    /// Elements `attachShadow()` accepts: custom elements (a valid custom
    /// element name) and a fixed list of HTML elements.
    fn valid_shadow_host(&self, n: NodeId) -> bool {
        let (t, ns) = self.el(n);
        if *ns != Namespace::Html {
            return false;
        }
        matches!(t, "article" | "aside" | "blockquote" | "body" | "div" | "footer" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "main" | "nav" | "p" | "section" | "span")
            || (t.contains('-') && t.starts_with(|c: char| c.is_ascii_lowercase()) && !matches!(t, "annotation-xml" | "color-profile" | "font-face" | "font-face-src" | "font-face-uri" | "font-face-format" | "font-face-name" | "missing-glyph"))
    }

    fn generic_raw(&mut self, t: &Tag, state: State) {
        self.insert_html(t);
        self.switch_to = Some(state);
        self.original_mode = self.mode;
        self.mode = Mode::Text;
    }

    fn in_head_noscript(&mut self, tok: Tok) {
        match tok {
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::End(ref n) if n == "noscript" => {
                self.pop();
                self.mode = Mode::InHead;
            }
            Tok::Chars(ref s) if is_ws_str(s) => self.in_head(tok),
            Tok::Comment(_) => self.in_head(tok),
            Tok::Start(ref t) if matches!(t.name.as_str(), "basefont" | "bgsound" | "link" | "meta" | "noframes" | "style") => self.in_head(tok),
            Tok::Start(ref t) if t.name == "head" || t.name == "noscript" => {}
            Tok::End(ref n) if n != "br" => {}
            _ => {
                self.pop();
                self.mode = Mode::InHead;
                self.reprocess(tok);
            }
        }
    }

    fn after_head(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(ref s) if is_ws_str(s) => self.insert_text(s),
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::Start(ref t) if t.name == "body" => {
                self.insert_html(t);
                self.frameset_ok = false;
                self.mode = Mode::InBody;
            }
            Tok::Start(ref t) if t.name == "frameset" => {
                self.insert_html(t);
                self.mode = Mode::InFrameset;
            }
            Tok::Start(ref t)
                if matches!(t.name.as_str(), "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style" | "template" | "title") =>
            {
                let Some(head) = self.head else { return };
                self.open.push(head);
                self.in_head(tok);
                if let Some(i) = self.open.iter().rposition(|&n| n == head) {
                    self.open.remove(i);
                }
            }
            Tok::End(ref n) if n == "template" => self.in_head(tok),
            Tok::Start(ref t) if t.name == "head" => {}
            Tok::End(ref n) if !matches!(n.as_str(), "body" | "html" | "br") => {}
            _ => {
                self.insert_html_named("body");
                self.mode = Mode::InBody;
                self.reprocess(tok);
            }
        }
    }

    fn any_other_end_in_body(&mut self, name: &str) {
        let mut i = self.open.len();
        while i > 0 {
            i -= 1;
            let node = self.open[i];
            if self.is_html(node, name) {
                self.generate_implied_end_tags(Some(name));
                self.pop_until_node(node);
                return;
            }
            if self.is_special(node) {
                return;
            }
        }
    }

    /// A template element is open (or is the fragment's context).
    fn has_template(&self) -> bool {
        self.open.iter().any(|&n| self.is_html(n, "template")) || (self.fragment && self.context.map(|c| self.is_html(c, "template")).unwrap_or(false))
    }

    fn select_in_scope(&self) -> bool {
        self.in_scope("select")
    }

    fn in_body(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(s) => {
                if s.starts_with('\0') {
                    return;
                }
                self.reconstruct_afe();
                if !is_ws_str(&s) {
                    self.frameset_ok = false;
                }
                self.insert_text(&s);
            }
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(t) => self.in_body_start(t),
            Tok::End(n) => self.in_body_end(n),
            Tok::Eof => {
                if !self.template_modes.is_empty() {
                    return self.in_template(Tok::Eof);
                }
                self.done = true;
            }
        }
    }

    fn in_body_start(&mut self, t: Tag) {
        let name = t.name.clone();
        match name.as_str() {
            "html" => {
                if self.has_template() {
                    return;
                }
                if let Some(&html) = self.open.first() {
                    for (k, v) in &t.attrs {
                        if self.doc.attr(html, k).is_none() {
                            self.doc.set_attr(html, k, Some(v));
                        }
                    }
                }
            }
            "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style" | "template" | "title" => self.in_head(Tok::Start(t)),
            "body" => {
                if self.open.len() < 2 || !self.is_html(self.open[1], "body") || self.has_template() {
                    return;
                }
                self.frameset_ok = false;
                let body = self.open[1];
                for (k, v) in &t.attrs {
                    if self.doc.attr(body, k).is_none() {
                        self.doc.set_attr(body, k, Some(v));
                    }
                }
            }
            "frameset" => {
                if self.open.len() < 2 || !self.is_html(self.open[1], "body") || !self.frameset_ok {
                    return;
                }
                let body = self.open[1];
                self.doc.detach(body);
                self.open.truncate(1);
                self.insert_html(&t);
                self.mode = Mode::InFrameset;
            }
            "address" | "article" | "aside" | "blockquote" | "center" | "details" | "dialog" | "dir" | "div" | "dl" | "fieldset" | "figcaption" | "figure"
            | "footer" | "header" | "hgroup" | "main" | "menu" | "nav" | "ol" | "p" | "search" | "section" | "summary" | "ul" => {
                if self.in_button_scope("p") {
                    self.close_p();
                }
                self.insert_html(&t);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if self.in_button_scope("p") {
                    self.close_p();
                }
                if self.current_is_any(&["h1", "h2", "h3", "h4", "h5", "h6"]) {
                    self.pop();
                }
                self.insert_html(&t);
            }
            "pre" | "listing" => {
                if self.in_button_scope("p") {
                    self.close_p();
                }
                self.insert_html(&t);
                self.skip_lf = true;
                self.frameset_ok = false;
            }
            "form" => {
                let in_template = self.has_template();
                if self.form.is_some() && !in_template {
                    return;
                }
                if self.in_button_scope("p") {
                    self.close_p();
                }
                let f = self.insert_html(&t);
                if !in_template {
                    self.form = Some(f);
                }
            }
            "li" | "dd" | "dt" => {
                self.frameset_ok = false;
                let names: &[&str] = if name == "li" { &["li"] } else { &["dd", "dt"] };
                let mut i = self.open.len();
                while i > 0 {
                    i -= 1;
                    let node = self.open[i];
                    if self.is_html_any(node, names) {
                        let nt = self.el(node).0.to_string();
                        self.generate_implied_end_tags(Some(&nt));
                        self.pop_until(&nt);
                        break;
                    }
                    if self.is_special(node) && !self.is_html_any(node, &["address", "div", "p"]) {
                        break;
                    }
                }
                if self.in_button_scope("p") {
                    self.close_p();
                }
                self.insert_html(&t);
            }
            "plaintext" => {
                if self.in_button_scope("p") {
                    self.close_p();
                }
                self.insert_html(&t);
                self.switch_to = Some(State::Plaintext);
            }
            "button" => {
                if self.in_scope("button") {
                    self.generate_implied_end_tags(None);
                    self.pop_until("button");
                }
                self.reconstruct_afe();
                self.insert_html(&t);
                self.frameset_ok = false;
            }
            "a" => {
                let mut existing = None;
                for e in self.afe.iter().rev() {
                    match e {
                        Afe::Marker => break,
                        Afe::El(n, tg) if tg.name == "a" => {
                            existing = Some(*n);
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(a) = existing {
                    self.adoption_agency("a");
                    if let Some(i) = self.afe_index(a) {
                        self.afe.remove(i);
                    }
                    if let Some(i) = self.open.iter().position(|&n| n == a) {
                        self.open.remove(i);
                    }
                }
                self.reconstruct_afe();
                let el = self.insert_html(&t);
                self.push_afe(el, &t);
            }
            "b" | "big" | "code" | "em" | "font" | "i" | "s" | "small" | "strike" | "strong" | "tt" | "u" => {
                self.reconstruct_afe();
                let el = self.insert_html(&t);
                self.push_afe(el, &t);
            }
            "nobr" => {
                self.reconstruct_afe();
                if self.in_scope("nobr") {
                    if !self.adoption_agency("nobr") {
                        self.any_other_end_in_body("nobr");
                    }
                    self.reconstruct_afe();
                }
                let el = self.insert_html(&t);
                self.push_afe(el, &t);
            }
            "applet" | "marquee" | "object" => {
                self.reconstruct_afe();
                self.insert_html(&t);
                self.afe.push(Afe::Marker);
                self.frameset_ok = false;
            }
            "table" => {
                if self.doc.quirks_mode != QuirksMode::Quirks && self.in_button_scope("p") {
                    self.close_p();
                }
                self.insert_html(&t);
                self.frameset_ok = false;
                self.mode = Mode::InTable;
            }
            "area" | "br" | "embed" | "img" | "keygen" | "wbr" => {
                self.reconstruct_afe();
                self.insert_html(&t);
                self.pop();
                self.frameset_ok = false;
            }
            "input" => {
                if self.fragment && self.context.map(|c| self.is_html(c, "select")).unwrap_or(false) {
                    return;
                }
                if self.select_in_scope() {
                    self.pop_until("select");
                }
                self.reconstruct_afe();
                self.insert_html(&t);
                self.pop();
                let hidden = t.attrs.iter().any(|(k, v)| k == "type" && v.eq_ignore_ascii_case("hidden"));
                if !hidden {
                    self.frameset_ok = false;
                }
            }
            "param" | "source" | "track" => {
                self.insert_html(&t);
                self.pop();
            }
            "hr" => {
                if self.in_button_scope("p") {
                    self.close_p();
                }
                if self.select_in_scope() {
                    self.generate_implied_end_tags(None);
                }
                self.insert_html(&t);
                self.pop();
                self.frameset_ok = false;
            }
            "image" => {
                let mut t2 = t;
                t2.name = "img".into();
                self.in_body_start(t2);
            }
            "textarea" => {
                self.insert_html(&t);
                self.skip_lf = true;
                self.switch_to = Some(State::Rcdata);
                self.original_mode = self.mode;
                self.frameset_ok = false;
                self.mode = Mode::Text;
            }
            "xmp" => {
                if self.in_button_scope("p") {
                    self.close_p();
                }
                self.reconstruct_afe();
                self.frameset_ok = false;
                self.generic_raw(&t, State::Rawtext);
            }
            "iframe" => {
                self.frameset_ok = false;
                self.generic_raw(&t, State::Rawtext);
            }
            "noembed" => self.generic_raw(&t, State::Rawtext),
            "noscript" if self.scripting => self.generic_raw(&t, State::Rawtext),
            "select" => {
                if self.fragment && self.context.map(|c| self.is_html(c, "select")).unwrap_or(false) {
                    return;
                }
                if self.select_in_scope() {
                    self.pop_until("select");
                    return;
                }
                self.reconstruct_afe();
                self.insert_html(&t);
                self.frameset_ok = false;
            }
            "option" => {
                if self.select_in_scope() {
                    self.generate_implied_end_tags(Some("optgroup"));
                } else if self.current_is("option") {
                    self.pop();
                }
                self.reconstruct_afe();
                self.insert_html(&t);
            }
            "optgroup" => {
                if self.select_in_scope() {
                    self.generate_implied_end_tags(None);
                } else if self.current_is("option") {
                    self.pop();
                }
                self.reconstruct_afe();
                self.insert_html(&t);
            }
            "rb" | "rtc" => {
                if self.in_scope("ruby") {
                    self.generate_implied_end_tags(None);
                }
                self.insert_html(&t);
            }
            "rp" | "rt" => {
                if self.in_scope("ruby") {
                    self.generate_implied_end_tags(Some("rtc"));
                }
                self.insert_html(&t);
            }
            "math" => {
                self.reconstruct_afe();
                self.insert_foreign(t, Namespace::MathMl);
            }
            "svg" => {
                self.reconstruct_afe();
                self.insert_foreign(t, Namespace::Svg);
            }
            "caption" | "col" | "colgroup" | "frame" | "head" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr" => {}
            _ => {
                self.reconstruct_afe();
                self.insert_html(&t);
            }
        }
    }

    fn in_body_end(&mut self, name: String) {
        match name.as_str() {
            "template" => self.in_head(Tok::End(name)),
            "body" | "html" => {
                if !self.in_scope("body") {
                    return;
                }
                self.mode = Mode::AfterBody;
                if name == "html" {
                    self.reprocess(Tok::End(name));
                }
            }
            "address" | "article" | "aside" | "blockquote" | "button" | "center" | "details" | "dialog" | "dir" | "div" | "dl" | "fieldset"
            | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "listing" | "main" | "menu" | "nav" | "ol" | "pre" | "search"
            | "section" | "select" | "summary" | "ul" => {
                if !self.in_scope(&name) {
                    return;
                }
                self.generate_implied_end_tags(None);
                self.pop_until(&name);
            }
            "form" => {
                if !self.has_template() {
                    let node = self.form.take();
                    let Some(node) = node else { return };
                    if !self.node_in_scope(node) {
                        return;
                    }
                    self.generate_implied_end_tags(None);
                    if let Some(i) = self.open.iter().position(|&n| n == node) {
                        self.open.remove(i);
                    }
                } else {
                    if !self.in_scope("form") {
                        return;
                    }
                    self.generate_implied_end_tags(None);
                    self.pop_until("form");
                }
            }
            "p" => {
                if !self.in_button_scope("p") {
                    self.insert_html_named("p");
                }
                self.close_p();
            }
            "li" => {
                if !self.in_list_scope("li") {
                    return;
                }
                self.generate_implied_end_tags(Some("li"));
                self.pop_until("li");
            }
            "dd" | "dt" => {
                if !self.in_scope(&name) {
                    return;
                }
                self.generate_implied_end_tags(Some(&name));
                self.pop_until(&name);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let hs = ["h1", "h2", "h3", "h4", "h5", "h6"];
                if !self.in_scope_with(&hs, &[]) {
                    return;
                }
                self.generate_implied_end_tags(None);
                self.pop_until_any(&hs);
            }
            "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small" | "strike" | "strong" | "tt" | "u" => {
                if !self.adoption_agency(&name) {
                    self.any_other_end_in_body(&name);
                }
            }
            "applet" | "marquee" | "object" => {
                if !self.in_scope(&name) {
                    return;
                }
                self.generate_implied_end_tags(None);
                self.pop_until(&name);
                self.clear_afe_to_marker();
            }
            "br" => {
                self.in_body_start(Tag { name: "br".into(), attrs: Vec::new(), self_closing: false });
            }
            _ => self.any_other_end_in_body(&name),
        }
    }

    fn text_mode(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(s) => self.insert_text(&s),
            Tok::Eof => {
                self.pop();
                self.mode = self.original_mode;
                self.reprocess(Tok::Eof);
            }
            Tok::End(_) => {
                let el = self.pop();
                self.mode = self.original_mode;
                if let Some(el) = el {
                    if self.pause_on_scripts && self.is_html(el, "script") {
                        self.pending_script = Some(el);
                    }
                }
            }
            _ => {}
        }
    }

    fn clear_to_table_context(&mut self, names: &[&str]) {
        while let Some(c) = self.current() {
            if self.is_html_any(c, names) {
                break;
            }
            self.pop();
        }
    }

    fn in_table(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(_) if self.current_is_any(&["table", "tbody", "template", "tfoot", "thead", "tr"]) => {
                self.pending_table_text.clear();
                self.original_mode = self.mode;
                self.mode = Mode::InTableText;
                self.reprocess(tok);
            }
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "caption" => {
                self.clear_to_table_context(&["table", "template", "html"]);
                self.afe.push(Afe::Marker);
                self.insert_html(t);
                self.mode = Mode::InCaption;
            }
            Tok::Start(ref t) if t.name == "colgroup" => {
                self.clear_to_table_context(&["table", "template", "html"]);
                self.insert_html(t);
                self.mode = Mode::InColumnGroup;
            }
            Tok::Start(ref t) if t.name == "col" => {
                self.clear_to_table_context(&["table", "template", "html"]);
                self.insert_html_named("colgroup");
                self.mode = Mode::InColumnGroup;
                self.reprocess(tok);
            }
            Tok::Start(ref t) if matches!(t.name.as_str(), "tbody" | "tfoot" | "thead") => {
                self.clear_to_table_context(&["table", "template", "html"]);
                self.insert_html(t);
                self.mode = Mode::InTableBody;
            }
            Tok::Start(ref t) if matches!(t.name.as_str(), "td" | "th" | "tr") => {
                self.clear_to_table_context(&["table", "template", "html"]);
                self.insert_html_named("tbody");
                self.mode = Mode::InTableBody;
                self.reprocess(tok);
            }
            Tok::Start(ref t) if t.name == "table" => {
                if !self.in_table_scope(&["table"]) {
                    return;
                }
                self.pop_until("table");
                self.reset_insertion_mode();
                self.reprocess(tok);
            }
            Tok::End(ref n) if n == "table" => {
                if !self.in_table_scope(&["table"]) {
                    return;
                }
                self.pop_until("table");
                self.reset_insertion_mode();
            }
            Tok::End(ref n) if matches!(n.as_str(), "body" | "caption" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr") => {}
            Tok::Start(ref t) if matches!(t.name.as_str(), "style" | "script" | "template") => self.in_head(tok),
            Tok::End(ref n) if n == "template" => self.in_head(tok),
            Tok::Start(ref t) if t.name == "input" && t.attrs.iter().any(|(k, v)| k == "type" && v.eq_ignore_ascii_case("hidden")) => {
                self.insert_html(t);
                self.pop();
            }
            Tok::Start(ref t) if t.name == "form" => {
                let in_template = self.has_template();
                if self.form.is_some() && !in_template {
                    return;
                }
                let f = self.insert_html(t);
                if !in_template {
                    self.form = Some(f);
                }
                self.pop();
            }
            Tok::Eof => self.in_body(tok),
            _ => {
                self.foster = true;
                self.in_body(tok);
                self.foster = false;
            }
        }
    }

    fn in_table_text(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(s) => {
                if s.starts_with('\0') {
                    return;
                }
                self.pending_table_text.push(s);
            }
            _ => {
                let pending = std::mem::take(&mut self.pending_table_text);
                if pending.iter().any(|s| !is_ws_str(s)) {
                    for s in pending {
                        self.foster = true;
                        self.in_body(Tok::Chars(s));
                        self.foster = false;
                    }
                } else {
                    for s in pending {
                        self.insert_text(&s);
                    }
                }
                self.mode = self.original_mode;
                self.reprocess(tok);
            }
        }
    }

    fn in_caption(&mut self, tok: Tok) {
        match tok {
            Tok::End(ref n) if n == "caption" => {
                if !self.in_table_scope(&["caption"]) {
                    return;
                }
                self.generate_implied_end_tags(None);
                self.pop_until("caption");
                self.clear_afe_to_marker();
                self.mode = Mode::InTable;
            }
            Tok::Start(ref t) if matches!(t.name.as_str(), "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr") => {
                if !self.in_table_scope(&["caption"]) {
                    return;
                }
                self.generate_implied_end_tags(None);
                self.pop_until("caption");
                self.clear_afe_to_marker();
                self.mode = Mode::InTable;
                self.reprocess(tok);
            }
            Tok::End(ref n) if n == "table" => {
                if !self.in_table_scope(&["caption"]) {
                    return;
                }
                self.generate_implied_end_tags(None);
                self.pop_until("caption");
                self.clear_afe_to_marker();
                self.mode = Mode::InTable;
                self.reprocess(tok);
            }
            Tok::End(ref n) if matches!(n.as_str(), "body" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr") => {}
            _ => self.in_body(tok),
        }
    }

    fn in_column_group(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(ref s) if is_ws_str(s) => self.insert_text(s),
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::Start(ref t) if t.name == "col" => {
                self.insert_html(t);
                self.pop();
            }
            Tok::End(ref n) if n == "colgroup" => {
                if !self.current_is("colgroup") {
                    return;
                }
                self.pop();
                self.mode = Mode::InTable;
            }
            Tok::End(ref n) if n == "col" => {}
            Tok::Start(ref t) if t.name == "template" => self.in_head(tok),
            Tok::End(ref n) if n == "template" => self.in_head(tok),
            Tok::Eof => self.in_body(tok),
            _ => {
                if !self.current_is("colgroup") {
                    return;
                }
                self.pop();
                self.mode = Mode::InTable;
                self.reprocess(tok);
            }
        }
    }

    fn in_table_body(&mut self, tok: Tok) {
        match tok {
            Tok::Start(ref t) if t.name == "tr" => {
                self.clear_to_table_context(&["tbody", "tfoot", "thead", "template", "html"]);
                self.insert_html(t);
                self.mode = Mode::InRow;
            }
            Tok::Start(ref t) if t.name == "th" || t.name == "td" => {
                self.clear_to_table_context(&["tbody", "tfoot", "thead", "template", "html"]);
                self.insert_html_named("tr");
                self.mode = Mode::InRow;
                self.reprocess(tok);
            }
            Tok::End(ref n) if matches!(n.as_str(), "tbody" | "tfoot" | "thead") => {
                if !self.in_table_scope(&[n.as_str()]) {
                    return;
                }
                self.clear_to_table_context(&["tbody", "tfoot", "thead", "template", "html"]);
                self.pop();
                self.mode = Mode::InTable;
            }
            Tok::Start(ref t) if matches!(t.name.as_str(), "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead") => self.table_body_exit(tok),
            Tok::End(ref n) if n == "table" => self.table_body_exit(tok),
            Tok::End(ref n) if matches!(n.as_str(), "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr") => {}
            _ => self.in_table(tok),
        }
    }

    fn table_body_exit(&mut self, tok: Tok) {
        if !self.in_table_scope(&["tbody", "thead", "tfoot"]) {
            return;
        }
        self.clear_to_table_context(&["tbody", "tfoot", "thead", "template", "html"]);
        self.pop();
        self.mode = Mode::InTable;
        self.reprocess(tok);
    }

    fn in_row(&mut self, tok: Tok) {
        match tok {
            Tok::Start(ref t) if t.name == "th" || t.name == "td" => {
                self.clear_to_table_context(&["tr", "template", "html"]);
                self.insert_html(t);
                self.mode = Mode::InCell;
                self.afe.push(Afe::Marker);
            }
            Tok::End(ref n) if n == "tr" => {
                if !self.in_table_scope(&["tr"]) {
                    return;
                }
                self.clear_to_table_context(&["tr", "template", "html"]);
                self.pop();
                self.mode = Mode::InTableBody;
            }
            Tok::Start(ref t) if matches!(t.name.as_str(), "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" | "tr") => self.row_exit(tok),
            Tok::End(ref n) if n == "table" => self.row_exit(tok),
            Tok::End(ref n) if matches!(n.as_str(), "tbody" | "tfoot" | "thead") => {
                if !self.in_table_scope(&[n.as_str()]) {
                    return;
                }
                if !self.in_table_scope(&["tr"]) {
                    return;
                }
                self.clear_to_table_context(&["tr", "template", "html"]);
                self.pop();
                self.mode = Mode::InTableBody;
                self.reprocess(tok);
            }
            Tok::End(ref n) if matches!(n.as_str(), "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th") => {}
            _ => self.in_table(tok),
        }
    }

    fn row_exit(&mut self, tok: Tok) {
        if !self.in_table_scope(&["tr"]) {
            return;
        }
        self.clear_to_table_context(&["tr", "template", "html"]);
        self.pop();
        self.mode = Mode::InTableBody;
        self.reprocess(tok);
    }

    fn close_cell(&mut self) {
        self.generate_implied_end_tags(None);
        self.pop_until_any(&["td", "th"]);
        self.clear_afe_to_marker();
        self.mode = Mode::InRow;
    }

    fn in_cell(&mut self, tok: Tok) {
        match tok {
            Tok::End(ref n) if n == "td" || n == "th" => {
                if !self.in_table_scope(&[n.as_str()]) {
                    return;
                }
                self.generate_implied_end_tags(None);
                self.pop_until(n);
                self.clear_afe_to_marker();
                self.mode = Mode::InRow;
            }
            Tok::Start(ref t) if matches!(t.name.as_str(), "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr") => {
                if !self.in_table_scope(&["td", "th"]) {
                    return;
                }
                self.close_cell();
                self.reprocess(tok);
            }
            Tok::End(ref n) if matches!(n.as_str(), "body" | "caption" | "col" | "colgroup" | "html") => {}
            Tok::End(ref n) if matches!(n.as_str(), "table" | "tbody" | "tfoot" | "thead" | "tr") => {
                if !self.in_table_scope(&[n.as_str()]) {
                    return;
                }
                self.close_cell();
                self.reprocess(tok);
            }
            _ => self.in_body(tok),
        }
    }

    fn in_template(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(_) | Tok::Comment(_) | Tok::Doctype(_) => self.in_body(tok),
            Tok::Start(ref t)
                if matches!(t.name.as_str(), "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style" | "template" | "title") =>
            {
                self.in_head(tok)
            }
            Tok::End(ref n) if n == "template" => self.in_head(tok),
            Tok::Start(ref t) => {
                let m = match t.name.as_str() {
                    "caption" | "colgroup" | "tbody" | "tfoot" | "thead" => Mode::InTable,
                    "col" => Mode::InColumnGroup,
                    "tr" => Mode::InTableBody,
                    "td" | "th" => Mode::InRow,
                    _ => Mode::InBody,
                };
                self.template_modes.pop();
                self.template_modes.push(m);
                self.mode = m;
                self.reprocess(tok);
            }
            Tok::End(_) => {}
            Tok::Eof => {
                if !self.has_template() {
                    self.done = true;
                    return;
                }
                self.pop_until("template");
                self.clear_afe_to_marker();
                self.template_modes.pop();
                self.reset_insertion_mode();
                self.reprocess(Tok::Eof);
            }
        }
    }

    fn after_body(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(ref s) if is_ws_str(s) => self.in_body(tok),
            Tok::Comment(c) => {
                let html = self.open[0];
                self.insert_comment(c, Some((html, None)));
            }
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::End(ref n) if n == "html" => {
                if self.fragment {
                    return;
                }
                self.mode = Mode::AfterAfterBody;
            }
            Tok::Eof => self.done = true,
            _ => {
                self.mode = Mode::InBody;
                self.reprocess(tok);
            }
        }
    }

    fn in_frameset(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(ref s) if is_ws_str(s) => self.insert_text(s),
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::Start(ref t) if t.name == "frameset" => {
                self.insert_html(t);
            }
            Tok::End(ref n) if n == "frameset" => {
                if self.open.len() == 1 {
                    return;
                }
                self.pop();
                if !self.fragment && !self.current_is("frameset") {
                    self.mode = Mode::AfterFrameset;
                }
            }
            Tok::Start(ref t) if t.name == "frame" => {
                self.insert_html(t);
                self.pop();
            }
            Tok::Start(ref t) if t.name == "noframes" => self.in_head(tok),
            Tok::Eof => self.done = true,
            _ => {}
        }
    }

    fn after_frameset(&mut self, tok: Tok) {
        match tok {
            Tok::Chars(ref s) if is_ws_str(s) => self.insert_text(s),
            Tok::Comment(c) => self.insert_comment(c, None),
            Tok::Doctype(_) => {}
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::End(ref n) if n == "html" => self.mode = Mode::AfterAfterFrameset,
            Tok::Start(ref t) if t.name == "noframes" => self.in_head(tok),
            Tok::Eof => self.done = true,
            _ => {}
        }
    }

    fn after_after_body(&mut self, tok: Tok) {
        match tok {
            Tok::Comment(c) => self.insert_comment(c, Some((Document::ROOT, None))),
            Tok::Doctype(_) => self.in_body(tok),
            Tok::Chars(ref s) if is_ws_str(s) => self.in_body(tok),
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::Eof => self.done = true,
            _ => {
                self.mode = Mode::InBody;
                self.reprocess(tok);
            }
        }
    }

    fn after_after_frameset(&mut self, tok: Tok) {
        match tok {
            Tok::Comment(c) => self.insert_comment(c, Some((Document::ROOT, None))),
            Tok::Doctype(_) => self.in_body(tok),
            Tok::Chars(ref s) if is_ws_str(s) => self.in_body(tok),
            Tok::Start(ref t) if t.name == "html" => self.in_body(tok),
            Tok::Eof => self.done = true,
            Tok::Start(ref t) if t.name == "noframes" => self.in_head(tok),
            _ => {}
        }
    }
}

fn attrs_equal(a: &[(String, String)], b: &[(String, String)]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
}

/// Quirks mode from a DOCTYPE token (identifiers may be missing).
fn doctype_quirks(d: &Doctype) -> QuirksMode {
    let data = DoctypeData { name: d.name.clone().unwrap_or_default(), public_id: d.public_id.clone().unwrap_or_default(), system_id: d.system_id.clone().unwrap_or_default() };
    let public = data.public_id.to_ascii_lowercase();
    let html401 = public.starts_with("-//w3c//dtd html 4.01 frameset//") || public.starts_with("-//w3c//dtd html 4.01 transitional//");
    if d.system_id.is_none() && html401 {
        return QuirksMode::Quirks;
    }
    if d.system_id.is_some() && html401 {
        // Limited quirks when a system identifier is present.
        let system_id = if data.system_id.is_empty() { "-".to_string() } else { data.system_id.clone() };
        let q = quirks_mode_for(&DoctypeData { system_id, ..data.clone() }, d.force_quirks);
        return if q == QuirksMode::Quirks { q } else { QuirksMode::LimitedQuirks };
    }
    quirks_mode_for(&data, d.force_quirks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implied_structure() {
        let doc = parse("<title>T</title><p>Hello<p>World");
        let d = doc.dump();
        assert!(d.contains("<html>"), "{d}");
        assert!(d.contains("<head>"), "{d}");
        assert!(d.contains("<title>"), "{d}");
        assert!(d.contains("<body>"), "{d}");
        assert_eq!(doc.title().as_deref(), Some("T"));
        let body = doc.body().unwrap();
        let ps: Vec<_> = doc.child_elements(body).collect();
        assert_eq!(ps.len(), 2, "{d}");
    }

    /// Whitespace and comments inside `<head>` stay where they are (SSR
    /// hydration walks those siblings by count).
    #[test]
    fn head_keeps_whitespace_and_comments() {
        let doc = parse("<!doctype html>\n<html>\n<head>\n<meta charset=utf-8>\n<!--h--><!--[--><link rel=a><!--]--> <meta name=x> <title>T</title></head>\n<body><p>x</p></body></html>");
        let head = doc.head().unwrap();
        let kinds: Vec<String> = doc
            .children(head)
            .map(|c| match &doc.node(c).data {
                NodeData::Text(t) => format!("text({})", t.escape_debug()),
                NodeData::Comment(t) => format!("comment({t})"),
                NodeData::Element(e) => e.tag.clone(),
                _ => "?".into(),
            })
            .collect();
        assert_eq!(kinds, ["text(\\n)", "meta", "text(\\n)", "comment(h)", "comment([)", "link", "comment(])", "text( )", "meta", "text( )", "title"]);
        // Whitespace after </head> goes to the html element (as browsers do).
        let html = doc.html_element().unwrap();
        assert_eq!(doc.children(html).filter(|&c| doc.element(c).is_none()).count(), 1, "{}", doc.dump());
    }

    #[test]
    fn doctype_node_and_quirks_mode() {
        let doc = parse("<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01//EN\" \"http://www.w3.org/TR/html4/strict.dtd\">\n<html><p>x");
        let dt = doc.first_child(Document::ROOT).unwrap();
        let NodeData::Doctype(d) = &doc.node(dt).data else { panic!("{}", doc.dump()) };
        assert_eq!(d.name, "html");
        assert_eq!(d.public_id, "-//W3C//DTD HTML 4.01//EN");
        assert_eq!(d.system_id, "http://www.w3.org/TR/html4/strict.dtd");
        assert_eq!(doc.doctype(), Some(dt));
        assert_eq!(doc.quirks_mode, QuirksMode::NoQuirks);
        assert!(doc.html_element().is_some());
        assert_eq!(parse("<!doctype html><p>").quirks_mode, QuirksMode::NoQuirks);
        assert_eq!(parse("<p>no doctype").quirks_mode, QuirksMode::Quirks);
        assert!(parse("<p>no doctype").doctype().is_none());
        assert_eq!(parse("<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01 Transitional//EN\">").quirks_mode, QuirksMode::Quirks);
        assert_eq!(parse("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \"x\">").quirks_mode, QuirksMode::LimitedQuirks);
        // Foreign content is namespaced.
        let doc = parse("<svg><circle/><foreignObject><div></div></foreignObject></svg><math><mi>x</mi></math>");
        let circle = doc.find_element("circle").unwrap();
        assert_eq!(doc.element(circle).unwrap().ns, Namespace::Svg);
        let div = doc.find_element("div").unwrap();
        assert_eq!(doc.element(div).unwrap().ns, Namespace::Html);
        let mi = doc.find_element("mi").unwrap();
        assert_eq!(doc.element(mi).unwrap().ns, Namespace::MathMl);
    }

    #[test]
    fn lists_and_tables() {
        let doc = parse("<ul><li>a<li>b<ul><li>c</ul><li>d</ul><table><tr><td>1<td>2<tr><td>3</table>");
        let d = doc.dump();
        let body = doc.body().unwrap();
        let ul = doc.child_elements(body).next().unwrap();
        let lis: Vec<_> = doc.child_elements(ul).collect();
        assert_eq!(lis.len(), 3, "{d}");
        let nested_lis = doc.descendants(lis[1]).filter(|&n| doc.is_element(n, "li")).count();
        assert_eq!(nested_lis, 2, "{d}");
        let table = doc.find_element("table").unwrap();
        let trs = doc.descendants(table).filter(|&n| doc.is_element(n, "tr")).count();
        assert_eq!(trs, 2, "{d}");
        let tds = doc.descendants(table).filter(|&n| doc.is_element(n, "td")).count();
        assert_eq!(tds, 3, "{d}");
    }

    #[test]
    fn implied_tbody_tr_and_colgroup() {
        let doc = parse("<table><col><tr><td>1<td>2<tr><td>3</table><table><td>x</td></table><table><tr><td><p></tbody> </table>");
        let d = doc.dump();
        let tables: Vec<_> = doc.descendants(Document::ROOT).filter(|&n| doc.is_element(n, "table")).collect();
        let kids = |n| doc.child_elements(n).map(|c| doc.tag(c).unwrap_or("").to_string()).collect::<Vec<_>>();
        assert_eq!(kids(tables[0]), vec!["colgroup", "tbody"], "{d}");
        let tbody = doc.child_elements(tables[0]).nth(1).unwrap();
        assert_eq!(kids(tbody), vec!["tr", "tr"], "{d}");
        assert_eq!(kids(tables[1]), vec!["tbody"], "{d}");
        let tb = doc.child_elements(tables[1]).next().unwrap();
        assert_eq!(kids(tb), vec!["tr"], "{d}");
        // Acid3: whitespace after `</tbody>` stays a child of the table.
        let last = doc.children(tables[2]).last().unwrap();
        assert!(matches!(doc.node(last).data, NodeData::Text(ref t) if t == " "), "{d}");
        assert_eq!(doc.children(tables[2]).count(), 2, "{d}");
    }

    #[test]
    fn misnested_end_tags_ignored() {
        let doc = parse("<div><span>a</div></span>b");
        let body = doc.body().unwrap();
        let kids: Vec<_> = doc.children(body).collect();
        assert_eq!(kids.len(), 2); // div, text "b"
        assert!(matches!(doc.node(kids[1]).data, NodeData::Text(ref t) if t == "b"));
    }

    #[test]
    fn svg_self_closing() {
        let doc = parse("<p><svg viewBox='0 0 1 1'><path d='M0 0'/><circle r='1'/></svg>after</p>");
        let svg = doc.find_element("svg").unwrap();
        assert_eq!(doc.child_elements(svg).count(), 2);
        let p = doc.find_element("p").unwrap();
        assert!(doc.text_content(p).contains("after"));
    }
}
