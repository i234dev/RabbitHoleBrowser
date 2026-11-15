//! Windows registration of the current JS x64 frame and pure descriptor tests.
//! Offsets come from decoding bytes emitted by the real prologue/epilogue helpers.
//! This deliberately rejects other frame recipes rather than guessing unwind data.

use super::emit::INNER_ENTRY;

#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    pub begin: usize,
    pub prologue_end: usize,
    pub inner_entry: usize,
    pub epilogue_start: usize,
    pub end: usize,
    pub table_start: usize,
    pub table_end: usize,
    pub unwind_info: usize,
    pub dispatch_jump: usize,
    pub status_jump: usize,
}

#[derive(Debug)]
struct Descriptor {
    layout: Layout,
    info: Vec<u8>,
    prologue_steps: Vec<(usize, u8)>,
    epilogue_steps: Vec<(usize, u8)>,
}

struct Bytes<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bytes<'_> {
    fn exact(&mut self, expected: &[u8]) -> Result<(), &'static str> {
        let next = self.at.checked_add(expected.len()).ok_or("instruction overflow")?;
        if self.bytes.get(self.at..next) != Some(expected) {
            return Err("unsupported JS frame instruction");
        }
        self.at = next;
        Ok(())
    }

    fn immediate(&mut self, len: usize) -> Result<(), &'static str> {
        let next = self.at.checked_add(len).ok_or("immediate overflow")?;
        self.bytes.get(self.at..next).ok_or("truncated immediate")?;
        self.at = next;
        Ok(())
    }

    fn push(&mut self, reg: u8) -> Result<(), &'static str> {
        if reg >= 8 { self.exact(&[0x41])?; }
        self.exact(&[0x50 + (reg & 7)])
    }

    fn pop(&mut self, reg: u8) -> Result<(), &'static str> {
        if reg >= 8 { self.exact(&[0x41])?; }
        self.exact(&[0x58 + (reg & 7)])
    }
}

impl Descriptor {
    fn from_image(image: &[u8], layout: Layout) -> Result<Self, &'static str> {
        // These are image-relative values, not pointers. Check the encoding
        // width before indexing, including on a 64-bit host.
        for value in [layout.begin, layout.prologue_end, layout.inner_entry,
            layout.epilogue_start, layout.end, layout.table_start, layout.table_end,
            layout.unwind_info] {
            u32::try_from(value).map_err(|_| "RVA overflow")?;
        }
        if !(layout.begin < layout.prologue_end
            && layout.prologue_end <= layout.epilogue_start
            && layout.epilogue_start < layout.end
            && layout.end <= layout.table_start
            && layout.table_start <= layout.table_end
            && layout.table_end <= layout.unwind_info) {
            return Err("overlapping or reversed ranges");
        }
        if layout.table_start % 4 != 0 || (layout.table_end - layout.table_start) % 4 != 0
            || layout.unwind_info % 4 != 0 {
            return Err("unaligned table or unwind info");
        }
        let prologue_len = layout.prologue_end - layout.begin;
        let prologue_size = u8::try_from(prologue_len).map_err(|_| "prologue too long")?;
        for pc in [layout.dispatch_jump, layout.status_jump] {
            if pc < layout.prologue_end || pc.checked_add(2).is_none_or(|end| end > layout.epilogue_start)
                || image.get(pc..pc + 2) != Some(&[0xff, 0xe0]) {
                return Err("unsupported JS indirect jump");
            }
        }
        if layout.inner_entry != layout.prologue_end || prologue_len != INNER_ENTRY {
            return Err("inner entry must follow the shared complete JS prologue");
        }
        let prologue = image.get(layout.begin..layout.prologue_end).ok_or("prologue outside image")?;
        let mut p = Bytes { bytes: prologue, at: 0 };
        let mut codes = Vec::new();
        let mut prologue_steps = Vec::new();
        let mut stack = 0u8;
        p.push(5)?; // RBP, saved before its otherwise-unused mov from RSP.
        stack += 8;
        codes.push((p.at as u8, 5 << 4)); // UWOP_PUSH_NONVOL
        prologue_steps.push((layout.begin + p.at, stack));
        p.exact(&[0x48, 0x89, 0xe5])?; // mov RBP,RSP; NOT UWOP_SET_FPREG.
        for reg in [3, 12, 13, 14, 15] { // RBX, R12..R15
            p.push(reg)?;
            stack += 8;
            codes.push((p.at as u8, reg << 4));
            prologue_steps.push((layout.begin + p.at, stack));
        }
        p.exact(&[0x48, 0x83, 0xec, 8])?; // sub RSP,8
        stack += 8;
        codes.push((p.at as u8, 2)); // UWOP_ALLOC_SMALL, info=0 => 8 bytes.
        prologue_steps.push((layout.begin + p.at, stack));
        p.exact(&[0x48, 0x89, 0xfb])?; // mov RBX,RDI
        for opcode in [0xbd, 0xbe] { // mov R13/14,imm64; value does not affect unwind.
            p.exact(&[0x49, opcode])?;
            p.immediate(8)?;
        }
        if p.at != prologue.len() || stack != 56 { return Err("incomplete JS frame"); }

        let epilogue = image.get(layout.epilogue_start..layout.end).ok_or("epilogue outside image")?;
        let mut e = Bytes { bytes: epilogue, at: 0 };
        let mut epilogue_steps = Vec::new();
        e.exact(&[0x48, 0x83, 0xc4, 8])?; // add RSP,8
        stack -= 8;
        epilogue_steps.push((layout.epilogue_start + e.at, stack));
        for reg in [15, 14, 13, 12, 3, 5] {
            e.pop(reg)?;
            stack -= 8;
            epilogue_steps.push((layout.epilogue_start + e.at, stack));
        }
        e.exact(&[0xc3])?;
        if e.at != epilogue.len() || stack != 0 { return Err("noncanonical JS epilogue"); }

        // V1, no handlers, actual prologue length, no frame register.
        let mut info = vec![1, prologue_size, codes.len() as u8, 0];
        for (offset, op) in codes.into_iter().rev() { info.extend_from_slice(&[offset, op]); }
        while info.len() % 4 != 0 { info.push(0); }
        let info_end = layout.unwind_info.checked_add(info.len()).ok_or("unwind range overflow")?;
        u32::try_from(info_end).map_err(|_| "unwind RVA overflow")?;
        image.get(layout.unwind_info..info_end).ok_or("unwind info outside image")?;
        Ok(Self { layout, info, prologue_steps, epilogue_steps })
    }

    fn runtime_function_bytes(&self) -> [u8; 12] {
        let mut bytes = [0; 12];
        for (field, value) in bytes.chunks_exact_mut(4).zip([
            self.layout.begin, self.layout.end, self.layout.unwind_info,
        ]) {
            // All values were checked before constructing this descriptor.
            field.copy_from_slice(&(value as u32).to_le_bytes());
        }
        bytes
    }

    // Structural expectation for later real RtlVirtualUnwind tests, not an
    // implementation of the Windows unwinder or proof of its epilogue scanner.
    fn stack_bytes_below_entry(&self, pc: usize) -> Option<u8> {
        if pc < self.layout.begin || pc >= self.layout.end { return None; }
        if pc >= self.layout.epilogue_start {
            Some(self.epilogue_steps.iter().rev().find(|(at, _)| *at <= pc).map_or(56, |(_, n)| *n))
        } else if pc < self.layout.prologue_end {
            Some(self.prologue_steps.iter().rev().find(|(at, _)| *at <= pc).map_or(0, |(_, n)| *n))
        } else { Some(56) }
    }
}


#[cfg(windows)]
mod registry {
    use super::{Descriptor, Layout};
    use rabbithole_jit::ExecMem;
    use std::cell::{Cell, RefCell};
    use std::mem::ManuallyDrop;

    #[repr(C)]
    struct RuntimeFunction { begin: u32, end: u32, unwind: u32 }

    #[link(name = "kernel32")]
    extern "system" {
        fn RtlAddFunctionTable(table: *const RuntimeFunction, count: u32, base: u64) -> u8;
        fn RtlDeleteFunctionTable(table: *const RuntimeFunction) -> u8;
    }

    // Stable table storage and its RX xdata/code form one ownership unit. On
    // Delete failure both ManuallyDrop fields deliberately remain allocated:
    // Windows must never retain a pointer to freed table/xdata/code memory.
    struct Registration {
        mem: ManuallyDrop<ExecMem>,
        table: ManuallyDrop<Box<RuntimeFunction>>,
        registered: bool,
        accounted: Cell<bool>,
        #[cfg(test)]
        layout: Layout,
    }

    impl Registration {
        fn unregister(&mut self) -> bool {
            if !self.registered { return true; }
            #[cfg(test)]
            if FAIL_DELETE.try_with(|flag| flag.replace(false)).unwrap_or(false) { return false; }
            // No generated activation may still own a return address here.
            if unsafe { RtlDeleteFunctionTable(&**self.table) } == 0 { return false; }
            self.registered = false;
            true
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // During TLS teardown, retaining the complete allocation is safer
            // than unmapping code whose accounting/retirement state is gone.
            if self.accounted.get() && super::super::CODE_BYTES.try_with(|_| ()).is_err() { return; }
            if !self.unregister() { return; }
            let bytes = self.mem.mapped_len();
            unsafe {
                ManuallyDrop::drop(&mut self.mem);
                ManuallyDrop::drop(&mut self.table);
            }
            if self.accounted.get() {
                let _ = super::super::CODE_BYTES.try_with(|n| n.set(n.get().saturating_sub(bytes)));
            }
            #[cfg(test)]
            let _ = DISPOSALS.try_with(|n| n.set(n.get() + 1));
        }
    }

    #[derive(Default)]
    struct Retirement { active: usize, retired: Vec<Box<Registration>> }

    impl Drop for Retirement {
        fn drop(&mut self) {
            // TLS destructor ordering gives no guarantee that other native
            // owners/bookkeeping are still available. Normal last-entry exit
            // drains this list; an exceptional shutdown keeps it intact.
            std::mem::forget(std::mem::take(&mut self.retired));
        }
    }

    thread_local! {
        static RETIREMENT: RefCell<Retirement> = RefCell::new(Retirement::default());
        #[cfg(test)]
        static FAIL_ADD: Cell<bool> = const { Cell::new(false) };
        #[cfg(test)]
        static FAIL_DELETE: Cell<bool> = const { Cell::new(false) };
        #[cfg(test)]
        static DISPOSALS: Cell<usize> = const { Cell::new(0) };
    }

    /// One generated machine activation, including synchronous host reentry.
    /// There is no per-switch clone or search and no change on Unix targets.
    pub(crate) struct EntryLease;

    impl EntryLease {
        pub(crate) fn new() -> Option<Self> {
            RETIREMENT.try_with(|tls| {
                let mut state = tls.try_borrow_mut().ok()?;
                state.active = state.active.checked_add(1)?;
                Some(Self)
            }).ok().flatten()
        }
    }

    impl Drop for EntryLease {
        fn drop(&mut self) {
            let retired = RETIREMENT.try_with(|tls| {
                let Ok(mut state) = tls.try_borrow_mut() else { return Vec::new() };
                state.active = state.active.saturating_sub(1);
                if state.active == 0 { std::mem::take(&mut state.retired) } else { Vec::new() }
            }).unwrap_or_default();
            // Deletion/free and any destructor work run outside the TLS borrow.
            for owner in retired { release(owner); }
        }
    }

    pub(crate) struct RegisteredCode { inner: Option<Box<Registration>> }

    impl RegisteredCode {
        pub(crate) fn new(mut image: Vec<u8>, layout: Layout) -> Result<Self, String> {
            let d = Descriptor::from_image(&image, layout).map_err(str::to_string)?;
            image[layout.unwind_info..layout.unwind_info + d.info.len()].copy_from_slice(&d.info);
            let mem = ExecMem::new(&image, |_, _| {}).ok_or("executable memory unavailable")?;
            let table = Box::new(RuntimeFunction {
                begin: layout.begin as u32, end: layout.end as u32, unwind: layout.unwind_info as u32,
            });
            // Allocate owners before registration; there are no fallible
            // allocations between successful registration and owning it.
            let mut inner = Box::new(Registration {
                mem: ManuallyDrop::new(mem), table: ManuallyDrop::new(table),
                registered: false, accounted: Cell::new(false),
                #[cfg(test)]
                layout,
            });
            #[cfg(test)]
            if FAIL_ADD.try_with(|flag| flag.replace(false)).unwrap_or(false) { return Err("Windows unwind registration refused".into()); }
            if unsafe { RtlAddFunctionTable(&**inner.table, 1, inner.mem.ptr() as u64) } == 0 {
                return Err("Windows unwind registration refused".into());
            }
            inner.registered = true;
            Ok(Self { inner: Some(inner) })
        }

        pub(crate) fn account(&self) { self.inner.as_ref().unwrap().accounted.set(true); }
        pub(crate) fn ptr(&self) -> *const u8 { self.inner.as_ref().unwrap().mem.ptr() }
        pub(crate) fn len(&self) -> usize { self.inner.as_ref().unwrap().mem.len() }
        pub(crate) fn mapped_len(&self) -> usize { self.inner.as_ref().unwrap().mem.mapped_len() }
    }

    fn release(mut owner: Box<Registration>) {
        if owner.accounted.get() && super::super::CODE_BYTES.try_with(|_| ()).is_err() {
            std::mem::forget(owner);
            return;
        }
        if owner.unregister() { return; } // Its Drop now releases the allocation.
        // Keep failed removals owned and counted. The next last-entry exit
        // retries them; unavailable/reentrant TLS falls back to a complete leak.
        let mut owner = Some(owner);
        let queued = RETIREMENT.try_with(|tls| {
            let Ok(mut state) = tls.try_borrow_mut() else { return false };
            state.retired.push(owner.take().unwrap());
            true
        }).unwrap_or(false);
        if !queued { std::mem::forget(owner); }
    }

    impl Drop for RegisteredCode {
        fn drop(&mut self) {
            let mut owner = self.inner.take();
            let available = RETIREMENT.try_with(|tls| {
                let Ok(mut state) = tls.try_borrow_mut() else { return false };
                if state.active != 0 { state.retired.push(owner.take().unwrap()); }
                true
            }).unwrap_or(false);
            if !available { std::mem::forget(owner); }
            else if let Some(owner) = owner { release(owner); }
        }
    }

    #[cfg(test)]
    mod os_tests {
        use super::*;
        use crate::{JsResult, Realm, Value};
        use windows_sys::Win32::System::Diagnostics::Debug::{
            CONTEXT, CONTEXT_FULL_AMD64, RtlLookupFunctionEntry, RtlVirtualUnwind, IMAGE_RUNTIME_FUNCTION_ENTRY,
        };

        unsafe fn lookup(pc: usize) -> (*mut IMAGE_RUNTIME_FUNCTION_ENTRY, u64) {
            let mut base = 0;
            let entry = RtlLookupFunctionEntry(pc as u64, &mut base, std::ptr::null_mut());
            (entry, base)
        }

        fn real_code(realm: &mut Realm) -> RegisteredCode {
            let code = realm.compile("var sum=0; for(var i=0;i<10;i++) sum+=i; sum", "unwind-image.js", false).unwrap();
            super::super::super::build(&code).unwrap().0
        }

        const SAVED: [u64; 6] = [0x501, 0x302, 0xc03, 0xd04, 0xe05, 0xf06];
        const RETURN: u64 = 0x1234_5678_0000;
        #[repr(align(16))]
        struct Stack([u64; 64]);

        // Set the actual register/stack state at this instruction boundary.
        // The OS performs the unwind; no private unwinder supplies the result.
        unsafe fn unwind_at(mem: &RegisteredCode, pc: usize, depth: usize, epilogue: Option<usize>) {
            let mut stack = Stack([0; 64]);
            let entry_index = 33; // Entry RSP is 8 mod 16 under sysv64.
            stack.0[entry_index] = RETURN;
            for (i, value) in SAVED.iter().enumerate() { stack.0[entry_index - 1 - i] = *value; }
            let entry_rsp = stack.0.as_mut_ptr().add(entry_index) as u64;
            let mut c = CONTEXT::default();
            c.ContextFlags = CONTEXT_FULL_AMD64;
            c.Rip = (mem.ptr() as usize + pc) as u64;
            c.Rsp = entry_rsp - depth as u64;
            [c.Rbp, c.Rbx, c.R12, c.R13, c.R14, c.R15] = SAVED;
            let layout = mem.inner.as_ref().unwrap().layout;
            if pc >= layout.begin + 4 { c.Rbp = entry_rsp - 8; }
            if pc >= layout.begin + 20 { c.Rbx = 0x901; }
            if pc >= layout.begin + 30 { c.R13 = 0x902; }
            if pc >= layout.prologue_end { c.R12 = 0x903; c.R14 = 0x904; c.R15 = 0x905; }
            if let Some(offset) = epilogue {
                if offset >= 6 { c.R15 = SAVED[5]; }
                if offset >= 8 { c.R14 = SAVED[4]; }
                if offset >= 10 { c.R13 = SAVED[3]; }
                if offset >= 12 { c.R12 = SAVED[2]; }
                if offset >= 13 { c.Rbx = SAVED[1]; }
                if offset >= 14 { c.Rbp = SAVED[0]; }
            }
            // A false epilogue recognition of FF E0 must not quietly pass.
            c.Rax = 0x5555_6666_0000;
            let (entry, base) = lookup(c.Rip as usize);
            assert!(!entry.is_null(), "registered pc {pc}");
            assert_eq!(base, mem.ptr() as u64);
            assert_eq!((*entry).BeginAddress, layout.begin as u32);
            assert_eq!((*entry).EndAddress, layout.end as u32);
            assert_eq!((*entry).Anonymous.UnwindInfoAddress, layout.unwind_info as u32);
            let mut handler_data = std::ptr::null_mut();
            let mut establisher = 0;
            let _ = RtlVirtualUnwind(0, base, c.Rip, entry, &mut c, &mut handler_data,
                &mut establisher, std::ptr::null_mut());
            assert_eq!(c.Rip, RETURN, "pc={pc} return PC");
            assert_eq!(c.Rsp, entry_rsp + 8, "pc={pc} caller RSP");
            assert_eq!([c.Rbp, c.Rbx, c.R12, c.R13, c.R14, c.R15], SAVED, "pc={pc} saved registers");
        }

        #[test]
        fn windows_os_unwinds_actual_js_prologue_body_inner_jumps_and_epilogue() {
            let mut realm = Realm::new();
            let mem = real_code(&mut realm);
            let base = mem.ptr() as usize;
            let layout = mem.inner.as_ref().unwrap().layout;
            unsafe {
                for (offset, depth) in [(0,0), (1,8), (4,8), (5,16), (7,24), (9,32),
                    (11,40), (13,48), (17,56), (20,56), (30,56), (40,56)] {
                    unwind_at(&mem, offset, depth, None);
                }
                for pc in [layout.prologue_end, layout.dispatch_jump, layout.status_jump] {
                    unwind_at(&mem, pc, 56, None);
                }
                for (offset, depth) in [(0,56), (4,48), (6,40), (8,32), (10,24), (12,16), (13,8), (14,0)] {
                    unwind_at(&mem, layout.epilogue_start + offset, depth, Some(offset));
                }
                for pc in [layout.end, layout.table_start, layout.unwind_info] {
                    assert!(lookup(base + pc).0.is_null(), "data/padding is not a function");
                }
            }
            drop(mem);
            assert!(unsafe { lookup(base).0.is_null() }, "unregistered before freeing");
            eprintln!("windows-unwind-os actual-js-image prologue=12 inner=40 jumps=2 epilogue=8 lookup_after_drop=none");
        }

        #[test]
        fn windows_registration_failure_never_publishes_native_entry() {
            let mut realm = Realm::new();
            let code = realm.compile("21+21", "registration-failure.js", false).unwrap();
            let before = super::super::super::code_bytes();
            let disposals = DISPOSALS.with(Cell::get);
            FAIL_ADD.with(|flag| flag.set(true));
            let mut stats = super::super::super::Stats::default();
            super::super::super::compile(&code, &mut stats);
            assert_eq!(code.jit.s.entry.get(), 0);
            assert!(code.jit.s.code.get().is_none());
            assert_eq!(code.jit.s.state.get(), super::super::super::REJECTED);
            assert_eq!(stats.compiled, 0);
            assert_eq!(super::super::super::code_bytes(), before);
            assert_eq!(DISPOSALS.with(Cell::get), disposals + 1);
            let value = realm.run_script(code).unwrap();
            assert_eq!(realm.to_string(value).unwrap().to_rust_string(), "42");
        }

        #[test]
        fn windows_failed_removal_retains_registered_image_and_budget_until_retry() {
            let mut realm = Realm::new();
            let mem = real_code(&mut realm);
            let base = mem.ptr() as usize;
            let before = super::super::super::code_bytes();
            let bytes = mem.mapped_len();
            super::super::super::CODE_BYTES.with(|n| n.set(n.get() + bytes));
            mem.account();
            let disposals = DISPOSALS.with(Cell::get);
            FAIL_DELETE.with(|flag| flag.set(true));
            drop(mem);
            assert!(!unsafe { lookup(base).0.is_null() });
            assert_eq!(super::super::super::code_bytes(), before + bytes);
            assert_eq!(DISPOSALS.with(Cell::get), disposals);
            // Fill the remaining budget while the failed removal is retained:
            // compilation must reject rather than recycle its counted pages.
            let held_budget = super::super::super::code_bytes();
            super::super::super::CODE_BYTES.with(|n| n.set(super::super::super::MAX_CODE_BYTES));
            let next = realm.compile("1+2", "retained-budget.js", false).unwrap();
            let mut stats = super::super::super::Stats::default();
            super::super::super::compile(&next, &mut stats);
            assert_eq!(next.jit.s.entry.get(), 0);
            assert_eq!(next.jit.s.state.get(), super::super::super::REJECTED);
            super::super::super::CODE_BYTES.with(|n| n.set(held_budget));
            // A subsequent real entry lease retries the owned failed removal.
            drop(EntryLease::new().unwrap());
            assert!(unsafe { lookup(base).0.is_null() });
            assert_eq!(super::super::super::code_bytes(), before);
            assert_eq!(DISPOSALS.with(Cell::get), disposals + 1);
        }

        #[derive(Default)]
        struct RetireProbe {
            base: Cell<usize>, bytes: Cell<usize>, budget: Cell<usize>,
            nested_entries: Cell<u64>, nested_checked: Cell<bool>,
        }

        fn retire_from_callback(r: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
            let probe = r.native_data::<RetireProbe>(r.current_native().unwrap()).unwrap();
            assert!(r.jit.live_entries > 0, "callback has a generated return address");
            retire_code(r, &probe);
            let before = r.jit_entries();
            let result = r.eval("nestedRetireProbe(true)", "nested-retire.js")?;
            assert!(r.jit_entries() > before, "nested JS really entered native code");
            probe.nested_entries.set(r.jit_entries() - before);
            assert!(!unsafe { lookup(probe.base.get()).0.is_null() }, "outer entry still owns retired code");
            let collections = r.heap.collections;
            let unreachable = r.new_object();
            let freed = r.gc();
            assert!(freed > 0 && r.heap.try_get(unreachable).is_none());
            assert!(r.heap.collections > collections, "a full collection actually ran");
            assert!(r.jit.live_entries > 0);
            assert!(!unsafe { lookup(probe.base.get()).0.is_null() }, "GC cannot unregister a live lease");
            Ok(result)
        }

        fn retire_code(r: &mut Realm, probe: &RetireProbe) {
            let code = r.compile("var temporary=1; for(var j=0;j<20;j++)temporary+=j;temporary", "retired.js", false).unwrap();
            super::super::super::compile(&code, &mut r.jit.stats);
            let owner = code.jit.s.code.get().unwrap();
            let base = owner.mem.ptr() as usize;
            probe.base.set(base);
            probe.bytes.set(owner.mem.mapped_len());
            probe.budget.set(super::super::super::code_bytes());
            let disposals = DISPOSALS.with(Cell::get);
            drop(code); // Actual FunctionCode/JitCode owner dies inside the host.
            assert!(!unsafe { lookup(base).0.is_null() });
            assert_eq!(super::super::super::code_bytes(), probe.budget.get());
            assert_eq!(DISPOSALS.with(Cell::get), disposals);
        }

        fn nested_probe(r: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
            let probe = r.native_data::<RetireProbe>(r.current_native().unwrap()).unwrap();
            assert!(r.jit.live_entries >= 2, "nested and outer generated frames coexist");
            assert!(!unsafe { lookup(probe.base.get()).0.is_null() });
            probe.nested_checked.set(true);
            Ok(Value::Number(73.0))
        }

        fn install(r: &mut Realm, name: &str, f: crate::NativeFn, probe: std::rc::Rc<RetireProbe>) {
            let native = r.new_native_function_full(name, 0, f, Some(probe), Vec::new(), false);
            let global = r.global();
            r.define_property(global, name, Value::Object(native), crate::HIDDEN);
            r.mark_gc_safe(global, name);
        }

        #[test]
        fn windows_retired_real_jit_code_survives_nested_callbacks_gc_then_unregisters() {
            let before = super::super::super::code_bytes();
            let disposals = DISPOSALS.with(Cell::get);
            let mut r = Realm::new();
            r.set_jit(true, 0);
            r.gc_enabled = true;
            r.gcx.no_gc = false;
            let probe = std::rc::Rc::new(RetireProbe::default());
            install(&mut r, "__retireCode", retire_from_callback, probe.clone());
            install(&mut r, "__nestedProbe", nested_probe, probe.clone());
            // Both functions exceed the entry-minimum and are compiled before
            // taking the budget snapshot in the retirement callback.
            r.eval("function nestedRetireProbe(go){var n=0;for(var i=0;i<30;i++)n+=i;return go?__nestedProbe():n;} function outerRetireProbe(go){var n=0;for(var i=0;i<30;i++)n+=i;return go?__retireCode():n;} nestedRetireProbe(false); outerRetireProbe(false);", "retire-prepare.js").unwrap();
            let result = r.eval("outerRetireProbe(true)", "retire-call.js").unwrap();
            assert_eq!(r.to_string(result).unwrap().to_rust_string(), "73");
            assert!(probe.nested_checked.get());
            assert!(probe.nested_entries.get() > 0);
            assert!(unsafe { lookup(probe.base.get()).0.is_null() });
            assert_eq!(r.jit.live_entries, 0);
            assert!(RETIREMENT.with(|tls| { let state=tls.borrow(); state.active==0 && state.retired.is_empty() }));
            drop(r);
            assert_eq!(super::super::super::code_bytes(), before);
            assert!(DISPOSALS.with(Cell::get) > disposals);
            eprintln!("windows-unwind-retirement actual-jit callback nested_native_entries={} gc=1 retired_mapping_gone=true", probe.nested_entries.get());
        }

        fn request_panic(_: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
            crate::request_post_native();
            Ok(Value::Undefined)
        }

        fn contained_post_native_panic(r: &mut Realm) {
            let probe = r.native_data::<RetireProbe>(r.current_native().unwrap()).unwrap();
            assert!(r.jit.live_entries > 0, "panic occurs below a generated caller");
            retire_code(r, &probe);
            panic!("contained-windows-jit-panic");
        }

        #[test]
        fn windows_contained_helper_panic_returns_before_retirement_and_rust_resume() {
            let before = super::super::super::code_bytes();
            let mut r = Realm::new();
            r.set_jit(true, 0);
            r.gc_enabled = true;
            r.gcx.no_gc = false;
            let probe = std::rc::Rc::new(RetireProbe::default());
            install(&mut r, "__requestPanic", request_panic, probe.clone());
            r.eval("function panicCaller(){var n=0;for(var i=0;i<30;i++)n+=i;return __requestPanic();} panicCaller();", "panic-warm.js").unwrap();
            let entries = r.jit_entries();
            r.post_native = Some(contained_post_native_panic);
            // The h_call guard catches this panic before the generated frame;
            // ST_PANIC returns normally, releases its lease, then resumes here.
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                r.eval("panicCaller()", "panic-call.js")
            })).expect_err("the original payload resumes only in Rust");
            assert_eq!(panic.downcast_ref::<&'static str>().copied(), Some("contained-windows-jit-panic"));
            assert!(r.jit_entries() > entries);
            assert_eq!(r.jit.live_entries, 0);
            assert!(unsafe { lookup(probe.base.get()).0.is_null() }, "retirement finished before resume_unwind");
            assert!(RETIREMENT.with(|tls| { let state=tls.borrow(); state.active==0 && state.retired.is_empty() }));
            drop(r);
            assert_eq!(super::super::super::code_bytes(), before);
            eprintln!("windows-unwind-panic native_entry=true caught_before_generated_frame=true retired_before_resume=true");
        }

        #[test]
        fn windows_reentrant_lease_bookkeeping_declines_new_native_entry() {
            RETIREMENT.with(|tls| {
                let state = tls.borrow_mut();
                assert_eq!(state.active, 0);
                assert!(EntryLease::new().is_none());
            });
            assert!(EntryLease::new().is_some());
        }

    }

}

#[cfg(windows)]
pub(super) use registry::{EntryLease, RegisteredCode};

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::emit::{emit_epilogue, emit_prologue};
    use rabbithole_jit::x64::{Asm, RAX};

    fn emitted_image(prefix: usize) -> (Vec<u8>, Layout, [usize; 2]) {
        let mut a = Asm::new(false);
        a.buf.resize(prefix, 0x90);
        let begin = a.pos();
        emit_prologue(&mut a);
        let prologue_end = a.pos();
        a.mov_imm32(RAX, 7);
        // Both current indirect-JMP encodings must be future Windows scanner
        // gates: pure CFA expectations do not prove FF E0 is not an epilogue.
        let dispatch_jump = a.pos();
        a.jmp_r(RAX);
        let status_jump = a.pos();
        a.jmp_r(RAX);
        a.int3(); // Defensive, non-executed body marker, also forces alignment padding.
        let epilogue_start = a.pos();
        emit_epilogue(&mut a);
        let end = a.pos();
        a.align(4);
        let table_start = a.pos();
        a.buf.extend_from_slice(&[0; 4]);
        let table_end = a.pos();
        let unwind_info = a.pos();
        a.buf.resize(unwind_info + 20, 0);
        (a.buf, Layout { begin, prologue_end, inner_entry: begin + INNER_ENTRY,
            epilogue_start, end, table_start, table_end, unwind_info, dispatch_jump, status_jump }, [dispatch_jump, status_jump])
    }

    #[test]
    fn real_emitted_js_frame_encodes_the_v1_golden() {
        let (image, layout, _) = emitted_image(0);
        let d = Descriptor::from_image(&image, layout).unwrap();
        assert_eq!(d.info, [0x01, 0x28, 0x07, 0x00, 0x11, 0x02, 0x0d, 0xf0,
            0x0b, 0xe0, 0x09, 0xd0, 0x07, 0xc0, 0x05, 0x30, 0x01, 0x50, 0x00, 0x00]);
        assert_eq!(d.runtime_function_bytes()[..4], 0u32.to_le_bytes());
        assert_eq!(d.runtime_function_bytes()[4..8], (layout.end as u32).to_le_bytes());
        assert_eq!(d.runtime_function_bytes()[8..], (layout.unwind_info as u32).to_le_bytes());
        assert!(layout.end < layout.table_start, "alignment bytes are outside the function");
    }

    #[test]
    fn partial_prologue_and_inner_entry_describe_the_same_full_frame() {
        let (image, layout, jumps) = emitted_image(8);
        let d = Descriptor::from_image(&image, layout).unwrap();
        for (offset, bytes) in [(0, 0), (1, 8), (4, 8), (5, 16), (7, 24),
            (9, 32), (11, 40), (13, 48), (17, 56), (20, 56), (30, 56), (40, 56)] {
            assert_eq!(d.stack_bytes_below_entry(layout.begin + offset), Some(bytes), "offset={offset}");
        }
        assert_eq!(d.stack_bytes_below_entry(layout.inner_entry), Some(56));
        for pc in jumps {
            assert_eq!(&image[pc..pc + 2], &[0xff, 0xe0]);
            assert_eq!(d.stack_bytes_below_entry(pc), Some(56));
        }
        assert_eq!(d.runtime_function_bytes()[..4], 8u32.to_le_bytes());
    }

    #[test]
    fn epilogue_boundaries_exclude_returned_state_padding_and_data() {
        let (image, layout, _) = emitted_image(8);
        let d = Descriptor::from_image(&image, layout).unwrap();
        for (offset, bytes) in [(0, 56), (4, 48), (6, 40), (8, 32), (10, 24),
            (12, 16), (13, 8), (14, 0)] {
            assert_eq!(d.stack_bytes_below_entry(layout.epilogue_start + offset), Some(bytes));
        }
        for pc in [layout.begin - 1, layout.end, layout.table_start, layout.unwind_info] {
            assert_eq!(d.stack_bytes_below_entry(pc), None);
        }
    }

    #[test]
    fn immediate_values_do_not_invent_stack_effects() {
        let (mut image, layout, _) = emitted_image(0);
        let before = Descriptor::from_image(&image, layout).unwrap().info;
        image[22..30].fill(0x11);
        image[32..40].fill(0x22);
        assert_eq!(Descriptor::from_image(&image, layout).unwrap().info, before);
    }

    #[test]
    fn rejects_leaf_wasm_wrong_saves_and_wrong_stack_allocation() {
        let (image, layout, _) = emitted_image(0);
        for (at, replacement) in [(0, vec![0xc3]), (0, vec![0x48, 0x83, 0xec, 8]),
            (0, vec![0x50]), (4, vec![0x52]), (13, vec![0x48, 0x83, 0xec, 16]),
            (1, vec![0x55, 0x55, 0x55])] {
            let mut bad = image.clone();
            bad[at..at + replacement.len()].copy_from_slice(&replacement);
            assert!(Descriptor::from_image(&bad, layout).is_err());
        }
        let mut bad = image;
        let at = layout.epilogue_start + 4;
        bad[at..at + 3].copy_from_slice(&[0x31, 0xc0, 0xc3]);
        assert!(Descriptor::from_image(&bad, layout).is_err(), "Wasm's add/xor/ret is not this JS recipe");
    }

    #[test]
    fn rejects_overlapping_unaligned_out_of_image_and_overflowing_ranges() {
        let (image, layout, _) = emitted_image(8);
        for bad in [
            Layout { begin: layout.end, ..layout },
            Layout { prologue_end: layout.prologue_end - 1, ..layout },
            Layout { inner_entry: layout.inner_entry - 1, ..layout },
            Layout { epilogue_start: layout.prologue_end - 1, ..layout },
            Layout { end: layout.end + 1, ..layout },
            Layout { table_start: layout.end - 1, ..layout },
            Layout { table_start: layout.table_start + 1, ..layout },
            Layout { table_end: layout.unwind_info + 4, ..layout },
            Layout { unwind_info: layout.unwind_info + 1, ..layout },
            Layout { unwind_info: usize::MAX, ..layout },
            Layout { unwind_info: u32::MAX as usize - 3, ..layout },
        ] {
            assert!(Descriptor::from_image(&image, bad).is_err(), "layout={bad:?}");
        }
        for end in [0, layout.prologue_end - 1, layout.end - 1, image.len() - 1] {
            assert!(Descriptor::from_image(&image[..end], layout).is_err());
        }
        let long = Layout { begin: 0, prologue_end: 256, inner_entry: 256,
            epilogue_start: 256, end: 271, table_start: 272, table_end: 276, unwind_info: 276,
            dispatch_jump: 256, status_jump: 256 };
        assert_eq!(Descriptor::from_image(&vec![0; 296], long).err(), Some("prologue too long"));
    }
}
