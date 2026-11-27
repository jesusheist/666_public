//! Everything specific to the title: its container formats, build
//! identities, and the hooks we place in its code (located by per-build
//! offsets or byte-pattern signatures). What may and may not be stored here
//! is defined in docs/asset-policy.md.

pub mod prompts;
pub mod sgd5;

/// A specific build of the title executable we know how to run.
#[derive(Debug, Clone, Copy)]
pub struct KnownBuild {
    /// Our own label, e.g. "1.0.0 retail".
    pub label: &'static str,
    /// BLAKE3 of the original executable, hex.
    pub blake3: &'static str,
    /// Where the build keeps what the keyboard prompts hook needs.
    pub key_prompts: Option<prompts::HookOffsets>,
}

/// Builds we know. The offsets are imjustprism's (Marvisak/VIctory#3433).
pub const KNOWN_BUILDS: &[KnownBuild] = &[KnownBuild {
    label: "Series 1.0.2.1",
    blake3: "b99f876b74d4a77f03af5f878223b27bd647b5c35a79d5decd9f716504239e93",
    key_prompts: Some(prompts::HookOffsets {
        movie_address_builder: prompts::PointerOffset(0x62f_3388),
        string_assign: prompts::CodeOffset(0xeb_ba60),
    }),
}];

/// Looks up a build by the BLAKE3 hash of its executable.
pub fn identify(blake3_hex: &str) -> Option<&'static KnownBuild> {
    KNOWN_BUILDS
        .iter()
        .find(|b| b.blake3.eq_ignore_ascii_case(blake3_hex))
}
