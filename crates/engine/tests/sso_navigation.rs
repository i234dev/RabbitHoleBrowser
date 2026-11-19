//! Page-initiated navigations of sign-in flows: auto-submitted `form_post`
//! forms, the document's referrer policy, and `document.cookie` against
//! HttpOnly session cookies.

use rabbithole_engine::net::{CookieJar, ReferrerPolicy, Response, Url};
use rabbithole_engine::{Engine, Page, Viewport};
use std::sync::{Arc, Mutex};

const VIEW: Viewport = Viewport { width: 800.0, height: 600.0 };

fn engine() -> Engine {
    Engine::new_bundled_fonts_only()
}

fn idp() -> Url {
    Url::parse("https://accounts.idp.test/signin/continue?x=1").unwrap()
}

fn response(html: &str, headers: &[(&str, &str)]) -> Response {
    Response {
        url: idp(),
        status: 200,
        content_type: "text/html".into(),
        charset: Some("utf-8".into()),
        body: html.as_bytes().to_vec(),
        headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

fn load(engine: &mut Engine, html: &str, headers: &[(&str, &str)]) -> Page {
    let loaded = engine.loader.load_response(response(html, headers), VIEW);
    engine.build_page(loaded, VIEW)
}

const FORM_POST: &str = r#"<!doctype html><body onload="document.forms[0].submit()"><form method=post action="https://team.access.test/cdn-cgi/access/callback"><input type=hidden name=code value=c0de><input type=hidden name=state value=1234></form>"#;

#[test]
fn auto_submitted_form_post_is_a_cross_site_post_navigation() {
    let mut e = engine();
    let mut page = load(&mut e, FORM_POST, &[]);
    let nav = e.take_page_navigation(&mut page).expect("onload submit navigates");
    let req = nav.request;
    assert_eq!(req.method, "POST");
    assert_eq!(req.url.to_string(), "https://team.access.test/cdn-cgi/access/callback");
    assert_eq!(req.body.as_ref().map(|b| String::from_utf8_lossy(&b.data).into_owned()).as_deref(), Some("code=c0de&state=1234"));
    assert_eq!(req.referer.as_ref(), Some(&idp()), "the IdP document is the initiator");
    assert_eq!(req.referrer_policy, ReferrerPolicy::StrictOriginWhenCrossOrigin);
}

#[test]
fn navigations_follow_the_document_referrer_policy() {
    let mut e = engine();
    let mut page = load(&mut e, FORM_POST, &[("referrer-policy", "origin, bogus")]);
    assert_eq!(page.referrer_policy, ReferrerPolicy::Origin);
    assert_eq!(e.take_page_navigation(&mut page).unwrap().request.referrer_policy, ReferrerPolicy::Origin);
    // <meta name=referrer> is applied after the header; legacy keywords.
    let html = format!("<meta name=referrer content=never>{FORM_POST}");
    let mut page = load(&mut e, &html, &[("referrer-policy", "unsafe-url")]);
    assert_eq!(e.take_page_navigation(&mut page).unwrap().request.referrer_policy, ReferrerPolicy::NoReferrer);
    // rel=noreferrer on the form.
    let html = FORM_POST.replace("<form ", "<form rel=noreferrer ");
    let mut page = load(&mut e, &html, &[]);
    assert_eq!(e.take_page_navigation(&mut page).unwrap().request.referrer_policy, ReferrerPolicy::NoReferrer);
    // location assignments and meta refresh too.
    let mut page = load(&mut e, "<meta name=referrer content=same-origin><script>location.href='https://app.test/'</script>", &[]);
    assert_eq!(e.take_page_navigation(&mut page).unwrap().request.referrer_policy, ReferrerPolicy::SameOrigin);
    let mut page = load(&mut e, "<meta name=referrer content=origin><meta http-equiv=refresh content=\"0; url=https://app.test/\">", &[]);
    let nav = e.take_page_navigation(&mut page).expect("meta refresh");
    assert_eq!(nav.request.referrer_policy, ReferrerPolicy::Origin);
}

#[test]
fn document_cookie_neither_reads_nor_replaces_http_only_cookies() {
    let mut e = engine();
    let jar = Arc::new(Mutex::new(CookieJar::new()));
    jar.lock().unwrap().store_from_headers(&idp(), &["SID=secret; Path=/; Secure; HttpOnly; SameSite=None".into(), "pref=1; Path=/".into()]);
    e.loader.fetcher.cookies = Some(jar.clone());
    let page = load(&mut e, "<script>var before=document.cookie;document.cookie='SID=forged; path=/';document.cookie='js=2; path=/';document.title=before+'|'+document.cookie</script>", &[]);
    assert_eq!(page.doc.title().unwrap_or_default(), "pref=1|pref=1; js=2");
    let header = jar.lock().unwrap().header_for(&idp()).unwrap();
    assert_eq!(header, "SID=secret; pref=1; js=2");
}
