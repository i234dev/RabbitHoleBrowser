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
    /// In `Realm`: the collector's mark bytes (buffer pointer).
    pub marks_ptr: i32,
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
    /// (`ObjectKind::Scope`) and its value, and the `ScopeBox` pointer.
    pub scope_tag: i32,
    pub scope_tag_value: u64,
    /// How variants are told apart: `false`, an explicit tag byte at
    /// `scope_tag` (compare one byte); `true`, a niche in the array
    /// variant's capacity word at `scope_tag` (scopes have one 64-bit
    /// value there, every other variant has the top bit set, arrays have
    /// it clear).
    pub tag_niche: bool,
    pub scope_box: i32,
    /// In an object table entry: the tag byte value of arrays
    /// (`ObjectKind::Array`, same byte as `scope_tag`) and their
    /// elements' pointer and length.
    pub array_tag_value: u8,
    pub array_elems_ptr: i32,
    pub array_elems_len: i32,
    /// In an object table entry holding an array: its `length` (u32).
    pub array_length: i32,
    /// In `Realm`: the megamorphic cache's buffer pointer and length.
    pub mega_ptr: i32,
    pub mega_len: i32,
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

fn probe() -> Result<Layout, u32> {
    let w = std::mem::size_of::<usize>();
    let i32_of = |x: usize| i32::try_from(x).ok();
    // Object table entry: where the object sits in `Some(object)`.
    let entry = Some(Object::new(Some(ObjectId(0x1234_5678)), ObjectKind::Ordinary));
    let base = &entry as *const Option<Object> as usize;
    let obj = entry.as_ref().ok_or(1u32)? as *const Object as usize - base;
    let props = obj + std::mem::offset_of!(Object, props);
    // Prototype: `Option<ObjectId>` as two 32-bit words.
    if std::mem::size_of::<Option<ObjectId>>() != 8 {
        return Err(1000 + line!());
    }
    let words = |o: Option<ObjectId>| -> [u32; 2] {
        // SAFETY: the size is checked above; any bit pattern is a valid [u32; 2].
        unsafe { std::mem::transmute::<Option<ObjectId>, [u32; 2]>(o) }
    };
    let (a, b, none) = (words(Some(ObjectId(0x1234_5678))), words(Some(ObjectId(0x0765_4321))), words(None));
    let id_word = (0..2).find(|&i| a[i] == 0x1234_5678 && b[i] == 0x0765_4321).ok_or(2u32)?;
    let tag_word = 1 - id_word;
    if a[tag_word] != b[tag_word] || none[tag_word] == a[tag_word] {
        return Err(1000 + line!());
    }
    // Check on the probe object that the tag is where it was measured.
    let proto = obj + std::mem::offset_of!(Object, proto);
    // SAFETY: `proto` is within the probe entry.
    let seen = unsafe { std::ptr::read_unaligned((base + proto + id_word * 4) as *const u32) };
    if seen != 0x1234_5678 {
        return Err(1000 + line!());
    }
    // Property map values.
    let (vp, vl) = vec_words::<Property>(Vec::new(), Property::data(crate::value::Value::Undefined, 0)).ok_or(3u32)?;
    let (hp, hl) = vec_words::<Option<Object>>(Vec::new(), None).ok_or(4u32)?;
    let heap = std::mem::offset_of!(Realm, heap) + Heap::OBJECTS_OFFSET;
    let (mp, _) = vec_words::<u8>(Vec::new(), 7).ok_or(39u32)?;
    let marks = std::mem::offset_of!(Realm, heap) + Heap::MARKS_OFFSET;
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
    let (fp, fl) = vec_words::<crate::vm::Frame>(Vec::new(), frame).ok_or(5u32)?;
    let frames = std::mem::offset_of!(Realm, frames);
    let (gp, gl) = vec_words::<crate::ic::MegaEntry>(Vec::new(), crate::ic::MegaEntry::default()).ok_or(42u32)?;
    let mega = std::mem::offset_of!(Realm, mega);
    // Scope objects: where the variant's tag byte and its box are.
    let kind = obj + std::mem::offset_of!(Object, kind);
    let ksize = std::mem::size_of::<ObjectKind>();
    let scope = |parent: Option<ObjectId>, n: usize| {
        Some(Object::new(None, ObjectKind::Scope(crate::heap::ScopeBox::new(n, parent, None))))
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
    let (b1, b2) = (box_addr(&s1).ok_or(7u32)?, box_addr(&s2).ok_or(8u32)?);
    let (k1, k2) = (bytes(&s1), bytes(&s2));
    let word = |k: &[u8], i: usize| usize::from_ne_bytes(k[i..i + w].try_into().unwrap());
    let box_off = (0..=ksize - w).step_by(w).find(|&i| word(&k1, i) == b1 && word(&k2, i) == b2).ok_or(9u32)?;
    let ob: Vec<Vec<u8>> = others.iter().map(bytes).collect();
    let a1 = &others[1];
    let ka = bytes(a1);
    let (ka2, k0) = (bytes(&others[2]), &ob[0]);
    let top = 1usize << (usize::BITS - 1);
    // An explicit tag byte (equal in both scopes and both arrays, different
    // between kinds), else a niche in the arrays' capacity word.
    let byte_tag = (0..ksize).filter(|i| !(box_off..box_off + w).contains(i)).find(|&i| k1[i] == k2[i] && ob.iter().all(|o| o[i] != k1[i]) && ka[i] == ka2[i] && ka[i] != k0[i]);
    let (tag, tag_niche) = match byte_tag {
        Some(t) => (t, false),
        None => {
            let t = (0..=ksize - w)
                .step_by(w)
                .filter(|i| !(box_off..box_off + w).contains(i))
                .find(|&i| word(&k1, i) == word(&k2, i) && word(&k1, i) & top != 0 && word(k0, i) & top != 0 && word(k0, i) != word(&k1, i) && word(&ka, i) & top == 0 && word(&ka2, i) & top == 0 && word(&ka, i) != word(&ka2, i))
                .ok_or(10u32)?;
            (t, true)
        }
    };
    // Arrays: their elements vector is found by its pointer and length.
    let (eptr, elen) = match &a1.as_ref().ok_or(11u32)?.kind {
        ObjectKind::Array { elements, .. } => (elements.as_ptr() as usize, elements.len()),
        _ => return Err(11u32),
    };
    let ep = (0..=ksize - w).step_by(w).find(|&i| word(&ka, i) == eptr).ok_or(12u32)?;
    let el = (0..=ksize - w).step_by(w).find(|&i| word(&ka, i) == elen && word(&ka2, i) == 0).ok_or(13u32)?;
    if (0..=ksize - w).step_by(w).filter(|&i| word(&ka, i) == elen && word(&ka2, i) == 0).count() != 1 {
        return Err(1000 + line!());
    }
    if [ep, el].iter().any(|&o| o <= tag && tag < o + w) {
        return Err(1000 + line!());
    }
    // Arrays: `length` is the one 32-bit word that is 3 in the first
    // probe array and 0x7fff_ffff in the second.
    let u32_at = |k: &[u8], i: usize| u32::from_ne_bytes(k[i..i + 4].try_into().unwrap());
    let alen = (0..=ksize - 4).step_by(4).filter(|&i| u32_at(&ka, i) == 3 && u32_at(&ka2, i) == 0x7fff_ffff).collect::<Vec<_>>();
    let [alen] = alen[..] else { return Err(1000 + line!()) };
    if [ep, el, tag].iter().any(|&o| alen < o + w && o < alen + 4) {
        return Err(1000 + line!());
    }
    let sd = std::mem::offset_of!(crate::heap::ScopeData, slots);
    let (sp, sl) = crate::heap::ScopeSlots::WORDS;
    // Check them on the probe scope.
    {
        let ObjectKind::Scope(b) = &s1.as_ref().ok_or(14u32)?.kind else { return Err(14) };
        let at = |i: usize| -> usize {
            // SAFETY: reading a word of the live `ScopeData` within its size.
            unsafe { std::ptr::read((b1 + sd + i * w) as *const usize) }
        };
        if at(sp) != b.slots.as_ptr() as usize || at(sl) != 3 {
            return Err(1000 + line!());
        }
    }
    let parent = std::mem::offset_of!(crate::heap::ScopeData, parent);
    Ok(Layout {
        heap_ptr: i32_of(heap + hp * w).ok_or(15u32)?,
        heap_len: i32_of(heap + hl * w).ok_or(16u32)?,
        marks_ptr: i32_of(marks + mp * w).ok_or(40u32)?,
        obj_size: i32_of(std::mem::size_of::<Option<Object>>()).ok_or(17u32)?,
        sid: i32_of(props + PropertyMap::SID_OFFSET).ok_or(18u32)?,
        vals_ptr: i32_of(props + PropertyMap::VALUES_OFFSET + vp * w).ok_or(19u32)?,
        vals_len: i32_of(props + PropertyMap::VALUES_OFFSET + vl * w).ok_or(20u32)?,
        proto_id: i32_of(proto + id_word * 4).ok_or(21u32)?,
        proto_tag: i32_of(proto + tag_word * 4).ok_or(22u32)?,
        proto_some: a[tag_word],
        frames_ptr: i32_of(frames + fp * w).ok_or(23u32)?,
        frames_len: i32_of(frames + fl * w).ok_or(24u32)?,
        frame_size: i32_of(std::mem::size_of::<crate::vm::Frame>()).ok_or(25u32)?,
        frame_this: i32_of(std::mem::offset_of!(crate::vm::Frame, this)).ok_or(26u32)?,
        frame_scope: i32_of(std::mem::offset_of!(crate::vm::Frame, scope)).ok_or(27u32)?,
        scope_tag: i32_of(kind + tag).ok_or(28u32)?,
        scope_tag_value: if tag_niche { word(&k1, tag) as u64 } else { k1[tag] as u64 },
        tag_niche,
        scope_box: i32_of(kind + box_off).ok_or(29u32)?,
        array_tag_value: if tag_niche { 0 } else { ka[tag] },
        array_elems_ptr: i32_of(kind + ep).ok_or(30u32)?,
        array_elems_len: i32_of(kind + el).ok_or(31u32)?,
        array_length: i32_of(kind + alen).ok_or(41u32)?,
        mega_ptr: i32_of(mega + gp * w).ok_or(43u32)?,
        mega_len: i32_of(mega + gl * w).ok_or(44u32)?,
        sd_slots_ptr: i32_of(sd + sp * w).ok_or(32u32)?,
        sd_slots_len: i32_of(sd + sl * w).ok_or(33u32)?,
        sd_parent_id: i32_of(parent + id_word * 4).ok_or(34u32)?,
        sd_parent_tag: i32_of(parent + tag_word * 4).ok_or(35u32)?,
        prop_size: i32_of(std::mem::size_of::<Property>()).ok_or(36u32)?,
        prop_value: i32_of(std::mem::offset_of!(Property, value)).ok_or(37u32)?,
        prop_flags: i32_of(std::mem::offset_of!(Property, flags)).ok_or(38u32)?,
    })
}

/// The probed layout (`None`: inline object access is off).
pub(crate) fn get() -> Option<Layout> {
    static L: std::sync::OnceLock<Option<Layout>> = std::sync::OnceLock::new();
    *L.get_or_init(|| {
        if std::env::var_os("RABBITHOLE_JIT_NO_INLINE_IC").is_some() {
            return None;
        }
        match probe() {
            Ok(l) => Some(l),
            Err(step) => {
                if super::options().trace {
                    eprintln!("[jit] object layout probe failed at step {step}: inline object access off");
                }
                None
            }
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn layout_probe_succeeds() {
        assert!(super::probe().is_ok(), "{:?}", super::probe().err());
    }

    #[test]
    fn call_layout_probe_succeeds() {
        assert!(super::call_probe().is_ok(), "{:?}", super::call_probe().err());
    }
}

/// Offsets for native calls and returns between compiled functions
/// (`CallR` / `ReturnR` without helpers): the value stack and frame stack
/// of the realm, the fields of a frame, of a closure and of its code.
/// Probed like [`Layout`]; `None` keeps calls and returns in helpers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallLayout {
    /// In `Realm`: the value stack (pointer, length, capacity), the frame
    /// stack's capacity, `max_frames`, `current_realm` (u16), the global
    /// object's id and `jit.on` (a byte).
    pub vs_ptr: i32,
    pub vs_len: i32,
    pub vs_cap: i32,
    pub frames_cap: i32,
    pub max_frames: i32,
    pub current_realm: i32,
    pub global: i32,
    pub jit_on: i32,
    /// In `Realm`: `jit.stats.enabled` (a byte) and `jit.stats.switches`
    /// (u64), counted like the helpers' frame switches.
    pub stats_on: i32,
    pub switches: i32,
    /// In `Frame`.
    pub f_code: i32,
    pub f_pc: i32,
    pub f_base: i32,
    pub f_argc: i32,
    pub f_scope: i32,
    pub f_scope_depth: i32,
    pub f_this: i32,
    pub f_func_id: i32,
    pub f_func_tag: i32,
    pub f_new_target: i32,
    pub f_ret_slot: i32,
    pub f_is_construct: i32,
    pub f_entry: i32,
    pub f_extra: i32,
    /// Tag word of `Some` in an `Option<ObjectId>`.
    pub some_id: u32,
    /// In an object table entry: the value of the variant tag (byte or
    /// niche word at `Layout::scope_tag`) of `ObjectKind::Function`, and
    /// its `Box<Closure>`.
    pub fn_tag_value: u64,
    pub closure_box: i32,
    /// In `Closure`: the code (`Rc` word; the `FunctionCode` is at that
    /// address plus `code_adj`), the scope (`Option<ObjectId>`), and the
    /// lexical `this`, `new.target` and `arguments` of arrows
    /// (`Option<Value>`: tag word and value word, `Some` tag).
    pub cl_code: i32,
    pub code_adj: i32,
    pub cl_scope_id: i32,
    pub cl_scope_tag: i32,
    pub cl_this_tag: i32,
    pub cl_this_val: i32,
    pub cl_nt_tag: i32,
    pub cl_nt_val: i32,
    pub cl_args_tag: i32,
    pub some_value: u64,
    /// In `FunctionCode`: the constants' pointer, the number of
    /// parameter locations, `call_flags` (byte), `nregs` and `max_depth`
    /// (u32), the native entry and the realm (u16).
    pub fc_consts: i32,
    pub fc_params: i32,
    pub fc_flags: i32,
    pub fc_nregs: i32,
    pub fc_max_depth: i32,
    pub fc_entry: i32,
    pub fc_realm: i32,
}

/// Word indices of the pointer, length and capacity of a `Vec<T>`.
fn vec_words3<T>(mut v: Vec<T>, one: T) -> Option<(usize, usize, usize)> {
    v.reserve_exact(7);
    v.push(one);
    let (ptr, len, cap) = (v.as_ptr() as usize, v.len(), v.capacity());
    if std::mem::size_of::<Vec<T>>() != 3 * std::mem::size_of::<usize>() || cap == len || len == ptr || cap == ptr {
        return None;
    }
    // SAFETY: a `Vec` is three words (checked above); read them in place.
    let words: [usize; 3] = unsafe { std::ptr::read(&v as *const Vec<T> as *const [usize; 3]) };
    let find = |x: usize| {
        let mut it = (0..3).filter(|&i| words[i] == x);
        let i = it.next()?;
        it.next().is_none().then_some(i)
    };
    Some((find(ptr)?, find(len)?, find(cap)?))
}

/// Compile-time check of the types of the `Realm` fields native calls use.
#[allow(dead_code)]
fn _realm_field_types(r: &Realm) -> (u16, usize, ObjectId, bool, bool, u64) {
    (r.current_realm, r.max_frames, r.global, r.jit.on, r.jit.stats.enabled, r.jit.stats.switches)
}

fn call_probe() -> Result<CallLayout, u32> {
    use crate::heap::Closure;
    use crate::value::Value;
    let l = get().ok_or(100u32)?;
    let i32_of = |x: usize| i32::try_from(x).map_err(|_| 101u32);
    let w = std::mem::size_of::<usize>();
    // Value stack and frame stack.
    let (vp, vl, vc) = vec_words3::<Value>(Vec::new(), Value::Number(2.5)).ok_or(102u32)?;
    let vs = std::mem::offset_of!(Realm, vstack);
    let mk_frame = || crate::vm::Frame {
        code: std::ptr::NonNull::dangling(),
        pc: 0,
        base: 0,
        argc: 0,
        scope: ObjectId(0),
        scope_depth: 0,
        this: Value::Undefined,
        func: None,
        new_target: Value::Undefined,
        ret_slot: 0,
        is_construct: false,
        entry: false,
        extra: None,
    };
    let (fp, fl, fc) = vec_words3::<crate::vm::Frame>(Vec::new(), mk_frame()).ok_or(103u32)?;
    let frames = std::mem::offset_of!(Realm, frames);
    if l.frames_ptr != i32_of(frames + fp * w)? || l.frames_len != i32_of(frames + fl * w)? {
        return Err(104);
    }
    // Field sizes the templates assume.
    if std::mem::size_of::<usize>() != 8
        || std::mem::size_of::<Option<Box<crate::vm::FrameExtra>>>() != 8
        || std::mem::size_of::<Option<ObjectId>>() != 8
        || std::mem::size_of::<ObjectId>() != 4
        || std::mem::size_of::<Value>() != 8
    {
        return Err(105);
    }
    // Option<ObjectId>: id word and tag word (as for prototypes).
    let words = |o: Option<ObjectId>| -> [u32; 2] {
        // SAFETY: the size is checked above.
        unsafe { std::mem::transmute::<Option<ObjectId>, [u32; 2]>(o) }
    };
    let (a, b, none) = (words(Some(ObjectId(0x1234_5678))), words(Some(ObjectId(0x0765_4321))), words(None));
    let id_word = (0..2).find(|&i| a[i] == 0x1234_5678 && b[i] == 0x0765_4321).ok_or(106u32)?;
    let tag_word = 1 - id_word;
    if a[tag_word] != b[tag_word] || none[tag_word] == a[tag_word] || a[tag_word] != l.proto_some {
        return Err(107);
    }
    // Option<Value>: two words (tag, value).
    if std::mem::size_of::<Option<Value>>() != 16 {
        return Err(108);
    }
    let vwords = |o: Option<Value>| -> [u64; 2] {
        // SAFETY: the size is checked above; any bit pattern is a valid [u64; 2].
        unsafe { std::mem::transmute::<Option<Value>, [u64; 2]>(o) }
    };
    let (x1, x2, xn) = (vwords(Some(Value::Number(1.5))), vwords(Some(Value::Number(-7.25))), vwords(None));
    let vw = (0..2).find(|&i| x1[i] == 1.5f64.to_bits() && x2[i] == (-7.25f64).to_bits()).ok_or(109u32)?;
    let tw = 1 - vw;
    if x1[tw] != x2[tw] || xn[tw] == x1[tw] {
        return Err(110);
    }
    let some_value = x1[tw];
    if some_value > i32::MAX as u64 {
        return Err(131);
    }
    // The tag words are written whole (no stale padding bytes): store
    // over garbage and compare.
    {
        let mut buf = [0xABu8; 16];
        // SAFETY: the buffer holds 16 bytes; `Option<Value>` is 16 bytes
        // (checked above) and its alignment is not needed for a byte copy.
        let words = unsafe {
            std::ptr::write_unaligned(buf.as_mut_ptr() as *mut Option<Value>, Some(Value::Number(3.5)));
            std::ptr::read_unaligned(buf.as_ptr() as *const [u64; 2])
        };
        if words[tw] != some_value {
            return Err(132);
        }
        let mut buf = [0xABu8; 8];
        // SAFETY: as above, 8 bytes.
        let words = unsafe {
            std::ptr::write_unaligned(buf.as_mut_ptr() as *mut Option<ObjectId>, Some(ObjectId(77)));
            std::ptr::read_unaligned(buf.as_ptr() as *const [u32; 2])
        };
        if words[tag_word] != a[tag_word] || words[id_word] != 77 {
            return Err(133);
        }
    }
    // A function object: its variant tag and closure box.
    let code = {
        let program = crate::parser::parse_program("0", false).map_err(|_| 111u32)?;
        crate::compiler::compile_program(&program, "probe", "0").map_err(|_| 112u32)?
    };
    let code_addr = &*code as *const crate::bytecode::FunctionCode as usize;
    let closure = |scope: Option<ObjectId>| Closure { code: code.clone(), scope, this_value: Some(Value::Number(1.5)), new_target: None, home_object: None, fields: None, arguments: None };
    let f1 = Some(crate::heap::Object::new(None, ObjectKind::Function(Box::new(closure(Some(ObjectId(0x2233_4455)))))));
    let kind = std::mem::offset_of!(crate::heap::Object, kind);
    let obj = {
        let base = &f1 as *const Option<crate::heap::Object> as usize;
        f1.as_ref().ok_or(113u32)? as *const crate::heap::Object as usize - base
    };
    let fo = f1.as_ref().ok_or(114u32)?;
    let ObjectKind::Function(cb) = &fo.kind else { return Err(115) };
    let box_addr = &**cb as *const Closure as usize;
    let entry_base = &f1 as *const Option<crate::heap::Object> as usize;
    let rd = |off: usize| -> usize {
        // SAFETY: reading a word within the live probe entry.
        unsafe { std::ptr::read_unaligned((entry_base + off) as *const usize) }
    };
    let ksize = std::mem::size_of::<ObjectKind>();
    let kstart = obj + kind;
    let closure_box = (0..=ksize - w).step_by(w).map(|i| kstart + i).find(|&o| rd(o) == box_addr).ok_or(116u32)?;
    let tag_at = l.scope_tag as usize;
    // Tag value, checked against the probe kinds of `Layout`.
    let fn_tag_value = if l.tag_niche {
        let v = rd(tag_at) as u64;
        if v == l.scope_tag_value || v & (1 << 63) == 0 {
            return Err(117);
        }
        v
    } else {
        // SAFETY: a byte within the probe entry.
        let v = unsafe { std::ptr::read((entry_base + tag_at) as *const u8) } as u64;
        if v == l.scope_tag_value || v == l.array_tag_value as u64 {
            return Err(118);
        }
        v
    };
    if (closure_box..closure_box + w).contains(&tag_at) {
        return Err(119);
    }
    // Other kinds must not carry the same tag value there.
    for other in [ObjectKind::Ordinary, ObjectKind::Arguments, ObjectKind::Error] {
        let e = Some(crate::heap::Object::new(None, other));
        let b = &e as *const Option<crate::heap::Object> as usize;
        // SAFETY: reading within the live probe entry.
        let v = unsafe { if l.tag_niche { std::ptr::read_unaligned((b + tag_at) as *const usize) as u64 } else { std::ptr::read((b + tag_at) as *const u8) as u64 } };
        if v == fn_tag_value {
            return Err(120);
        }
    }
    // Closure fields.
    let cl_code = std::mem::offset_of!(Closure, code);
    // SAFETY: the `Rc` word inside the live closure.
    let rc_word = unsafe { std::ptr::read((box_addr + cl_code) as *const usize) };
    let code_adj = code_addr.checked_sub(rc_word).ok_or(121u32)?;
    if code_adj > 64 {
        return Err(122);
    }
    let opt_id_at = |field: usize| (field + id_word * 4, field + tag_word * 4);
    let (cl_scope_id, cl_scope_tag) = opt_id_at(std::mem::offset_of!(Closure, scope));
    // SAFETY: reading the live closure's scope words.
    if unsafe { std::ptr::read((box_addr + cl_scope_id) as *const u32) } != 0x2233_4455 {
        return Err(123);
    }
    let opt_val_at = |field: usize| (field + tw * 8, field + vw * 8);
    let (cl_this_tag, cl_this_val) = opt_val_at(std::mem::offset_of!(Closure, this_value));
    let (cl_nt_tag, cl_nt_val) = opt_val_at(std::mem::offset_of!(Closure, new_target));
    let (cl_args_tag, _) = opt_val_at(std::mem::offset_of!(Closure, arguments));
    // SAFETY: reading the live closure's `this_value` words.
    unsafe {
        if std::ptr::read((box_addr + cl_this_val) as *const u64) != 1.5f64.to_bits() || std::ptr::read((box_addr + cl_this_tag) as *const u64) != some_value {
            return Err(124);
        }
    }
    drop(f1);
    // FunctionCode.
    use crate::bytecode::FunctionCode;
    let (cp, _) = vec_words::<Value>(Vec::new(), Value::Number(1.0)).ok_or(125u32)?;
    let (_, pl) = vec_words::<crate::bytecode::VarLoc>(Vec::new(), crate::bytecode::VarLoc::Reg(3)).ok_or(126u32)?;
    let entry_off = std::mem::offset_of!(FunctionCode, jit) + std::mem::offset_of!(crate::bytecode::JitSlot, s) + std::mem::offset_of!(super::Slot, entry);
    // SAFETY: reading fields of the live probe code.
    unsafe {
        let c = code_addr;
        let consts = std::ptr::read((c + std::mem::offset_of!(FunctionCode, constants) + cp * w) as *const usize);
        if consts != code.constants.as_ptr() as usize {
            return Err(127);
        }
        let np = std::ptr::read((c + std::mem::offset_of!(FunctionCode, param_locs) + pl * w) as *const usize);
        if np != code.param_locs.len() {
            return Err(128);
        }
        code.jit.s.entry.set(0x5a5a);
        let e = std::ptr::read((c + entry_off) as *const usize);
        code.jit.s.entry.set(0);
        if e != 0x5a5a {
            return Err(129);
        }
        let r = std::ptr::read((c + std::mem::offset_of!(FunctionCode, realm)) as *const u16);
        if r != code.realm.get() {
            return Err(130);
        }
    }
    let jit_on = std::mem::offset_of!(Realm, jit) + std::mem::offset_of!(super::State, on);
    let stats = std::mem::offset_of!(Realm, jit) + std::mem::offset_of!(super::State, stats);
    let func = std::mem::offset_of!(crate::vm::Frame, func);
    Ok(CallLayout {
        vs_ptr: i32_of(vs + vp * w)?,
        vs_len: i32_of(vs + vl * w)?,
        vs_cap: i32_of(vs + vc * w)?,
        frames_cap: i32_of(frames + fc * w)?,
        max_frames: i32_of(std::mem::offset_of!(Realm, max_frames))?,
        current_realm: i32_of(std::mem::offset_of!(Realm, current_realm))?,
        global: i32_of(std::mem::offset_of!(Realm, global))?,
        jit_on: i32_of(jit_on)?,
        stats_on: i32_of(stats + std::mem::offset_of!(super::Stats, enabled))?,
        switches: i32_of(stats + std::mem::offset_of!(super::Stats, switches))?,
        f_code: i32_of(std::mem::offset_of!(crate::vm::Frame, code))?,
        f_pc: i32_of(std::mem::offset_of!(crate::vm::Frame, pc))?,
        f_base: i32_of(std::mem::offset_of!(crate::vm::Frame, base))?,
        f_argc: i32_of(std::mem::offset_of!(crate::vm::Frame, argc))?,
        f_scope: i32_of(std::mem::offset_of!(crate::vm::Frame, scope))?,
        f_scope_depth: i32_of(std::mem::offset_of!(crate::vm::Frame, scope_depth))?,
        f_this: i32_of(std::mem::offset_of!(crate::vm::Frame, this))?,
        f_func_id: i32_of(func + id_word * 4)?,
        f_func_tag: i32_of(func + tag_word * 4)?,
        f_new_target: i32_of(std::mem::offset_of!(crate::vm::Frame, new_target))?,
        f_ret_slot: i32_of(std::mem::offset_of!(crate::vm::Frame, ret_slot))?,
        f_is_construct: i32_of(std::mem::offset_of!(crate::vm::Frame, is_construct))?,
        f_entry: i32_of(std::mem::offset_of!(crate::vm::Frame, entry))?,
        f_extra: i32_of(std::mem::offset_of!(crate::vm::Frame, extra))?,
        some_id: a[tag_word],
        fn_tag_value,
        closure_box: i32_of(closure_box)?,
        cl_code: i32_of(cl_code)?,
        code_adj: i32_of(code_adj)?,
        cl_scope_id: i32_of(cl_scope_id)?,
        cl_scope_tag: i32_of(cl_scope_tag)?,
        cl_this_tag: i32_of(cl_this_tag)?,
        cl_this_val: i32_of(cl_this_val)?,
        cl_nt_tag: i32_of(cl_nt_tag)?,
        cl_nt_val: i32_of(cl_nt_val)?,
        cl_args_tag: i32_of(cl_args_tag)?,
        some_value,
        fc_consts: i32_of(std::mem::offset_of!(FunctionCode, constants) + cp * w)?,
        fc_params: i32_of(std::mem::offset_of!(FunctionCode, param_locs) + pl * w)?,
        fc_flags: i32_of(std::mem::offset_of!(FunctionCode, call_flags))?,
        fc_nregs: i32_of(std::mem::offset_of!(FunctionCode, nregs))?,
        fc_max_depth: i32_of(std::mem::offset_of!(FunctionCode, max_depth))?,
        fc_entry: i32_of(entry_off)?,
        fc_realm: i32_of(std::mem::offset_of!(FunctionCode, realm))?,
    })
}

/// The probed call layout (`None`: calls and returns go through helpers;
/// `RABBITHOLE_JIT_NO_INLINE_CALL` turns it off).
pub(crate) fn call_layout() -> Option<CallLayout> {
    static L: std::sync::OnceLock<Option<CallLayout>> = std::sync::OnceLock::new();
    *L.get_or_init(|| {
        if std::env::var_os("RABBITHOLE_JIT_NO_INLINE_CALL").is_some() {
            return None;
        }
        match call_probe() {
            Ok(l) => Some(l),
            Err(step) => {
                if super::options().trace {
                    eprintln!("[jit] call layout probe failed at step {step}: calls through helpers");
                }
                None
            }
        }
    })
}
