//! Pure, test-only prototype for the current JS x64 frame, not an OS registry.
//! Offsets come from decoding bytes emitted by the real prologue/epilogue helpers.
//! This deliberately rejects other frame recipes rather than guessing unwind data.

use super::emit::INNER_ENTRY;

#[derive(Clone, Copy, Debug)]
struct Layout {
    begin: usize,
    prologue_end: usize,
    inner_entry: usize,
    epilogue_start: usize,
    end: usize,
    table_start: usize,
    table_end: usize,
    unwind_info: usize,
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
            epilogue_start, end, table_start, table_end, unwind_info }, [dispatch_jump, status_jump])
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
            epilogue_start: 256, end: 271, table_start: 272, table_end: 276, unwind_info: 276 };
        assert_eq!(Descriptor::from_image(&vec![0; 296], long).err(), Some("prologue too long"));
    }
}
