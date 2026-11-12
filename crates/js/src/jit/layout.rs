//! Field offsets of runtime structures that native code reads directly
//! (object table, property maps, prototypes), probed once at run time:
//! Rust does not fix the layout of `Vec`, `Option` or enum variants, so
//! instead of assuming one, each is measured on live instances and checked.
//! When a probe fails, native code keeps using helpers for these accesses.

use crate::heap::{Heap, Object, ObjectKind, Property};
use crate::realm::Realm;
use crate::shape::PropertyMap;
use crate::value::ObjectId;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Layout {
    /// In `Realm`: the object table's buffer pointer and length.
    pub heap_ptr: i32,
    pub heap_len: i32,
    /// Size of an object table entry (`Option<Object>`).
    pub obj_size: i32,
    /// In an object table entry holding an object: the property map's
    /// shape id, values pointer and length, and the prototype
    /// (`Option<ObjectId>`: id word and tag word, and the tag of `Some`).
    pub sid: i32,
    pub vals_ptr: i32,
    pub vals_len: i32,
    pub proto_id: i32,
    pub proto_tag: i32,
    pub proto_some: u32,
    /// In `Realm`: the frame stack's buffer pointer and length; size of a
    /// frame and offset of its `this`.
    pub frames_ptr: i32,
    pub frames_len: i32,
    pub frame_size: i32,
    pub frame_this: i32,
    /// Offset of the frame's scope id.
    pub frame_scope: i32,
    /// In an object table entry: the byte that tells a scope object
    /// (`ObjectKind::Scope`) and its value, and the `Box<ScopeData>`.
    pub scope_tag: i32,
    pub scope_tag_value: u8,
    pub scope_box: i32,
    /// In an object table entry: the tag byte value of arrays
    /// (`ObjectKind::Array`, same byte as `scope_tag`) and their
    /// elements' pointer and length.
    pub array_tag_value: u8,
    pub array_elems_ptr: i32,
    pub array_elems_len: i32,
    /// In `ScopeData`: slots pointer and length, parent id and tag.
    pub sd_slots_ptr: i32,
    pub sd_slots_len: i32,
    pub sd_parent_id: i32,
    pub sd_parent_tag: i32,
    /// `Property`: size and offsets.
    pub prop_size: i32,
    pub prop_value: i32,
    pub prop_flags: i32,
}

/// Word indices of the buffer pointer and length in a `Vec<T>`.
fn vec_words<T>(mut v: Vec<T>, one: T) -> Option<(usize, usize)> {
    v.reserve_exact(5);
    v.push(one);
    let ptr = v.as_ptr() as usize;
    let (len, cap) = (v.len(), v.capacity());
    if std::mem::size_of::<Vec<T>>() != 3 * std::mem::size_of::<usize>() || cap == len {
        return None;
    }
    // SAFETY: a `Vec` is three words (checked above); read them in place.
    let words: [usize; 3] = unsafe { std::ptr::read(&v as *const Vec<T> as *const [usize; 3]) };
    let p = words.iter().position(|&w| w == ptr)?;
    let l = words.iter().position(|&w| w == len)?;
    if words.iter().filter(|&&w| w == len).count() != 1 || words.iter().filter(|&&w| w == ptr).count() != 1 {
        return None;
    }
    Some((p, l))
}

fn probe() -> Option<Layout> {
    let w = std::mem::size_of::<usize>();
    let i32_of = |x: usize| i32::try_from(x).ok();
    // Object table entry: where the object sits in `Some(object)`.
    let entry = Some(Object::new(Some(ObjectId(0x1234_5678)), ObjectKind::Ordinary));
    let base = &entry as *const Option<Object> as usize;
    let obj = entry.as_ref()? as *const Object as usize - base;
    let props = obj + std::mem::offset_of!(Object, props);
    // Prototype: `Option<ObjectId>` as two 32-bit words.
    if std::mem::size_of::<Option<ObjectId>>() != 8 {
        return None;
    }
    let words = |o: Option<ObjectId>| -> [u32; 2] {
        // SAFETY: the size is checked above; any bit pattern is a valid [u32; 2].
        unsafe { std::mem::transmute::<Option<ObjectId>, [u32; 2]>(o) }
    };
    let (a, b, none) = (words(Some(ObjectId(0x1234_5678))), words(Some(ObjectId(0x0765_4321))), words(None));
    let id_word = (0..2).find(|&i| a[i] == 0x1234_5678 && b[i] == 0x0765_4321)?;
    let tag_word = 1 - id_word;
    if a[tag_word] != b[tag_word] || none[tag_word] == a[tag_word] {
        return None;
    }
    // Check on the probe object that the tag is where it was measured.
    let proto = obj + std::mem::offset_of!(Object, proto);
    // SAFETY: `proto` is within the probe entry.
    let seen = unsafe { std::ptr::read_unaligned((base + proto + id_word * 4) as *const u32) };
    if seen != 0x1234_5678 {
        return None;
    }
    // Property map values.
    let (vp, vl) = vec_words::<Property>(Vec::new(), Property::data(crate::value::Value::Undefined, 0))?;
    let (hp, hl) = vec_words::<Option<Object>>(Vec::new(), None)?;
    let heap = std::mem::offset_of!(Realm, heap) + Heap::OBJECTS_OFFSET;
    let frame = crate::vm::Frame {
        code: std::ptr::NonNull::dangling(),
        pc: 0,
        base: 0,
        argc: 0,
        scope: ObjectId(0),
        scope_depth: 0,
        this: crate::value::Value::Undefined,
        func: None,
        new_target: crate::value::Value::Undefined,
        ret_slot: 0,
        is_construct: false,
        entry: false,
        extra: None,
    };
    let (fp, fl) = vec_words::<crate::vm::Frame>(Vec::new(), frame)?;
    let frames = std::mem::offset_of!(Realm, frames);
    // Scope objects: where the variant's tag byte and its box are.
    let kind = obj + std::mem::offset_of!(Object, kind);
    let ksize = std::mem::size_of::<ObjectKind>();
    let scope = |parent: Option<ObjectId>, n: usize| {
        Some(Object::new(None, ObjectKind::Scope(Box::new(crate::heap::ScopeData { slots: vec![crate::value::Value::Undefined; n], parent, names: None }))))
    };
    let bytes = |e: &Option<Object>| -> Vec<u8> {
        let b = e as *const Option<Object> as *const u8;
        // SAFETY: reading the bytes of a live value (padding may be
        // uninitialized in theory; only compared, never interpreted).
        (0..ksize).map(|i| unsafe { std::ptr::read_volatile(b.add(kind + i)) }).collect()
    };
    let (s1, s2) = (scope(Some(ObjectId(0x1122_3344)), 3), scope(None, 0));
    let others = [
        Some(Object::new(None, ObjectKind::Ordinary)),
        Some(Object::new(None, ObjectKind::Array { elements: { let mut e = vec![crate::value::Value::Number(1.5); 3]; e.reserve_exact(5); e }, length: 3 })),
        Some(Object::new(None, ObjectKind::Array { elements: Vec::new(), length: 0x7fff_ffff })),
    ];
    let box_addr = |e: &Option<Object>| match &e.as_ref()?.kind {
        ObjectKind::Scope(b) => Some(&**b as *const crate::heap::ScopeData as usize),
        _ => None,
    };
    let (b1, b2) = (box_addr(&s1)?, box_addr(&s2)?);
    let (k1, k2) = (bytes(&s1), bytes(&s2));
    let word = |k: &[u8], i: usize| usize::from_ne_bytes(k[i..i + w].try_into().unwrap());
    let box_off = (0..=ksize - w).step_by(w).find(|&i| word(&k1, i) == b1 && word(&k2, i) == b2)?;
    let ob: Vec<Vec<u8>> = others.iter().map(bytes).collect();
    let tag = (0..ksize).filter(|i| !(box_off..box_off + w).contains(i)).find(|&i| k1[i] == k2[i] && ob.iter().all(|o| o[i] != k1[i]) && ob[1][i] == ob[2][i])?;
    // Arrays: the same tag byte tells them apart from scopes and plain
    // objects; their elements vector is found by its pointer and length.
    let a1 = &others[1];
    let ka = bytes(a1);
    let (ka2, k0) = (bytes(&others[2]), &ob[0]);
    if ka[tag] != ka2[tag] || ka[tag] == k1[tag] || ka[tag] == k0[tag] {
        return None;
    }
    let (eptr, elen) = match &a1.as_ref()?.kind {
        ObjectKind::Array { elements, .. } => (elements.as_ptr() as usize, elements.len()),
        _ => return None,
    };
    let ep = (0..=ksize - w).step_by(w).find(|&i| word(&ka, i) == eptr)?;
    let el = (0..=ksize - w).step_by(w).find(|&i| word(&ka, i) == elen && word(&ka2, i) == 0)?;
    if (0..=ksize - w).step_by(w).filter(|&i| word(&ka, i) == elen && word(&ka2, i) == 0).count() != 1 {
        return None;
    }
    if [ep, el].iter().any(|&o| (tag..tag + 1).contains(&o) || (o <= tag && tag < o + w)) {
        return None;
    }
    let sd = std::mem::offset_of!(crate::heap::ScopeData, slots);
    let (sp, sl) = vec_words::<crate::value::Value>(Vec::new(), crate::value::Value::Undefined)?;
    let parent = std::mem::offset_of!(crate::heap::ScopeData, parent);
    Some(Layout {
        heap_ptr: i32_of(heap + hp * w)?,
        heap_len: i32_of(heap + hl * w)?,
        obj_size: i32_of(std::mem::size_of::<Option<Object>>())?,
        sid: i32_of(props + PropertyMap::SID_OFFSET)?,
        vals_ptr: i32_of(props + PropertyMap::VALUES_OFFSET + vp * w)?,
        vals_len: i32_of(props + PropertyMap::VALUES_OFFSET + vl * w)?,
        proto_id: i32_of(proto + id_word * 4)?,
        proto_tag: i32_of(proto + tag_word * 4)?,
        proto_some: a[tag_word],
        frames_ptr: i32_of(frames + fp * w)?,
        frames_len: i32_of(frames + fl * w)?,
        frame_size: i32_of(std::mem::size_of::<crate::vm::Frame>())?,
        frame_this: i32_of(std::mem::offset_of!(crate::vm::Frame, this))?,
        frame_scope: i32_of(std::mem::offset_of!(crate::vm::Frame, scope))?,
        scope_tag: i32_of(kind + tag)?,
        scope_tag_value: k1[tag],
        scope_box: i32_of(kind + box_off)?,
        array_tag_value: ka[tag],
        array_elems_ptr: i32_of(kind + ep)?,
        array_elems_len: i32_of(kind + el)?,
        sd_slots_ptr: i32_of(sd + sp * w)?,
        sd_slots_len: i32_of(sd + sl * w)?,
        sd_parent_id: i32_of(parent + id_word * 4)?,
        sd_parent_tag: i32_of(parent + tag_word * 4)?,
        prop_size: i32_of(std::mem::size_of::<Property>())?,
        prop_value: i32_of(std::mem::offset_of!(Property, value))?,
        prop_flags: i32_of(std::mem::offset_of!(Property, flags))?,
    })
}

/// The probed layout (`None`: inline object access is off).
pub(crate) fn get() -> Option<Layout> {
    static L: std::sync::OnceLock<Option<Layout>> = std::sync::OnceLock::new();
    *L.get_or_init(|| if std::env::var_os("RABBITHOLE_JIT_NO_INLINE_IC").is_some() { None } else { probe() })
}
