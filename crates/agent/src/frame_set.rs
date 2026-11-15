//! Flat agent indices over the engine's single, owned browsing-context tree.
//! No document is copied and no pointer into a movable Page is retained.

use std::ops::{Index, IndexMut};
use rabbithole_engine::{Engine, Page};
use rabbithole_engine::dom::{Document, DocumentWeak, NodeId};
use rabbithole_engine::frames::FrameSource;
use crate::browser::Frame;

struct Entry {
    id: u64,
    parent: Option<(usize, NodeId)>,
    document: DocumentWeak,
}

pub struct FrameRef<'a> {
    pub page: &'a Page,
    pub parent: Option<(usize, NodeId)>,
}

#[derive(Default)]
pub struct FrameSet {
    root: Option<Page>,
    entries: Vec<Entry>,
}

fn page_by_id(page: &Page, id: u64) -> Option<&Page> {
    if page.frame_id == id { return Some(page); }
    page.frames.children.iter().filter_map(|child| child.page.as_deref())
        .find_map(|child| page_by_id(child, id))
}

fn allowed(url: &rabbithole_engine::net::Url, allowlist: &[String]) -> bool {
    allowlist.is_empty() || allowlist.iter().any(|domain| {
        let domain=domain.trim().trim_start_matches("*.").to_ascii_lowercase();
        let host=url.host.to_ascii_lowercase();
        host==domain || host.ends_with(&format!(".{domain}"))
    })
}

impl FrameSet {
    pub fn from_page(page: Page) -> Self {
        let entry=Entry { id:page.frame_id, parent:None, document:page.doc.weak_identity() };
        Self { root:Some(page), entries:vec![entry] }
    }

    /// Compatibility insertion for a newly created top-level tab only.
    pub fn push(&mut self, frame: Frame) {
        assert!(self.root.is_none() && frame.parent.is_none(), "a tab owns one top-level page");
        *self=Self::from_page(frame.page);
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.root.is_none() }
    pub fn root(&self) -> Option<&Page> { self.root.as_ref() }
    pub fn root_mut(&mut self) -> Option<&mut Page> { self.root.as_mut() }
    pub fn get(&self, index: usize) -> Option<FrameRef<'_>> {
        let entry=self.entries.get(index)?;
        Some(FrameRef { page:page_by_id(self.root.as_ref()?,entry.id)?, parent:entry.parent })
    }
    pub fn document(&self,index:usize)->Option<DocumentWeak> { self.entries.get(index).map(|entry|entry.document.clone()) }
    pub fn first(&self) -> Option<FrameRef<'_>> { self.get(0) }
    pub fn iter(&self) -> impl Iterator<Item=FrameRef<'_>> {
        (0..self.entries.len()).filter_map(|index| self.get(index))
    }

    /// Rebuild only metadata; loading, DOM and script ownership remain in Engine.
    /// A changed frame/document order invalidates old flat refs before reuse.
    pub fn refresh(&mut self, allowlist: &[String]) -> Vec<usize> {
        let mut entries=Vec::new();
        if let Some(root)=self.root.as_ref() {
            entries.push(Entry { id:root.frame_id,parent:None,document:root.doc.weak_identity() });
            let mut index=0;
            while index<entries.len() && entries.len()<12 {
                let mut depth=0;
                let mut parent=entries[index].parent;
                while let Some((ancestor,_))=parent { depth+=1;parent=entries[ancestor].parent; }
                if depth<3 {
                    if let Some(page)=page_by_id(root,entries[index].id) {
                        let doc=&page.doc;
                        for node in doc.descendants(Document::ROOT) {
                            if entries.len()>=12 { break; }
                            if !matches!(doc.tag(node),Some("iframe"|"frame")) || !crate::ax::displayed(page,node) { continue; }
                            let Some(child)=page.frames.children.iter().find(|child| child.node==node) else { continue };
                            let permitted=match &child.source {
                                FrameSource::Blank|FrameSource::Srcdoc(_) => true,
                                FrameSource::Url(url) => (url.is_http() || url.scheme=="data") && allowed(url,allowlist),
                            };
                            if !permitted { continue; }
                            let Some(child_page)=child.page.as_deref() else { continue };
                            if child_page.url.is_http() && !allowed(&child_page.url,allowlist) { continue; }
                            entries.push(Entry { id:child.id,parent:Some((index,node)),document:child_page.doc.weak_identity() });
                        }
                    }
                }
                index+=1;
            }
        }
        let changed=(0..self.entries.len().max(entries.len())).filter(|&index| {
            match (self.entries.get(index),entries.get(index)) {
                (Some(old),Some(new)) => old.id!=new.id || old.parent!=new.parent || self.root.as_ref()
                    .and_then(|root|page_by_id(root,new.id)).is_none_or(|page|!old.document.is_for(&page.doc)),
                _ => true,
            }
        }).collect();
        self.entries=entries;
        changed
    }

    /// The engine detaches the selected child while retaining every ancestor
    /// on its existing dynamic guard, then reinserts it before returning.
    pub fn with_page<R>(&mut self, engine: &Engine, index: usize, run: impl FnOnce(&mut Page)->R) -> Result<R,String> {
        let id=self.entries.get(index).ok_or("the target frame is gone; take a new snapshot")?.id;
        let root=self.root.as_mut().ok_or("the tab has no page")?;
        engine.with_browsing_context(root,id,run).ok_or_else(||"the target frame is gone; take a new snapshot".into())
    }

    pub fn install(&mut self,engine:&Engine,index:usize,loaded:rabbithole_engine::Loaded)->Result<rabbithole_engine::ScriptOutcome,String> {
        let id=self.entries.get(index).ok_or("the target frame is gone")?.id;
        let root=self.root.as_mut().ok_or("the tab has no page")?;
        engine.install_browsing_context(root,id,loaded).ok_or_else(||"the target frame is gone".into())
    }

    pub fn visit_mut(&mut self, mut visit: impl FnMut(&mut Page)) {
        fn walk(page:&mut Page,visit:&mut dyn FnMut(&mut Page)) {
            visit(page);
            for child in &mut page.frames.children {
                if let Some(child)=child.page.as_deref_mut() {walk(child,visit);}
            }
        }
        if let Some(root)=self.root.as_mut() {walk(root,&mut visit);}
    }
}

// Indexing is for a frame checked by resolve/get in the same synchronous
// operation. Script-running mutation uses with_page and handles a missing ID.
impl Index<usize> for FrameSet {
    type Output=Page;
    fn index(&self,index:usize)->&Page { self.get(index).expect("checked frame index").page }
}
impl IndexMut<usize> for FrameSet {
    fn index_mut(&mut self,index:usize)->&mut Page {
        assert_eq!(index,0,"child script activity requires with_page");
        self.root.as_mut().expect("loaded tab")
    }
}
