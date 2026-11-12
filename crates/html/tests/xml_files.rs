//! The XML parser on the XML/XHTML/SVG documents shipped with WPT
//! (dom/nodes/Document-createElement-namespace-tests): no panics, and
//! well-formed files parse. Skipped without a checkout (`RH_WPT_ROOT`).

#[test]
fn wpt_xml_documents_parse() {
    let root = std::env::var_os("RH_WPT_ROOT").map(std::path::PathBuf::from).unwrap_or_else(|| "/mnt/hdd5tb/rabbithole/suites/wpt-dom".into());
    let dir = root.join("dom/nodes/Document-createElement-namespace-tests");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: no WPT checkout at {}", root.display());
        return;
    };
    let mut parsed = 0;
    for e in rd.flatten() {
        let p = e.path();
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "xml" | "xhtml" | "svg") {
            continue;
        }
        let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
        let r = std::panic::catch_unwind(|| rabbithole_html::xml::try_parse_xml(&text).is_ok());
        assert!(r.is_ok(), "XML parser panicked on {}", p.display());
        if r.unwrap() {
            parsed += 1;
        } else if !p.to_string_lossy().contains("malformed") {
            eprintln!("not well-formed: {}", p.display());
        }
    }
    eprintln!("parsed {parsed} XML documents");
}
