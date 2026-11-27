//! shader tunables (see ../known.rs).

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

tunable!(pub HALF_VIEWS: bool = false, "VICTORY_HALF_VIEWS",
    "16-bit values of a table's typed buffer views move as they are through unsigned twin views (fixes faces and mouths in cutscenes; the translator reads the environment variable itself, so set it there; its value is in the shader cache key)", live = Restart);

tunable_list!(pub(super) fn register: HALF_VIEWS);
