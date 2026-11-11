//! Cookie-consent (GDPR / ePrivacy) dialogs: detection, button matching
//! and clicking, shared by the desktop browser (automatic accept/reject)
//! and the agent interface (its `cookies` shortcut).
//!
//! * Detection relies on structure and text: a box drawn over the page
//!   (fixed/sticky bar, modal, dialog role, a raised iframe) that is
//!   actually hit at some viewport point and whose text talks about
//!   cookies/consent in one of the major European languages. Known CMP
//!   containers (OneTrust, Cookiebot, Didomi, Quantcast, Usercentrics,
//!   Sourcepoint, ...) are a fast path, not a requirement. Dialogs inside
//!   iframes and shadow trees are found too.
//! * Buttons are matched by accessible name ([`classify_button`]):
//!   accept-all, accept, reject, settings, save. Anything that looks like a
//!   login, subscription, paywall or age gate is never a candidate, and
//!   only controls inside the dialog are considered.
//! * Clicks go through the real pointer path ([`Engine::click_at`]) at a
//!   point where the button is topmost, so the site's handlers run as for
//!   a user (synthetic dispatch is the fallback when it cannot be hit).
//! * [`ConsentDriver`] runs the whole flow for one page load (wait for the
//!   dialog, click, open the settings panel when rejecting needs it,
//!   verify the dialog went away, give up after a couple of attempts).

use std::time::{Duration, Instant};

use rabbithole_dom::{Document, NodeId};
use rabbithole_style::{Display, Position, Visibility};

use crate::pointer::{ClickAction, Modifiers};
use crate::scripting::ScriptOutcome;
use crate::{Engine, Page};

/// What to do with consent dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Choice {
    Accept,
    Reject,
}

/// What a consent-dialog control does, judged by its accessible name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// "Accept all", "Alle akzeptieren", "Tout accepter", ...
    AcceptAll,
    /// "Accept", "Agree", "OK", ...
    Accept,
    /// "Reject all", "Only necessary", "Rechazar", ...
    Reject,
    /// Opens the detailed panel: "Settings", "Manage options", ...
    Settings,
    /// Saves the current selection in the detailed panel.
    Save,
}

/// Lower-case, fold accents (Latin and Greek), turn punctuation into
/// spaces (keeping apostrophes), `&` into "and", collapse whitespace.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for ch in s.chars().flat_map(|c| c.to_lowercase()) {
        let mapped: &str = match ch {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ą' | 'ă' | 'ā' => "a",
            'æ' => "ae",
            'ç' | 'ć' | 'č' => "c",
            'ď' => "d",
            'é' | 'è' | 'ê' | 'ë' | 'ę' | 'ě' | 'ē' => "e",
            'í' | 'ì' | 'î' | 'ï' | 'ī' => "i",
            'ł' => "l",
            'ñ' | 'ń' | 'ň' => "n",
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ő' | 'ō' => "o",
            'ř' => "r",
            'ś' | 'š' | 'ș' | 'ş' => "s",
            'ß' => "ss",
            'ť' | 'ț' | 'ţ' => "t",
            'ú' | 'ù' | 'û' | 'ü' | 'ů' | 'ű' | 'ū' => "u",
            'ý' | 'ÿ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'ά' => "α",
            'έ' => "ε",
            'ή' => "η",
            'ί' | 'ϊ' | 'ΐ' => "ι",
            'ό' => "ο",
            'ύ' | 'ϋ' | 'ΰ' => "υ",
            'ώ' => "ω",
            'ς' => "σ",
            '’' | '‘' | '`' | '\'' => "'",
            '&' => " and ",
            c if c.is_alphanumeric() => {
                out.push(c);
                space = false;
                continue;
            }
            _ => " ",
        };
        if mapped == " " || mapped == " and " {
            if mapped == " and " {
                if !space {
                    out.push(' ');
                }
                out.push_str("and");
                space = false;
            }
            if !space {
                out.push(' ');
                space = true;
            }
            continue;
        }
        out.push_str(mapped);
        space = false;
    }
    let t = out.trim_end();
    t.to_string()
}

/// Does `folded` contain `phrase` as whole words?
fn has_phrase(folded: &str, phrase: &str) -> bool {
    if phrase.is_empty() || folded.len() < phrase.len() {
        return false;
    }
    let mut start = 0;
    while let Some(i) = folded[start..].find(phrase) {
        let a = start + i;
        let b = a + phrase.len();
        let before_ok = a == 0 || folded.as_bytes()[a - 1] == b' ';
        let after_ok = b == folded.len() || folded.as_bytes()[b] == b' ';
        if before_ok && after_ok {
            return true;
        }
        start = a + 1;
        while !folded.is_char_boundary(start) {
            start += 1;
        }
        if start >= folded.len() {
            break;
        }
    }
    false
}

fn any_phrase(folded: &str, list: &[&str]) -> bool {
    list.iter().any(|p| has_phrase(folded, p))
}

/// Words that make a text a cookie/consent notice (folded, substring
/// match): es, en, fr, de, it, pt, nl, pl, sv, da, no, fi, cs, el, ro, hu,
/// ca, eu, gl.
const CONSENT_WORDS: &[&str] = &[
    "cookie", "consent", "gdpr", "rgpd", "dsgvo", "ePrivacy", "tcf", "consentimiento", "consentement", "einwilligung", "consenso", "consentimento", "toestemming",
    "zgod", "samtycke", "samtykke", "suostumu", "souhlas", "συγκαταθεσ", "consimtamant", "consimtamint", "hozzajarul", "consentiment", "baimen", "galleta", "galetes",
    "ciasteczk", "kakor", "evaste", "suti", "informasjonskapsl", "traceurs", "temoins", "rastreadores", "trackers",
];

/// Privacy words (supporting evidence only).
const PRIVACY_WORDS: &[&str] = &[
    "privacy", "privacidad", "confidentialite", "datenschutz", "riservatezza", "privacidade", "prywatnosc", "integritet", "privatliv", "personvern", "yksityisyy",
    "ochrana osobnich", "απορρητ", "confidentialitate", "adatvedelm", "privadesa", "pribatutasun",
];

/// Does the text look like a cookie/consent notice?
pub fn is_consent_text(text: &str) -> bool {
    let f = fold(text);
    CONSENT_WORDS.iter().any(|w| f.contains(&w.to_lowercase()))
}

/// Does the text mention privacy (weaker than [`is_consent_text`])?
pub fn mentions_privacy(text: &str) -> bool {
    let f = fold(text);
    PRIVACY_WORDS.iter().any(|w| f.contains(w))
}

/// Names that are never part of a consent decision (login, subscription,
/// paywall, age gate, policy links).
const EXCLUDE: &[&str] = &[
    "login", "log in", "sign in", "sign up", "subscribe", "subscription", "newsletter", "register", "create account", "iniciar sesion", "suscribirse", "suscribete",
    "registrarse", "suscripcion", "s'abonner", "abonnez vous", "connexion", "se connecter", "abonnieren", "abonnement", "abo", "anmelden", "einloggen", "registrieren",
    "abbonati", "abbonamento", "accedi", "assinar", "assinatura", "entrar", "inloggen", "abonneren", "zaloguj", "subskrypcja", "logga in", "prenumerera", "log ind",
    "logg inn", "kirjaudu", "tilaa", "prihlasit", "predplatne", "συνδεση", "autentificare", "bejelentkezes", "elofizetes", "iniciar sessio", "subscriu", "saioa hasi",
    "years", "anos", "jahre", "ans", "anni", "jaar", "lat", "ar", "år", "vuotta", "let", "ετων", "ani", "ev", "eves", "urte", "18", "21", "buy", "pay", "comprar",
    "pagar", "kaufen", "bezahlen", "acheter", "payer", "acquista", "kopen", "donate", "contact", "privacy policy", "cookie policy", "politica de cookies",
    "politica de privacidad", "datenschutzerklarung", "impressum", "politique de confidentialite", "learn more", "read more", "more information", "mas informacion",
    "en savoir plus", "mehr erfahren", "mehr informationen", "per saperne di piu", "saiba mais", "lees meer", "vendors", "partners", "partenaires", "socios",
    "proveedores", "partner", "fornitori", "parceiros", "partnerzy", "partnere", "kumppanit", "partneri", "συνεργατες",
];

const REJECT: &[&str] = &[
    // en
    "reject", "reject all", "decline", "decline all", "deny", "deny all", "refuse", "refuse all", "disagree", "do not agree", "i do not agree", "do not accept",
    "don't accept", "dont accept", "i do not accept", "only necessary", "necessary only", "only essential", "essential only", "only required", "required only",
    "strictly necessary only", "necessary cookies only", "essential cookies only", "use necessary cookies only", "accept necessary", "accept essential",
    "accept only necessary", "accept only essential", "allow necessary", "allow essential", "allow only necessary", "continue without accepting",
    "continue without agreeing", "continue without consent", "no thanks", "no thank you", "reject cookies",
    // es / gl / ca
    "rechazar", "rechazar todo", "rechazar todas", "rechazo", "denegar", "no acepto", "no aceptar", "solo necesarias", "solo las necesarias", "solo esenciales",
    "solo cookies necesarias", "solamente necesarias", "unicamente necesarias", "continuar sin aceptar", "aceptar necesarias", "aceptar solo necesarias", "rexeitar",
    "rexeitar todo", "rexeitar todas", "so necesarias", "so as necesarias", "rebutjar", "rebutja", "rebutjar ho tot", "rebutjar totes", "nomes necessaries",
    "nomes les necessaries", "continuar sense acceptar",
    // fr
    "refuser", "tout refuser", "refuser tout", "je refuse", "rejeter", "tout rejeter", "continuer sans accepter", "uniquement necessaires", "necessaires uniquement",
    "strictement necessaires", "non merci", "refuser et fermer",
    // de
    "ablehnen", "alle ablehnen", "alles ablehnen", "nur notwendige", "nur erforderliche", "nur essenzielle", "nur technisch notwendige", "nur notwendige cookies",
    "verweigern", "nicht zustimmen", "nicht einverstanden", "ohne zustimmung fortfahren", "weiter ohne einwilligung", "zustimmung verweigern", "nein danke",
    // it
    "rifiuta", "rifiuta tutto", "rifiuta tutti", "rifiuto", "non accetto", "solo necessari", "solo cookie necessari", "continua senza accettare", "nega", "rifiuta e chiudi",
    // pt
    "rejeitar", "rejeitar tudo", "rejeitar todos", "recusar", "recusar tudo", "recusar todos", "nao aceito", "nao aceitar", "apenas necessarios", "apenas essenciais",
    "somente necessarios", "continuar sem aceitar",
    // nl
    "weigeren", "alles weigeren", "alle weigeren", "afwijzen", "alles afwijzen", "alle afwijzen", "alleen noodzakelijk", "alleen noodzakelijke", "alleen functionele",
    "niet akkoord", "weiger", "weiger alles",
    // pl
    "odrzuc", "odrzuc wszystko", "odrzuc wszystkie", "nie zgadzam sie", "tylko niezbedne", "tylko wymagane", "odmow", "odmawiam",
    // sv / da / no
    "avvisa", "avvisa alla", "neka", "neka alla", "avboj", "avboj alla", "endast nodvandiga", "bara nodvandiga", "afvis", "afvis alle", "afsla", "afsla alle",
    "kun nodvendige", "avvis", "avvis alle", "avsla", "avsla alle", "bare nodvendige", "kun nodvendige informasjonskapsler",
    // fi
    "hylkaa", "hylkaa kaikki", "kiella", "kiella kaikki", "kieltaydy", "vain valttamattomat", "vain pakolliset", "vain valttamattomat evasteet",
    // cs
    "odmitnout", "odmitnout vse", "odmitnout vsechny", "pouze nezbytne", "jen nezbytne", "nesouhlasim",
    // el
    "απορριψη", "απορριψη ολων", "αρνουμαι", "μονο απαραιτητα", "μονο τα απαραιτητα", "απορριπτω",
    // ro
    "respinge", "respinge tot", "respinge toate", "refuza", "refuz", "refuza tot", "doar necesare", "numai necesare", "nu accept", "resping",
    // hu
    "elutasit", "elutasitom", "osszes elutasitasa", "elutasitas", "csak a szukseges", "csak szukseges", "nem fogadom el", "csak a szukseges sutik",
    // eu
    "baztertu", "guztiak baztertu", "baztertu guztiak", "ukatu", "beharrezkoak soilik", "beharrezkoak bakarrik",
];

const ACCEPT_ALL: &[&str] = &[
    // en
    "accept all", "accept all cookies", "allow all", "allow all cookies", "agree to all", "accept and close", "agree and close", "agree and continue",
    "accept and continue", "i accept all", "yes i accept", "yes i agree", "consent to all", "accept cookies", "allow cookies", "enable all", "accept all and close",
    "accept all and continue", "agree to all cookies", "i accept cookies", "accept and proceed",
    // es / gl / ca
    "aceptar todo", "aceptar todas", "aceptar todas las cookies", "aceptar y cerrar", "aceptar y continuar", "acepto todas", "permitir todas", "permitir todo",
    "aceptar cookies", "aceptar las cookies", "aceptar todos", "aceptalas todas", "acepto las cookies", "acceptar tot", "acceptar totes", "accepta ho tot",
    "acceptar ho tot", "acceptar i tancar", "permetre totes", "acceptar totes les galetes",
    // fr
    "tout accepter", "accepter tout", "accepter et fermer", "accepter et continuer", "accepter les cookies", "autoriser tous", "autoriser tout", "tout autoriser",
    "j'accepte tout", "accepter tous les cookies", "accepter tous",
    // de
    "alle akzeptieren", "alles akzeptieren", "alle annehmen", "alle zulassen", "alle cookies akzeptieren", "akzeptieren und schliessen", "akzeptieren und weiter",
    "zustimmen und weiter", "einverstanden und weiter", "alle erlauben", "allen zustimmen", "alle cookies zulassen", "alle cookies erlauben", "alles erlauben",
    "akzeptieren und schließen", "alle bestatigen",
    // it
    "accetta tutto", "accetta tutti", "accetta e chiudi", "accetta e continua", "accetto tutto", "consenti tutti", "accetta i cookie", "accetta tutti i cookie",
    "accetta e prosegui",
    // pt
    "aceitar tudo", "aceitar todos", "aceitar e fechar", "aceitar e continuar", "permitir todos", "aceitar cookies", "aceitar todos os cookies", "aceito todos",
    // nl
    "alles accepteren", "alle accepteren", "accepteer alles", "alle cookies accepteren", "alles toestaan", "akkoord en doorgaan", "accepteren en sluiten",
    "accepteer alle", "accepteer alle cookies", "alle toestaan", "ja ik ga akkoord",
    // pl
    "akceptuj wszystko", "zaakceptuj wszystko", "akceptuj wszystkie", "zaakceptuj wszystkie", "zezwol na wszystkie", "zgadzam sie na wszystkie",
    "przejdz do serwisu", "akceptuje wszystkie", "akceptuje i przechodze do serwisu",
    // sv / da / no
    "acceptera alla", "godkann alla", "tillat alla", "acceptera alla cookies", "godkann alla cookies", "accepter alle", "tillad alle", "accepter alle cookies",
    "godkend alle", "godta alle", "aksepter alle", "godkjenn alle", "tillat alle informasjonskapsler",
    // fi
    "hyvaksy kaikki", "salli kaikki", "hyvaksy evasteet", "hyvaksy kaikki evasteet",
    // cs
    "prijmout vse", "povolit vse", "prijmout vsechny", "souhlasim se vsemi", "prijmout vsechny cookies", "povolit vsechny",
    // el
    "αποδοχη ολων", "αποδοχη ολων των cookies", "συμφωνω με ολα", "αποδεχομαι ολα", "αποδοχη cookies",
    // ro
    "accepta tot", "accepta toate", "accept toate", "permite toate", "sunt de acord cu toate", "accepta toate cookie urile", "accept tot",
    // hu
    "osszes elfogadasa", "mindet elfogadom", "elfogadom mindet", "osszes cookie elfogadasa", "mindent elfogadok", "mindegyik elfogadasa", "osszes suti elfogadasa",
    // eu
    "onartu guztiak", "guztiak onartu", "denak onartu", "onartu denak", "onartu cookieak", "cookie guztiak onartu",
];

/// "Save the selection" names that also contain accept words.
const SAVE_SELECTION: &[&str] = &[
    "accept selection", "allow selection", "accept selected", "allow selected", "accept current selection", "aceptar seleccion", "permitir seleccion",
    "aceptar seleccionadas", "accepter la selection", "auswahl akzeptieren", "auswahl erlauben", "auswahl zulassen", "accetta selezionati", "accetta selezione",
    "aceitar selecao", "selectie toestaan", "selectie accepteren", "zezwol na wybrane", "akceptuj wybrane", "tillat urval", "tillad valgte", "tillat valgte",
    "salli valitut", "povolit vybrane", "αποδοχη επιλεγμενων", "permite selectia", "kivalasztottak elfogadasa", "acceptar seleccio", "onartu hautapena",
];

const SAVE: &[&str] = &[
    "save", "save settings", "save preferences", "save choices", "save my choices", "save and exit", "save and close", "confirm choices", "confirm my choices",
    "confirm selection", "save selection", "guardar", "guardar configuracion", "guardar preferencias", "guardar y salir", "guardar y cerrar", "confirmar seleccion",
    "confirmar mis preferencias", "confirmar preferencias", "enregistrer", "enregistrer mes choix", "valider", "valider mes choix", "confirmer mes choix",
    "speichern", "auswahl speichern", "auswahl bestatigen", "einstellungen speichern", "speichern und schliessen", "salva", "salva ed esci", "salva le impostazioni",
    "conferma le scelte", "salva le preferenze", "salvar", "confirmar escolhas", "guardar preferencias", "salvar preferencias", "opslaan", "voorkeuren opslaan",
    "keuze opslaan", "instellingen opslaan", "zapisz", "zapisz ustawienia", "potwierdz wybor", "zapisz i zamknij", "zapisz wybor", "spara", "spara installningar",
    "bekrafta val", "spara val", "gem", "gem indstillinger", "bekraeft valg", "gem valg", "lagre", "lagre innstillinger", "bekreft valg", "tallenna",
    "tallenna asetukset", "vahvista valinnat", "ulozit", "ulozit nastaveni", "potvrdit vyber", "αποθηκευση", "επιβεβαιωση επιλογων", "αποθηκευση ρυθμισεων",
    "salveaza", "salveaza preferintele", "confirma alegerile", "salveaza setarile", "mentes", "beallitasok mentese", "valasztas megerositese", "kivalasztas mentese",
    "desar", "desa", "desar preferencies", "confirmar seleccio", "gorde", "gorde hobespenak", "berretsi", "gardar", "gardar preferencias", "confirmar escolla",
];

const SETTINGS: &[&str] = &[
    "settings", "cookie settings", "cookies settings", "manage", "manage options", "manage preferences", "manage cookies", "manage settings", "customize",
    "customise", "preferences", "cookie preferences", "more options", "options", "configure", "set preferences", "show purposes", "personalize", "personalise",
    "configurar", "configuracion", "personalizar", "gestionar", "ajustes", "preferencias", "mas opciones", "configurar cookies", "administrar", "gestionar cookies",
    "parametrer", "parametres", "personnaliser", "gerer", "gerer les cookies", "plus d'options", "gerer mes preferences", "parametrer les cookies", "einstellungen",
    "anpassen", "verwalten", "mehr optionen", "individuelle einstellungen", "praferenzen", "cookie einstellungen", "einstellungen verwalten", "impostazioni",
    "personalizza", "gestisci", "preferenze", "piu opzioni", "gestisci opzioni", "gestisci preferenze", "configuracoes", "definicoes", "gerir", "gerenciar",
    "mais opcoes", "gerir preferencias", "instellingen", "aanpassen", "beheren", "voorkeuren", "meer opties", "cookie instellingen", "ustawienia", "dostosuj",
    "zarzadzaj", "preferencje", "wiecej opcji", "ustawienia cookies", "installningar", "anpassa", "hantera", "mer alternativ", "indstillinger", "tilpas",
    "administrer", "flere muligheder", "innstillinger", "tilpass", "flere valg", "asetukset", "mukauta", "hallinnoi", "lisaa vaihtoehtoja", "evasteasetukset",
    "nastaveni", "prizpusobit", "spravovat", "vice moznosti", "ρυθμισεις", "διαχειριση", "προσαρμογη", "περισσοτερες επιλογες", "setari", "personalizeaza",
    "gestioneaza", "preferinte", "mai multe optiuni", "beallitasok", "testreszabas", "kezeles", "tovabbi lehetosegek", "suti beallitasok", "configuracio",
    "personalitzar", "preferencies", "mes opcions", "ezarpenak", "konfiguratu", "pertsonalizatu", "kudeatu", "hobespenak", "xestionar", "mais opcions",
];

/// Short acknowledgements accepted only as the whole name (notice-only
/// banners).
const ACK_EXACT: &[&str] = &[
    "ok", "okay", "ok thanks", "got it", "understood", "i understand", "entendido", "vale", "de acuerdo", "d'accord", "compris", "j'ai compris", "verstanden",
    "ok verstanden", "alles klar", "capito", "ho capito", "entendi", "de acordo", "begrepen", "prima", "rozumiem", "jag forstar", "forstaet", "jeg forstar",
    "selva", "rozumim", "καταλαβα", "am inteles", "ertem", "rendben", "entesos", "d'acord", "ados", "ulertu dut", "conforme",
];

/// Single words/phrases meaning "accept" (whole-word match).
const ACCEPT: &[&str] = &[
    "accept", "i accept", "agree", "i agree", "allow", "consent", "aceptar", "acepto", "permitir", "accepter", "j'accepte", "autoriser", "akzeptieren", "zustimmen",
    "einverstanden", "annehmen", "erlauben", "zulassen", "accetta", "accetto", "acconsento", "consenti", "aceitar", "aceito", "concordo", "permitir", "accepteren",
    "akkoord", "accepteer", "toestaan", "akceptuj", "zaakceptuj", "akceptuje", "zgadzam sie", "zgoda", "acceptera", "godkann", "accepter", "godkend", "tillad",
    "godta", "aksepter", "godkjenn", "hyvaksy", "hyvaksyn", "salli", "prijmout", "souhlasim", "povolit", "αποδοχη", "αποδεχομαι", "συμφωνω", "accepta",
    "sunt de acord", "elfogad", "elfogadom", "elfogadas", "acceptar", "accepto", "accepta", "onartu", "onartzen dut", "ados nago", "acepto as cookies",
];

/// Classify a control of a consent dialog by its accessible name.
pub fn classify_button(name: &str) -> Option<ButtonKind> {
    let f = fold(name);
    if f.is_empty() || f.chars().count() > 60 || f.split(' ').count() > 9 {
        return None;
    }
    if any_phrase(&f, EXCLUDE) {
        return None;
    }
    if any_phrase(&f, REJECT) {
        return Some(ButtonKind::Reject);
    }
    if any_phrase(&f, SAVE_SELECTION) {
        return Some(ButtonKind::Save);
    }
    if any_phrase(&f, ACCEPT_ALL) {
        return Some(ButtonKind::AcceptAll);
    }
    if any_phrase(&f, SAVE) {
        return Some(ButtonKind::Save);
    }
    if any_phrase(&f, SETTINGS) {
        return Some(ButtonKind::Settings);
    }
    if ACK_EXACT.contains(&f.as_str()) || any_phrase(&f, ACCEPT) {
        return Some(ButtonKind::Accept);
    }
    None
}

/// Known consent-management containers (fast path): id or class prefix.
const VENDORS: &[(&str, &str)] = &[
    ("#onetrust-banner-sdk", "OneTrust"),
    ("#onetrust-pc-sdk", "OneTrust"),
    ("#onetrust-consent-sdk", "OneTrust"),
    ("#CybotCookiebotDialog", "Cookiebot"),
    ("#didomi-host", "Didomi"),
    ("#didomi-popup", "Didomi"),
    ("#didomi-notice", "Didomi"),
    ("#qc-cmp2-container", "Quantcast"),
    ("#qc-cmp2-ui", "Quantcast"),
    (".qc-cmp2-container", "Quantcast"),
    ("#usercentrics-root", "Usercentrics"),
    ("#usercentrics-cmp-ui", "Usercentrics"),
    ("#uc-banner", "Usercentrics"),
    ("^sp_message_container_", "Sourcepoint"),
    ("#cmpbox", "consentmanager"),
    ("#cmpwrapper", "consentmanager"),
    (".fc-consent-root", "Google Funding Choices"),
    ("#truste-consent-track", "TrustArc"),
    ("#truste-consent-content", "TrustArc"),
    ("#cookie-law-info-bar", "CookieYes"),
    (".cky-consent-container", "CookieYes"),
    (".cc-window", "Cookie Consent"),
    ("#cc-main", "CookieConsent"),
    ("#cmplz-cookiebanner-container", "Complianz"),
    ("#BorlabsCookieBox", "Borlabs"),
    ("#iubenda-cs-banner", "iubenda"),
    ("#axeptio_overlay", "Axeptio"),
    ("#tarteaucitronRoot", "tarteaucitron"),
    ("#CookieConsent", "Cookie consent"),
    (".osano-cm-window", "Osano"),
    ("#klaro", "Klaro"),
    ("#gdpr-consent-tool-wrapper", "Admiral"),
    ("#cookiescript_injected", "CookieScript"),
    ("#sd-cmp", "Sirdata"),
    ("#appconsent", "AppConsent"),
    ("#pubtech-cmp", "Pubtech"),
    ("#lgpd-cookie", "generic"),
];

fn vendor_of(doc: &Document, n: NodeId) -> Option<&'static str> {
    let el = doc.element(n)?;
    let id = el.attr("id").unwrap_or("");
    let class = el.attr("class").unwrap_or("");
    for (sel, name) in VENDORS {
        if let Some(i) = sel.strip_prefix('#') {
            if id == i {
                return Some(name);
            }
        } else if let Some(c) = sel.strip_prefix('.') {
            if class.split_whitespace().any(|x| x == c) {
                return Some(name);
            }
        } else if let Some(p) = sel.strip_prefix('^') {
            if id.starts_with(p) {
                return Some(name);
            }
        }
    }
    None
}

/// Elements whose id/class hints at a consent banner (static banners at
/// the top or bottom of the page that are not raised).
fn has_consent_hint(doc: &Document, n: NodeId) -> bool {
    let Some(el) = doc.element(n) else { return false };
    let s = format!("{} {}", el.attr("id").unwrap_or(""), el.attr("class").unwrap_or("")).to_ascii_lowercase();
    ["cookie", "consent", "gdpr", "cmp-", "cmp_", "privacy-banner", "rgpd"].iter().any(|w| s.contains(w))
}

// ------------------------------------------------------------- the tree

/// Element children in the flat tree (shadow trees included).
fn kids(doc: &Document, n: NodeId) -> Vec<NodeId> {
    doc.flat_children(n)
}

/// Is `a` an inclusive flat-tree ancestor of `b`?
fn contains(doc: &Document, a: NodeId, b: NodeId) -> bool {
    let mut cur = Some(b);
    while let Some(c) = cur {
        if c == a {
            return true;
        }
        cur = doc.flat_parent(c).or_else(|| doc.shadow_host(c));
    }
    false
}

fn skip_tag(doc: &Document, n: NodeId) -> bool {
    matches!(doc.tag(n), Some("script" | "style" | "noscript" | "template" | "head" | "svg"))
}

/// Text of the flat subtree (bounded to `limit` bytes).
fn flat_text(doc: &Document, n: NodeId, limit: usize) -> String {
    fn walk(doc: &Document, n: NodeId, out: &mut String, limit: usize) {
        if out.len() >= limit {
            return;
        }
        if let Some(t) = doc.text(n) {
            out.push_str(t);
            out.push(' ');
            return;
        }
        if skip_tag(doc, n) {
            return;
        }
        for c in kids(doc, n) {
            walk(doc, c, out, limit);
        }
    }
    let mut out = String::new();
    walk(doc, n, &mut out, limit);
    out
}

/// Is the element rendered (no `display:none`/`visibility:hidden`/zero
/// opacity on it or an ancestor)?
fn rendered(page: &Page, n: NodeId) -> bool {
    let doc = &page.doc;
    let Some(st) = page.styles.get(n) else { return false };
    if st.visibility == Visibility::Hidden || st.display == Display::None {
        return false;
    }
    let mut cur = Some(n);
    while let Some(c) = cur {
        if doc.element(c).is_some() {
            match page.styles.get(c) {
                Some(s) if s.display == Display::None || s.opacity <= 0.01 => return false,
                None if doc.tag(c).map(|t| t != "#shadow-root").unwrap_or(false) => return false,
                _ => {}
            }
        }
        cur = doc.flat_parent(c).or_else(|| doc.shadow_host(c));
    }
    true
}

/// Inside a `position: fixed` subtree (its rect is in viewport units)?
fn in_fixed(page: &Page, n: NodeId) -> bool {
    let doc = &page.doc;
    let mut cur = Some(n);
    while let Some(c) = cur {
        if page.styles.get(c).map(|s| s.position == Position::Fixed).unwrap_or(false) {
            return true;
        }
        cur = doc.flat_parent(c).or_else(|| doc.shadow_host(c));
    }
    false
}

/// Viewport rect of an element of `page`.
fn viewport_rect(page: &Page, n: NodeId) -> Option<rabbithole_gfx::Rect> {
    let r = page.rect_of(n)?;
    if r.w < 1.0 || r.h < 1.0 {
        return None;
    }
    Some(if in_fixed(page, n) { r } else { r.translate(-page.page_scroll.0, -page.page_scroll.1) })
}

/// A viewport point of `page` where `n` (or a descendant) is topmost.
fn hit_point(page: &Page, n: NodeId) -> Option<(f32, f32)> {
    let r = viewport_rect(page, n)?;
    let (vw, vh) = (page.viewport.width, page.viewport.height);
    for (fx, fy) in [(0.5, 0.5), (0.3, 0.5), (0.7, 0.5), (0.5, 0.3), (0.5, 0.7), (0.15, 0.5), (0.85, 0.5)] {
        let (x, y) = (r.x + r.w * fx, r.y + r.h * fy);
        if x < 0.0 || y < 0.0 || x >= vw || y >= vh {
            continue;
        }
        if let Some(h) = page.node_at_viewport(x, y) {
            if contains(&page.doc, n, h) {
                return Some((x, y));
            }
        }
    }
    None
}

/// Fraction of a sample grid over the element's visible area where it is
/// topmost (0 = covered or off screen).
fn topmost_share(page: &Page, n: NodeId) -> f32 {
    let Some(r) = viewport_rect(page, n) else { return 0.0 };
    let vis = r.intersect(&rabbithole_gfx::Rect::new(0.0, 0.0, page.viewport.width, page.viewport.height));
    if vis.is_empty() {
        return 0.0;
    }
    let mut hit = 0;
    let steps = 5;
    for i in 0..steps {
        for j in 0..steps {
            let x = vis.x + vis.w * (i as f32 + 0.5) / steps as f32;
            let y = vis.y + vis.h * (j as f32 + 0.5) / steps as f32;
            if page.node_at_viewport(x, y).map(|h| contains(&page.doc, n, h)).unwrap_or(false) {
                hit += 1;
            }
        }
    }
    hit as f32 / (steps * steps) as f32
}

fn is_dialog_el(doc: &Document, n: NodeId) -> bool {
    matches!(doc.attr(n, "role"), Some("dialog" | "alertdialog")) || doc.attr(n, "aria-modal") == Some("true") || (doc.is_element(n, "dialog") && doc.attr(n, "open").is_some())
}

/// Raised above the page: fixed, sticky, absolutely positioned with a
/// z-index, or a dialog.
fn raised(page: &Page, n: NodeId) -> bool {
    let Some(st) = page.styles.get(n) else { return false };
    matches!(st.position, Position::Fixed | Position::Sticky) || (st.position == Position::Absolute && st.z_index.unwrap_or(0) >= 10) || is_dialog_el(&page.doc, n)
}

// ------------------------------------------------------------- dialogs

/// A control of a consent dialog.
#[derive(Debug, Clone)]
pub struct ConsentButton {
    pub node: NodeId,
    /// Accessible name (whitespace collapsed).
    pub name: String,
    pub kind: ButtonKind,
}

/// A consent dialog found on a page.
#[derive(Debug, Clone)]
pub struct ConsentDialog {
    /// Iframe elements from the top page down to the dialog's document
    /// (empty: the dialog is in the top page).
    pub frames: Vec<NodeId>,
    /// The dialog's root element in its document.
    pub root: NodeId,
    /// Known CMP, if recognised.
    pub vendor: Option<&'static str>,
    pub buttons: Vec<ConsentButton>,
    /// Controls that are checked (purposes/vendors toggles) in the dialog.
    pub toggles: Vec<NodeId>,
}

impl ConsentDialog {
    pub fn first(&self, kind: ButtonKind) -> Option<&ConsentButton> {
        self.buttons.iter().find(|b| b.kind == kind)
    }
    pub fn count(&self, kind: ButtonKind) -> usize {
        self.buttons.iter().filter(|b| b.kind == kind).count()
    }
}

/// Accessible name of a control (aria-label, text, value, title).
pub fn accessible_name(doc: &Document, n: NodeId) -> String {
    let collapse = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(l) = doc.attr(n, "aria-label").map(collapse).filter(|s| !s.is_empty()) {
        return l;
    }
    if let Some(ids) = doc.attr(n, "aria-labelledby") {
        let t: Vec<String> = ids.split_whitespace().filter_map(|id| doc.element_by_id(id)).map(|e| collapse(&flat_text(doc, e, 200))).collect();
        let t = t.join(" ");
        if !t.trim().is_empty() {
            return t;
        }
    }
    let t = collapse(&flat_text(doc, n, 400));
    if !t.is_empty() {
        return t;
    }
    if doc.is_element(n, "input") {
        if let Some(v) = doc.attr(n, "value").map(collapse).filter(|s| !s.is_empty()) {
            return v;
        }
    }
    doc.attr(n, "title").map(collapse).unwrap_or_default()
}

fn is_button_like(doc: &Document, n: NodeId) -> bool {
    let Some(el) = doc.element(n) else { return false };
    match el.tag.as_str() {
        "button" => true,
        "a" => true,
        "input" => matches!(el.attr("type").map(|t| t.to_ascii_lowercase()).as_deref(), Some("button" | "submit")),
        _ => matches!(el.attr("role"), Some("button" | "link" | "menuitem")) || el.attr("onclick").is_some(),
    }
}

fn is_toggle(page: &Page, n: NodeId) -> bool {
    let doc = &page.doc;
    let Some(el) = doc.element(n) else { return false };
    if el.attr("disabled").is_some() || el.attr("aria-disabled") == Some("true") {
        return false;
    }
    if el.tag == "input" && matches!(el.attr("type").map(|t| t.to_ascii_lowercase()).as_deref(), Some("checkbox")) {
        return doc.is_checked(n);
    }
    matches!(el.attr("role"), Some("switch" | "checkbox")) && el.attr("aria-checked") == Some("true")
}

/// Collect the classified controls (and checked toggles) under `root`.
fn collect_controls(page: &Page, root: NodeId) -> (Vec<ConsentButton>, Vec<NodeId>) {
    let doc = &page.doc;
    let mut buttons: Vec<ConsentButton> = Vec::new();
    let mut toggles = Vec::new();
    let mut stack = vec![(root, false)];
    let mut seen = 0usize;
    while let Some((n, in_button)) = stack.pop() {
        seen += 1;
        if seen > 20000 {
            break;
        }
        if doc.element(n).is_none() && !doc.is_shadow_root(n) {
            continue;
        }
        if skip_tag(doc, n) {
            continue;
        }
        let mut inside = in_button;
        if !in_button && doc.element(n).is_some() {
            if is_toggle(page, n) && rendered(page, n) {
                toggles.push(n);
            }
            let pointer = page.styles.get(n).map(|s| s.cursor == "pointer").unwrap_or(false);
            let parent_pointer = doc.flat_parent(n).and_then(|p| page.styles.get(p)).map(|s| s.cursor == "pointer").unwrap_or(false);
            let candidate = is_button_like(doc, n) || (pointer && !parent_pointer && !doc.is_element(n, "label"));
            if candidate {
                inside = true;
                let name = accessible_name(doc, n);
                if let Some(kind) = classify_button(&name) {
                    if rendered(page, n) && page.rect_of(n).map(|r| r.w >= 1.0 && r.h >= 1.0).unwrap_or(false) && doc.attr(n, "disabled").is_none() {
                        buttons.push(ConsentButton { node: n, name, kind });
                    }
                }
            }
        }
        let mut ks = kids(doc, n);
        if let Some(sr) = doc.shadow_root(n) {
            if !ks.contains(&sr) {
                ks = doc.children(sr).collect();
            }
        }
        for c in ks.into_iter().rev() {
            stack.push((c, inside));
        }
    }
    (buttons, toggles)
}

/// The dialog must not be a login/newsletter/paywall form.
fn has_credential_inputs(doc: &Document, root: NodeId) -> bool {
    doc.descendants(root).any(|n| doc.is_element(n, "input") && matches!(doc.attr(n, "type").map(|t| t.to_ascii_lowercase()).as_deref(), Some("password" | "email")))
}

/// Build a dialog from a candidate root (None when it does not qualify).
fn qualify(page: &Page, frames: &[NodeId], root: NodeId, vendor: Option<&'static str>) -> Option<ConsentDialog> {
    let doc = &page.doc;
    let text = flat_text(doc, root, 12000);
    if text.len() >= 12000 && vendor.is_none() {
        return None; // a whole page, not a dialog
    }
    if !is_consent_text(&text) {
        return None;
    }
    if has_credential_inputs(doc, root) {
        return None;
    }
    let (buttons, toggles) = collect_controls(page, root);
    if buttons.is_empty() && vendor.is_none() {
        return None;
    }
    Some(ConsentDialog { frames: frames.to_vec(), root, vendor, buttons, toggles })
}

/// Find the consent dialog shown on the page (its frames included).
pub fn detect(page: &Page) -> Option<ConsentDialog> {
    detect_in(page, &[], 0)
}

fn detect_in(page: &Page, frames: &[NodeId], depth: u32) -> Option<ConsentDialog> {
    let doc = &page.doc;
    let mut raised_cands: Vec<NodeId> = Vec::new();
    let mut hinted: Vec<NodeId> = Vec::new();
    let mut vendor_hits: Vec<(NodeId, &'static str)> = Vec::new();
    let mut iframes: Vec<NodeId> = Vec::new();
    // Walk the flat tree once.
    let mut stack = vec![Document::ROOT];
    let mut seen = 0usize;
    while let Some(n) = stack.pop() {
        seen += 1;
        if seen > 200_000 {
            break;
        }
        let is_el = doc.element(n).is_some();
        if is_el {
            if skip_tag(doc, n) {
                continue;
            }
            // Not rendered: nothing below is either.
            if page.styles.get(n).map(|s| s.display == Display::None).unwrap_or(false) {
                continue;
            }
            if let Some(v) = vendor_of(doc, n) {
                vendor_hits.push((n, v));
            }
            if doc.is_element(n, "iframe") || doc.is_element(n, "frame") {
                iframes.push(n);
            }
            if raised(page, n) {
                raised_cands.push(n);
            } else if has_consent_hint(doc, n) {
                hinted.push(n);
            }
        }
        let mut ks = kids(doc, n);
        if is_el {
            if let Some(sr) = doc.shadow_root(n) {
                if !ks.contains(&sr) {
                    ks = doc.children(sr).collect();
                }
            }
        }
        for c in ks.into_iter().rev() {
            stack.push(c);
        }
    }

    // 1. Known CMP containers.
    for &(n, v) in &vendor_hits {
        if topmost_share(page, n) <= 0.0 {
            continue;
        }
        if let Some(d) = qualify(page, frames, n, Some(v)) {
            if !d.buttons.is_empty() {
                return Some(d);
            }
        }
    }

    // 2. Generic raised boxes (outermost qualifying one wins).
    let mut found: Vec<ConsentDialog> = Vec::new();
    for &n in &raised_cands {
        if found.iter().any(|d| contains(doc, d.root, n)) {
            continue;
        }
        if topmost_share(page, n) <= 0.0 {
            continue;
        }
        if let Some(d) = qualify(page, frames, n, None) {
            found.retain(|f| !contains(doc, n, f.root));
            found.push(d);
        }
    }
    if let Some(d) = found.into_iter().next() {
        return Some(d);
    }

    // 3. Raised iframes holding a consent document (Sourcepoint & co.).
    if depth < 3 {
        for &f in &iframes {
            let Some(child) = page.frames.get(f).and_then(|c| c.page.as_deref()) else { continue };
            if topmost_share(page, f) <= 0.0 {
                continue;
            }
            let mut path = frames.to_vec();
            path.push(f);
            if let Some(d) = detect_in(child, &path, depth + 1) {
                return Some(d);
            }
            // The whole frame document is the dialog when the iframe is
            // raised over the page (or is a CMP container's child).
            let raised_frame = {
                let mut cur = Some(f);
                let mut r = false;
                while let Some(c) = cur {
                    if raised(page, c) || vendor_of(doc, c).is_some() {
                        r = true;
                        break;
                    }
                    cur = doc.flat_parent(c).or_else(|| doc.shadow_host(c));
                }
                r
            };
            let vendor = {
                let mut cur = Some(f);
                let mut v = None;
                while let Some(c) = cur {
                    if let Some(x) = vendor_of(doc, c) {
                        v = Some(x);
                        break;
                    }
                    cur = doc.flat_parent(c);
                }
                v.or_else(|| doc.attr(f, "id").filter(|i| i.starts_with("sp_message_iframe")).map(|_| "Sourcepoint"))
            };
            if raised_frame || frames.is_empty() && vendor.is_some() {
                if let Some(body) = child.doc.body() {
                    if let Some(d) = qualify(child, &path, body, vendor) {
                        if !d.buttons.is_empty() {
                            return Some(d);
                        }
                    }
                }
            }
        }
    }

    // 4. Static banners with a cookie-ish id/class, visible on screen.
    for &n in &hinted {
        if hinted.iter().any(|&o| o != n && contains(doc, o, n)) {
            continue;
        }
        if topmost_share(page, n) <= 0.0 {
            continue;
        }
        if let Some(d) = qualify(page, frames, n, None) {
            return Some(d);
        }
    }

    // 5. A dedicated consent page (consent.google.com, consent.youtube.com,
    //    ...): the document itself is the dialog.
    let host = page.url.host.to_ascii_lowercase();
    if host.starts_with("consent.") || host.contains(".consent.") || page.url.path.contains("consent") {
        if let Some(body) = doc.body() {
            let text = flat_text(doc, body, 20000);
            if text.len() < 20000 && is_consent_text(&text) {
                let (buttons, toggles) = collect_controls(page, body);
                if !buttons.is_empty() {
                    let vendor = if host.starts_with("consent.google") || host.starts_with("consent.youtube") { Some("Google") } else { None };
                    return Some(ConsentDialog { frames: frames.to_vec(), root: body, vendor, buttons, toggles });
                }
            }
        }
    }
    None
}

/// The page holding a dialog (following its frame path).
pub fn frame_page<'a>(page: &'a Page, frames: &[NodeId]) -> Option<&'a Page> {
    let mut p = page;
    for &f in frames {
        p = p.frames.get(f)?.page.as_deref()?;
    }
    Some(p)
}

/// Is the dialog still shown? (its root still connected, rendered and
/// topmost somewhere, or another consent dialog took its place).
pub fn still_shown(page: &Page, d: &ConsentDialog) -> bool {
    if let Some(p) = frame_page(page, &d.frames) {
        if d.root < p.doc.nodes.len() && p.doc.is_connected(d.root) && rendered(p, d.root) {
            // A frame document: visible if its iframe still is.
            let visible = if d.frames.is_empty() {
                topmost_share(p, d.root) > 0.0
            } else {
                let parent = frame_page(page, &d.frames[..d.frames.len() - 1]);
                let f = *d.frames.last().unwrap();
                parent.map(|pp| rendered(pp, f) && topmost_share(pp, f) > 0.0).unwrap_or(false) && topmost_share(p, d.root) > 0.0
            };
            if visible {
                return true;
            }
        }
    }
    false
}

// ------------------------------------------------------------- clicking

impl Engine {
    /// Click an element of the page or of one of its frames (`frames`: the
    /// iframe path) like a user: through the pointer path at a point where
    /// it is topmost, else by dispatching the click to it directly.
    /// Returns the click action (top page only), the script outcome and
    /// whether the real pointer path was used.
    pub fn consent_click(&self, page: &mut Page, frames: &[NodeId], node: NodeId) -> (ClickAction, ScriptOutcome, bool) {
        let m = Modifiers::default();
        // Top-page viewport point of the target, if it can be hit there.
        if let Some((x, y)) = top_point(page, frames, node) {
            let mut up = self.click_at(page, x, y, m);
            let out = std::mem::take(&mut up.outcome);
            return (up.action, out, true);
        }
        if frames.is_empty() {
            let (a, o) = self.click_element(page, node, None, m);
            return (a, o, false);
        }
        let out = self.click_in_frames(page, frames, node);
        (ClickAction::Changed, out, false)
    }

    fn click_in_frames(&self, page: &mut Page, frames: &[NodeId], node: NodeId) -> ScriptOutcome {
        let Some(i) = page.frames.children.iter().position(|c| c.node == frames[0]) else { return ScriptOutcome::default() };
        let rest = &frames[1..];
        let o = self.with_child(page, i, |child| {
            if rest.is_empty() {
                self.click_element(child, node, None, Modifiers::default()).1
            } else {
                self.click_in_frames(child, rest, node)
            }
        });
        let mut out = ScriptOutcome::default();
        if let Some(o) = o {
            self.frame_click_done(page, frames[0], i, o, &mut out);
        }
        out
    }
}

/// Point in the top page's viewport where `node` (in the frame `frames`)
/// is the topmost element, hit-tested through every level.
fn top_point(page: &Page, frames: &[NodeId], node: NodeId) -> Option<(f32, f32)> {
    let Some((&f, rest)) = frames.split_first() else { return hit_point(page, node) };
    let child = page.frames.get(f)?.page.as_deref()?;
    let (cx, cy) = top_point(child, rest, node)?;
    let r = page.frame_content_rect(f)?;
    let (x, y) = (cx + r.x - page.page_scroll.0, cy + r.y - page.page_scroll.1);
    if x < 0.0 || y < 0.0 || x >= page.viewport.width || y >= page.viewport.height {
        return None;
    }
    (page.node_at_viewport(x, y) == Some(f)).then_some((x, y))
}

// --------------------------------------------------------------- driver

/// How a page's consent dialog was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentOutcome {
    Accepted,
    Rejected,
    /// A dialog was found but no button matched the choice (e.g. no
    /// reject option, or an ambiguous settings panel): left to the user.
    NoButton,
    /// Clicked, but the dialog stayed.
    StillShown,
}

impl ConsentOutcome {
    pub fn id(self) -> &'static str {
        match self {
            ConsentOutcome::Accepted => "accepted",
            ConsentOutcome::Rejected => "rejected",
            ConsentOutcome::NoButton => "no-button",
            ConsentOutcome::StillShown => "still-shown",
        }
    }
    pub fn from_id(s: &str) -> Option<ConsentOutcome> {
        Some(match s {
            "accepted" => ConsentOutcome::Accepted,
            "rejected" => ConsentOutcome::Rejected,
            "no-button" => ConsentOutcome::NoButton,
            "still-shown" => ConsentOutcome::StillShown,
            _ => return None,
        })
    }
}

/// The result reported for a page.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsentReport {
    pub outcome: ConsentOutcome,
    /// Name of the button that was clicked last (empty: none).
    pub button: String,
    pub vendor: Option<String>,
    /// The dialog lived in an iframe.
    pub in_frame: bool,
    /// Steps taken, e.g. ["Manage options", "Reject all"].
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Stage {
    /// Looking for a dialog.
    Watch,
    /// Clicked the final button: check that the dialog went away.
    Verify { final_kind: ButtonKind },
    /// Opened the settings panel: look for reject/save there.
    InSettings,
    Done,
}

/// What the embedder must do after [`ConsentDriver::step`].
#[derive(Debug)]
pub enum ConsentStep {
    /// Nothing happened; call again at `next` (None: finished).
    Idle,
    /// A click happened: apply the outcome (repaint, navigation, ...) and
    /// the click action (links, form submission) like a user click.
    Clicked { action: ClickAction, outcome: ScriptOutcome },
    /// Finished with a result to record (the page is left alone from now
    /// on).
    Finished(ConsentReport),
}

/// Handles the consent dialog of one page load.
#[derive(Debug, Clone)]
pub struct ConsentDriver {
    pub choice: Choice,
    stage: Stage,
    /// Clicks on the final button so far.
    attempts: u8,
    settings_opened: bool,
    toggles_done: bool,
    /// Next time to look (None: idle until `arm`).
    pub next: Option<Instant>,
    /// Stop watching for a dialog after this.
    deadline: Instant,
    vendor: Option<&'static str>,
    in_frame: bool,
    steps: Vec<String>,
    last_button: String,
    dialog: Option<ConsentDialog>,
    /// Toggles still to switch off before saving.
    pending_toggles: Vec<NodeId>,
}

/// How long after a page load a dialog may still appear.
pub const WATCH_FOR: Duration = Duration::from_secs(20);
/// Delay between a click and the check that the dialog went away.
pub const VERIFY_AFTER: Duration = Duration::from_millis(700);
/// Most clicks on a final button per page load.
pub const MAX_ATTEMPTS: u8 = 2;

impl ConsentDriver {
    pub fn new(choice: Choice, now: Instant) -> ConsentDriver {
        ConsentDriver {
            choice,
            stage: Stage::Watch,
            attempts: 0,
            settings_opened: false,
            toggles_done: false,
            next: Some(now + Duration::from_millis(300)),
            deadline: now + WATCH_FOR,
            vendor: None,
            in_frame: false,
            steps: Vec::new(),
            last_button: String::new(),
            dialog: None,
            pending_toggles: Vec::new(),
        }
    }

    pub fn finished(&self) -> bool {
        self.stage == Stage::Done
    }

    /// Extend the watch window (e.g. once scripts finished loading).
    pub fn extend_watch(&mut self, now: Instant) {
        if self.stage == Stage::Watch {
            self.deadline = self.deadline.max(now + WATCH_FOR);
            if self.next.is_none() {
                self.next = Some(now);
            }
        }
    }

    fn poll_interval(&self, now: Instant) -> Duration {
        let left = self.deadline.saturating_duration_since(now);
        if left > WATCH_FOR - Duration::from_secs(6) {
            Duration::from_millis(400)
        } else {
            Duration::from_millis(1000)
        }
    }

    fn report(&mut self, outcome: ConsentOutcome) -> ConsentStep {
        self.stage = Stage::Done;
        self.next = None;
        ConsentStep::Finished(ConsentReport { outcome, button: self.last_button.clone(), vendor: self.vendor.map(|v| v.to_string()), in_frame: self.in_frame, steps: self.steps.clone() })
    }

    fn click(&mut self, engine: &Engine, page: &mut Page, d: &ConsentDialog, b: &ConsentButton, now: Instant) -> ConsentStep {
        let (action, outcome, _real) = engine.consent_click(page, &d.frames, b.node);
        self.last_button = b.name.clone();
        self.steps.push(b.name.clone());
        self.next = Some(now + VERIFY_AFTER);
        ConsentStep::Clicked { action, outcome }
    }

    /// The page navigated away after the final click: success.
    pub fn navigated(&mut self) -> Option<ConsentReport> {
        match self.stage {
            Stage::Verify { .. } => match self.step_verify_done() {
                ConsentStep::Finished(r) => Some(r),
                _ => None,
            },
            _ => None,
        }
    }

    fn step_verify_done(&mut self) -> ConsentStep {
        let outcome = match self.choice {
            Choice::Accept => ConsentOutcome::Accepted,
            Choice::Reject => ConsentOutcome::Rejected,
        };
        self.report(outcome)
    }

    /// Run when `next` is due. `page` is the top-level page.
    pub fn step(&mut self, engine: &Engine, page: &mut Page, now: Instant) -> ConsentStep {
        if self.stage == Stage::Done {
            return ConsentStep::Idle;
        }
        if let Some(n) = self.next {
            if n > now {
                return ConsentStep::Idle;
            }
        }
        engine.restyle(page);
        match self.stage.clone() {
            Stage::Done => ConsentStep::Idle,
            Stage::Watch => {
                let Some(d) = detect(page) else {
                    if now >= self.deadline {
                        self.stage = Stage::Done;
                        self.next = None;
                    } else {
                        self.next = Some(now + self.poll_interval(now));
                    }
                    return ConsentStep::Idle;
                };
                self.vendor = d.vendor;
                self.in_frame = !d.frames.is_empty();
                self.act(engine, page, d, now)
            }
            Stage::InSettings => {
                // Switch off checked purposes one per step (each click runs
                // the site's handlers), then save.
                if let Some(t) = self.pending_toggles.pop() {
                    if let Some(d) = self.dialog.clone() {
                        let (action, outcome, _) = engine.consent_click(page, &d.frames, t);
                        self.next = Some(now + Duration::from_millis(80));
                        return ConsentStep::Clicked { action, outcome };
                    }
                }
                let Some(d) = detect(page) else {
                    // The panel closed by itself: done.
                    return self.step_verify_done();
                };
                self.act(engine, page, d, now)
            }
            Stage::Verify { final_kind } => {
                let gone = match &self.dialog {
                    Some(d) => !still_shown(page, d) && detect(page).map(|n| n.first(final_kind).is_none()).unwrap_or(true),
                    None => detect(page).is_none(),
                };
                if gone {
                    return self.step_verify_done();
                }
                if self.attempts >= MAX_ATTEMPTS {
                    return self.report(ConsentOutcome::StillShown);
                }
                match detect(page) {
                    Some(d) => self.act(engine, page, d, now),
                    None => self.step_verify_done(),
                }
            }
        }
    }

    /// Decide and click for a detected dialog.
    fn act(&mut self, engine: &Engine, page: &mut Page, d: ConsentDialog, now: Instant) -> ConsentStep {
        let pick = match self.choice {
            Choice::Accept => d.first(ButtonKind::AcceptAll).or_else(|| d.first(ButtonKind::Accept)).cloned(),
            Choice::Reject => d.first(ButtonKind::Reject).cloned(),
        };
        if let Some(b) = pick {
            self.attempts += 1;
            self.stage = Stage::Verify { final_kind: b.kind };
            self.dialog = Some(d.clone());
            return self.click(engine, page, &d, &b, now);
        }
        if self.choice == Choice::Reject {
            // One level deeper: the detailed panel.
            if self.settings_opened {
                if d.count(ButtonKind::Save) == 1 && self.attempts < MAX_ATTEMPTS {
                    let save = d.first(ButtonKind::Save).cloned().unwrap();
                    // First switch every checked purpose off (each click
                    // runs the site's handlers), then save.
                    if !d.toggles.is_empty() && !self.toggles_done {
                        self.toggles_done = true;
                        self.pending_toggles = d.toggles.clone();
                        self.dialog = Some(d);
                        self.stage = Stage::InSettings;
                        self.next = Some(now);
                        return ConsentStep::Idle;
                    }
                    self.attempts += 1;
                    self.stage = Stage::Verify { final_kind: ButtonKind::Save };
                    self.dialog = Some(d.clone());
                    return self.click(engine, page, &d, &save, now);
                }
            } else if d.count(ButtonKind::Settings) == 1 {
                let s = d.first(ButtonKind::Settings).cloned().unwrap();
                self.settings_opened = true;
                self.stage = Stage::InSettings;
                self.dialog = Some(d.clone());
                return self.click(engine, page, &d, &s, now);
            }
        }
        self.dialog = Some(d);
        self.report(ConsentOutcome::NoButton)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding() {
        assert_eq!(fold("  Aceptar   TODO! "), "aceptar todo");
        assert_eq!(fold("Tout refuser ›"), "tout refuser");
        assert_eq!(fold("Accept & close"), "accept and close");
        assert_eq!(fold("Odrzuć wszystko"), "odrzuc wszystko");
        assert_eq!(fold("Αποδοχή όλων"), "αποδοχη ολων");
        assert_eq!(fold("J’accepte"), "j'accepte");
    }

    #[test]
    fn multilingual_buttons() {
        use ButtonKind::*;
        let cases: &[(&str, Option<ButtonKind>)] = &[
            // accept-all
            ("Accept all", Some(AcceptAll)),
            ("Accept All Cookies", Some(AcceptAll)),
            ("Allow all", Some(AcceptAll)),
            ("Aceptar todo", Some(AcceptAll)),
            ("Aceptar y continuar", Some(AcceptAll)),
            ("Tout accepter", Some(AcceptAll)),
            ("Accepter et fermer", Some(AcceptAll)),
            ("Alle akzeptieren", Some(AcceptAll)),
            ("Akzeptieren und weiter", Some(AcceptAll)),
            ("Accetta tutto", Some(AcceptAll)),
            ("Aceitar tudo", Some(AcceptAll)),
            ("Alles accepteren", Some(AcceptAll)),
            ("Zaakceptuj wszystko", Some(AcceptAll)),
            ("Acceptera alla", Some(AcceptAll)),
            ("Tillad alle", Some(AcceptAll)),
            ("Godta alle", Some(AcceptAll)),
            ("Hyväksy kaikki", Some(AcceptAll)),
            ("Přijmout vše", Some(AcceptAll)),
            ("Αποδοχή όλων", Some(AcceptAll)),
            ("Acceptă toate", Some(AcceptAll)),
            ("Összes elfogadása", Some(AcceptAll)),
            ("Acceptar-ho tot", Some(AcceptAll)),
            ("Onartu guztiak", Some(AcceptAll)),
            ("Agree and close", Some(AcceptAll)),
            // accept
            ("Aceptar", Some(Accept)),
            ("Accept", Some(Accept)),
            ("AGREE", Some(Accept)),
            ("I agree", Some(Accept)),
            ("Accetta", Some(Accept)),
            ("J'accepte", Some(Accept)),
            ("Akzeptieren", Some(Accept)),
            ("Zustimmen", Some(Accept)),
            ("OK", Some(Accept)),
            ("Got it!", Some(Accept)),
            ("Elfogadom", Some(Accept)),
            // reject
            ("Reject all", Some(Reject)),
            ("Reject All", Some(Reject)),
            ("Decline", Some(Reject)),
            ("DISAGREE", Some(Reject)),
            ("Only necessary", Some(Reject)),
            ("Use necessary cookies only", Some(Reject)),
            ("Accept only necessary cookies", Some(Reject)),
            ("Rechazar", Some(Reject)),
            ("Rechazar todo", Some(Reject)),
            ("Solo necesarias", Some(Reject)),
            ("Continuar sin aceptar", Some(Reject)),
            ("Tout refuser", Some(Reject)),
            ("Continuer sans accepter →", Some(Reject)),
            ("Alle ablehnen", Some(Reject)),
            ("Nur notwendige Cookies akzeptieren", Some(Reject)),
            ("Rifiuta", Some(Reject)),
            ("Rifiuta tutto", Some(Reject)),
            ("Rejeitar tudo", Some(Reject)),
            ("Alles weigeren", Some(Reject)),
            ("Odrzuć wszystko", Some(Reject)),
            ("Avvisa alla", Some(Reject)),
            ("Afvis alle", Some(Reject)),
            ("Avslå alle", Some(Reject)),
            ("Hylkää kaikki", Some(Reject)),
            ("Odmítnout vše", Some(Reject)),
            ("Απόρριψη όλων", Some(Reject)),
            ("Respinge tot", Some(Reject)),
            ("Összes elutasítása", Some(Reject)),
            ("Rebutjar", Some(Reject)),
            ("Baztertu guztiak", Some(Reject)),
            ("Rexeitar todo", Some(Reject)),
            ("Disagree and close", Some(Reject)),
            // settings / save
            ("Cookie Settings", Some(Settings)),
            ("Manage options", Some(Settings)),
            ("More options", Some(Settings)),
            ("Más opciones", Some(Settings)),
            ("Configurar", Some(Settings)),
            ("Paramétrer les cookies", Some(Settings)),
            ("Einstellungen", Some(Settings)),
            ("Personalizza", Some(Settings)),
            ("Save settings", Some(Save)),
            ("Confirm my choices", Some(Save)),
            ("Guardar preferencias", Some(Save)),
            ("Enregistrer mes choix", Some(Save)),
            ("Auswahl speichern", Some(Save)),
            ("Allow selection", Some(Save)),
            ("Auswahl erlauben", Some(Save)),
            // never
            ("Sign in", None),
            ("Subscribe now", None),
            ("Suscríbete", None),
            ("Jetzt abonnieren", None),
            ("I am over 18", None),
            ("Tengo más de 18 años", None),
            ("Privacy policy", None),
            ("Learn more", None),
            ("Cookie policy", None),
            ("Vendors", None),
            ("", None),
            ("Read our full privacy statement and the list of all the partners we work with to understand how cookies are used on this site", None),
        ];
        let mut bad = Vec::new();
        for (name, want) in cases {
            let got = classify_button(name);
            if got != *want {
                bad.push(format!("{name:?}: got {got:?}, want {want:?}"));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn consent_text() {
        for t in [
            "We use cookies to improve your experience",
            "Utilizamos cookies propias y de terceros",
            "Nous utilisons des traceurs",
            "Wir verwenden Cookies. Ihre Einwilligung",
            "Questo sito utilizza i cookie",
            "Este site usa cookies",
            "Wij gebruiken cookies",
            "Używamy plików cookie. Ciasteczka",
            "Vi använder kakor",
            "Vi bruger cookies",
            "Käytämme evästeitä",
            "Používáme soubory cookie",
            "Χρησιμοποιούμε cookies",
            "Folosim cookie-uri",
            "Sütiket használunk",
            "Utilitzem galetes",
            "Cookieak erabiltzen ditugu",
            "Usamos cookies",
        ] {
            assert!(is_consent_text(t), "{t}");
        }
        assert!(!is_consent_text("Subscribe to our newsletter. We respect your privacy."));
        assert!(mentions_privacy("We respect your privacy."));
    }
}
