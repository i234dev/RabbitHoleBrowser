//! `.wast` scripts (the specification test suite format).

use super::{err, parse_sexprs, Sx};
use crate::Error;

#[derive(Clone, Debug)]
pub enum ModuleSrc {
    /// Module fields in text form.
    Text(Vec<Sx>),
    Binary(Vec<u8>),
    Quote(String),
}

#[derive(Clone, Debug)]
pub struct ModuleDecl {
    pub id: Option<String>,
    pub src: ModuleSrc,
    /// `(module definition ...)`: compile only.
    pub definition: bool,
}

impl ModuleDecl {
    /// The binary encoding of this module.
    pub fn to_binary(&self) -> Result<Vec<u8>, Error> {
        match &self.src {
            ModuleSrc::Binary(b) => Ok(b.clone()),
            ModuleSrc::Text(fields) => super::wat::encode_module(fields),
            ModuleSrc::Quote(q) => {
                let sx = parse_sexprs(q)?;
                super::wat::encode_module(&sx)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    Invoke { module: Option<String>, name: String, args: Vec<Sx> },
    Get { module: Option<String>, name: String },
}

#[derive(Clone, Debug)]
pub enum Cmd {
    Module(ModuleDecl),
    Instance { id: Option<String>, def: Option<String> },
    Register { name: String, module: Option<String> },
    Action(Action),
    AssertReturn(Action, Vec<Sx>),
    AssertTrap(Action, String),
    AssertTrapModule(ModuleDecl, String),
    AssertExhaustion(Action, String),
    AssertException(Action),
    AssertInvalid(ModuleDecl, String),
    AssertMalformed(ModuleDecl, String),
    AssertUnlinkable(ModuleDecl, String),
    /// Unsupported command (threads...).
    Other(String),
}

fn module_decl(l: &[Sx]) -> Result<ModuleDecl, Error> {
    let mut i = 1;
    let mut definition = false;
    if l.get(i).and_then(|s| s.atom()) == Some("definition") {
        definition = true;
        i += 1;
    }
    let id = match l.get(i) {
        Some(Sx::Id(n)) => {
            i += 1;
            Some(n.clone())
        }
        _ => None,
    };
    let src = match l.get(i).and_then(|s| s.atom()) {
        Some("binary") => {
            let mut b = Vec::new();
            for s in &l[i + 1..] {
                match s {
                    Sx::Str(x) => b.extend_from_slice(x),
                    _ => return err("expected string"),
                }
            }
            ModuleSrc::Binary(b)
        }
        Some("quote") => {
            let mut b = Vec::new();
            for s in &l[i + 1..] {
                match s {
                    Sx::Str(x) => {
                        b.extend_from_slice(x);
                        b.push(b' ');
                    }
                    _ => return err("expected string"),
                }
            }
            ModuleSrc::Quote(String::from_utf8_lossy(&b).into_owned())
        }
        _ => ModuleSrc::Text(l[i..].to_vec()),
    };
    Ok(ModuleDecl { id, src, definition })
}

fn action(sx: &Sx) -> Result<Action, Error> {
    let l = sx.list().ok_or_else(|| Error::Malformed("expected action".into()))?;
    let mut i = 1;
    let module = match l.get(1) {
        Some(Sx::Id(n)) => {
            i = 2;
            Some(n.clone())
        }
        _ => None,
    };
    let name = match l.get(i) {
        Some(Sx::Str(s)) => String::from_utf8_lossy(s).into_owned(),
        _ => return err("expected export name"),
    };
    match sx.head() {
        Some("invoke") => Ok(Action::Invoke { module, name, args: l[i + 1..].to_vec() }),
        Some("get") => Ok(Action::Get { module, name }),
        _ => err("unknown action"),
    }
}

fn msg(l: &[Sx], i: usize) -> String {
    match l.get(i) {
        Some(Sx::Str(s)) => String::from_utf8_lossy(s).into_owned(),
        _ => String::new(),
    }
}

/// Parse a script.
pub fn parse_script(text: &str) -> Result<Vec<Cmd>, Error> {
    let sx = parse_sexprs(text)?;
    let mut cmds = Vec::new();
    for s in &sx {
        let Some(l) = s.list() else { return err("unexpected token in script") };
        let cmd = match s.head() {
            Some("module") => {
                if l.get(1).and_then(|s| s.atom()) == Some("instance") {
                    let mut ids = l[2..].iter().filter_map(|s| match s {
                        Sx::Id(n) => Some(n.clone()),
                        _ => None,
                    });
                    let a = ids.next();
                    let b = ids.next();
                    match b {
                        Some(def) => Cmd::Instance { id: a, def: Some(def) },
                        None => Cmd::Instance { id: None, def: a },
                    }
                } else {
                    Cmd::Module(module_decl(l)?)
                }
            }
            Some("register") => {
                let name = msg(l, 1);
                let module = match l.get(2) {
                    Some(Sx::Id(n)) => Some(n.clone()),
                    _ => None,
                };
                Cmd::Register { name, module }
            }
            Some("invoke") | Some("get") => Cmd::Action(action(s)?),
            Some("assert_return") => Cmd::AssertReturn(action(&l[1])?, l[2..].to_vec()),
            Some("assert_trap") => {
                if l[1].is_list_of("module") {
                    Cmd::AssertTrapModule(module_decl(l[1].list().unwrap())?, msg(l, 2))
                } else {
                    Cmd::AssertTrap(action(&l[1])?, msg(l, 2))
                }
            }
            Some("assert_exhaustion") => Cmd::AssertExhaustion(action(&l[1])?, msg(l, 2)),
            Some("assert_exception") => Cmd::AssertException(action(&l[1])?),
            Some("assert_invalid") => Cmd::AssertInvalid(module_decl(l[1].list().unwrap_or(&[]))?, msg(l, 2)),
            Some("assert_malformed") => Cmd::AssertMalformed(module_decl(l[1].list().unwrap_or(&[]))?, msg(l, 2)),
            Some("assert_unlinkable") => Cmd::AssertUnlinkable(module_decl(l[1].list().unwrap_or(&[]))?, msg(l, 2)),
            Some(h) => Cmd::Other(h.to_string()),
            None => return err("unexpected token in script"),
        };
        cmds.push(cmd);
    }
    Ok(cmds)
}
