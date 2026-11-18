//! Unicode BCP 47 locale identifiers (UTS #35): structural validation
//! (IsStructurallyValidLanguageTag) and canonicalization
//! (CanonicalizeUnicodeLocaleId) with the CLDR alias data Intl needs.

/// A parsed `unicode_locale_id`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocaleId {
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
    pub variants: Vec<String>,
    /// `-u-` attributes and (key, value) keywords (value "" for true).
    pub u_attributes: Vec<String>,
    pub u_keywords: Vec<(String, String)>,
    /// `-t-` language and fields.
    pub t_lang: Option<Box<LocaleId>>,
    pub t_fields: Vec<(String, String)>,
    /// Other extensions (singleton, subtags) and private use subtags.
    pub other: Vec<(char, Vec<String>)>,
    pub private: Vec<String>,
    has_t: bool,
}

fn alpha(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic())
}
fn digit(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
fn alnum(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn is_language(s: &str) -> bool {
    alpha(s) && matches!(s.len(), 2 | 3 | 5..=8)
}
fn is_script(s: &str) -> bool {
    alpha(s) && s.len() == 4
}
fn is_region(s: &str) -> bool {
    (alpha(s) && s.len() == 2) || (digit(s) && s.len() == 3)
}
fn is_variant(s: &str) -> bool {
    alnum(s) && ((5..=8).contains(&s.len()) || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit()))
}

/// Parse `unicode_language_id` subtags from `parts[*i..]` (lower-cased
/// input). Returns None when malformed.
fn parse_language_id(parts: &[String], i: &mut usize, allow_und_only: bool) -> Option<LocaleId> {
    let _ = allow_und_only;
    let lang = parts.get(*i)?;
    if !is_language(lang) {
        return None;
    }
    let mut id = LocaleId { language: lang.clone(), ..Default::default() };
    *i += 1;
    if let Some(p) = parts.get(*i) {
        if is_script(p) {
            id.script = Some(p.clone());
            *i += 1;
        }
    }
    if let Some(p) = parts.get(*i) {
        if is_region(p) {
            id.region = Some(p.clone());
            *i += 1;
        }
    }
    while let Some(p) = parts.get(*i) {
        if !is_variant(p) {
            break;
        }
        if id.variants.contains(p) {
            return None;
        }
        id.variants.push(p.clone());
        *i += 1;
    }
    Some(id)
}

/// IsStructurallyValidLanguageTag + parse.
pub fn parse(tag: &str) -> Option<LocaleId> {
    if tag.is_empty() || tag.starts_with('-') || tag.ends_with('-') || tag.contains("--") || !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    let parts: Vec<String> = tag.split('-').map(|p| p.to_ascii_lowercase()).collect();
    if parts.iter().any(|p| p.len() > 8) {
        return None;
    }
    let mut i = 0;
    let mut id = parse_language_id(&parts, &mut i, true)?;
    let mut singletons: Vec<char> = Vec::new();
    while i < parts.len() {
        let p = &parts[i];
        if p.len() != 1 {
            return None;
        }
        let s = p.as_bytes()[0] as char;
        if s != 'x' && singletons.contains(&s) {
            return None;
        }
        singletons.push(s);
        i += 1;
        match s {
            'x' => {
                let start = i;
                while i < parts.len() {
                    if !alnum(&parts[i]) {
                        return None;
                    }
                    id.private.push(parts[i].clone());
                    i += 1;
                }
                if i == start {
                    return None;
                }
            }
            'u' => {
                let start = i;
                while i < parts.len() && parts[i].len() >= 3 && alnum(&parts[i]) {
                    id.u_attributes.push(parts[i].clone());
                    i += 1;
                }
                while i < parts.len() && parts[i].len() == 2 {
                    let key = &parts[i];
                    let kb = key.as_bytes();
                    if !(kb[0].is_ascii_alphanumeric() && kb[1].is_ascii_alphabetic()) {
                        return None;
                    }
                    i += 1;
                    let mut vals = Vec::new();
                    while i < parts.len() && (3..=8).contains(&parts[i].len()) && alnum(&parts[i]) {
                        vals.push(parts[i].clone());
                        i += 1;
                    }
                    id.u_keywords.push((key.clone(), vals.join("-")));
                }
                if i == start {
                    return None;
                }
            }
            't' => {
                id.has_t = true;
                let start = i;
                if i < parts.len() && is_language(&parts[i]) {
                    let tl = parse_language_id(&parts, &mut i, false)?;
                    id.t_lang = Some(Box::new(tl));
                }
                while i < parts.len() && parts[i].len() == 2 {
                    let key = &parts[i];
                    let kb = key.as_bytes();
                    if !(kb[0].is_ascii_alphabetic() && kb[1].is_ascii_digit()) {
                        return None;
                    }
                    i += 1;
                    let mut vals = Vec::new();
                    while i < parts.len() && (3..=8).contains(&parts[i].len()) && alnum(&parts[i]) {
                        vals.push(parts[i].clone());
                        i += 1;
                    }
                    if vals.is_empty() {
                        return None;
                    }
                    id.t_fields.push((key.clone(), vals.join("-")));
                }
                if i == start {
                    return None;
                }
            }
            c if c.is_ascii_alphanumeric() => {
                let start = i;
                let mut subs = Vec::new();
                while i < parts.len() && (2..=8).contains(&parts[i].len()) && alnum(&parts[i]) {
                    subs.push(parts[i].clone());
                    i += 1;
                }
                if i == start {
                    return None;
                }
                id.other.push((c, subs));
            }
            _ => return None,
        }
    }
    Some(id)
}

// ------------------------------------------------------------- alias data

/// languageAlias: (from, to) where `to` may carry script/region.
const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("aam", "aas"), ("aar", "aa"), ("abk", "ab"), ("adp", "dz"), ("afr", "af"), ("aju", "jrb"), ("aka", "ak"),
    ("alb", "sq"), ("als", "sq"), ("amh", "am"), ("ara", "ar"), ("arb", "ar"), ("arg", "an"), ("arm", "hy"),
    ("asd", "snz"), ("asm", "as"), ("aue", "ktz"), ("ava", "av"), ("ave", "ae"), ("aym", "ay"), ("ayr", "ay"),
    ("ayx", "nun"), ("aze", "az"), ("azj", "az"), ("bak", "ba"), ("bam", "bm"), ("baq", "eu"), ("bcc", "bal"),
    ("bcl", "bik"), ("bel", "be"), ("ben", "bn"), ("bgm", "bcg"), ("bh", "bho"), ("bih", "bho"), ("bis", "bi"),
    ("bjd", "drl"), ("bod", "bo"), ("bos", "bs"), ("bre", "br"), ("bul", "bg"), ("bur", "my"), ("bxk", "luy"),
    ("bxr", "bua"), ("cat", "ca"), ("ccq", "rki"), ("ces", "cs"), ("cha", "ch"), ("che", "ce"), ("chi", "zh"),
    ("chu", "cu"), ("chv", "cv"), ("cjr", "mom"), ("cka", "cmr"), ("cld", "syr"), ("cmk", "xch"), ("cmn", "zh"),
    ("cor", "kw"), ("cos", "co"), ("coy", "pij"), ("cqu", "quh"), ("cre", "cr"), ("cwd", "cr"), ("cym", "cy"),
    ("cze", "cs"), ("dan", "da"), ("deu", "de"), ("dgo", "doi"), ("dhd", "mwr"), ("dik", "din"), ("diq", "zza"),
    ("dit", "dif"), ("div", "dv"), ("drh", "mn"), ("dut", "nl"), ("dzo", "dz"), ("ekk", "et"), ("ell", "el"),
    ("emk", "man"), ("eng", "en"), ("epo", "eo"), ("esk", "ik"), ("est", "et"), ("eus", "eu"), ("ewe", "ee"),
    ("fao", "fo"), ("fas", "fa"), ("fat", "ak"), ("fij", "fj"), ("fin", "fi"), ("fra", "fr"), ("fre", "fr"),
    ("fry", "fy"), ("fuc", "ff"), ("ful", "ff"), ("gav", "dev"), ("gaz", "om"), ("gbo", "grb"), ("geo", "ka"),
    ("ger", "de"), ("gfx", "vaj"), ("ggn", "gvr"), ("gla", "gd"), ("gle", "ga"), ("glg", "gl"), ("glv", "gv"),
    ("gno", "gon"), ("gre", "el"), ("grn", "gn"), ("gti", "nyc"), ("gug", "gn"), ("guj", "gu"), ("guv", "duz"),
    ("gya", "gba"), ("hat", "ht"), ("hau", "ha"), ("hdn", "hai"), ("hea", "hmn"), ("heb", "he"), ("her", "hz"),
    ("him", "srx"), ("hin", "hi"), ("hmo", "ho"), ("hrr", "jal"), ("hrv", "hr"), ("hun", "hu"), ("hye", "hy"),
    ("ibi", "opa"), ("ibo", "ig"), ("ice", "is"), ("ido", "io"), ("iii", "ii"), ("ike", "iu"), ("iku", "iu"),
    ("ile", "ie"), ("ilw", "gal"), ("in", "id"), ("ina", "ia"), ("ind", "id"), ("ipk", "ik"), ("isl", "is"),
    ("ita", "it"), ("iw", "he"), ("jav", "jv"), ("jeg", "oyb"), ("ji", "yi"), ("jpn", "ja"), ("jw", "jv"),
    ("kal", "kl"), ("kan", "kn"), ("kas", "ks"), ("kat", "ka"), ("kau", "kr"), ("kaz", "kk"), ("kgc", "tdf"),
    ("kgh", "kml"), ("khk", "mn"), ("khm", "km"), ("kik", "ki"), ("kin", "rw"), ("kir", "ky"), ("kmr", "ku"),
    ("knc", "kr"), ("kng", "kg"), ("knn", "kok"), ("koj", "kwv"), ("kom", "kv"), ("kon", "kg"), ("kor", "ko"),
    ("kpv", "kv"), ("krm", "bmf"), ("ktr", "dtp"), ("kua", "kj"), ("kur", "ku"), ("kvs", "gdj"), ("kwq", "yam"),
    ("kxe", "tvd"), ("kzj", "dtp"), ("kzt", "dtp"), ("lao", "lo"), ("lat", "la"), ("lav", "lv"), ("lbk", "bnc"),
    ("lii", "raq"), ("lim", "li"), ("lin", "ln"), ("lit", "lt"), ("llo", "ngt"), ("lmm", "rmx"), ("ltz", "lb"),
    ("lub", "lu"), ("lug", "lg"), ("lvs", "lv"), ("mac", "mk"), ("mah", "mh"), ("mal", "ml"), ("mao", "mi"),
    ("mar", "mr"), ("may", "ms"), ("meg", "cir"), ("mhr", "chm"), ("mkd", "mk"), ("mlg", "mg"), ("mlt", "mt"),
    ("mnk", "man"), ("mo", "ro"), ("mol", "ro"), ("mon", "mn"), ("mri", "mi"), ("msa", "ms"), ("mst", "mry"),
    ("mup", "raj"), ("mwj", "vaj"), ("mya", "my"), ("myt", "mry"), ("nad", "xny"), ("nau", "na"), ("nav", "nv"),
    ("nbl", "nr"), ("ncp", "kdz"), ("nde", "nd"), ("ndo", "ng"), ("nep", "ne"), ("nld", "nl"), ("nno", "nn"),
    ("nns", "nbr"), ("nnx", "ngv"), ("no", "no"), ("nob", "nb"), ("nor", "no"), ("npi", "ne"), ("nts", "pij"),
    ("nya", "ny"), ("oci", "oc"), ("ojg", "oj"), ("oji", "oj"), ("ori", "or"), ("orm", "om"), ("ory", "or"),
    ("oss", "os"), ("oun", "vaj"), ("pan", "pa"), ("pbu", "ps"), ("pcr", "adx"), ("per", "fa"), ("pes", "fa"),
    ("pli", "pi"), ("plt", "mg"), ("pmc", "huw"), ("pmu", "phr"), ("pnb", "lah"), ("pol", "pl"), ("por", "pt"),
    ("ppa", "bfy"), ("ppr", "lcq"), ("pry", "prt"), ("pus", "ps"), ("puz", "pub"), ("que", "qu"), ("quz", "qu"),
    ("rmy", "rom"), ("roh", "rm"), ("ron", "ro"), ("rum", "ro"), ("run", "rn"), ("rus", "ru"), ("sag", "sg"),
    ("san", "sa"), ("sca", "hle"), ("scc", "sr"), ("scr", "hr"), ("sin", "si"), ("skk", "oyb"), ("slk", "sk"),
    ("slo", "sk"), ("slv", "sl"), ("sme", "se"), ("smo", "sm"), ("sna", "sn"), ("snd", "sd"), ("som", "so"),
    ("sot", "st"), ("spa", "es"), ("spy", "kln"), ("sqi", "sq"), ("src", "sc"), ("srd", "sc"), ("srp", "sr"),
    ("ssw", "ss"), ("sun", "su"), ("swa", "sw"), ("swc", "sw"), ("swe", "sv"), ("swh", "sw"), ("tah", "ty"),
    ("tam", "ta"), ("tat", "tt"), ("tdu", "dtp"), ("tel", "te"), ("tgk", "tg"), ("tgl", "fil"), ("tha", "th"),
    ("thc", "tpo"), ("thx", "oyb"), ("tib", "bo"), ("tie", "ras"), ("tir", "ti"), ("tkk", "twm"), ("tl", "fil"),
    ("tlw", "weo"), ("tmp", "tyj"), ("tne", "kak"), ("ton", "to"), ("tsf", "taj"), ("tsn", "tn"), ("tso", "ts"),
    ("ttq", "tmh"), ("tuk", "tk"), ("tur", "tr"), ("tw", "ak"), ("twi", "ak"), ("uig", "ug"), ("ukr", "uk"),
    ("umu", "del"), ("urd", "ur"), ("uok", "ema"), ("uzb", "uz"), ("uzn", "uz"), ("ven", "ve"), ("vie", "vi"),
    ("vol", "vo"), ("wel", "cy"), ("wln", "wa"), ("wol", "wo"), ("xba", "cax"), ("xho", "xh"), ("xia", "acn"),
    ("xkh", "waw"), ("xpe", "kpe"), ("xsj", "suj"), ("xsl", "den"), ("ybd", "rki"), ("ydd", "yi"), ("yid", "yi"),
    ("yma", "lrr"), ("ymt", "mtm"), ("yor", "yo"), ("yos", "zom"), ("yuu", "yug"), ("zai", "zap"), ("zha", "za"),
    ("zho", "zh"), ("zsm", "ms"), ("zul", "zu"), ("zyb", "za"),
    // Complex replacements (language → language + script/region).
    ("sh", "sr-Latn"), ("cnr", "sr-ME"), ("hbs", "sr-Latn"), ("prs", "fa-AF"), ("swc", "sw-CD"), ("tnf", "fa-AF"),
    ("bh", "bho"),
];

/// territoryAlias: region → replacement list (first is the default).
const REGION_ALIASES: &[(&str, &[&str])] = &[
    ("aa", &["AA"]), ("an", &["CW", "SX", "BQ"]), ("bu", &["MM"]), ("cs", &["RS", "ME"]), ("ct", &["KI"]),
    ("dd", &["DE"]), ("dy", &["BJ"]), ("fq", &["AQ", "TF"]), ("fx", &["FR"]), ("hv", &["BF"]), ("jt", &["UM"]),
    ("mi", &["UM"]), ("nh", &["VU"]), ("nq", &["AQ"]), ("nt", &["SA", "IQ"]), ("pc", &["FM", "MH", "MP", "PW"]),
    ("pu", &["UM"]), ("pz", &["PA"]), ("qu", &["EU"]), ("rh", &["ZW"]), ("su", &["RU", "AM", "AZ", "BY", "EE", "GE", "KZ", "KG", "LV", "LT", "MD", "TJ", "TM", "UA", "UZ"]),
    ("tp", &["TL"]), ("uk", &["GB"]), ("vd", &["VN"]), ("wk", &["UM"]), ("yd", &["YE"]), ("yu", &["RS", "ME"]),
    ("zr", &["CD"]), ("062", &["034", "143"]), ("172", &["RU", "AM", "AZ", "BY", "GE", "KG", "KZ", "MD", "TJ", "TM", "UA", "UZ"]),
    ("200", &["CZ", "SK"]), ("230", &["ET"]), ("280", &["DE"]), ("532", &["CW", "SX", "BQ"]), ("582", &["FM", "MH", "MP", "PW"]),
    ("736", &["SD"]), ("810", &["RU", "AM", "AZ", "BY", "EE", "GE", "KZ", "KG", "LV", "LT", "MD", "TJ", "TM", "UA", "UZ"]),
    ("830", &["JE", "GG"]), ("886", &["YE"]), ("890", &["RS", "ME", "SI", "HR", "MK", "BA"]),
    ("004", &["AF"]), ("008", &["AL"]), ("010", &["AQ"]), ("012", &["DZ"]), ("016", &["AS"]), ("020", &["AD"]),
    ("024", &["AO"]), ("028", &["AG"]), ("031", &["AZ"]), ("032", &["AR"]), ("036", &["AU"]), ("040", &["AT"]),
    ("044", &["BS"]), ("048", &["BH"]), ("050", &["BD"]), ("051", &["AM"]), ("052", &["BB"]), ("056", &["BE"]),
    ("076", &["BR"]), ("100", &["BG"]), ("124", &["CA"]), ("152", &["CL"]), ("156", &["CN"]), ("170", &["CO"]),
    ("191", &["HR"]), ("203", &["CZ"]), ("208", &["DK"]), ("246", &["FI"]), ("250", &["FR"]), ("276", &["DE"]),
    ("300", &["GR"]), ("344", &["HK"]), ("348", &["HU"]), ("356", &["IN"]), ("360", &["ID"]), ("372", &["IE"]),
    ("376", &["IL"]), ("380", &["IT"]), ("392", &["JP"]), ("410", &["KR"]), ("484", &["MX"]), ("528", &["NL"]),
    ("554", &["NZ"]), ("578", &["NO"]), ("616", &["PL"]), ("620", &["PT"]), ("642", &["RO"]), ("643", &["RU"]),
    ("724", &["ES"]), ("752", &["SE"]), ("756", &["CH"]), ("792", &["TR"]), ("804", &["UA"]), ("826", &["GB"]),
    ("840", &["US"]),
];

/// The likely region of a language or script (for complex region aliases
/// and Intl.Locale maximize/minimize).
pub fn likely_region(language: &str, script: Option<&str>) -> Option<&'static str> {
    if language == "und" {
        return match script? {
            "armn" => Some("AM"),
            "geor" => Some("GE"),
            "cyrl" => Some("RU"),
            "latn" => Some("US"),
            "arab" => Some("EG"),
            "thai" => Some("TH"),
            "hani" | "hans" => Some("CN"),
            "hant" => Some("TW"),
            "jpan" | "kana" | "hira" => Some("JP"),
            "kore" | "hang" => Some("KR"),
            "grek" => Some("GR"),
            "hebr" => Some("IL"),
            "deva" => Some("IN"),
            "shaw" => Some("GB"),
            _ => None,
        };
    }
    LIKELY.iter().find(|(l, _, _)| *l == language).map(|(_, _, r)| *r)
}

/// Likely subtags: language → (script, region).
pub const LIKELY: &[(&str, &str, &str)] = &[
    ("af", "Latn", "ZA"), ("am", "Ethi", "ET"), ("ar", "Arab", "EG"), ("az", "Latn", "AZ"), ("be", "Cyrl", "BY"),
    ("bg", "Cyrl", "BG"), ("bn", "Beng", "BD"), ("bs", "Latn", "BA"), ("ca", "Latn", "ES"), ("cs", "Latn", "CZ"),
    ("cy", "Latn", "GB"), ("da", "Latn", "DK"), ("de", "Latn", "DE"), ("el", "Grek", "GR"), ("en", "Latn", "US"),
    ("es", "Latn", "ES"), ("et", "Latn", "EE"), ("eu", "Latn", "ES"), ("fa", "Arab", "IR"), ("fi", "Latn", "FI"),
    ("fil", "Latn", "PH"), ("fr", "Latn", "FR"), ("ga", "Latn", "IE"), ("gl", "Latn", "ES"), ("gu", "Gujr", "IN"),
    ("he", "Hebr", "IL"), ("hi", "Deva", "IN"), ("hr", "Latn", "HR"), ("hu", "Latn", "HU"), ("hy", "Armn", "AM"),
    ("id", "Latn", "ID"), ("is", "Latn", "IS"), ("it", "Latn", "IT"), ("ja", "Jpan", "JP"), ("ka", "Geor", "GE"),
    ("kk", "Cyrl", "KZ"), ("km", "Khmr", "KH"), ("kn", "Knda", "IN"), ("ko", "Kore", "KR"), ("ky", "Cyrl", "KG"),
    ("lo", "Laoo", "LA"), ("lt", "Latn", "LT"), ("lv", "Latn", "LV"), ("mk", "Cyrl", "MK"), ("ml", "Mlym", "IN"),
    ("mn", "Cyrl", "MN"), ("mr", "Deva", "IN"), ("ms", "Latn", "MY"), ("my", "Mymr", "MM"), ("nb", "Latn", "NO"),
    ("ne", "Deva", "NP"), ("nl", "Latn", "NL"), ("nn", "Latn", "NO"), ("no", "Latn", "NO"), ("pa", "Guru", "IN"),
    ("pl", "Latn", "PL"), ("pt", "Latn", "BR"), ("ro", "Latn", "RO"), ("ru", "Cyrl", "RU"), ("si", "Sinh", "LK"),
    ("sk", "Latn", "SK"), ("sl", "Latn", "SI"), ("sq", "Latn", "AL"), ("sr", "Cyrl", "RS"), ("sv", "Latn", "SE"),
    ("sw", "Latn", "TZ"), ("ta", "Taml", "IN"), ("te", "Telu", "IN"), ("tg", "Cyrl", "TJ"), ("th", "Thai", "TH"),
    ("tk", "Latn", "TM"), ("tr", "Latn", "TR"), ("uk", "Cyrl", "UA"), ("ur", "Arab", "PK"), ("uz", "Latn", "UZ"),
    ("vi", "Latn", "VN"), ("zh", "Hans", "CN"), ("zu", "Latn", "ZA"), ("mo", "Latn", "RO"),
];

/// Unicode extension value aliases: (key, alias, canonical).
const U_VALUE_ALIASES: &[(&str, &str, &str)] = &[
    ("ca", "ethiopic-amete-alem", "ethioaa"), ("ca", "islamicc", "islamic-civil"),
    ("ks", "primary", "level1"), ("ks", "secondary", "level2"), ("ks", "tertiary", "level3"), ("ks", "quaternary", "level4"), ("ks", "quarternary", "level4"), ("ks", "identical", "identic"),
    ("ms", "imperial", "uksystem"),
    ("tz", "cnckg", "cnsha"), ("tz", "eire", "iedub"), ("tz", "est", "papty"), ("tz", "gmt0", "gmt"), ("tz", "uct", "utc"), ("tz", "zulu", "utc"),
    ("tz", "aqams", "nzakl"), ("tz", "cnhrb", "cnsha"), ("tz", "cnkhg", "cnurc"), ("tz", "cuba", "cuhav"), ("tz", "egypt", "egcai"),
    ("tz", "hongkong", "hkhkg"), ("tz", "iceland", "isrey"), ("tz", "iran", "irthr"), ("tz", "israel", "jeruslm"), ("tz", "jamaica", "jmkin"),
    ("tz", "japan", "jptyo"), ("tz", "libya", "lytip"), ("tz", "navajo", "usden"), ("tz", "poland", "plwaw"), ("tz", "portugal", "ptlis"),
    ("tz", "prc", "cnsha"), ("tz", "roc", "twtpe"), ("tz", "rok", "krsel"), ("tz", "singapore", "sgsin"), ("tz", "turkey", "trist"),
    ("tz", "uslpi", "uschi"),
];

/// Subdivision aliases (`rg`, `sd`).
const SUBDIVISION_ALIASES: &[(&str, &str)] = &[
    ("no23", "no50"), ("cn11", "cnbj"), ("cz10a", "cz110"), ("fra", "frges"), ("frg", "frges"), ("lud", "lucl"),
    ("cn12", "cntj"), ("cn31", "cnsh"), ("cn50", "cncq"), ("no16", "no50"), ("no17", "no50"), ("frb", "frbfc"),
    ("frc", "frara"), ("frd", "frbfc"), ("fre", "frbre"), ("frf", "frcvl"), ("frh", "frcor"), ("fri", "frbfc"),
    ("frj", "fridf"), ("frk", "frocc"), ("frl", "frnaq"), ("frm", "frges"), ("frn", "frocc"), ("fro", "frhdf"),
    ("frp", "frnor"), ("frq", "frnor"), ("frr", "frpdl"), ("frs", "frhdf"), ("frt", "frnaq"), ("fru", "frpac"),
    ("frv", "frara"), ("luc", "lucl"), ("lul", "lulu"), ("lug", "lugr"),
];

fn title(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase(),
        None => String::new(),
    }
}

/// Apply language, variant, script and region aliases to a language id.
fn canonicalize_language_id(id: &mut LocaleId) {
    // Grandfathered-style language + variant replacements.
    let pairs: &[(&str, &str, &str)] = &[
        ("art", "lojban", "jbo"), ("cel", "gaulish", "xtg"), ("zh", "guoyu", "zh"), ("zh", "hakka", "hak"),
        ("zh", "xiang", "hsn"), ("hy", "arevmda", "hyw"), ("hy", "arevela", "hy"), ("sgn", "", ""),
    ];
    for (lang, var, to) in pairs {
        if id.language == *lang && !var.is_empty() {
            if let Some(p) = id.variants.iter().position(|v| v == var) {
                id.variants.remove(p);
                id.language = to.to_string();
            }
        }
    }
    // Sign languages identified by region.
    if id.language == "sgn" {
        if let Some(r) = id.region.clone() {
            let map: &[(&str, &str)] = &[("gr", "gss"), ("br", "bzs"), ("co", "csn"), ("de", "gsg"), ("dk", "dsl"), ("es", "ssp"), ("fr", "fsl"), ("gb", "bfi"), ("ie", "isg"), ("it", "ise"), ("jp", "jsl"), ("mx", "mfs"), ("ni", "ncs"), ("nl", "dse"), ("no", "nsl"), ("pt", "psr"), ("se", "swl"), ("us", "ase"), ("za", "sfs")];
            if let Some((_, l)) = map.iter().find(|(k, _)| *k == r) {
                id.language = l.to_string();
                id.region = None;
            }
        }
    }
    // Variant aliases.
    if id.variants.iter().any(|v| v == "heploc") {
        id.variants.retain(|v| v != "heploc" && v != "hepburn");
        id.variants.push("alalc97".into());
    }
    // Language aliases (possibly adding a script or region).
    if let Some((_, to)) = LANGUAGE_ALIASES.iter().find(|(f, _)| *f == id.language) {
        let mut it = to.split('-');
        id.language = it.next().unwrap().to_string();
        for p in it {
            let p = p.to_ascii_lowercase();
            if is_script(&p) {
                if id.script.is_none() {
                    id.script = Some(p);
                }
            } else if is_region(&p) && id.region.is_none() {
                id.region = Some(p);
            }
        }
    }
    if id.script.as_deref() == Some("qaai") {
        id.script = Some("zinh".into());
    }
    // Region aliases (complex ones pick the language's likely region when
    // it is a candidate).
    if let Some(r) = id.region.clone() {
        if let Some((_, to)) = REGION_ALIASES.iter().find(|(f, _)| *f == r) {
            let likely = likely_region(&id.language, id.script.as_deref());
            let pick = match likely {
                Some(l) if to.contains(&l) => l,
                _ => to[0],
            };
            id.region = Some(pick.to_ascii_lowercase());
        }
    }
    id.variants.sort();
}

/// The canonical string of a language id.
fn language_id_string(id: &LocaleId, lower: bool) -> String {
    let mut s = id.language.clone();
    if let Some(sc) = &id.script {
        s.push('-');
        s.push_str(&if lower { sc.clone() } else { title(sc) });
    }
    if let Some(r) = &id.region {
        s.push('-');
        s.push_str(&if lower { r.clone() } else { r.to_ascii_uppercase() });
    }
    for v in &id.variants {
        s.push('-');
        s.push_str(v);
    }
    s
}

/// CanonicalizeUnicodeLocaleId: the canonical tag, or None when `tag` is
/// not structurally valid.
pub fn canonicalize(tag: &str) -> Option<String> {
    let mut id = parse(tag)?;
    canonicalize_id(&mut id);
    Some(to_string(&id))
}

pub fn canonicalize_id(id: &mut LocaleId) {
    canonicalize_language_id(id);
    // -u-: sorted, deduplicated attributes; keywords sorted by key (first
    // occurrence wins), "true"/"yes" values dropped where canonical.
    id.u_attributes.sort();
    id.u_attributes.dedup();
    let mut kws: Vec<(String, String)> = Vec::new();
    for (k, v) in id.u_keywords.drain(..) {
        if kws.iter().any(|(x, _)| *x == k) {
            continue;
        }
        let mut v = v;
        if v == "true" || (v == "yes" && matches!(k.as_str(), "kb" | "kc" | "kh" | "kk" | "kn")) {
            v = String::new();
        }
        if let Some((_, _, to)) = U_VALUE_ALIASES.iter().find(|(key, from, _)| *key == k && *from == v) {
            v = to.to_string();
        }
        if matches!(k.as_str(), "rg" | "sd") {
            if let Some((_, to)) = SUBDIVISION_ALIASES.iter().find(|(f, _)| *f == v) {
                v = to.to_string();
            }
        }
        kws.push((k, v));
    }
    kws.sort_by(|a, b| a.0.cmp(&b.0));
    id.u_keywords = kws;
    // -t-: canonical tlang (lower case), fields sorted by key.
    if let Some(tl) = &mut id.t_lang {
        canonicalize_language_id(tl);
    }
    for (k, v) in id.t_fields.iter_mut() {
        if k == "m0" && v == "names" {
            *v = "prprname".into();
        }
    }
    id.t_fields.sort_by(|a, b| a.0.cmp(&b.0));
    id.other.sort_by(|a, b| a.0.cmp(&b.0));
}

/// The tag of a (canonicalized) locale id.
pub fn to_string(id: &LocaleId) -> String {
    let mut s = language_id_string(id, false);
    let mut exts: Vec<(char, String)> = Vec::new();
    if id.has_t || id.t_lang.is_some() || !id.t_fields.is_empty() {
        let mut t = String::new();
        if let Some(tl) = &id.t_lang {
            t.push_str(&language_id_string(tl, true));
        }
        for (k, v) in &id.t_fields {
            if !t.is_empty() {
                t.push('-');
            }
            t.push_str(k);
            t.push('-');
            t.push_str(v);
        }
        exts.push(('t', t));
    }
    if !id.u_attributes.is_empty() || !id.u_keywords.is_empty() {
        let mut u: Vec<String> = id.u_attributes.clone();
        for (k, v) in &id.u_keywords {
            u.push(k.clone());
            if !v.is_empty() {
                u.push(v.clone());
            }
        }
        exts.push(('u', u.join("-")));
    }
    for (c, subs) in &id.other {
        exts.push((*c, subs.join("-")));
    }
    exts.sort_by(|a, b| a.0.cmp(&b.0));
    for (c, body) in exts {
        s.push('-');
        s.push(c);
        s.push('-');
        s.push_str(&body);
    }
    if !id.private.is_empty() {
        s.push_str("-x-");
        s.push_str(&id.private.join("-"));
    }
    s
}

/// The tag without any extensions (the locale's base name).
pub fn base_name(id: &LocaleId) -> String {
    language_id_string(id, false)
}
