//! Translator tests on synthetic ELF files.

use super::elf::{self, PF_R, PF_W, PF_X, PT_LOAD, PT_PHDR, Phdr};
use super::*;
use crate::a64::{self, Kind};

/// Builder for a small ET_DYN file laid out as lld does for Android:
/// `[ELF header + phdrs | R segment][RX segment: .text][RW segment]`.
pub(crate) struct Synth {
    pub text: Vec<u32>,
    /// (name, value offset into .text in words, size in words, type)
    pub syms: Vec<(&'static str, usize, usize, u8)>,
    /// Extra bss after the RW segment (bytes).
    pub bss: u64,
    pub with_phdr: bool,
    pub with_sections: bool,
    pub eh_frame_covers_text: bool,
    pub align: u64,
}

pub(crate) const TEXT_VADDR: u64 = 0x4000;

impl Synth {
    pub fn new(text: Vec<u32>) -> Synth {
        Synth {
            text,
            syms: Vec::new(),
            bss: 0,
            with_phdr: true,
            with_sections: true,
            eh_frame_covers_text: false,
            align: PAGE,
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let text_off = TEXT_VADDR;
        let text_len = self.text.len() as u64 * 4;
        let rw_vaddr = (TEXT_VADDR + text_len).next_multiple_of(PAGE);
        let rw_off = rw_vaddr;
        let rw_len = 16u64;
        let mut phdrs = Vec::new();
        let nph = 3 + self.with_phdr as usize;
        if self.with_phdr {
            phdrs.push(Phdr {
                p_type: PT_PHDR,
                p_flags: PF_R,
                p_offset: 64,
                p_vaddr: 64,
                p_paddr: 64,
                p_filesz: nph as u64 * 56,
                p_memsz: nph as u64 * 56,
                p_align: 8,
            });
        }
        let load = |flags, off, vaddr, filesz, memsz| Phdr {
            p_type: PT_LOAD,
            p_flags: flags,
            p_offset: off,
            p_vaddr: vaddr,
            p_paddr: vaddr,
            p_filesz: filesz,
            p_memsz: memsz,
            p_align: self.align,
        };
        phdrs.push(load(PF_R, 0, 0, 0x400, 0x400));
        phdrs.push(load(PF_R | PF_X, text_off, TEXT_VADDR, text_len, text_len));
        phdrs.push(load(
            PF_R | PF_W,
            rw_off,
            rw_vaddr,
            rw_len,
            rw_len + self.bss,
        ));

        let mut b = vec![0u8; (rw_off + rw_len) as usize];
        // ELF header.
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[6] = 1;
        b[16..18].copy_from_slice(&elf::ET_DYN.to_le_bytes());
        b[18..20].copy_from_slice(&elf::EM_AARCH64.to_le_bytes());
        b[20..24].copy_from_slice(&1u32.to_le_bytes());
        b[24..32].copy_from_slice(&TEXT_VADDR.to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[52..54].copy_from_slice(&64u16.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&(nph as u16).to_le_bytes());
        for (i, p) in phdrs.iter().enumerate() {
            b[64 + i * 56..64 + (i + 1) * 56].copy_from_slice(&p.to_bytes());
        }
        for (i, w) in self.text.iter().enumerate() {
            let o = text_off as usize + i * 4;
            b[o..o + 4].copy_from_slice(&w.to_le_bytes());
        }
        if !self.with_sections {
            return b;
        }
        // Non-allocated tail: .eh_frame (allocated, in the R segment),
        // .symtab, .strtab, .shstrtab, then the section headers.
        let eh_vaddr = 0x200u64;
        if self.eh_frame_covers_text {
            let eh = eh_frame(eh_vaddr, TEXT_VADDR, text_len);
            b[eh_vaddr as usize..eh_vaddr as usize + eh.len()].copy_from_slice(&eh);
        }
        let mut strtab = vec![0u8];
        let mut symtab = vec![0u8; 24];
        for (name, at, len, kind) in &self.syms {
            let n = strtab.len() as u32;
            strtab.extend_from_slice(name.as_bytes());
            strtab.push(0);
            let mut e = [0u8; 24];
            e[0..4].copy_from_slice(&n.to_le_bytes());
            e[4] = *kind;
            e[6..8].copy_from_slice(&1u16.to_le_bytes()); // .text
            e[8..16].copy_from_slice(&(TEXT_VADDR + *at as u64 * 4).to_le_bytes());
            e[16..24].copy_from_slice(&(*len as u64 * 4).to_le_bytes());
            symtab.extend_from_slice(&e);
        }
        let shstr = b"\0.text\0.symtab\0.strtab\0.shstrtab\0.eh_frame\0".to_vec();
        let symtab_off = b.len() as u64;
        b.extend_from_slice(&symtab);
        let strtab_off = b.len() as u64;
        b.extend_from_slice(&strtab);
        let shstr_off = b.len() as u64;
        b.extend_from_slice(&shstr);
        while b.len() % 8 != 0 {
            b.push(0);
        }
        let shoff = b.len() as u64;
        #[allow(clippy::too_many_arguments)]
        let sh = |name: u32,
                  ty: u32,
                  flags: u64,
                  addr: u64,
                  off: u64,
                  size: u64,
                  link: u32,
                  ent: u64| {
            let mut e = [0u8; 64];
            e[0..4].copy_from_slice(&name.to_le_bytes());
            e[4..8].copy_from_slice(&ty.to_le_bytes());
            e[8..16].copy_from_slice(&flags.to_le_bytes());
            e[16..24].copy_from_slice(&addr.to_le_bytes());
            e[24..32].copy_from_slice(&off.to_le_bytes());
            e[32..40].copy_from_slice(&size.to_le_bytes());
            e[40..44].copy_from_slice(&link.to_le_bytes());
            e[56..64].copy_from_slice(&ent.to_le_bytes());
            e
        };
        let eh_len = if self.eh_frame_covers_text { 0x40 } else { 0 };
        let shdrs = [
            [0u8; 64],
            sh(
                1,
                elf::SHT_PROGBITS,
                elf::SHF_ALLOC | elf::SHF_EXECINSTR,
                TEXT_VADDR,
                text_off,
                text_len,
                0,
                0,
            ),
            sh(
                7,
                elf::SHT_SYMTAB,
                0,
                0,
                symtab_off,
                symtab.len() as u64,
                3,
                24,
            ),
            sh(15, 3, 0, 0, strtab_off, strtab.len() as u64, 0, 0),
            sh(23, 3, 0, 0, shstr_off, shstr.len() as u64, 0, 0),
            sh(
                33,
                elf::SHT_PROGBITS,
                elf::SHF_ALLOC,
                eh_vaddr,
                eh_vaddr,
                eh_len,
                0,
                0,
            ),
        ];
        for s in &shdrs {
            b.extend_from_slice(s);
        }
        b[40..48].copy_from_slice(&shoff.to_le_bytes());
        b[58..60].copy_from_slice(&64u16.to_le_bytes());
        b[60..62].copy_from_slice(&(shdrs.len() as u16).to_le_bytes());
        b[62..64].copy_from_slice(&4u16.to_le_bytes());
        b
    }
}

/// A minimal `.eh_frame` with one CIE ("zR", pcrel|sdata4) and one FDE.
fn eh_frame(at: u64, begin: u64, len: u64) -> Vec<u8> {
    let mut v = Vec::new();
    // CIE: length, id 0, version 1, "zR", code align 4, data align -8,
    // ra 30, aug length 1, R = 0x1b; pad to 4.
    let cie: Vec<u8> = vec![0, 0, 0, 0, 1, b'z', b'R', 0, 4, 0x78, 30, 1, 0x1b, 0, 0, 0];
    v.extend_from_slice(&(cie.len() as u32).to_le_bytes());
    v.extend_from_slice(&cie);
    let fde_start = v.len();
    let mut fde = Vec::new();
    fde.extend_from_slice(&((fde_start + 4) as u32).to_le_bytes()); // CIE pointer
    let field = at + (fde_start + 8) as u64;
    fde.extend_from_slice(&(begin.wrapping_sub(field) as i32).to_le_bytes());
    fde.extend_from_slice(&(len as u32).to_le_bytes());
    fde.push(0); // aug data length
    while (fde.len() + 4) % 4 != 0 {
        fde.push(0);
    }
    v.extend_from_slice(&(fde.len() as u32).to_le_bytes());
    v.extend_from_slice(&fde);
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

const RET: u32 = 0xd65f_03c0;
const MOV_X8_172: u32 = 0xd280_1588;

fn translated(bytes: &[u8]) -> (Output, Report) {
    let t = translate(
        bytes,
        &Options {
            ctr_el0: 0x8444_c004,
        },
    )
    .unwrap();
    match t.outcome {
        Outcome::Translated(o) => (o, t.report),
        other => panic!("expected a translation, got {other:?}"),
    }
}

fn word(b: &[u8], off: u64) -> u32 {
    u32::from_le_bytes(b[off as usize..off as usize + 4].try_into().unwrap())
}

/// Follow the `b` at text word `i` into the stub; return its words up to
/// and including the branch back.
fn stub_of(out: &[u8], i: usize, stub_vaddr: u64, stub_off: u64) -> Vec<u32> {
    let site = TEXT_VADDR + i as u64 * 4;
    let w = word(out, site);
    assert!(a64::is_b(w), "site {i} is not a b: {w:#x}");
    let target = site.wrapping_add(a64::decode_b(w) as u64);
    assert!(target >= stub_vaddr, "stub outside the stub segment");
    let mut words = Vec::new();
    let mut at = target;
    loop {
        let x = word(out, at - stub_vaddr + stub_off);
        words.push(x);
        if a64::is_b(x) {
            let back = at.wrapping_add(a64::decode_b(x) as u64);
            assert_eq!(back, site + 4, "stub must return to site + 4");
            return words;
        }
        at += 4;
    }
}

#[test]
fn each_rewrite_kind_gets_its_stub() {
    let text = vec![
        MOV_X8_172,
        a64::SVC0,
        a64::MRS_TPIDR_EL0 | 3,
        a64::MSR_TPIDR_EL0 | 16,
        a64::SCS_PUSH,
        a64::SCS_POP,
        a64::MRS_CTR_EL0 | 9,
        RET,
    ];
    let mut s = Synth::new(text.clone());
    s.syms.push(("f", 0, text.len(), elf::STT_FUNC));
    let (o, r) = translated(&s.build());
    let out = o.to_bytes();
    assert_eq!(r.sites, [1, 1, 1, 1, 1, 1]);
    assert_eq!((r.brk_fallback, r.ambiguous), (0, 0));
    let sv = r.stub_vaddr;
    let so = o.stub_offset;
    let body = |k, rt| a64::stub_body(k, rt, 0x8444_c004);
    for (i, kind, rt) in [
        (1, Kind::Svc, 0),
        (2, Kind::MrsTp, 3),
        (3, Kind::MsrTp, 16),
        (4, Kind::ScsPush, 30),
        (5, Kind::ScsPop, 30),
        (6, Kind::MrsCtr, 9),
    ] {
        let st = stub_of(&out, i, sv, so);
        assert_eq!(&st[..st.len() - 1], &body(kind, rt)[..], "{kind:?}");
    }
    // Untouched words stay.
    assert_eq!(word(&out, TEXT_VADDR), MOV_X8_172);
    assert_eq!(word(&out, TEXT_VADDR + 28), RET);
    // SCS: push stores x30 and advances; pop pre-decrements and reloads.
    assert_eq!(body(Kind::ScsPush, 30)[3], a64::str_post(30, 17, 8));
    assert_eq!(body(Kind::ScsPop, 30)[3], a64::ldr_pre(30, 17, -8));
    // CTR: movz/movk of the configured value.
    assert_eq!(
        body(Kind::MrsCtr, 9),
        vec![a64::movz(9, 0xc004, 0), a64::movk(9, 0x8444, 16)]
    );
}

#[test]
fn stub_segment_is_a_loadable_rx_segment_holding_the_phdrs() {
    let mut s = Synth::new(vec![a64::SVC0, RET]);
    s.syms.push(("f", 0, 2, elf::STT_FUNC));
    let orig = s.build();
    let (o, r) = translated(&orig);
    let out = o.to_bytes();
    let e = elf::parse(&out).unwrap();
    let loads: Vec<_> = e.loads().copied().collect();
    assert_eq!(loads.len(), 4);
    assert!(
        loads.windows(2).all(|w| w[0].p_vaddr < w[1].p_vaddr),
        "sorted"
    );
    let stub = loads[3];
    assert_eq!(stub.p_flags, PF_R | PF_X);
    assert_eq!(
        (stub.p_offset % PAGE, stub.p_vaddr % PAGE, stub.p_align),
        (0, 0, PAGE)
    );
    assert_eq!(stub.p_vaddr, r.stub_vaddr);
    assert!(stub.p_vaddr >= loads[2].p_vaddr + loads[2].p_memsz);
    assert!(stub.p_offset >= orig.len() as u64);
    // e_phoff points into the stub segment at the same delta as the first
    // PT_LOAD, so base + e_phoff finds the table (linker64's self-lookup).
    assert_eq!(e.ehdr.e_phoff, stub.p_offset);
    assert_eq!(
        stub.p_vaddr - stub.p_offset,
        loads[0].p_vaddr - loads[0].p_offset
    );
    let phdr = e.phdrs.iter().find(|p| p.p_type == PT_PHDR).unwrap();
    assert_eq!(phdr.p_vaddr, stub.p_vaddr);
    assert_eq!(phdr.p_filesz, e.phdrs.len() as u64 * 56);
    // Original bytes keep their offsets.
    assert_eq!(out.len() as u64, stub.p_offset + stub.p_filesz);
    assert_eq!(o.patched.len(), orig.len());
    let diff: Vec<usize> = (0..orig.len())
        .filter(|&i| orig[i] != o.patched[i])
        .collect();
    assert!(diff.iter().all(|&i| (32..40).contains(&i)
        || (56..58).contains(&i)
        || (TEXT_VADDR as usize..TEXT_VADDR as usize + 4).contains(&i)));
}

#[test]
fn file_without_pt_phdr_keeps_header_lookup_working() {
    let mut s = Synth::new(vec![a64::SVC0, RET]);
    s.with_phdr = false;
    s.syms.push(("f", 0, 2, elf::STT_FUNC));
    let (o, _) = translated(&s.build());
    let bytes = o.to_bytes();
    let e = elf::parse(&bytes).unwrap();
    assert!(e.phdrs.iter().all(|p| p.p_type != PT_PHDR));
    let first = e.loads().next().unwrap();
    let stub = e.loads().last().unwrap();
    // base + e_phoff lies in the stub segment's file image.
    let at = first.p_vaddr + e.ehdr.e_phoff;
    assert!(at >= stub.p_vaddr && at + e.phdrs.len() as u64 * 56 <= stub.p_vaddr + stub.p_filesz);
}

#[test]
fn data_in_text_is_not_rewritten() {
    // A literal pool marked `$d`, an STT_OBJECT in .text, then code.
    let text = vec![
        a64::SVC0,              // 0: code ($x)
        a64::SVC0,              // 1: data ($d)
        a64::MRS_TPIDR_EL0 | 1, // 2: data ($d)
        a64::SCS_PUSH,          // 3: code ($x)
        a64::MSR_TPIDR_EL0 | 2, // 4: STT_OBJECT
        a64::SVC0,              // 5: code, covered by the FDE only
        RET,
    ];
    let mut s = Synth::new(text);
    s.syms.push(("$x", 0, 0, 0));
    s.syms.push(("$d.1", 1, 0, 0));
    s.syms.push(("$x.2", 3, 0, 0));
    s.syms.push(("table", 4, 1, elf::STT_OBJECT));
    s.eh_frame_covers_text = true;
    let (o, r) = translated(&s.build());
    let out = o.to_bytes();
    assert_eq!(r.data_excluded, 3);
    assert_eq!(r.total_sites(), 3);
    assert_eq!(r.ambiguous, 0);
    assert_eq!(word(&out, TEXT_VADDR + 4), a64::SVC0);
    assert_eq!(word(&out, TEXT_VADDR + 8), a64::MRS_TPIDR_EL0 | 1);
    assert_eq!(word(&out, TEXT_VADDR + 16), a64::MSR_TPIDR_EL0 | 2);
    for i in [0, 3, 5] {
        assert!(a64::is_b(word(&out, TEXT_VADDR + i * 4)), "word {i}");
    }
}

#[test]
fn symbols_of_stripped_sections_are_ignored() {
    // A Rust dylib's `rust_metadata_*` object: section index past the table,
    // value 0, size covering everything. It must not hide code.
    let mut s = Synth::new(vec![a64::SVC0, RET]);
    s.eh_frame_covers_text = true;
    let mut b = s.build();
    let e = elf::parse(&b).unwrap();
    let symtab = e
        .shdrs
        .iter()
        .find(|x| x.sh_type == elf::SHT_SYMTAB)
        .unwrap();
    let mut sym = [0u8; 24];
    sym[4] = elf::STT_OBJECT;
    sym[6..8].copy_from_slice(&32u16.to_le_bytes());
    sym[16..24].copy_from_slice(&(1u64 << 20).to_le_bytes());
    let at = symtab.sh_offset as usize;
    b[at..at + 24].copy_from_slice(&sym); // overwrite the null symbol
    let (_, r) = translated(&b);
    assert_eq!((r.total_sites(), r.data_excluded), (1, 0));
}

#[test]
fn sites_without_evidence_are_reported_ambiguous() {
    let mut s = Synth::new(vec![a64::SVC0, RET, a64::SVC0, RET]);
    s.syms.push(("f", 0, 2, elf::STT_FUNC));
    let (_, r) = translated(&s.build());
    assert_eq!((r.total_sites(), r.ambiguous), (2, 1));
    assert_eq!(r.method, "sections+evidence");

    // No section headers: whole PF_X segment, all ambiguous.
    let mut s = Synth::new(vec![a64::SVC0, RET]);
    s.with_sections = false;
    let (_, r) = translated(&s.build());
    assert_eq!((r.total_sites(), r.ambiguous, r.method), (1, 1, "segments"));
}

#[test]
fn stubs_are_reachable_or_fall_back_to_brk() {
    // 200 MiB of bss pushes the stub segment out of b range of the text.
    let mut s = Synth::new(vec![a64::SVC0, a64::MRS_TPIDR_EL0, RET]);
    s.syms.push(("f", 0, 3, elf::STT_FUNC));
    s.bss = 200 << 20;
    let (o, r) = translated(&s.build());
    let out = o.to_bytes();
    assert_eq!(r.brk_fallback, 2);
    assert_eq!(word(&out, TEXT_VADDR), a64::brk_fallback(Kind::Svc, 0));
    assert_eq!(
        word(&out, TEXT_VADDR + 4),
        a64::brk_fallback(Kind::MrsTp, 0)
    );

    // Otherwise every rewritten site branches within ±128 MiB.
    let mut text = vec![RET; 4096];
    for i in (0..4096).step_by(3) {
        text[i] = a64::SVC0;
    }
    let mut s = Synth::new(text.clone());
    s.syms.push(("f", 0, text.len(), elf::STT_FUNC));
    s.bss = 100 << 20;
    let (o, r) = translated(&s.build());
    assert_eq!(r.brk_fallback, 0);
    let out = o.to_bytes();
    for i in (0..4096).step_by(3) {
        let st = stub_of(&out, i, r.stub_vaddr, o.stub_offset);
        assert_eq!(st.len(), a64::stub_len(Kind::Svc));
    }
    // The stub segment's file offset keeps the bss hole sparse.
    assert!(o.stub_offset >= 100 << 20);
    assert!(o.stub_segment.len() < 1 << 20);
}

#[test]
fn translation_is_deterministic_and_idempotent() {
    let mut s = Synth::new(vec![a64::SVC0, a64::MRS_TPIDR_EL0 | 4, a64::SCS_POP, RET]);
    s.syms.push(("f", 0, 4, elf::STT_FUNC));
    let orig = s.build();
    let (a, _) = translated(&orig);
    let (b, _) = translated(&orig);
    assert_eq!(a, b);
    // Translating a translated file changes nothing.
    let t = translate(
        &a.to_bytes(),
        &Options {
            ctr_el0: 0x8444_c004,
        },
    )
    .unwrap();
    assert_eq!(t.outcome, Outcome::Identity);
    assert_eq!(t.report.total_sites(), 0);
}

#[test]
fn nothing_to_rewrite_is_identity() {
    let mut s = Synth::new(vec![MOV_X8_172, RET]);
    s.syms.push(("f", 0, 2, elf::STT_FUNC));
    let t = translate(&s.build(), &Options::default()).unwrap();
    assert_eq!(t.outcome, Outcome::Identity);
}

#[test]
fn four_kib_aligned_files_are_left_to_the_load_time_path() {
    let mut s = Synth::new(vec![a64::SVC0, RET]);
    s.align = 4096;
    let t = translate(&s.build(), &Options::default()).unwrap();
    assert!(matches!(t.outcome, Outcome::Unsupported(_)));
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    let good = Synth::new(vec![a64::SVC0, RET]).build();
    assert!(translate(b"not an elf", &Options::default()).is_err());
    for cut in [16, 63, 64, 100, 300, 0x4002] {
        let _ = translate(&good[..cut], &Options::default());
    }
    let mut bad = good.clone();
    bad[32..40].copy_from_slice(&u64::MAX.to_le_bytes()); // e_phoff
    assert!(translate(&bad, &Options::default()).is_err());
    let mut bad = good;
    bad[40..48].copy_from_slice(&(u64::MAX - 10).to_le_bytes()); // e_shoff
    let _ = translate(&bad, &Options::default());
}

#[test]
fn cache_key_is_stable() {
    // Known vector: sha256("abc").
    assert_eq!(
        cache_key(b"abc"),
        format!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad-v{VERSION}")
    );
    assert_eq!(cache_key(b"abc"), cache_key(b"abc"));
    assert_ne!(cache_key(b"abc"), cache_key(b"abd"));
    // Only the content matters, not the path or the translation.
    let s = Synth::new(vec![a64::SVC0, RET]).build();
    assert_eq!(cache_key(&s), key_for_digest(&sha256_hex(&s)));
}

// ---- BoringSSL FIPS module hash --------------------------------------------

#[test]
fn fips_digest_is_hmac_sha256_with_the_zero_key() {
    // Python: hmac.new(bytes(64), msg, hashlib.sha256)
    assert_eq!(
        hex(&fips::digest(b"abc", None)),
        "fd7adb152c05ef80dccf50a1fa4c05d5a3ec6da95575fc312ae7c5d091836351"
    );
    // Shared builds prefix text and rodata with their u64 LE lengths.
    assert_eq!(
        hex(&fips::digest(b"abc", Some(b"de"))),
        "8a5e8d08ebbe19610e7ff78b85222c37eb4d8f9de33dc4a95e54e4230552c886"
    );
}

/// Where the synthetic module stores its hash: the R segment, after the
/// headers and outside the hashed ranges.
const HASH_AT: usize = 0x300;

/// A file whose module is text words [0, 4) and "rodata" words [4, 6), with
/// a TPIDR_EL0 read inside the module, and either the hash BoringSSL's build
/// would have stored or garbage.
fn fips_file(good_hash: bool) -> Vec<u8> {
    let text = vec![
        MOV_X8_172,
        a64::MRS_TPIDR_EL0 | 3,
        RET,
        RET,
        0x1234_5678,
        0x9abc_def0,
    ];
    let mut s = Synth::new(text);
    s.syms.push(("f", 0, 4, elf::STT_FUNC));
    s.syms.push(("BORINGSSL_bcm_text_start", 0, 0, 0));
    s.syms.push(("BORINGSSL_bcm_text_end", 4, 0, 0));
    s.syms.push(("BORINGSSL_bcm_rodata_start", 4, 0, 0));
    s.syms.push(("BORINGSSL_bcm_rodata_end", 6, 0, 0));
    let mut b = s.build();
    let t = TEXT_VADDR as usize;
    let h = if good_hash {
        fips::digest(&b[t..t + 16], Some(&b[t + 16..t + 24]))
    } else {
        [0x5a; 32]
    };
    b[HASH_AT..HASH_AT + 32].copy_from_slice(&h);
    b
}

#[test]
fn fips_module_hash_is_reinjected_after_rewriting() {
    let original = fips_file(true);
    let m = analyze(&elf::parse(&original).unwrap()).fips.unwrap();
    assert_eq!(m.text, (TEXT_VADDR, TEXT_VADDR + 16));
    assert_eq!(m.rodata, Some((TEXT_VADDR + 16, TEXT_VADDR + 24)));
    assert_eq!(
        (m.hash_offset, m.hash_vaddr),
        (HASH_AT as u64, HASH_AT as u64)
    );

    let (o, r) = translated(&original);
    assert!(r.fips_rehashed);
    let out = o.to_bytes();
    let t = TEXT_VADDR as usize;
    assert_ne!(
        out[t..t + 16],
        original[t..t + 16],
        "the module was rewritten"
    );
    let want = fips::digest(&out[t..t + 16], Some(&out[t + 16..t + 24]));
    assert_eq!(out[HASH_AT..HASH_AT + 32], want);
    assert_ne!(want, m.original);
    // The stubs lie outside the hashed ranges.
    assert!(!m.covers(r.stub_vaddr, r.stub_vaddr + r.stub_size));
    // Past the ELF header, only the rewritten word and the hash changed.
    let diff: Vec<usize> = (64..o.patched.len())
        .filter(|&i| o.patched[i] != original[i])
        .collect();
    assert!(
        diff.iter()
            .all(|&i| (HASH_AT..HASH_AT + 32).contains(&i) || (t + 4..t + 8).contains(&i)),
        "{diff:x?}"
    );
}

#[test]
fn a_fips_hash_that_does_not_verify_is_left_alone() {
    let original = fips_file(false);
    assert!(analyze(&elf::parse(&original).unwrap()).fips.is_none());
    let (o, r) = translated(&original);
    assert!(!r.fips_rehashed);
    assert_eq!(o.patched[HASH_AT..HASH_AT + 32], [0x5a; 32]);
}
