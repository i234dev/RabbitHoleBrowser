//! Automatic handling of cookie-consent dialogs, driven through the
//! content thread like the browser does: fixtures imitating the common
//! CMP shapes (bottom bar, modal with backdrop, iframe, late injection,
//! two-step reject, several languages) and a non-cookie modal that must
//! be left alone.

use super::*;
use crate::autocookies::{ConsentEvent, ConsentMode, ConsentPolicy, SiteRule};
use rabbithole_engine::consent::{Choice, ConsentOutcome};

fn spawn_mode(tab: u64, sink: &Sink, policy: ConsentPolicy) -> ContentHandle {
    let view = ViewSize::new(1000, 700, 1.0);
    let mut c = config(view);
    c.settings.consent = policy;
    let h = ContentHandle::spawn(tab, 1, c, sink.clone());
    h.send(ToContent::SetActive(true));
    h
}

fn mode(m: ConsentMode) -> ConsentPolicy {
    ConsentPolicy { mode: m, ..Default::default() }
}

fn load_at(h: &ContentHandle, html: &str, u: &str) {
    h.send(ToContent::LoadHtml { html: html.to_string(), url: url(u), push_history: true });
}

fn eval(h: &ContentHandle, rx: &Received, tab: u64, js: &str) -> String {
    h.send(ToContent::Eval(js.to_string()));
    let (_, r) = wait_for(rx, tab, Duration::from_secs(10), |m| matches!(m, FromContent::EvalResult(_))).expect("eval result");
    let FromContent::EvalResult(r) = r else { unreachable!() };
    r.trim().trim_matches(|c| c == '\'' || c == '"').to_string()
}

fn consent_event(rx: &Received, tab: u64, timeout: Duration) -> Option<ConsentEvent> {
    match wait_for(rx, tab, timeout, |m| matches!(m, FromContent::Consent(_))) {
        Some((_, FromContent::Consent(e))) => Some(e),
        _ => None,
    }
}

/// Page content under the dialogs (long enough to scroll).
const ARTICLE: &str = "<h1>Daily News</h1><p>Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p><div style='height:1500px'>article</div>";

/// Records pointer events (proof the real pointer path was used) and the
/// choice the page saw.
const PROBE: &str = "<script>window.__choice='none';window.__pd=0;document.addEventListener('pointerdown',function(){window.__pd++});</script>";

fn bottom_bar(text: &str, accept: &str, reject: &str) -> String {
    format!(
        "<html><body style='margin:0;font-family:sans-serif'>{PROBE}{ARTICLE}
<div id='banner' style='position:fixed;left:0;right:0;bottom:0;background:#222;color:#fff;padding:16px;z-index:1000'>
 <p>{text} <a href='/privacy'>Privacy policy</a></p>
 <button id='acc' style='padding:8px'>{accept}</button> <button id='rej' style='padding:8px'>{reject}</button>
</div>
<script>
function closeBanner(c){{ window.__choice=c; document.getElementById('banner').remove(); }}
document.getElementById('acc').addEventListener('click',function(){{closeBanner('accept')}});
document.getElementById('rej').addEventListener('click',function(){{closeBanner('reject')}});
</script></body></html>"
    )
}

#[test]
fn bottom_bar_is_rejected_through_the_pointer_path() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    load_at(&h, &bottom_bar("We use cookies to personalise content and ads and to analyse our traffic.", "Accept all", "Reject all"), "https://news.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(10)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::Rejected, "{e:?}");
    assert_eq!(e.button, "Reject all");
    assert_eq!(e.host, "news.example.test");
    assert_eq!(e.choice, Choice::Reject);
    assert_eq!(eval(&h, &rx, 1, "window.__choice"), "reject");
    assert_eq!(eval(&h, &rx, 1, "document.getElementById('banner') ? 'present' : 'gone'"), "gone");
    assert!(eval(&h, &rx, 1, "window.__pd").parse::<i32>().unwrap_or(0) >= 1, "clicked through the pointer path");
}

#[test]
fn modal_with_backdrop_is_accepted() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Accept));
    let html = format!(
        "<html><body style='margin:0'>{PROBE}{ARTICLE}
<div id='cmp' style='position:fixed;inset:0;background:rgba(0,0,0,.6);z-index:9999'>
 <div role='dialog' aria-modal='true' style='position:absolute;left:250px;top:150px;width:500px;background:#fff;padding:20px'>
  <h2>Tu privacidad es importante</h2>
  <p>Utilizamos cookies propias y de terceros para analizar el tráfico y mostrar publicidad personalizada.</p>
  <div><span role='button' tabindex='0' id='more' style='cursor:pointer'>Configurar</span>
   <button id='no'>Rechazar todo</button> <button id='yes'>Aceptar todo</button></div>
 </div>
</div>
<script>
document.getElementById('yes').onclick=function(){{window.__choice='accept';document.getElementById('cmp').style.display='none'}};
document.getElementById('no').onclick=function(){{window.__choice='reject';document.getElementById('cmp').style.display='none'}};
</script></body></html>"
    );
    load_at(&h, &html, "https://www.diario.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(10)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::Accepted, "{e:?}");
    assert_eq!(e.button, "Aceptar todo");
    assert_eq!(eval(&h, &rx, 1, "window.__choice"), "accept");
    assert!(eval(&h, &rx, 1, "window.__pd").parse::<i32>().unwrap_or(0) >= 1);
}

#[test]
fn dialog_inside_an_iframe_is_rejected() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    // Sourcepoint-style: a fixed container holding an iframe whose
    // document is the dialog; the buttons talk to the parent with
    // postMessage and the parent removes the container.
    let frame_doc = "<html><body style='margin:0;font-family:sans-serif;background:#fff'><p>Nous et nos partenaires utilisons des cookies et des traceurs pour mesurer l&#39;audience.</p><button id='a' onclick=\"parent.postMessage('accept','*')\">Tout accepter</button> <button id='r' onclick=\"parent.postMessage('reject','*')\">Tout refuser</button></body></html>";
    let html = format!(
        "<html><body style='margin:0'>{PROBE}{ARTICLE}
<div id='sp_message_container_1' style='position:fixed;left:100px;top:100px;width:600px;height:220px;z-index:2147483647'>
 <iframe id='sp_message_iframe_1' style='width:600px;height:220px;border:0' srcdoc=\"{}\"></iframe>
</div>
<script>
window.addEventListener('message',function(e){{ window.__choice=e.data; var c=document.getElementById('sp_message_container_1'); if(c) c.remove(); }});
</script></body></html>",
        frame_doc.replace('"', "&quot;")
    );
    load_at(&h, &html, "https://www.journal.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(15)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::Rejected, "{e:?}");
    assert_eq!(e.button, "Tout refuser");
    assert!(e.in_frame);
    assert_eq!(e.vendor.as_deref(), Some("Sourcepoint"));
    assert_eq!(eval(&h, &rx, 1, "window.__choice"), "reject");
    assert_eq!(eval(&h, &rx, 1, "document.getElementById('sp_message_container_1') ? 'present' : 'gone'"), "gone");
}

#[test]
fn dialog_injected_after_load_is_handled() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    let html = format!(
        "<html><body style='margin:0'>{PROBE}{ARTICLE}
<script>
setTimeout(function(){{
  var d=document.createElement('div'); d.id='late';
  d.setAttribute('style','position:fixed;left:0;right:0;bottom:0;padding:14px;background:#eee;z-index:500');
  d.innerHTML='<p>Wir verwenden Cookies, um Inhalte zu personalisieren. Mit Ihrer Einwilligung ...</p><button id=\"ok\">Alle akzeptieren</button> <button id=\"no\">Alle ablehnen</button>';
  document.body.appendChild(d);
  document.getElementById('no').addEventListener('click',function(){{window.__choice='reject';d.remove()}});
  document.getElementById('ok').addEventListener('click',function(){{window.__choice='accept';d.remove()}});
  window.__shown=Date.now();
}},1000);
</script></body></html>"
    );
    let start = Instant::now();
    load_at(&h, &html, "https://zeitung.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(12)).expect("consent event");
    assert!(start.elapsed() >= Duration::from_millis(900), "handled only after it appeared");
    assert_eq!(e.outcome, ConsentOutcome::Rejected, "{e:?}");
    assert_eq!(e.button, "Alle ablehnen");
    assert_eq!(eval(&h, &rx, 1, "window.__choice"), "reject");
    assert_eq!(eval(&h, &rx, 1, "document.getElementById('late') ? 'present' : 'gone'"), "gone");
}

/// First layer: accept + "Paramétrer"; second layer: purpose switches
/// (checked) and "Enregistrer mes choix". Rejecting opens the panel,
/// switches every purpose off and saves.
const TWO_STEP: &str = "<html><body style='margin:0'><script>window.__choice='none';window.__pd=0;document.addEventListener('pointerdown',function(){window.__pd++});</script><h1>Le Quotidien</h1><div style='height:1500px'>article</div>
<div id='layer1' style='position:fixed;left:0;right:0;bottom:0;padding:16px;background:#fff;border-top:1px solid #000;z-index:1000'>
 <p>Avec votre accord, nous utilisons des cookies pour la publicité personnalisée.</p>
 <button id='all'>Tout accepter</button> <button id='prefs'>Paramétrer</button>
</div>
<div id='layer2' role='dialog' style='display:none;position:fixed;left:200px;top:80px;width:500px;padding:16px;background:#fff;border:1px solid #000;z-index:1001'>
 <p>Gérer vos préférences de cookies</p>
 <label><input type='checkbox' id='p1' checked> Publicité personnalisée</label><br>
 <label><input type='checkbox' id='p2' checked> Mesure d'audience</label><br>
 <label><input type='checkbox' id='p0' checked disabled> Cookies strictement nécessaires</label><br>
 <button id='save'>Enregistrer mes choix</button>
</div>
<script>
document.getElementById('all').onclick=function(){window.__choice='accept';document.getElementById('layer1').remove()};
document.getElementById('prefs').onclick=function(){document.getElementById('layer1').style.display='none';document.getElementById('layer2').style.display='block'};
document.getElementById('save').onclick=function(){window.__choice='saved:'+document.getElementById('p1').checked+','+document.getElementById('p2').checked;document.getElementById('layer2').remove()};
</script></body></html>";

#[test]
fn two_step_reject_through_the_settings_panel() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    load_at(&h, TWO_STEP, "https://www.quotidien.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(12)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::Rejected, "{e:?}");
    assert_eq!(e.button, "Enregistrer mes choix");
    assert_eq!(e.steps.first().map(|s| s.as_str()), Some("Paramétrer"), "{:?}", e.steps);
    assert_eq!(eval(&h, &rx, 1, "window.__choice"), "saved:false,false");
    assert_eq!(eval(&h, &rx, 1, "document.getElementById('layer2') ? 'present' : 'gone'"), "gone");
}

#[test]
fn accept_mode_never_opens_the_settings_panel() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Accept));
    load_at(&h, TWO_STEP, "https://www.quotidien.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(10)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::Accepted);
    assert_eq!(e.button, "Tout accepter");
    assert_eq!(eval(&h, &rx, 1, "window.__choice"), "accept");
}

#[test]
fn non_cookie_modal_is_left_alone() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    let html = format!(
        "<html><body style='margin:0'>{PROBE}{ARTICLE}
<div id='nl' role='dialog' aria-modal='true' style='position:fixed;left:200px;top:100px;width:500px;padding:20px;background:#fff;border:1px solid #000;z-index:1000'>
 <h2>Subscribe to our newsletter</h2><p>Get the best stories every morning. We respect your privacy.</p>
 <input type='email' placeholder='you@example.com'> <button id='sub'>Subscribe</button> <button id='no'>No thanks</button>
</div>
<div id='age' style='position:fixed;left:0;right:0;bottom:0;padding:12px;background:#000;color:#fff;z-index:900'>
 <p>You must be of legal age to enter this section.</p><button id='adult'>I am over 18</button> <button id='leave'>Leave</button>
</div>
<script>
['sub','no','adult','leave'].forEach(function(id){{document.getElementById(id).addEventListener('click',function(){{window.__choice=id}})}});
</script></body></html>"
    );
    load_at(&h, &html, "https://blog.example.test/");
    assert!(consent_event(&rx, 1, Duration::from_secs(3)).is_none(), "no consent event for a newsletter modal / age gate");
    assert_eq!(eval(&h, &rx, 1, "window.__choice + ',' + window.__pd"), "none,0");
}

#[test]
fn reject_mode_without_reject_option_is_recorded_and_left() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    let html = format!(
        "<html><body style='margin:0'>{PROBE}{ARTICLE}
<div id='banner' style='position:fixed;left:0;right:0;bottom:0;padding:12px;background:#ddd;z-index:100'>
 <p>This site uses cookies. By continuing you agree. <a href='/cookies'>Learn more</a></p><button id='ok'>Accept</button>
</div>
<script>document.getElementById('ok').onclick=function(){{window.__choice='accept'}};</script></body></html>"
    );
    load_at(&h, &html, "https://shop.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(10)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::NoButton);
    assert!(e.button.is_empty());
    assert!(e.summary().contains("no reject option"), "{}", e.summary());
    assert_eq!(eval(&h, &rx, 1, "window.__choice + ',' + (document.getElementById('banner') ? 'present' : 'gone')"), "none,present");
}

#[test]
fn banners_in_several_languages() {
    let cases = [
        ("Questo sito utilizza cookie tecnici e di profilazione.", "Accetta tutto", "Rifiuta tutto", "it"),
        ("Używamy plików cookie, aby zapewnić najlepszą jakość.", "Zaakceptuj wszystko", "Odrzuć wszystko", "pl"),
        ("Käytämme evästeitä sisällön personointiin.", "Hyväksy kaikki", "Hylkää kaikki", "fi"),
        ("Χρησιμοποιούμε cookies για να βελτιώσουμε την εμπειρία σας.", "Αποδοχή όλων", "Απόρριψη όλων", "el"),
        ("Wij gebruiken cookies om onze website te verbeteren.", "Alles accepteren", "Alles weigeren", "nl"),
        ("Utilitzem galetes pròpies i de tercers.", "Acceptar-ho tot", "Rebutjar", "ca"),
    ];
    let (sink, rx) = sink();
    let handles: Vec<ContentHandle> = cases
        .iter()
        .enumerate()
        .map(|(i, (text, acc, rej, lang))| {
            let h = spawn_mode(10 + i as u64, &sink, mode(if i % 2 == 0 { ConsentMode::Reject } else { ConsentMode::Accept }));
            load_at(&h, &bottom_bar(text, acc, rej), &format!("https://{lang}.example.test/"));
            h
        })
        .collect();
    // Collect the events of every tab (they arrive in any order).
    let mut events: std::collections::HashMap<u64, ConsentEvent> = std::collections::HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while events.len() < cases.len() && Instant::now() < deadline {
        if let Ok((_, tab, FromContent::Consent(e))) = rx.recv_timeout(Duration::from_millis(200)) {
            events.insert(tab, e);
        }
    }
    for (i, (_, acc, rej, lang)) in cases.iter().enumerate() {
        let tab = 10 + i as u64;
        let e = events.remove(&tab).unwrap_or_else(|| panic!("{lang}: no consent event"));
        let (want, button, choice) = if i % 2 == 0 { (ConsentOutcome::Rejected, rej, "reject") } else { (ConsentOutcome::Accepted, acc, "accept") };
        assert_eq!(e.outcome, want, "{lang}: {e:?}");
        assert_eq!(&e.button, button, "{lang}");
        assert_eq!(eval(&handles[i], &rx, tab, "window.__choice"), choice, "{lang}");
    }
}

#[test]
fn off_mode_and_never_rule_leave_dialogs_alone() {
    let (sink, rx) = sink();
    let off = spawn_mode(1, &sink, mode(ConsentMode::Off));
    load_at(&off, &bottom_bar("We use cookies.", "Accept all", "Reject all"), "https://a.example.test/");
    let mut never = ConsentPolicy { mode: ConsentMode::Accept, ..Default::default() };
    never.sites.insert("b.example.test".into(), SiteRule::Never);
    let n = spawn_mode(2, &sink, never.clone());
    load_at(&n, &bottom_bar("We use cookies.", "Accept all", "Reject all"), "https://www.b.example.test/");
    // A per-site "always reject" wins over the global accept.
    let mut rule = never;
    rule.sites.insert("c.example.test".into(), SiteRule::Reject);
    let r = spawn_mode(3, &sink, rule);
    load_at(&r, &bottom_bar("We use cookies.", "Accept all", "Reject all"), "https://c.example.test/");
    let e = consent_event(&rx, 3, Duration::from_secs(10)).expect("site rule");
    assert_eq!(e.outcome, ConsentOutcome::Rejected);
    assert!(consent_event(&rx, 1, Duration::from_millis(1500)).is_none());
    assert!(consent_event(&rx, 2, Duration::from_millis(500)).is_none());
    assert_eq!(eval(&off, &rx, 1, "window.__choice"), "none");
    assert_eq!(eval(&n, &rx, 2, "window.__choice"), "none");
    // Switching the mode on applies to the page on screen.
    let mut s = config(ViewSize::new(1000, 700, 1.0)).settings;
    s.consent = mode(ConsentMode::Reject);
    off.send(ToContent::Settings(s));
    let e = consent_event(&rx, 1, Duration::from_secs(10)).expect("after enabling");
    assert_eq!(e.outcome, ConsentOutcome::Rejected);
    assert_eq!(eval(&off, &rx, 1, "window.__choice"), "reject");
}

#[test]
fn consent_page_with_form_submits_the_reject_button() {
    // A dedicated consent page (like consent.google.com): the document is
    // the dialog and the buttons submit forms.
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    let html = "<html><body><h1>Before you continue</h1><p>We use cookies and data to deliver and maintain our services and measure audience engagement.</p>
<form action='https://consent.example.test/save' method='POST'><input type='hidden' name='set_eom' value='true'><button>Reject all</button></form>
<form action='https://consent.example.test/save' method='POST'><input type='hidden' name='set_eom' value='false'><button>Accept all</button></form>
<a href='https://consent.example.test/more'>More options</a></body></html>";
    load_at(&h, html, "https://consent.example.test/?continue=https://www.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(10)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::Rejected, "{e:?}");
    assert_eq!(e.button, "Reject all");
    let (_, nav) = wait_for(&rx, 1, Duration::from_secs(5), |m| matches!(m, FromContent::Navigate { .. })).expect("form submission");
    let FromContent::Navigate { request, .. } = nav else { unreachable!() };
    assert_eq!(request.method, "POST");
    assert!(request.url.to_string().starts_with("https://consent.example.test/save"));
    let body = request.body.as_ref().map(|b| String::from_utf8_lossy(&b.data).to_string()).unwrap_or_default();
    assert!(body.contains("set_eom=true"), "{body}");
}

#[test]
fn settings_links_to_other_pages_are_not_followed() {
    // BBC-style: "No, take me to settings" is a link to a settings page;
    // rejecting must not navigate away.
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    let html = format!(
        "<html><body style='margin:0'>{PROBE}{ARTICLE}
<div id='cookiePrompt' style='position:fixed;left:0;right:0;top:0;padding:12px;background:#fff;z-index:100'>
 <p>Let us know you agree to cookies. We use cookies to give you the best online experience.</p>
 <button id='yes'>Yes, I agree</button> <a href='https://news.example.test/usingthebbc/cookies/'>No, take me to settings</a>
</div></body></html>"
    );
    load_at(&h, &html, "https://news.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(10)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::NoButton, "{e:?}");
    assert!(e.steps.is_empty(), "{:?}", e.steps);
    assert!(wait_for(&rx, 1, Duration::from_millis(800), |m| matches!(m, FromContent::Navigate { .. })).is_none(), "no navigation");
}

#[test]
fn settings_panel_that_never_shows_is_left_alone() {
    let (sink, rx) = sink();
    let h = spawn_mode(1, &sink, mode(ConsentMode::Reject));
    let html = format!(
        "<html><body style='margin:0'>{PROBE}{ARTICLE}
<div id='b' style='position:fixed;left:0;right:0;bottom:0;padding:12px;background:#fff;z-index:100'>
 <p>We and our partners use cookies.</p><button id='ok'>Consent and continue</button> <button id='prefs'>Preferences</button>
</div>
<script>document.getElementById('prefs').onclick=function(){{document.getElementById('b').remove()}};</script></body></html>"
    );
    load_at(&h, &html, "https://mag.example.test/");
    let e = consent_event(&rx, 1, Duration::from_secs(15)).expect("consent event");
    assert_eq!(e.outcome, ConsentOutcome::NoButton, "{e:?}");
    assert_eq!(e.steps, vec!["Preferences".to_string()]);
    assert_eq!(eval(&h, &rx, 1, "window.__choice"), "none");
}
