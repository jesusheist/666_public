# 666

**Branch `master`** (default). The current state of the fork, everything in one place. Work branches:

- [`kb`](../../tree/kb): keyboard and mouse (automatic vehicle layout, key prompts, remote pad).
- [`hotswap`](../../tree/hotswap): live settings, then soft reload and swapping code while the game runs.
- [`texture-pool`](../../tree/texture-pool): the texture pool, eviction and the memory budget.

All four start from the same commit; each branch moves on with its own work.

A fork of the VIctory runtime (by way of cool_fox) that plays the Xbox Series build
of the title on a Windows PC. The focus here is playing it with a keyboard and
mouse, plus tools to find what the title keeps in memory.

Everything this fork adds is behind a switch. With no switch set it behaves like
cool_fox `f4b48ad`, which this branch starts from.

## Keyboard and mouse

- **A PC layout.** `[controls]` in `victory.toml` maps keys and mouse buttons onto
  a stand-in gamepad, with separate on-foot, vehicle and aircraft layouts. See
  `victory.example.toml`.
- **The layout follows the car by itself.** `VICTORY_VEHICLE_FLAG` points at the
  title's own "in a vehicle" byte, so getting in and out of a car switches the
  layout with no toggle key. For build 1.0.2.1:

  ```
  VICTORY_VEHICLE_FLAG=img+74dfca5
  ```

  The byte is 0 on foot and 1 in a car. It held across separate launches. Format:
  `<hex address | img+hex offset>[:width 1|2|4][:value meaning in a vehicle]`.
  The toggle key (G by default) still works and wins until the flag next changes.
- **Key prompts.** `VICTORY_PROMPTS=keys` draws key caps in place of the gamepad
  button pictures, named from the `[controls]` layout in force. It hooks the UI's
  icon movie for build 1.0.2.1. The prompts are drawn once, so they keep the
  layout that was in force at that moment, and animated prompts keep their own
  pictures.
- **Remote pad.** Over MCP: `press`, `stick`, `release_all`, `input_script`. It
  works without the window having focus.

## Finding values in memory

An MCP memory scanner over the title's own memory only (never the runtime's):
`mem_regions`, `mem_scan_start`, `mem_scan_filter` (changed, unchanged, increased,
decreased, equals, not_equals), `mem_scan_list`, `mem_read`, `mem_scan_reset`.
It reads safely and never writes. Hits in the title image are reported as
`img+<offset>`, which stays the same from one launch to the next. That is how the
vehicle flag above was found: snapshot in the car, then out, in, out, keeping the
bytes that follow along.

## Renderer and speed switches

Ported from other forks, all off by default and still being measured. Turn one on
at a time and compare.

| Switch | What it does | From |
|---|---|---|
| `VICTORY_SKIP_WC_BACK` | Shader writes into write-combined copies stay on the card (1 MiB floor) | fxceless, yuriko |
| `VICTORY_WAVE_PAIRS` | Wave64 compute runs on 32-lane hardware instead of being left out | Vice-Tay |
| `VICTORY_HALF_VIEWS` | 16-bit unsigned twin views (faces and mouths in cutscenes) | fxceless via MAXAWER |
| `VICTORY_HEAP_WORKER` | The heap texture pass on its own thread (needs `VICTORY_LATE_WAIT`) | fxceless via MAXAWER |
| `VICTORY_PORT_CBV`, `_GUARD`, `_JOURNAL` | Renderer ports 5, 6 and 7 | DoubleGhosty |
| `VICTORY_PORT_CONSTANTS` | Renderer port 1 (slower here so far) | DoubleGhosty |
| `VICTORY_SKIP_HAIR`, `VOXEL_VIS_COPIES`, `AMBIENT_*`, `READ_ZERO_GAPS`, `TEXTURE_ROW_RUNS`, `LIBRARY_STORE_LATER`, `PIPELINE_COMPUTE_WAIT` | Smaller fixes and tests | DoubleGhosty |

Switches live in one registry (`crates/victory-core/src/tunables`). Over MCP:
`list_tunables`, `get_tunable`, `set_tunable`, `reset_tunable`. The F10 menu has a
Live settings page. Some switches change while the game runs; others need a
restart, and the registry says which.

## Building and running

You need Windows 10 or 11 x64, a CPU with AVX2, a Direct3D 12 GPU with shader
model 6.6 and mesh shaders, Rust (the toolchain is pinned), the MSVC build tools,
and your own dump of the title.

```powershell
cargo build --locked --release -p victory
copy victory.example.toml victory.toml   # set your paths, enable [controls]
$env:VICTORY_GAME_DIR = "<folder with game_scarlett_master.exe>"
$env:VICTORY_VEHICLE_FLAG = "img+74dfca5"
$env:VICTORY_PROMPTS = "keys"
.\Play-VIctory.cmd pareto-residency
```

`VICTORY_MCP=47820` opens the MCP server on 127.0.0.1 for the tools above.

## Credits

Most of this is other people's work, and the history is kept.

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its
  contributors: the loader, the runtime services, the Direct3D 12 back end and the
  shader recompiler.
- **cool_fox** (CGBJ, codec-d): the base of this branch.
- **DoubleGhosty**: renderer ports and the smaller fixes listed above.
- **fxceless**, **p3n1cillin (yuriko)**, **TaynanGT (Vice-Tay)**, **MAXAWER**:
  the copy-back skip, paired wave64, half views and the heap worker.
- **imjustprism** and **avatarmanz**: the key caps over the button prompts.
- **dystopia**: the PC control layout.
- Everyone credited in the forks above, whose work these carry.

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file it does not
cover is named in [docs/asset-policy.md](docs/asset-policy.md). No part of the
title is included. You need your own copy.
