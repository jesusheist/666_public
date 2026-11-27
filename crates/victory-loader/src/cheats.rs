//! Checks of the title's own cheat table (Series 1.0.2.1). Pure, so they test without the game.

pub(crate) const TABLE: usize = 0x063b_6d10;
pub(crate) const PLAYER: usize = 0x0ab5_6e38;
pub(crate) const COUNT: usize = victory_core::cheats::COUNT;
/// Entry 5 is the title's own no-op.
pub(crate) const DISABLED: usize = 5;
const EXECUTE: u32 = 0x2000_0000;

/// An executable range of the image: (rva, size).
pub(crate) type Range = (usize, usize);

/// The executable section ranges from the parsed headers.
pub(crate) fn executable_ranges(sections: &[victory_pe::SectionInfo]) -> Vec<Range> {
    sections
        .iter()
        .filter(|s| s.characteristics & EXECUTE != 0)
        .map(|s| (s.rva as usize, s.virtual_size.max(s.file_size) as usize))
        .collect()
}

/// Each entry (but the disabled one) must point inside an executable section of the image at `base`.
pub(crate) fn validate(entries: &[usize; COUNT], base: usize, exec: &[Range]) -> Result<(), String> {
    for (n, &entry) in entries.iter().enumerate() {
        if n == DISABLED {
            continue;
        }
        let ok = entry
            .checked_sub(base)
            .is_some_and(|rva| exec.iter().any(|&(start, size)| rva >= start && rva < start + size));
        if !ok {
            return Err(format!("cheat table entry {n} ({entry:#x}) is not in an executable section"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: usize = 0x1_4000_0000;

    fn good() -> [usize; COUNT] {
        std::array::from_fn(|n| BASE + 0x3011_950 + n * 0x10)
    }

    #[test]
    fn a_table_inside_the_code_passes() {
        assert_eq!(validate(&good(), BASE, &[(0x1000, 0x4000_0000)]), Ok(()));
    }

    #[test]
    fn an_entry_outside_the_code_or_the_image_fails() {
        let exec = [(0x1000, 0x3012_000)];
        let mut t = good();
        t[7] = BASE + 0x5000_0000;
        assert!(validate(&t, BASE, &exec).unwrap_err().contains("entry 7"));
        t[7] = 0x1000;
        assert!(validate(&t, BASE, &exec).is_err());
        t[7] = 0;
        assert!(validate(&t, BASE, &exec).is_err());
    }

    #[test]
    fn the_disabled_entry_may_be_anything() {
        let mut t = good();
        t[DISABLED] = 0;
        assert_eq!(validate(&t, BASE, &[(0x1000, 0x4000_0000)]), Ok(()));
    }

    #[test]
    fn only_executable_sections_count() {
        let s = |rva, flags| victory_pe::SectionInfo {
            name: String::new(),
            rva,
            virtual_size: 0x100,
            file_offset: 0,
            file_size: 0x80,
            characteristics: flags,
        };
        assert_eq!(executable_ranges(&[s(0x1000, 0x6000_0020), s(0x2000, 0x4000_0040)]), vec![(0x1000, 0x100)]);
    }
}
